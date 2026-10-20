import re
import unittest

import helpers
from helpers import KEY_CTRL_C, KEY_DOWN, KEY_ENTER, KEY_ESC, KEY_SPACE, KEY_UP, run_in_pty

FORBIDDEN = ("\x1b[?1049", "\x1b[?1047", "\x1b[?47h", "\x1b[2J", "\x1b[3J", "\x1bc")

SELECT = """
from cubi import ui
from cubi.ui import Item
print("VORHER")
result = ui.select("App wählen", [Item(f"app-{i}", i, "hint") for i in range(40)])
print("ERGEBNIS", result)
"""

MULTI = """
from cubi import ui
from cubi.ui import Item
print("VORHER")
result = ui.multiselect("Apps wählen", [Item(f"app-{i}", f"app-{i}") for i in range(40)], min_count=1)
print("ERGEBNIS", result)
"""

MODES = """
import sys, termios
from cubi import ui
from cubi.ui import Item
try:
    ui.select("App wählen", [Item("a", 1), Item("b", 2)])
finally:
    flags = termios.tcgetattr(sys.stdin.fileno())[3]
    print("MODUS", bool(flags & termios.ICANON), bool(flags & termios.ECHO), bool(flags & termios.ISIG))
"""

ENTRY = f"""
import runpy, sys
sys.argv = ["cubi"]
runpy.run_path({str(helpers.SRC.parent / "bin" / "cubi")!r}, run_name="__main__")
"""

CONFIRM = """
from cubi import ui
print("ERGEBNIS", ui.confirm("Ausführen?"))
"""

PROMPT = """
from cubi import ui
print("ERGEBNIS", repr(ui.prompt("Ziel-Branch", "main")))
"""

TABLE = """
from rich.text import Text
from cubi import ui
from cubi.ui import Row, TableData
rows = [Row([Text(f"app-{i}"), Text("Synced")], i) for i in range(30)]
key, value = ui.table_select("ArgoCD", ["App", "Sync"], lambda: TableData(rows, Text("30 Apps\\nSync 30")), hotkeys={"f": "Filter"})
print("ERGEBNIS", key, value)
"""


def clean(text):
    return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", text)


