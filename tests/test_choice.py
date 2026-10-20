import unittest

import helpers
from rich.text import Text

from cubi.choice import Item, ListPrompt, Row, TableData, Viewport, fit_widths, items_to_rows
from cubi.errors import CubiError


def prompt_for(count=5, **kwargs):
    rows = [Row([Text(f"item-{index}")], index) for index in range(count)]
    return ListPrompt("Frage", rows, **kwargs)


class ViewportTest(unittest.TestCase):
    def test_wraps_around(self):
        view = Viewport(3, 3)
        view.move("up")
        self.assertEqual(view.cursor, 2)
        view.move("down")
        self.assertEqual(view.cursor, 0)

    def test_window_scrolls_with_cursor(self):
        view = Viewport(20, 5)
        for _ in range(7):
            view.move("down")
        self.assertEqual(view.cursor, 7)
        self.assertEqual(list(view.window()), [3, 4, 5, 6, 7])
        view.move("end")
        self.assertEqual(list(view.window()), [15, 16, 17, 18, 19])
        view.move("home")
        self.assertEqual(list(view.window()), [0, 1, 2, 3, 4])

    def test_page_keys_clamp(self):
        view = Viewport(12, 5)
        view.move("pagedown")
        view.move("pagedown")
        view.move("pagedown")
        self.assertEqual(view.cursor, 11)
        view.move("pageup")
        self.assertEqual(view.cursor, 6)

    def test_wrap_up_shows_last_entries(self):
        view = Viewport(20, 5)
        view.move("up")
        self.assertEqual(view.cursor, 19)
        self.assertEqual(list(view.window()), [15, 16, 17, 18, 19])

    def test_empty_list_ignores_movement(self):
        view = Viewport(0, 5)
        view.move("down")
        self.assertEqual(view.cursor, 0)

    def test_resize_keeps_cursor_visible(self):
        view = Viewport(20, 10)
        view.move("end")
        view.resize(20, 4)
        self.assertIn(view.cursor, list(view.window()))
        view.resize(3, 4)
        self.assertEqual(view.cursor, 2)


class SingleSelectTest(unittest.TestCase):
    def test_enter_returns_value_under_cursor(self):
        prompt = prompt_for()
        prompt.handle("down")
        prompt.handle("down")
        self.assertEqual(prompt.handle("enter"), ("enter", 2))

    def test_escape_goes_back(self):
        self.assertEqual(prompt_for().handle("esc"), ("back", None))

    def test_ctrl_c_raises_keyboard_interrupt(self):
        with self.assertRaises(KeyboardInterrupt):
            prompt_for().handle("ctrl-c")

    def test_space_does_nothing_in_single_mode(self):
        prompt = prompt_for()
        self.assertIsNone(prompt.handle(" "))
        self.assertEqual(prompt.checked, [])

    def test_initial_focuses_value(self):
        prompt = prompt_for(initial=3)
        self.assertEqual(prompt.current_value(), 3)

    def test_hotkey_returns_key_and_current_value(self):
        prompt = prompt_for(hotkeys={"f": "Filter"})
        prompt.handle("down")
        self.assertEqual(prompt.handle("f"), ("f", 1))

    def test_answer_uses_row_label(self):
        rows = [Row([Text("ns"), Text("pod")], ("ns", "pod"), "pod")]
        prompt = ListPrompt("Pods", rows)
        self.assertEqual(prompt.answer(("ns", "pod")), "pod")

    def test_answer_falls_back_to_first_cell(self):
        self.assertEqual(prompt_for().answer(1), "item-1")

    def test_items_to_rows_adds_hint_column_only_when_needed(self):
        self.assertEqual(len(items_to_rows([Item("a", 1)])[0].cells), 1)
        self.assertEqual(len(items_to_rows([Item("a", 1, "x")])[0].cells), 2)


class MultiSelectTest(unittest.TestCase):
    def test_space_toggles_entry(self):
        prompt = prompt_for(multi=True)
        prompt.handle(" ")
        prompt.handle("down")
        prompt.handle(" ")
        prompt.handle("up")
        prompt.handle(" ")
        self.assertEqual(prompt.checked_values(), [1])

    def test_enter_returns_checked_in_row_order(self):
        prompt = prompt_for(multi=True)
        prompt.handle("end")
        prompt.handle(" ")
        prompt.handle("home")
        prompt.handle(" ")
        self.assertEqual(prompt.handle("enter"), ("enter", [0, 4]))

    def test_a_toggles_all(self):
        prompt = prompt_for(multi=True)
        prompt.handle("a")
        self.assertEqual(prompt.checked_values(), [0, 1, 2, 3, 4])
        prompt.handle("a")
        self.assertEqual(prompt.checked_values(), [])

    def test_a_selects_remaining_when_partially_checked(self):
        prompt = prompt_for(multi=True)
        prompt.handle(" ")
        prompt.handle("a")
        self.assertEqual(len(prompt.checked_values()), 5)

    def test_min_count_blocks_enter(self):
        prompt = prompt_for(multi=True, min_count=1)
        self.assertIsNone(prompt.handle("enter"))
        self.assertIn("Mindestens", prompt.message)
        prompt.handle(" ")
        self.assertEqual(prompt.handle("enter"), ("enter", [0]))

    def test_empty_selection_allowed_without_min_count(self):
        self.assertEqual(prompt_for(multi=True).handle("enter"), ("enter", []))

    def test_checked_rows_are_marked(self):
        prompt = prompt_for(multi=True, checked=[1])
        text = [line.plain for line in prompt.lines(60)]
        self.assertTrue(any("[x] item-1" in line for line in text))
        self.assertTrue(any("[ ] item-0" in line for line in text))

    def test_answer_lists_labels_and_count(self):
        prompt = prompt_for(multi=True)
        self.assertEqual(prompt.answer([0, 2]), "2 gewählt: item-0, item-2")
        self.assertEqual(prompt.answer([]), "keine")

    def test_reserved_hotkeys_rejected(self):
        with self.assertRaises(ValueError):
            prompt_for(multi=True, hotkeys={"a": "Alle"})

    def test_hotkey_in_multi_mode_returns_checked(self):
        prompt = prompt_for(multi=True, hotkeys={"f": "Filter"})
        prompt.handle(" ")
        self.assertEqual(prompt.handle("f"), ("f", [0]))


