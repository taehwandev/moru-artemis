"""Host MCP protocol, device targeting and failure contracts without a device."""
import asyncio
import base64
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock, patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from moru_elements import element_view  # noqa: E402

NODE_DEFAULTS = {
    'index': '0', 'text': '', 'resource-id': '', 'class': 'android.widget.TextView',
    'package': 'pkg', 'content-desc': '', 'checkable': 'false', 'checked': 'false',
    'clickable': 'false', 'enabled': 'true', 'focusable': 'false', 'focused': 'false',
    'scrollable': 'false', 'long-clickable': 'false', 'password': 'false',
    'selected': 'false', 'bounds': '[0,0][100,100]',
}


def node_xml(**overrides):
    """One UIAutomator dump holding a single node, as the device would emit it."""
    attributes = ' '.join(
        f'{name}="{value}"' for name, value in {**NODE_DEFAULTS, **overrides}.items())
    return ("<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>"
            f'<hierarchy rotation="0"><node {attributes} /></hierarchy>')


class MoruMcpTest(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.state = tempfile.TemporaryDirectory()
        cls.env = patch.dict(os.environ, {'ARTEMIS_APP_DIR': cls.state.name})
        cls.env.start()
        from moru_mcp import DirectDeviceServer
        cls.server = DirectDeviceServer()

    @classmethod
    def tearDownClass(cls):
        cls.env.stop()
        cls.state.cleanup()

    def setUp(self):
        self.controller = Mock()
        self.controller.tap_at = AsyncMock(return_value=SimpleNamespace(error=None))
        self.controller.take_screenshot = AsyncMock(return_value=base64.b64encode(b'image').decode())
        self.shot = Mock(return_value=base64.b64encode(b'image').decode())
        self.controller.press_key = AsyncMock(return_value=False)
        # hierarchy() now parses the dump itself instead of taking a screen read
        # that also captures a screenshot, so the fixture supplies real dump XML.
        self.dump = Mock(return_value=node_xml(text='Hello'))
        self.controller.ctx = SimpleNamespace(
            ui_adb_client=SimpleNamespace(get_hierarchy=self.dump,
                                          get_screenshot_base64=self.shot),
            device=SimpleNamespace(device_width=1080, device_height=2400),
        )
        self.get = patch.object(self.server._adb, '_get_controller', return_value=self.controller)
        self.mock_get = self.get.start()
        self.addCleanup(self.get.stop)

    async def test_manifest_requires_device_and_excludes_autonomous_tools(self):
        tools = {t.name: t for t in await self.server.mcp.list_tools()}
        self.assertEqual({'devices', 'screenshot', 'hierarchy', 'tap', 'swipe',
                          'input_text', 'press_key', 'launch_app', 'tap_element',
                          'tap_element_and_wait', 'input_element', 'wait_for_element',
                          'capture_baseline', 'perf_frame_stats', 'record_frames', 'scroll_to_element',
                          'compare_current'}, set(tools))
        self.assertTrue(tools['wait_for_element'].annotations.readOnlyHint)
        self.assertTrue(tools['capture_baseline'].annotations.readOnlyHint)
        self.assertTrue(tools['compare_current'].annotations.readOnlyHint)
        for name, tool in tools.items():
            if name != 'devices':
                self.assertIn('device_serial', tool.inputSchema['required'])

    async def test_empty_target_never_initializes_default_device(self):
        with self.assertRaises(ValueError):
            await self.server.tap(' ', 12, 34)
        self.mock_get.assert_not_called()

    def test_perf_device_resolves_explicit_serial_without_ui_controller(self):
        from moru_adb import selected_adb_device
        selected = SimpleNamespace(serial='device-b')
        with patch.object(self.server._adb, 'AdbClient') as client:
            client.return_value.device_list.return_value = [
                SimpleNamespace(serial='device-a'), selected]
            with patch.dict(os.environ, {'ADB_HOST': 'remote-host', 'ADB_PORT': '5040'}):
                self.assertIs(selected, selected_adb_device(self.server._adb, 'device-b'))
                client.assert_called_once_with(host='remote-host', port=5040)
        self.mock_get.assert_not_called()
        with self.assertRaises(ValueError):
            selected_adb_device(self.server._adb, ' ')

    async def test_tap_forwards_exact_device_and_pixels(self):
        self.assertEqual('Success', await self.server.tap('device-b', 12, 34))
        self.mock_get.assert_called_once_with(device_serial='device-b')
        self.controller.tap_at.assert_awaited_once_with(x=12, y=34)

    async def test_failed_action_does_not_report_success(self):
        with self.assertRaises(RuntimeError):
            await self.server.press_key('device-a', 'KEYCODE_BACK')

    async def test_screenshot_is_native_mcp_image_and_skips_the_dump(self):
        result = await self.server.screenshot('device-a')
        content = result.to_image_content()
        self.assertEqual('image', content.type)
        self.assertEqual('image/jpeg', content.mimeType)
        self.assertEqual(b'image', base64.b64decode(content.data))
        self.shot.assert_called_once_with()
        self.dump.assert_not_called()

    async def test_failed_capture_does_not_return_an_empty_image(self):
        for failure in (None, ''):
            self.shot.return_value = failure
            with self.assertRaisesRegex(RuntimeError, 'Screenshot capture failed'):
                await self.server.screenshot('device-a')

    async def test_hierarchy_reads_the_dump_without_capturing_a_screenshot(self):
        self.assertEqual([{'bounds': '[0,0][100,100]', 'class': 'android.widget.TextView',
                           'package': 'pkg', 'text': 'Hello', 'enabled': True}],
                         json.loads(await self.server.hierarchy('device-b')))
        self.dump.assert_called_once_with()
        self.shot.assert_not_called()
        self.controller.take_screenshot.assert_not_awaited()

    def test_element_view_keeps_targeting_signal_and_drops_redundant_fields(self):
        self.assertEqual(element_view({
            'text': 'Search', 'content-desc': 'Search', 'accessibilityText': 'Search',
            'resource-id': 'pkg:id/search', 'class': 'android.widget.EditText',
            'package': 'pkg', 'bounds': '[0,0][10,10]',
            'parsed_bounds': {'left': 0, 'top': 0, 'right': 10, 'bottom': 10},
            'enabled': 'true', 'clickable': 'true', 'scrollable': 'false',
            'checked': 'false', 'focused': 'false', 'selected': 'false',
            'checkable': 'false', 'long-clickable': 'false', 'focusable': 'true',
            'password': 'false', 'visible-to-user': 'true', 'index': '0',
            'drawing-order': '3', 'display-id': '0', 'hint': '',
        }), {
            'bounds': '[0,0][10,10]', 'class': 'android.widget.EditText',
            'content-desc': 'Search', 'enabled': True, 'package': 'pkg',
            'resource-id': 'pkg:id/search', 'text': 'Search', 'clickable': True,
        })

    def test_element_view_reports_affordances_and_distinguishes_unknown_state(self):
        self.assertEqual(
            [{'class': 'androidx.recyclerview.widget.RecyclerView', 'scrollable': True},
             {'text': 'Submit', 'enabled': False},
             {'text': 'Unknown'}],
            [element_view(e) for e in (
                {'class': 'androidx.recyclerview.widget.RecyclerView', 'scrollable': 'true'},
                {'text': 'Submit', 'enabled': 'false'},
                {'text': 'Unknown', 'enabled': 'partly'},
            )])

    async def test_hierarchy_projection_is_applied_end_to_end(self):
        """The dump's own redundant fields must not reach the host."""
        self.dump.return_value = node_xml(text='Search', **{'content-desc': 'Search',
                                                            'clickable': 'true'})
        observed = json.loads(await self.server.hierarchy('device-b'))
        self.assertEqual(1, len(observed))
        self.assertEqual('Search', observed[0]['content-desc'])
        self.assertIs(True, observed[0]['clickable'])
        for dropped in ('parsed_bounds', 'accessibilityText', 'index',
                        'password', 'focusable', 'scrollable'):
            self.assertNotIn(dropped, observed[0])

    async def test_hierarchy_filters_package_labels_and_bounds(self):
        elements = [
            {'package': 'ime', 'text': 'Keyboard', 'bounds': '[0,100][100,200]'},
            {'package': 'example.app', 'text': 'Search', 'bounds': '[0,0][100,50]'},
            {'package': 'example.app', 'content-desc': 'Send', 'bounds': '[0,50][100,100]'},
            {'package': 'example.app', 'bounds': '[100,50][200,100]'},
        ]
        with patch.object(self.server, 'elements', AsyncMock(return_value=elements)) as read:
            result = json.loads(await self.server.hierarchy(
                'device-b', package='example.app', max_nodes=1, labeled_only=True))
            bounds = json.loads(await self.server.hierarchy(
                'device-b', package='example.app', labeled_only=True, bounds_only=True))
        self.assertEqual([{'bounds': '[0,0][100,50]', 'package': 'example.app',
                           'text': 'Search'}], result)
        self.assertEqual([{'bounds': '[0,0][100,50]'},
                          {'bounds': '[0,50][100,100]'}], bounds)
        self.assertEqual(2, read.await_count)

    async def test_filtered_hierarchy_is_valid_json_bounded_to_four_kib(self):
        elements = [{'package': 'example.app', 'text': '한' * 500,
                     'content-desc': '긴' * 500, 'bounds': '[0,0][100,100]'}] * 64
        with patch.object(self.server, 'elements', AsyncMock(return_value=elements)):
            output = await self.server.hierarchy('device-b', max_nodes=64)
        self.assertLessEqual(len(output.encode('utf-8')), 4096)
        self.assertGreater(len(json.loads(output)), 0)

    async def test_invalid_hierarchy_filters_do_not_access_device(self):
        for options in ({'package': ''}, {'max_nodes': 0}, {'max_nodes': 65}):
            with self.assertRaises(ValueError):
                await self.server.hierarchy('device-b', **options)
        self.mock_get.assert_not_called()

    def test_hierarchy_backend_is_pinned_but_never_overrides_an_explicit_choice(self):
        from moru_mcp import DirectDeviceServer
        self.assertEqual('uiautomator', os.environ['ARTEMIS_HIERARCHY_BACKEND'])
        with patch.dict(os.environ, {'ARTEMIS_HIERARCHY_BACKEND': 'helper',
                                     'ARTEMIS_APP_DIR': self.state.name}):
            DirectDeviceServer()
            self.assertEqual('helper', os.environ['ARTEMIS_HIERARCHY_BACKEND'])

    async def test_swipe_rejects_invalid_duration_before_device_access(self):
        with self.assertRaises(ValueError):
            await self.server.swipe('device-a', 1, 2, 3, 4, duration_ms=0)
        self.mock_get.assert_not_called()

    async def test_factory_contract_drift_fails_before_device_access(self):
        from moru_mcp import _check_controller_factory
        for factory in (None, lambda: None, lambda serial: None, AsyncMock()):
            with self.subTest(factory=type(factory).__name__):
                with self.assertRaisesRegex(RuntimeError, 'Incompatible Artemis'):
                    _check_controller_factory(SimpleNamespace(_get_controller=factory))

    async def test_factory_contract_check_never_opens_device(self):
        from moru_mcp import _check_controller_factory
        def factory(device_serial=None):
            raise AssertionError('device access during startup')
        _check_controller_factory(SimpleNamespace(_get_controller=factory))

    async def test_input_focuses_from_its_own_observation_not_a_second_one(self):
        from artemis.mcp.actuators.adb import find_element_at_coords
        self.assertIs(self.server._hit_test, find_element_at_coords)
        self.dump.return_value = node_xml(**{'class': 'android.widget.EditText',
                                             'focusable': 'true'})
        self.controller.type_text = AsyncMock(return_value=True)
        with patch('moru_mcp.asyncio.sleep', AsyncMock()) as settle:
            self.assertEqual('Success',
                             await self.server.input_text('device-b', 50, 50, 'hello', True))
        self.dump.assert_called_once_with()
        self.shot.assert_not_called()
        self.controller.tap_at.assert_awaited_once_with(x=50, y=50)
        settle.assert_awaited_once()
        self.controller.type_text.assert_awaited_once_with('hello', clear_existing=True)

    async def test_an_already_focused_field_is_typed_into_without_a_tap(self):
        self.dump.return_value = node_xml(**{'class': 'android.widget.EditText',
                                             'focusable': 'true', 'focused': 'true'})
        self.controller.type_text = AsyncMock(return_value=True)
        self.assertEqual('Success', await self.server.input_text('device-b', 50, 50, 'hello'))
        self.controller.tap_at.assert_not_awaited()
        self.controller.type_text.assert_awaited_once_with('hello', clear_existing=False)

    async def test_a_failed_observation_refuses_to_type(self):
        self.dump.side_effect = RuntimeError('dump failed')
        self.controller.type_text = AsyncMock()
        with self.assertRaisesRegex(RuntimeError, 'dump failed'):
            await self.server.input_text('device-b', 12, 34, 'hello')
        self.controller.tap_at.assert_not_awaited()
        self.controller.type_text.assert_not_awaited()

    async def test_elements_updates_screen_size_from_root_parsed_bounds(self):
        status_bar = 'index="0" text="12:00" class="android.widget.TextView" package="com.android.systemui" bounds="[0,0][1080,84]"'
        window = 'index="1" text="Hello" class="android.widget.TextView" package="pkg" bounds="[0,0][2404,1080]"'
        self.dump.return_value = (
            "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>"
            f'<hierarchy rotation="1"><node {status_bar} /><node {window} /></hierarchy>'
        )
        await self.server.elements('device-b')
        self.assertEqual(2404, self.controller.ctx.device.device_width)
        self.assertEqual(1080, self.controller.ctx.device.device_height)

        self.dump.return_value = node_xml(bounds='[0,0][1080,84]')
        await self.server.elements('device-b')
        self.assertEqual(2404, self.controller.ctx.device.device_width)
        self.assertEqual(1080, self.controller.ctx.device.device_height)

    async def test_elements_uses_connected_u2_device_directly_and_recovers_on_error(self):
        raw_device = SimpleNamespace(
            dump_hierarchy=Mock(return_value=node_xml(text='FastPath'))
        )
        self.controller.ctx.ui_adb_client._device = raw_device
        elements = await self.server.elements('device-b')
        raw_device.dump_hierarchy.assert_called_once_with(compressed=True)
        self.dump.assert_not_called()
        self.assertEqual('FastPath', elements[0]['text'])

        raw_device.dump_hierarchy.side_effect = RuntimeError('rpc lost')
        self.dump.return_value = node_xml(text='Recovered')
        recovered = await self.server.elements('device-b')
        self.assertIsNone(self.controller.ctx.ui_adb_client._device)
        self.dump.assert_called_once_with()
        self.assertEqual('Recovered', recovered[0]['text'])

    async def test_screenshot_uses_connected_u2_device_directly_and_recovers_on_error(self):
        from PIL import Image as PilImage

        raw_device = SimpleNamespace(
            screenshot=Mock(return_value=PilImage.new('RGBA', (4, 4), (255, 0, 0, 255)))
        )
        self.controller.ctx.ui_adb_client._device = raw_device
        image = await self.server.screenshot('device-b')
        raw_device.screenshot.assert_called_once_with()
        self.shot.assert_not_called()
        self.assertTrue(image.data.startswith(b'\xff\xd8'))

        raw_device.screenshot.side_effect = RuntimeError('rpc lost')
        fallback = await self.server.screenshot('device-b')
        self.assertIsNone(self.controller.ctx.ui_adb_client._device)
        self.shot.assert_called_once_with()
        self.assertEqual(b'image', fallback.data)

    async def test_failed_typing_does_not_report_success(self):
        self.controller.type_text = AsyncMock(return_value=False)
        with patch('moru_mcp.asyncio.sleep', AsyncMock()):
            with self.assertRaisesRegex(RuntimeError, 'Text input failed'):
                await self.server.input_text('device-b', 12, 34, 'hello')

    async def test_stdio_initialize_and_schema_without_api_keys(self):
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client
        env = {k: v for k, v in os.environ.items()
               if not k.endswith('API_KEY') and not k.startswith(
                   ('ANTHROPIC_', 'OPENAI_', 'GOOGLE_', 'GEMINI_'))}
        env['ARTEMIS_APP_DIR'] = self.state.name
        params = StdioServerParameters(command=sys.executable,
                                       args=['-B', str(ROOT / 'moru_mcp.py')], env=env)
        async with asyncio.timeout(60):
            with tempfile.TemporaryFile(mode='w+') as errors:
                async with stdio_client(params, errlog=errors) as (read, write):
                    async with ClientSession(read, write) as session:
                        initialized = await session.initialize()
                        self.assertEqual('moru-artemis', initialized.serverInfo.name)
                        tools = await session.list_tools()
                        self.assertEqual(17, len(tools.tools))
                        missing = await session.call_tool('tap', {'x': 1, 'y': 2})
                        self.assertTrue(missing.isError)
                        for name in ('tap_element', 'tap_element_and_wait',
                                     'input_element', 'wait_for_element'):
                            arguments = {'selector': {'text': 'Search'}}
                            if name == 'input_element':
                                arguments['text'] = 'fixture'
                            if name == 'tap_element_and_wait':
                                arguments['expected_selector'] = {'text': 'Results'}
                            result = await session.call_tool(name, arguments)
                            self.assertTrue(result.isError, name)
                        invalid = await session.call_tool('wait_for_element', {
                            'device_serial': 'fixture-device', 'selector': {},
                        })
                        self.assertTrue(invalid.isError)
                        unprimed = await session.call_tool(
                            'compare_current', {'device_serial': 'fixture-device'}
                        )
                        self.assertTrue(unprimed.isError)


if __name__ == '__main__':
    unittest.main()
