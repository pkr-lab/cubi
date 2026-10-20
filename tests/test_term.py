import unittest

import helpers
from rich.text import Text

from cubi import term


class TokenizeTest(unittest.TestCase):
    def test_arrow_keys(self):
        self.assertEqual(term.tokenize(b"\x1b[A\x1b[B\x1bOA"), ["up", "down", "up"])

    def test_lone_escape_is_esc(self):
        self.assertEqual(term.tokenize(b"\x1b"), ["esc"])

    def test_ctrl_c_and_enter(self):
        self.assertEqual(term.tokenize(b"\x03\r\n"), ["ctrl-c", "enter", "enter"])

    def test_space_and_letters(self):
        self.assertEqual(term.tokenize(b" ax"), [" ", "a", "x"])

    def test_unknown_escape_sequence_is_swallowed(self):
        self.assertEqual(term.tokenize(b"\x1b[1;5Ca"), ["unknown", "a"])

    def test_utf8_character(self):
        self.assertEqual(term.tokenize("ä".encode()), ["ä"])

    def test_page_keys(self):
        self.assertEqual(term.tokenize(b"\x1b[5~\x1b[6~"), ["pageup", "pagedown"])


class RenderTest(unittest.TestCase):
    def test_render_counts_physical_lines(self):
        data, count = term.render_ansi([Text("a"), Text("b\nc")], 20)
        self.assertEqual(count, 3)
        self.assertEqual(data.count("\n"), 3)

    def test_long_line_is_cut_not_wrapped(self):
        data, count = term.render_ansi([Text("x" * 100)], 20)
        self.assertEqual(count, 1)
        self.assertEqual(data.count("\n"), 1)


if __name__ == "__main__":
    unittest.main()
