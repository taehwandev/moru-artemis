"""A capture that succeeded must not be discarded because of a warning line.

Devices with more than one display print `[Warning] Multiple displays...` to
stdout before the PNG. Opening that stream fails, so the screenshot was thrown
away and the caller silently fell back to UIAutomator2 on every single capture.
"""

import subprocess
import unittest
from unittest.mock import Mock, patch

from artemis.clients.ui_automator_client import (
    _PNG_SIGNATURE,
    UIAutomatorClient,
    _png_payload,
)


class PngPayloadTests(unittest.TestCase):
    def test_a_warning_line_before_the_image_is_dropped(self):
        image = _PNG_SIGNATURE + b"the rest of the image"
        captured = b"[Warning] Multiple displays were found, use --display\n" + image
        self.assertEqual(image, _png_payload(captured))

    def test_a_clean_capture_is_returned_unchanged(self):
        image = _PNG_SIGNATURE + b"the rest of the image"
        self.assertIs(image, _png_payload(image))

    def test_output_with_no_signature_is_left_for_the_caller_to_fail_on(self):
        """Without this the helper would mask a genuinely broken capture."""
        broken = b"error: device offline\n"
        self.assertIs(broken, _png_payload(broken))

    def test_only_the_first_signature_starts_the_image(self):
        image = _PNG_SIGNATURE + b"body" + _PNG_SIGNATURE + b"more"
        self.assertEqual(image, _png_payload(b"[Warning] two displays\n" + image))


class CaptureOrderTests(unittest.TestCase):
    """UIAutomator2 is several times cheaper and was measured to see the same
    overlays, so it leads and adb screencap only covers its failure."""

    def setUp(self):
        self.client = UIAutomatorClient(device_id="fixture-serial")
        self.device = Mock()
        self.connect = patch.object(
            self.client, "_ensure_connected", return_value=self.device
        )
        self.connect.start()
        self.addCleanup(self.connect.stop)

    def test_uiautomator2_serves_the_capture_without_shelling_out(self):
        with patch("artemis.clients.ui_automator_client.subprocess.run") as run:
            self.assertIs(self.device.screenshot.return_value, self.client.get_screenshot())
        run.assert_not_called()

    def test_adb_covers_a_uiautomator2_failure_and_still_skips_the_warning(self):
        self.device.screenshot.side_effect = RuntimeError("uiautomator2 is down")
        image = _PNG_SIGNATURE + b"body"
        captured = b"[Warning] Multiple displays\n" + image
        with patch("artemis.clients.ui_automator_client.subprocess.run") as run, \
                patch("artemis.clients.ui_automator_client.Image.open") as opened:
            run.return_value = Mock(stdout=captured)
            self.client.get_screenshot()
        self.assertEqual(image, opened.call_args.args[0].getvalue())

    def test_both_paths_failing_returns_none_rather_than_a_broken_image(self):
        self.device.screenshot.side_effect = RuntimeError("uiautomator2 is down")
        with patch("artemis.clients.ui_automator_client.subprocess.run",
                   side_effect=subprocess.SubprocessError("adb is down")):
            self.assertIsNone(self.client.get_screenshot())


if __name__ == "__main__":
    unittest.main()
