"""Host MCP protocol, device targeting and failure contracts without a device."""
import asyncio
import base64
import os
from pathlib import Path
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock, patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


class TaoMcpTest(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.state = tempfile.TemporaryDirectory()
        cls.env = patch.dict(os.environ, {'ARTEMIS_APP_DIR': cls.state.name})
        cls.env.start()
        from tao_mcp import DirectDeviceServer
        cls.server = DirectDeviceServer()

    @classmethod
    def tearDownClass(cls):
        cls.env.stop()
        cls.state.cleanup()

    def setUp(self):
        self.controller = Mock()
        self.controller.tap_at = AsyncMock(return_value=SimpleNamespace(error=None))
        self.controller.take_screenshot = AsyncMock(return_value=base64.b64encode(b'image').decode())
        self.controller.get_ui_elements = AsyncMock(return_value=[{'text': 'Hello'}])
        self.controller.press_key = AsyncMock(return_value=False)
        self.get = patch.object(self.server._adb, '_get_controller', return_value=self.controller)
        self.mock_get = self.get.start()
        self.addCleanup(self.get.stop)

    async def test_manifest_requires_device_and_excludes_autonomous_tools(self):
        tools = {t.name: t for t in await self.server.mcp.list_tools()}
        self.assertEqual({'devices', 'screenshot', 'hierarchy', 'tap', 'swipe',
                          'input_text', 'press_key', 'launch_app'}, set(tools))
        for name, tool in tools.items():
            if name != 'devices':
                self.assertIn('device_serial', tool.inputSchema['required'])

    async def test_empty_target_never_initializes_default_device(self):
        with self.assertRaises(ValueError):
            await self.server.tap(' ', 12, 34)
        self.mock_get.assert_not_called()

    async def test_tap_forwards_exact_device_and_pixels(self):
        self.assertEqual('Success', await self.server.tap('device-b', 12, 34))
        self.mock_get.assert_called_once_with(device_serial='device-b')
        self.controller.tap_at.assert_awaited_once_with(x=12, y=34)

    async def test_failed_action_does_not_report_success(self):
        with self.assertRaises(RuntimeError):
            await self.server.press_key('device-a', 'KEYCODE_BACK')

    async def test_screenshot_is_native_mcp_image(self):
        result = await self.server.screenshot('device-a')
        content = result.to_image_content()
        self.assertEqual('image', content.type)
        self.assertEqual('image/jpeg', content.mimeType)
        self.assertEqual(b'image', base64.b64decode(content.data))

    async def test_hierarchy_does_not_need_ocr(self):
        self.assertEqual('[{"text": "Hello"}]', await self.server.hierarchy('device-b'))
        self.controller.get_ui_elements.assert_awaited_once_with()

    async def test_swipe_rejects_invalid_duration_before_device_access(self):
        with self.assertRaises(ValueError):
            await self.server.swipe('device-a', 1, 2, 3, 4, duration_ms=0)
        self.mock_get.assert_not_called()

    async def test_stdio_initialize_and_schema_without_api_keys(self):
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client
        env = {k: v for k, v in os.environ.items()
               if not k.endswith('API_KEY') and not k.startswith(
                   ('ANTHROPIC_', 'OPENAI_', 'GOOGLE_', 'GEMINI_'))}
        env['ARTEMIS_APP_DIR'] = self.state.name
        params = StdioServerParameters(command=sys.executable,
                                       args=['-B', str(ROOT / 'tao_mcp.py')], env=env)
        async with asyncio.timeout(60):
            with tempfile.TemporaryFile(mode='w+') as errors:
                async with stdio_client(params, errlog=errors) as (read, write):
                    async with ClientSession(read, write) as session:
                        initialized = await session.initialize()
                        self.assertEqual('tao-artemis', initialized.serverInfo.name)
                        tools = await session.list_tools()
                        self.assertEqual(8, len(tools.tools))
                        missing = await session.call_tool('tap', {'x': 1, 'y': 2})
                        self.assertTrue(missing.isError)


if __name__ == '__main__':
    unittest.main()
