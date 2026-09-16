"""Exact UI targeting and bounded observation loops without model calls."""
import asyncio
import math
import re
import time


_FIELDS = {'text', 'content-desc', 'resource-id', 'class', 'package'}
_STATES = {'present', 'absent', 'enabled', 'disabled', 'focused', 'checked', 'selected'}
_VIEW_IDENTITY = _FIELDS | {'bounds'}
_VIEW_WHEN_TRUE = ('checkable', 'checked', 'clickable', 'focused',
                   'long-clickable', 'scrollable', 'selected')


def _selector(selector):
    if not isinstance(selector, dict) or not selector or set(selector) - _FIELDS:
        raise ValueError('Use a nonempty selector with text, content-desc, resource-id, class or package')
    if not all(isinstance(v, str) and v.strip() for v in selector.values()):
        raise ValueError('Selector values must be nonempty exact strings')
    return dict(selector)


def _match(elements, selector):
    matches = [e for e in elements if all(e.get(k) == v for k, v in selector.items())]
    if len(matches) > 1:
        raise ValueError('Ambiguous selector; add resource-id, class or package to identify one element')
    return matches[0] if matches else None


def _flag(element, name):
    value = element.get(name)
    if value is True or value == 'true':
        return True
    if value is False or value == 'false':
        return False
    return None


def element_view(element):
    """The fields this adapter's contracts expose, for one element or a whole screen.

    ``enabled`` is reported whenever upstream states it, true or false, because a
    false value is what explains a refused action. The remaining flags carry
    signal only when set, so they appear as ``true`` or not at all rather than
    repeating ``false`` on every element of a screen. Flags upstream leaves
    unknown are omitted, which is what ``_center`` already treats as unusable.
    """
    if element is None:
        return None
    view = {k: element[k] for k in sorted(_VIEW_IDENTITY)
            if element.get(k) not in ('', None)}
    enabled = _flag(element, 'enabled')
    if enabled is not None:
        view['enabled'] = enabled
    view.update({k: True for k in _VIEW_WHEN_TRUE if _flag(element, k) is True})
    return view


def _center(element):
    if _flag(element, 'enabled') is not True:
        raise ValueError('Element is disabled or enabled state is unknown; action refused')
    bounds = re.fullmatch(r'\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]', str(element.get('bounds', '')))
    if not bounds:
        raise ValueError('Element has no valid pixel bounds')
    x1, y1, x2, y2 = map(int, bounds.groups())
    if x1 < 0 or y1 < 0 or x2 <= x1 or y2 <= y1:
        raise ValueError('Element bounds are offscreen or empty')
    return (x1 + x2) // 2, (y1 + y2) // 2


class ElementTools:
    """Share the adapter's explicit-device and action error contracts."""

    def __init__(self, server):
        self.server = server

    async def _target(self, device_serial, selector):
        selector = _selector(selector)
        element = _match(await self.server.elements(device_serial), selector)
        if element is None:
            raise ValueError('Element not found')
        return element, _center(element)

    async def tap_element(self, device_serial: str, selector: dict[str, str]) -> dict:
        """Tap one exact UI match. Combine selector fields to disambiguate; never picks first."""
        element, (x, y) = await self._target(device_serial, selector)
        await self.server.tap(device_serial, x, y)
        return {'status': 'action_sent', 'element': element_view(element), 'point': [x, y]}

    async def input_element(self, device_serial: str, selector: dict[str, str],
                            text: str, clear_existing: bool = False) -> dict:
        """Focus and type into one exact match; text is never echoed in the result."""
        element, (x, y) = await self._target(device_serial, selector)
        await self.server.input_text(device_serial, x, y, text, clear_existing)
        return {'status': 'action_sent', 'point': [x, y]}

    async def wait_for_element(self, device_serial: str, selector: dict[str, str],
                               state: str = 'present', timeout_ms: int = 5000,
                               poll_interval_ms: int = 250) -> dict:
        """Poll locally for present/absent/enabled/disabled/focused/checked/selected.

        Exact AND matching; duplicates and device errors fail rather than imply absence.
        A timeout returns matched=false. It is not a passing assertion.
        """
        selector = _selector(selector)
        if state not in _STATES:
            raise ValueError('Unsupported UI state')
        if not 1 <= timeout_ms <= 30000 or not 100 <= poll_interval_ms <= 2000:
            raise ValueError('timeout_ms must be 1..30000; poll_interval_ms must be 100..2000')
        start = time.monotonic()
        deadline = start + timeout_ms / 1000
        observations = 0
        element = None
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                elements = await asyncio.wait_for(
                    self.server.elements(device_serial), remaining)
            except asyncio.TimeoutError:
                break
            observations += 1
            element = _match(elements, selector)
            matched = (element is None) if state == 'absent' else element is not None
            if matched and state not in {'present', 'absent'}:
                key = 'enabled' if state == 'disabled' else state
                matched = _flag(element, key) is (state != 'disabled')
            if matched and time.monotonic() <= deadline:
                return self._result(True, element, observations, start)
            await asyncio.sleep(min(poll_interval_ms / 1000, max(0, deadline - time.monotonic())))
        return self._result(False, element, observations, start)

    @staticmethod
    def _result(matched, element, observations, start):
        return {'matched': matched, 'status': 'matched' if matched else 'timeout',
                'observations': observations, 'elapsed_ms': math.ceil((time.monotonic() - start) * 1000),
                'element': element_view(element)}
