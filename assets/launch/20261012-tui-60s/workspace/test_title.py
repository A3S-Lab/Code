import unittest
from title import title

class TitleTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(title(""), "Untitled")
    def test_spaces(self):
        self.assertEqual(title("   "), "Untitled")
    def test_tabs(self):
        self.assertEqual(title("\t\n"), "Untitled")
    def test_trim(self):
        self.assertEqual(title("  A3S Code  "), "A3S Code")
    def test_unicode(self):
        self.assertEqual(title("你好"), "你好")

if __name__ == "__main__":
    unittest.main()
