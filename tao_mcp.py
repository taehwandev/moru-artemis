"""Host-driven Android MCP: existing Codex/Claude reasoning, no model API calls."""

import asyncio
import base64
import json
import inspect
import os
from pathlib import Path

from mcp.server.fastmcp import FastMCP, Image
from mcp.types import ToolAnnotations
from tao_comparison import ComparisonTools
from tao_elements import ElementTools, element_view, is_focused

# Upstream's focus helper waits this long for the keyboard after tapping a field.
_KEYBOARD_SETTLE_SECONDS = 1.0


def _check_controller_factory(adb_server):
    """Fail at startup when the pinned upstream private factory contract drifts."""
    factory = getattr(adb_server, '_get_controller', None)
    try:
        if not callable(factory) or inspect.iscoroutinefunction(factory):
            raise TypeError('expected synchronous controller factory')
        inspect.signature(factory).bind(device_serial='contract-check')
    except (TypeError, ValueError) as exc:
        raise RuntimeError(
            'Incompatible Artemis controller factory; restore the tested upstream '
            'revision or update the tao-artemis adapter before device testing.'
        ) from exc


def _check_hierarchy_helpers(parse, prune):
    """Fail at startup when the pinned upstream hierarchy helpers drift.

    Reading the hierarchy without a screenshot means calling upstream's parser and
    filter directly instead of the combined screen read, so their shapes are part
    of this adapter's contract the same way the controller factory is. This checks
    the call signatures only; behavioral drift still needs a device observation.
    """
    try:
        if not callable(parse) or not callable(prune):
            raise TypeError('expected callable hierarchy helpers')
        inspect.signature(parse).bind('<hierarchy/>')
        inspect.signature(prune).bind([], screen_width=1080, screen_height=2400)
    except (TypeError, ValueError) as exc:
        raise RuntimeError(
            'Incompatible Artemis hierarchy helpers; restore the tested upstream '
            'revision or update the tao-artemis adapter before device testing.'
        ) from exc


def _register_tools(server):
    """Wire every tool onto the MCP server, read-only ones marked as such."""
    read_only = ToolAnnotations(readOnlyHint=True)
    elements = ElementTools(server)
    comparisons = ComparisonTools(server)
    for tool in (server.devices, server.screenshot, server.hierarchy,
                 elements.wait_for_element, comparisons.capture_baseline,
                 comparisons.compare_current):
        server.mcp.add_tool(tool, annotations=read_only)
    for tool in (server.tap, server.swipe, server.input_text, server.press_key,
                 server.launch_app, elements.tap_element, elements.input_element):
        server.mcp.add_tool(tool)


def _read_elements(controller, parse, prune):
    """Parse and prune one UI dump, off the event loop because the client blocks."""
    device = controller.ctx.device
    return prune(
        parse(controller.ctx.ui_adb_client.get_hierarchy()),
        # Upstream defaults the same way when the context has no dimensions.
        screen_width=getattr(device, 'device_width', None) or 1080,
        screen_height=getattr(device, 'device_height', None) or 2400,
    )


def _read_screenshot(client) -> bytes:
    """Capture one screenshot, off the event loop because the client blocks.

    Going through the client skips the hierarchy dump and the fixed settling pause
    the combined read adds, neither of which an image caller uses. The pause
    protects an agent that chains actions in-process; these tools are separated by
    a host round trip that is far longer.
    """
    encoded = client.get_screenshot_base64()
    if not encoded:
        raise RuntimeError('Screenshot capture failed')
    return base64.b64decode(encoded, validate=True)


async def _focus_and_type(controller, x, y, text, clear_existing, focused):
    """Tap the field unless it already holds focus, then type into it.

    Upstream's focus helper makes the same skip-the-tap decision, but reads the
    screen again through the combined read to make it: the screenshot an element
    read exists to avoid, paid once more per text input, and a second
    observation that can disagree with the one the coordinates came from.
    """
    if not focused:
        error = (await controller.tap_at(x=x, y=y)).error
        if error:
            raise RuntimeError(error)
        await asyncio.sleep(_KEYBOARD_SETTLE_SECONDS)
    if not await controller.type_text(text, clear_existing=clear_existing):
        raise RuntimeError('Text input failed')


