"""Bounded in-memory baseline/current comparison for host-driven UI checks."""

import base64
import json
from io import BytesIO

from mcp.types import CallToolResult, ImageContent, TextContent

from moru_elements import element_view


_PIXEL_DELTA_THRESHOLD = 16
_SEMANTIC_SAMPLE_LIMIT = 20


class ComparisonTools:
    """Keep one same-device observation pair without persisting screen content."""

    def __init__(self, server):
        self.server = server
        self._baseline = None

    async def capture_baseline(self, device_serial: str) -> CallToolResult:
        """Capture the reference screen before replacing or changing the app under test."""
        image, elements, dimensions = await self._capture(device_serial)
        self._baseline = {
            "device_serial": device_serial.strip(),
            "image": image,
            "elements": elements,
            "dimensions": dimensions,
        }
        summary = {
            "status": "baseline_captured",
            "device_serial": device_serial.strip(),
            "dimensions": list(dimensions),
            "element_count": len(elements),
            "storage": "memory_only",
        }
        return _observation_result(summary, image)

    async def compare_current(self, device_serial: str) -> CallToolResult:
        """Compare the current static screen with the captured same-device baseline.

        The result reports differences, never a product pass/fail decision. Motion
        still requires a recording or bounded frame sequence.
        """
        if self._baseline is None:
            raise ValueError("Capture a baseline before comparing the current screen")
        serial = device_serial.strip()
        if serial != self._baseline["device_serial"]:
            raise ValueError("Baseline belongs to a different device_serial")

        image, elements, dimensions = await self._capture(serial)
        if dimensions != self._baseline["dimensions"]:
            raise ValueError(
                "Screen dimensions changed; compare the same orientation and display state"
            )

        summary = {
            "status": "comparison_observed",
            "device_serial": serial,
            "dimensions": list(dimensions),
            "pixel_delta": _pixel_delta(self._baseline["image"], image),
            "semantic_delta": _semantic_delta(self._baseline["elements"], elements),
            "verdict": "host_must_compare_against_explicit_acceptance_criteria",
        }
        return _observation_result(summary, image)

    async def _capture(self, device_serial):
        # The adapter's own screenshot and element reads, in the order upstream's
        # combined read takes them, so the two halves are as close together as
        # that read makes them. What they skip is its fixed 300 ms pre-capture
        # pause, which protects an agent chaining actions in-process and buys
        # nothing between two tools a host round trip apart.
        image = (await self.server.screenshot(device_serial)).data
        elements = tuple(element_view(item) for item in await self.server.elements(device_serial))
        return image, elements, _dimensions(image)


def _dimensions(image):
    from PIL import Image

    with Image.open(BytesIO(image)) as decoded:
        return decoded.size


def _pixel_delta(before, current):
    import numpy as np
    from PIL import Image

    with Image.open(BytesIO(before)) as first, Image.open(BytesIO(current)) as second:
        first_pixels = np.asarray(first.convert("RGB"), dtype=np.int16)
        second_pixels = np.asarray(second.convert("RGB"), dtype=np.int16)
    delta = np.abs(first_pixels - second_pixels)
    changed = np.any(delta >= _PIXEL_DELTA_THRESHOLD, axis=2)
    if np.any(changed):
        rows = np.where(np.any(changed, axis=1))[0]
        cols = np.where(np.any(changed, axis=0))[0]
        changed_bounds = [int(cols[0]), int(rows[0]), int(cols[-1]) + 1, int(rows[-1]) + 1]
    else:
        changed_bounds = None
    return {
        "changed_pixel_ratio": round(float(changed.mean()), 6),
        "mean_absolute_channel_delta": round(float(delta.mean()), 3),
        "threshold": _PIXEL_DELTA_THRESHOLD,
        "changed_bounds": changed_bounds,
    }


def _identity_without_bounds(element):
    return (
        str(element.get("resource-id") or ""),
        str(element.get("text") or ""),
        str(element.get("content-desc") or ""),
        str(element.get("class") or ""),
        str(element.get("package") or ""),
    )


def _without_bounds(element):
    return {key: value for key, value in element.items() if key != "bounds"}


def _identity_counts(elements):
    counts = {}
    for item in elements:
        ident = _identity_without_bounds(item)
        if any(ident[:3]):
            counts[ident] = counts.get(ident, 0) + 1
    return counts


def _semantic_delta(before, current):
    def indexed(elements):
        return {
            json.dumps(item, ensure_ascii=False, sort_keys=True, separators=(",", ":")): item
            for item in elements
        }

    previous = indexed(before)
    latest = indexed(current)
    added_keys = sorted(latest.keys() - previous.keys())
    removed_keys = sorted(previous.keys() - latest.keys())
    added_items = [latest[key] for key in added_keys]
    removed_items = [previous[key] for key in removed_keys]

    before_counts = _identity_counts(before)
    current_counts = _identity_counts(current)
    removed_by_id = {}
    for item in removed_items:
        ident = _identity_without_bounds(item)
        if before_counts.get(ident) == 1:
            removed_by_id.setdefault(ident, []).append(item)
    added_by_id = {}
    for item in added_items:
        ident = _identity_without_bounds(item)
        if current_counts.get(ident) == 1:
            added_by_id.setdefault(ident, []).append(item)

    moved = []
    for ident in sorted(set(removed_by_id) & set(added_by_id)):
        rem_group = removed_by_id[ident]
        add_group = added_by_id[ident]
        if len(rem_group) == 1 and len(add_group) == 1:
            prev_item, curr_item = rem_group[0], add_group[0]
            prev_bounds = prev_item.get("bounds")
            curr_bounds = curr_item.get("bounds")
            if (
                prev_bounds
                and curr_bounds
                and prev_bounds != curr_bounds
                and _without_bounds(prev_item) == _without_bounds(curr_item)
            ):
                moved.append(
                    {
                        "element": curr_item,
                        "previous_bounds": prev_bounds,
                        "current_bounds": curr_bounds,
                    }
                )

    return {
        "baseline_element_count": len(before),
        "current_element_count": len(current),
        "added_count": len(added_keys),
        "removed_count": len(removed_keys),
        "moved_count": len(moved),
        "added": added_items[:_SEMANTIC_SAMPLE_LIMIT],
        "removed": removed_items[:_SEMANTIC_SAMPLE_LIMIT],
        "moved": moved[:_SEMANTIC_SAMPLE_LIMIT],
        "samples_truncated": (
            len(added_keys) > _SEMANTIC_SAMPLE_LIMIT
            or len(removed_keys) > _SEMANTIC_SAMPLE_LIMIT
            or len(moved) > _SEMANTIC_SAMPLE_LIMIT
        ),
    }


def _observation_result(summary, image):
    return CallToolResult(
        content=[
            TextContent(type="text", text=json.dumps(summary, ensure_ascii=False)),
            ImageContent(
                type="image",
                data=base64.b64encode(image).decode("ascii"),
                mimeType="image/jpeg",
            ),
        ],
        structuredContent=summary,
        isError=False,
    )