class InlineBehaviourTest(unittest.TestCase):
    def assertInline(self, output):
        for sequence in FORBIDDEN:
            self.assertNotIn(sequence, output)

    def test_select_returns_value_and_collapses_to_summary(self):
        output, code = run_in_pty(SELECT, [KEY_DOWN, KEY_DOWN, KEY_ENTER])
        self.assertEqual(code, 0)
        self.assertInline(output)
        self.assertIn("ERGEBNIS 2", output)
        self.assertIn("? App wählen › app-2", clean(output))
        self.assertTrue(output.index("VORHER") < output.index("App wählen"))
        self.assertIn("\x1b[?25l", output)
        self.assertTrue(output.rstrip().endswith("ERGEBNIS 2"))

    def test_select_scrolls_inside_fixed_window(self):
        output, code = run_in_pty(SELECT, [KEY_UP, KEY_ENTER])
        self.assertEqual(code, 0)
        self.assertIn("ERGEBNIS 39", output)
        redraws = re.findall(r"\x1b\[(\d+)A", output)
        self.assertTrue(redraws)
        self.assertEqual(len(set(redraws)), 1)
        self.assertLessEqual(int(redraws[0]), 20)

    def test_escape_cancels_cleanly(self):
        output, code = run_in_pty(SELECT, [KEY_ESC])
        self.assertEqual(code, 0)
        self.assertInline(output)
        self.assertIn("ERGEBNIS None", output)
        self.assertIn("abgebrochen", clean(output))
        self.assertTrue(output.index("\x1b[?25h") < output.index("ERGEBNIS"))

    def test_ctrl_c_restores_terminal_and_cursor(self):
        output, _ = run_in_pty(MODES, [KEY_CTRL_C])
        self.assertIn("MODUS True True True", output)
        self.assertIn("\x1b[?25h", output)
        self.assertIn("abgebrochen", clean(output))
        self.assertInline(output)

    def test_ctrl_c_in_real_entry_point_exits_quietly(self):
        output, code = run_in_pty(ENTRY, [KEY_CTRL_C])
        self.assertEqual(code, 130)
        self.assertNotIn("Traceback", output)
        self.assertIn("Bereich wählen", clean(output))
        self.assertIn("abgebrochen", clean(output))
        self.assertTrue(output.rstrip().endswith("abgebrochen\x1b[0m") or "\x1b[?25h" in output)
        self.assertInline(output)

    def test_real_entry_point_navigates_and_goes_back(self):
        output, code = run_in_pty(ENTRY, [KEY_DOWN, KEY_ESC])
        self.assertEqual(code, 0)
        self.assertInline(output)
        self.assertIn("cubi · Bereich wählen", clean(output))
        self.assertIn("Auto-Sync", clean(output))
        self.assertIn("Feature-Testing", clean(output))

    def test_terminal_mode_restored_after_normal_exit(self):
        output, _ = run_in_pty(MODES, [KEY_ENTER])
        self.assertIn("MODUS True True True", output)

    def test_multiselect_space_toggles_and_enter_confirms(self):
        output, code = run_in_pty(MULTI, [KEY_SPACE, KEY_DOWN, KEY_DOWN, KEY_SPACE, KEY_ENTER])
        self.assertEqual(code, 0)
        self.assertInline(output)
        self.assertIn("['app-0', 'app-2']", output)
        self.assertIn("[x]", output)
        self.assertIn("? Apps wählen › 2 gewählt: app-0, app-2", clean(output))

    def test_multiselect_a_toggles_all(self):
        output, _ = run_in_pty(MULTI, [b"a", KEY_ENTER])
        self.assertIn("app-39", output.split("ERGEBNIS")[-1])
        self.assertIn("› 40 gewählt:", clean(output))

    def test_multiselect_requires_selection_when_min_count_set(self):
        output, _ = run_in_pty(MULTI, [KEY_ENTER, KEY_SPACE, KEY_ENTER])
        self.assertIn("Mindestens 1 Eintrag wählen", clean(output))
        self.assertIn("ERGEBNIS ['app-0']", output)

    def test_table_hotkey_erases_region_and_returns_key(self):
        output, _ = run_in_pty(TABLE, [KEY_DOWN, b"f"])
        self.assertInline(output)
        self.assertIn("ERGEBNIS f 1", output)
        self.assertIn("30 Apps", clean(output))

    def test_table_enter_leaves_one_line_summary(self):
        output, _ = run_in_pty(TABLE, [KEY_DOWN, KEY_ENTER])
        self.assertIn("ERGEBNIS enter 1", output)
        self.assertIn("? ArgoCD › app-1", clean(output))

    def test_confirm_default_is_no(self):
        output, _ = run_in_pty(CONFIRM, [KEY_ENTER])
        self.assertIn("(y/N)", clean(output))
        self.assertIn("ERGEBNIS False", output)

    def test_confirm_yes(self):
        output, _ = run_in_pty(CONFIRM, [b"y"])
        self.assertIn("ERGEBNIS True", output)
        self.assertIn("? Ausführen? › Ja", clean(output))

    def test_prompt_edits_default_value(self):
        output, _ = run_in_pty(PROMPT, [b"\x7f", b"\x7f", b"\x7f", b"\x7f", b"d", b"e", b"v", KEY_ENTER])
        self.assertIn("ERGEBNIS 'dev'", output)

    def test_prompt_escape_returns_none(self):
        output, _ = run_in_pty(PROMPT, [KEY_ESC])
        self.assertIn("ERGEBNIS None", output)

    def test_small_terminal_keeps_region_inside_screen(self):
        output, code = run_in_pty(SELECT, [KEY_DOWN, KEY_ENTER], rows=10, cols=40)
        self.assertEqual(code, 0)
        self.assertIn("ERGEBNIS 1", output)
        for height in re.findall(r"\x1b\[(\d+)A", output):
            self.assertLess(int(height), 10)


if __name__ == "__main__":
    unittest.main()