class DirectDeviceServer:
    """Small, explicitly targeted adapter around upstream Artemis controllers."""

    def __init__(self):
        # Set these before Artemis loads its process-wide settings.
        os.environ['ARTEMIS_CLOUD_MODE'] = '0'
        os.environ['ARTEMIS_KEEP_DEVICE_AWAKE'] = 'false'
        os.environ.setdefault('ARTEMIS_APP_DIR', str(Path.home() / '.tao' / 'artemis'))
        # This adapter observes the helper but never provisions it, so under
        # 'auto' every 30s helper retry stops and restarts the UIAutomator2
        # server on a device that has no helper. setdefault leaves an explicit
        # operator choice, including 'auto' once the helper is installed, intact.
        os.environ.setdefault('ARTEMIS_HIERARCHY_BACKEND', 'uiautomator')
        from artemis.mcp import adb_server

        from artemis.mcp.actuators.adb import find_element_at_coords
        from third_party.mobile_use.clients.ui_automator_client import parse_hierarchy_xml_to_elements
        from artemis.utils.ui_filter import filter_ui_hierarchy

        _check_controller_factory(adb_server)
        _check_hierarchy_helpers(parse_hierarchy_xml_to_elements, filter_ui_hierarchy)
        self._hit_test = find_element_at_coords
        self._parse_hierarchy = parse_hierarchy_xml_to_elements
        self._prune_hierarchy = filter_ui_hierarchy
        self._adb = adb_server
        self.mcp = FastMCP('tao-artemis')
        _register_tools(self)

    def _device(self, device_serial: str):
        if not device_serial.strip():
            raise ValueError('device_serial must be explicitly selected from devices')
        return self._adb._get_controller(device_serial=device_serial.strip())

    def devices(self) -> list[str]:
        """List connected ADB device serials. Select the intended device explicitly."""
        adb = self._adb.AdbClient(
            host=os.environ.get('ADB_SERVER_HOST', '127.0.0.1'),
            port=int(os.environ.get('ADB_SERVER_PORT', '5037')),
        )
        return [device.serial for device in adb.device_list()]

    async def screenshot(self, device_serial: str) -> Image:
        """Return the current device screen as an MCP image, not base64 text."""
        client = self._device(device_serial).ctx.ui_adb_client
        return Image(data=await asyncio.to_thread(_read_screenshot, client),
                     format='jpeg')

    async def elements(self, device_serial: str) -> list[dict]:
        """Read the UI hierarchy without the screenshot upstream captures with it.

        That screenshot dominates the combined read's cost and every element caller
        here discards it. Running the blocking client call off the event loop is
        also what lets ``wait_for_element``'s timeout cancel an observation.
        """
        return await asyncio.to_thread(
            _read_elements, self._device(device_serial),
            self._parse_hierarchy, self._prune_hierarchy)

    async def hierarchy(self, device_serial: str) -> str:
        """Return current UI elements without calling OCR or a model provider."""
        elements = await self.elements(device_serial)
        return json.dumps([element_view(e) for e in elements], ensure_ascii=False)

    async def tap(self, device_serial: str, x: int, y: int) -> str:
        """Tap screen pixel coordinates verified against a current observation."""
        result = await self._device(device_serial).tap_at(x=x, y=y)
        if result.error:
            raise RuntimeError(result.error)
        return 'Success'

    async def swipe(self, device_serial: str, start_x: int, start_y: int,
                    end_x: int, end_y: int, duration_ms: int = 400) -> str:
        """Swipe between screen pixel coordinates; duration is in milliseconds."""
        if duration_ms <= 0:
            raise ValueError('duration_ms must be positive')
        error = await self._device(device_serial).swipe_coords(
            start_x=start_x, start_y=start_y, end_x=end_x, end_y=end_y,
            duration=duration_ms,
        )
        if error:
            raise RuntimeError(error)
        return 'Success'

    async def input_text(self, device_serial: str, x: int, y: int,
                         text: str, clear_existing: bool = False) -> str:
        """Focus an observed text field and type text; optionally replace its contents."""
        field = self._hit_test(await self.elements(device_serial), x, y)
        return await self._type_into(device_serial, x, y, text, clear_existing,
                                     is_focused(field))

    async def _type_into(self, device_serial: str, x: int, y: int, text: str,
                         clear_existing: bool, focused: bool) -> str:
        """Type into the field the caller observed at these coordinates."""
        await _focus_and_type(self._device(device_serial), x, y, text,
                              clear_existing, focused)
        return 'Success'

    async def press_key(self, device_serial: str, keycode: str) -> str:
        """Press an Android keycode, for example KEYCODE_BACK or KEYCODE_ENTER."""
        if not await self._device(device_serial).press_key(keycode):
            raise RuntimeError('Key event failed')
        return 'Success'

    async def launch_app(self, device_serial: str, package_name: str) -> str:
        """Launch the specified installed Android app on the selected device."""
        controller = self._device(device_serial)
        success, error = await self._adb.launch_app_with_retries(controller.ctx, package_name)
        if not success:
            raise RuntimeError(error or 'App launch failed')
        return 'Success'

    def run(self):
        self._adb.configure_stdio_mode()
        self.mcp.run(transport='stdio')


if __name__ == '__main__':
    DirectDeviceServer().run()
