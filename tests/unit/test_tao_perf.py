"""Bounded graphics summaries and frame-sheet contracts without a device."""

import base64
import io
from pathlib import Path
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from PIL import Image as PILImage

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tao_perf import PerformanceTools, _parse_gfxinfo  # noqa: E402


GFXINFO = """Graphics info for pid 1234 [example.app]
Total frames rendered: 4
Janky frames: 1 (25.00%)
50th percentile: 8ms
90th percentile: 17ms
95th percentile: 18ms
99th percentile: 19ms
Number Missed Vsync: 2
Number Slow UI thread: 1
Number Slow issue draw commands: 3
---PROFILEDATA---
Flags,IntendedVsync,Vsync,HandleInputStart,PerformTraversalsStart,DrawStart,SyncStart,SwapBuffers,FrameCompleted,
0,1000000,1000000,2000000,4000000,6000000,7000000,12000000,18000000,
0,20000000,20000000,21000000,25000000,27000000,29000000,35000000,39000000,
---PROFILEDATA---
"""


class PerformanceTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.device = Mock()
        self.device.serial = 'device-b'
        self.device.shell.return_value = GFXINFO
        self.server = SimpleNamespace(_adb=SimpleNamespace(AdbClient=Mock()))
        self.server._adb.AdbClient.return_value.device_list.return_value = [self.device]
        self.tools = PerformanceTools(self.server)

    def test_parser_uses_summary_and_named_framestats_columns(self):
        result = _parse_gfxinfo(GFXINFO)
        self.assertEqual(4, result['total_frames'])
        self.assertEqual(25.0, result['jank_percent'])
        self.assertEqual({'p50': 8, 'p90': 17, 'p95': 18, 'p99': 19},
                         result['frame_time_ms'])
        self.assertEqual(2, result['missed_vsync'])
        self.assertEqual(1, result['slow_ui_thread'])
        self.assertEqual(3, result['slow_draw_commands'])
        self.assertEqual(2, result['framestats_frames'])
        self.assertEqual({'p50': 2.0, 'p90': 4.0, 'p99': 4.0},
                         result['stage_ms']['input_to_traversal'])
        self.assertEqual({'p50': 5.0, 'p90': 6.0, 'p99': 6.0},
                         result['stage_ms']['render_thread'])

    def test_missing_package_summary_is_an_error(self):
        with self.assertRaisesRegex(RuntimeError, 'frame summary'):
            _parse_gfxinfo('---PROFILEDATA---\nno data\n---PROFILEDATA---')

    async def test_read_returns_only_small_summary(self):
        result = await self.tools.perf_frame_stats('device-b', 'example.app')
        self.server._adb.AdbClient.assert_called_once()
        self.device.shell.assert_called_once_with(
            'dumpsys gfxinfo example.app framestats', timeout=20)
        self.assertEqual('example.app', result['package_name'])
        self.assertNotIn('PROFILEDATA', str(result))
        self.assertLess(len(str(result).encode('utf-8')), 4096)

    async def test_reset_and_summary_without_stages(self):
        reset = await self.tools.perf_frame_stats('device-b', 'example.app', action='reset')
        self.assertEqual({'status': 'reset', 'device_serial': 'device-b',
                          'package_name': 'example.app'}, reset)
        self.device.shell.assert_called_with('dumpsys gfxinfo example.app reset', timeout=20)
        result = await self.tools.perf_frame_stats('device-b', 'example.app', stages=False)
        self.device.shell.assert_called_with('dumpsys gfxinfo example.app', timeout=20)
        self.assertNotIn('stage_ms', result)

    async def test_missing_process_does_not_claim_reset(self):
        self.device.shell.return_value = 'No process found for: example.app'
        with self.assertRaisesRegex(RuntimeError, 'No gfxinfo process'):
            await self.tools.perf_frame_stats('device-b', 'example.app', action='reset')

    async def test_invalid_package_and_action_never_reach_device(self):
        for package in ('', 'example.app;rm', ' example.app', '../app'):
            with self.assertRaises(ValueError):
                await self.tools.perf_frame_stats('device-b', package)
        with self.assertRaises(ValueError):
            await self.tools.perf_frame_stats('device-b', 'example.app', action='other')
        self.server._adb.AdbClient.assert_not_called()

    async def test_recording_returns_bounded_png_and_cleans_temporary_device_file(self):
        def fake_pull(_remote, local):
            Path(local).write_bytes(b'video fixture')

        def fake_ffmpeg(args, **_kwargs):
            for number in range(1, 4):
                PILImage.new('RGB', (1080, 2400), 'red').save(
                    args[-1].replace('%03d', f'{number:03d}'))

        self.device.sync.pull.side_effect = fake_pull
        with patch('tao_perf.subprocess.run', side_effect=fake_ffmpeg) as ffmpeg:
            result = await self.tools.record_frames('device-b', 2, 2, [0, 0, 400, 800])
        image = result.to_image_content()
        self.assertEqual('image/png', image.mimeType)
        with PILImage.open(io.BytesIO(base64.b64decode(image.data))) as sheet:
            self.assertLessEqual(sheet.width, 1024)
            self.assertLessEqual(sheet.height, 1024)
        self.assertEqual(1, ffmpeg.call_count)
        self.assertEqual(2, self.device.shell.call_count)
        self.assertTrue(self.device.shell.call_args.args[0].startswith('rm -f /sdcard/tao-artemis-'))

    async def test_invalid_recording_limits_never_reach_device(self):
        for settings in ((0, 1, None), (10, 4, None), (1, 5, None),
                         (1, 1, [0, 0, 0, 10])):
            with self.assertRaises(ValueError):
                await self.tools.record_frames('device-b', *settings)
        self.server._adb.AdbClient.assert_not_called()

    async def test_unavailable_display_fails_before_pull_and_cleans_remote_file(self):
        self.device.shell.side_effect = ['ERROR: INVALID_LAYER_STACK', '']
        with self.assertRaisesRegex(RuntimeError, 'current display state'):
            await self.tools.record_frames('device-b', 1, 1)
        self.device.sync.pull.assert_not_called()
        self.assertTrue(self.device.shell.call_args.args[0].startswith('rm -f /sdcard/tao-artemis-'))


if __name__ == '__main__':
    unittest.main()
