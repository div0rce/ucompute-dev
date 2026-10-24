"""Offline checks of gen_notices' licence policy: python3 -m unittest discover -s scripts -p 'test_*.py'"""
import unittest

from gen_notices import allowed, satisfied

ALLOW = {"MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "Unicode-3.0"}


class Policy(unittest.TestCase):
    def test_expressions(self):
        self.assertTrue(satisfied("MIT", ALLOW))
        self.assertTrue(satisfied("MIT OR Apache-2.0", ALLOW))
        self.assertTrue(satisfied("GPL-3.0-only OR MIT", ALLOW))
        self.assertTrue(satisfied("MIT/Apache-2.0", ALLOW))
        self.assertTrue(satisfied("Apache-2.0 WITH LLVM-exception OR MIT", ALLOW))
        self.assertTrue(satisfied("(MIT OR Apache-2.0) AND Unicode-3.0", ALLOW))

    def test_copyleft_and_unknown_refused(self):
        self.assertFalse(satisfied("GPL-3.0-only", ALLOW))
        self.assertFalse(satisfied("AGPL-3.0-or-later", ALLOW))
        self.assertFalse(satisfied("MIT AND GPL-2.0-only", ALLOW))
        self.assertFalse(satisfied("(MIT OR Apache-2.0) AND LGPL-2.1-only", ALLOW))
        self.assertFalse(satisfied("GPL-2.0-only WITH Classpath-exception-2.0", ALLOW))

    def test_expressions_not_fully_understood_are_refused(self):
        self.assertFalse(satisfied("MIT and GPL-3.0-only", ALLOW))
        self.assertFalse(satisfied("MIT GPL-3.0-only", ALLOW))
        self.assertFalse(satisfied("MIT OR", ALLOW))
        self.assertFalse(satisfied("(MIT", ALLOW))

    def test_deny_toml_has_no_copyleft(self):
        self.assertFalse({a for a in allowed() if "GPL" in a})


if __name__ == "__main__":
    unittest.main()
