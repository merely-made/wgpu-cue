# SPDX-License-Identifier: MPL-2.0
import unittest
from validate_vulkan import assess

LAYER = 'Insert instance layer "VK_LAYER_KHRONOS_validation"\n'
GOOD = LAYER + "test result: ok. 2 passed; 0 failed; 0 ignored;\n"


class ValidationGate(unittest.TestCase):
    def test_accepts_executed_validation_clean_fixture(self):
        self.assertTrue(assess(GOOD, 0, 2)["passed"])

    def test_rejects_passing_pixels_with_invalid_shader(self):
        output = GOOD + "Validation Error: [ VUID-StandaloneSpirv-None-10684 ]\n"
        self.assertFalse(assess(output, 0, 2)["passed"])

    def test_rejects_sync_hazard(self):
        self.assertFalse(assess(GOOD + "SYNC-HAZARD-WRITE-AFTER-READ", 0, 2)["passed"])

    def test_rejects_absent_layer_zero_filtered_or_failed_tests(self):
        for output, code in [(GOOD.replace(LAYER, ""), 0), (LAYER, 0), (GOOD, 101),
                             (GOOD.replace("2 passed", "0 passed"), 0),
                             (GOOD.replace("0 ignored", "1 ignored"), 0)]:
            with self.subTest(output=output, code=code):
                self.assertFalse(assess(output, code, 2)["passed"])


if __name__ == "__main__":
    unittest.main()