class RenderTest(unittest.TestCase):
    def test_window_is_fixed_and_footer_shows_position(self):
        prompt = prompt_for(count=60)
        lines = prompt.lines(80)
        self.assertEqual(len(lines), 1 + prompt.view.size + 1)
        for _ in range(30):
            prompt.handle("down")
        scrolled = prompt.lines(80)
        self.assertEqual(len(scrolled), len(lines))
        self.assertIn("31/60", scrolled[-1].plain)

    def test_no_position_counter_when_everything_fits(self):
        prompt = prompt_for(count=4)
        self.assertNotIn("1/4", prompt.lines(80)[-1].plain)

    def test_question_stays_first_line(self):
        prompt = prompt_for(count=60)
        for _ in range(40):
            prompt.handle("down")
        self.assertEqual(prompt.lines(80)[0].plain, "? Frage")

    def test_cursor_marker_on_current_row(self):
        prompt = prompt_for()
        prompt.handle("down")
        marked = [line.plain for line in prompt.lines(60) if line.plain.startswith("❯")]
        self.assertEqual(marked, ["❯ item-1"])

    def test_lines_never_exceed_width(self):
        rows = [Row([Text("x" * 80), Text("y" * 80)], index) for index in range(3)]
        prompt = ListPrompt("Frage", rows, columns=["Eins", "Zwei"])
        for line in prompt.lines(50):
            self.assertLessEqual(len(line.plain), 50)

    def test_every_line_is_a_single_physical_line(self):
        summary = Text("a\nb\nc")
        prompt = ListPrompt("Frage", [Row([Text("x")], 1)], summary=summary)
        for line in prompt.lines(60):
            self.assertNotIn("\n", line.plain)

    def test_footer_wraps_instead_of_truncating(self):
        prompt = prompt_for(hotkeys={"f": "Filter", "c": "Kontext", "m": "Mehrere syncen", "s": "Auto-Sync", "b": "Branch"}, loader=lambda: TableData(), interval=10)
        footer = prompt.footer(60)
        self.assertGreater(len(footer), 1)
        joined = " ".join(line.plain for line in footer)
        for part in ("Filter", "Kontext", "Auto-Sync", "Branch", "Esc Abbrechen"):
            self.assertIn(part, joined)

    def test_header_shown_for_columns(self):
        prompt = ListPrompt("Frage", [Row([Text("a"), Text("b")], 1)], columns=["Name", "Wert"])
        self.assertIn("Name", prompt.lines(60)[1].plain)

    def test_fit_widths_shrinks_widest_first(self):
        widths = fit_widths([40, 10, 30], 50)
        self.assertLessEqual(sum(widths), 50)
        self.assertEqual(widths[1], 10)


class LoaderTest(unittest.TestCase):
    def test_loader_error_is_shown(self):
        def loader():
            raise CubiError("kaputt")

        prompt = ListPrompt("Frage", loader=loader)
        prompt.reload()
        self.assertEqual(prompt.error, "kaputt")
        self.assertTrue(any("kaputt" in line.plain for line in prompt.lines(60)))
        self.assertIsNone(prompt.handle("enter"))

    def test_reload_keeps_cursor_value_and_checks(self):
        state = {"names": ["a", "b", "c"]}

        def loader():
            return TableData([Row([Text(name)], name) for name in state["names"]])

        prompt = ListPrompt("Frage", columns=["N"], loader=loader, multi=True)
        prompt.reload()
        prompt.handle("down")
        prompt.handle(" ")
        state["names"] = ["x", "a", "b"]
        prompt.reload()
        self.assertEqual(prompt.current_value(), "b")
        self.assertEqual(prompt.checked_values(), ["b"])

    def test_preselection_from_first_load(self):
        loader = lambda: TableData([Row([Text(name)], name) for name in "abc"], None, ["b"])
        prompt = ListPrompt("Frage", columns=["N"], loader=loader, multi=True)
        prompt.reload()
        self.assertEqual(prompt.checked_values(), ["b"])
        prompt.handle(" ")
        prompt.reload()
        self.assertEqual(prompt.checked_values(), ["a", "b"])

    def test_initial_applies_after_first_load(self):
        loader = lambda: TableData([Row([Text(name)], name) for name in "abc"])
        prompt = ListPrompt("Frage", columns=["N"], loader=loader, initial="c")
        prompt.reload()
        self.assertEqual(prompt.current_value(), "c")

    def test_checked_passed_in_survives_empty_start(self):
        loader = lambda: TableData([Row([Text(name)], name) for name in "abc"])
        prompt = ListPrompt("Frage", columns=["N"], loader=loader, multi=True, checked=["c"])
        prompt.reload()
        self.assertEqual(prompt.checked_values(), ["c"])

    def test_r_requests_reload_only_with_loader(self):
        self.assertIsNone(prompt_for().handle("r"))
        prompt = prompt_for(loader=lambda: TableData())
        self.assertEqual(prompt.handle("r"), ("reload", None))


if __name__ == "__main__":
    unittest.main()
