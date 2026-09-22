"""Static baseline/current comparison contracts without a connected device."""

import base64
from io import BytesIO
from pathlib import Path
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tao_comparison import ComparisonTools


def encoded_image(color, size=(4, 4)):
    output = BytesIO()
    Image.new('RGB', size, color).save(output, format='JPEG', quality=100, subsampling=0)
    return base64.b64encode(output.getvalue()).decode('ascii')


def screen_image(color, size=(4, 4)):
    """One capture as the adapter's own screenshot tool returns it."""
    return SimpleNamespace(data=base64.b64decode(encoded_image(color, size), validate=True))


class ComparisonToolsTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        # Comparison wants the screenshot and the hierarchy together, and takes
        # each from the adapter's own read rather than upstream's combined one.
        self.screenshot = AsyncMock(return_value=screen_image('black'))
        self.elements = AsyncMock(return_value=())
        self.server = SimpleNamespace(screenshot=self.screenshot, elements=self.elements,
                                      _device=Mock())
        self.tools = ComparisonTools(self.server)

    def observe(self, image, elements):
        self.screenshot.return_value = SimpleNamespace(
            data=base64.b64decode(image, validate=True))
        self.elements.return_value = elements

    async def test_baseline_is_memory_only_and_returns_the_observed_image(self):
        self.observe(encoded_image('black'), [{'text': 'Before'}])

        result = await self.tools.capture_baseline('device-a')

        self.assertEqual('baseline_captured', result.structuredContent['status'])
        self.assertEqual('memory_only', result.structuredContent['storage'])
        self.assertEqual([4, 4], result.structuredContent['dimensions'])
        self.assertEqual('image', result.content[1].type)

    async def test_capture_pays_for_one_screenshot_and_one_element_read(self):
        """The adapter's own reads, so no combined read and no settling pause."""
        self.observe(encoded_image('black'), [{'text': 'Before'}])

        await self.tools.capture_baseline('device-a')

        self.screenshot.assert_awaited_once_with('device-a')
        self.elements.assert_awaited_once_with('device-a')
        self.server._device.assert_not_called()

    async def test_compare_requires_a_baseline_without_accessing_a_device(self):
        with self.assertRaisesRegex(ValueError, 'Capture a baseline'):
            await self.tools.compare_current('device-a')
        self.screenshot.assert_not_awaited()
        self.elements.assert_not_awaited()

    async def test_compare_refuses_another_device_before_observation(self):
        self.observe(encoded_image('black'), [])
        await self.tools.capture_baseline('device-a')
        self.screenshot.reset_mock()
        self.elements.reset_mock()

        with self.assertRaisesRegex(ValueError, 'different device_serial'):
            await self.tools.compare_current('device-b')
        self.screenshot.assert_not_awaited()
        self.elements.assert_not_awaited()

    async def test_compare_reports_pixel_and_compact_semantic_deltas_without_a_verdict(self):
        self.screenshot.side_effect = [screen_image('black'), screen_image('white')]
        self.elements.side_effect = [
            [{'text': 'Before', 'bounds': '[0,0][2,2]', 'enabled': 'true'}],
            [{'text': 'After', 'bounds': '[0,0][2,2]', 'enabled': 'true'}],
        ]
        await self.tools.capture_baseline('device-a')

        result = await self.tools.compare_current('device-a')
        summary = result.structuredContent

        self.assertEqual('comparison_observed', summary['status'])
        self.assertEqual(1.0, summary['pixel_delta']['changed_pixel_ratio'])
        self.assertEqual(1, summary['semantic_delta']['added_count'])
        self.assertEqual(1, summary['semantic_delta']['removed_count'])
        self.assertIn('host_must_compare', summary['verdict'])
        self.assertEqual('image', result.content[1].type)

    async def test_compare_rejects_changed_display_dimensions(self):
        self.screenshot.side_effect = [screen_image('black', (4, 4)),
                                       screen_image('black', (4, 5))]
        await self.tools.capture_baseline('device-a')

        with self.assertRaisesRegex(ValueError, 'dimensions changed'):
            await self.tools.compare_current('device-a')


if __name__ == '__main__':
    unittest.main()
