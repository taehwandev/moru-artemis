"""A capture that succeeded must not be discarded because of a warning line.

Devices with more than one display print `[Warning] Multiple displays...` to
stdout before the PNG. Opening that stream fails, so the screenshot was thrown
away and the caller silently fell back to UIAutomator2 on every single capture.
"""

import unittest

from artemis.clients.ui_automator_client import _PNG_SIGNATURE, _png_payload


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


if __name__ == "__main__":
    unittest.main()
