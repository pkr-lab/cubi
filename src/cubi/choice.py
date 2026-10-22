from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Sequence

from rich.cells import cell_len
from rich.text import Text

from . import term
from .errors import CubiError

GAP = 2
MIN_COLUMN = 14
MAX_WINDOW = 15
MIN_WINDOW = 3
NAV_KEYS = ("up", "down", "pageup", "pagedown", "home", "end")


@dataclass
class Item:
    label: str
    value: Any
    hint: str = ""


@dataclass
class Row:
    cells: list
    value: Any = None
    label: str | None = None


@dataclass
class TableData:
    rows: list = field(default_factory=list)
    summary: Text | None = None
    checked: list | None = None


class Viewport:
    def __init__(self, count: int = 0, size: int = MIN_WINDOW) -> None:
        self.count = count
        self.size = max(1, size)
        self.cursor = 0
        self.offset = 0

    def resize(self, count: int, size: int) -> None:
        self.count = count
        self.size = max(1, size)
        self.cursor = min(self.cursor, max(0, count - 1))
        self.offset = self.fit()

    def fit(self) -> int:
        offset = self.offset
        if self.cursor < offset:
            offset = self.cursor
        elif self.cursor >= offset + self.size:
            offset = self.cursor - self.size + 1
        return max(0, min(offset, max(0, self.count - self.size)))

    def move(self, key: str) -> None:
        if not self.count:
            return
        if key == "up":
            self.cursor = (self.cursor - 1) % self.count
        elif key == "down":
            self.cursor = (self.cursor + 1) % self.count
        elif key == "pageup":
            self.cursor = max(0, self.cursor - self.size)
        elif key == "pagedown":
            self.cursor = min(self.count - 1, self.cursor + self.size)
        elif key == "home":
            self.cursor = 0
        elif key == "end":
            self.cursor = self.count - 1
        self.offset = self.fit()

    def window(self) -> range:
        return range(self.offset, min(self.count, self.offset + self.size))


def cell_text(cell: Any) -> Text:
    return cell.copy() if isinstance(cell, Text) else Text(str(cell))


def cell_width(cell: Any) -> int:
    return cell_len(cell.plain if isinstance(cell, Text) else str(cell))


def fit_widths(naturals: list[int], available: int) -> list[int]:
    if sum(naturals) <= available:
        return list(naturals)
    for floor in (MIN_COLUMN, 6, 1):
        floors = [min(natural, floor) for natural in naturals]
        if sum(floors) > available:
            continue
        widths = list(naturals)
        while sum(widths) > available:
            index = max((position for position in range(len(widths)) if widths[position] > floors[position]), key=lambda position: widths[position])
            widths[index] -= 1
        return widths
    return [1] * len(naturals)


def fit_cell(cell: Any, width: int, justify: str) -> Text:
    text = cell_text(cell)
    text.truncate(width, overflow="ellipsis", pad=justify != "right")
    if justify == "right":
        text.align("right", width)
    return text


def window_size(count: int, chrome: int) -> int:
    _, rows = term.terminal_size()
    return max(1, min(count, MAX_WINDOW, max(MIN_WINDOW, rows - chrome - 2)))


def answer_line(question: str, answer: str) -> Text:
    line = Text()
    line.append("? ", style="bold green")
    line.append(question, style="bold")
    line.append(" › ", style="dim")
    line.append(answer, style="cyan")
    return line


def cancel_line(question: str) -> Text:
    line = Text()
    line.append("✖ ", style="red")
    line.append(question, style="bold")
    line.append(" · abgebrochen", style="dim")
    return line


def question_line(question: str) -> Text:
    line = Text()
    line.append("? ", style="bold green")
    line.append(question, style="bold")
    return line


class ListPrompt:
    def __init__(
        self,
        question: str,
        rows: Sequence[Row] = (),
        *,
        columns: Sequence | None = None,
        multi: bool = False,
        checked: Sequence = (),
        subtitle: str | Text | None = None,
        summary: Text | None = None,
        hotkeys: dict[str, str] | None = None,
        loader: Callable[[], TableData] | None = None,
        interval: float | None = None,
        initial: Any = None,
        min_count: int = 0,
        empty: str = "Keine Einträge",
    ) -> None:
        self.question = question
        self.columns = [column if isinstance(column, tuple) else (column, "left") for column in columns] if columns else None
        self.multi = multi
        self.checked: list = list(checked)
        self.subtitle = Text(subtitle, style="dim") if isinstance(subtitle, str) else subtitle
        self.hotkeys = dict(hotkeys or {})
        self.loader = loader
        self.interval = interval
        self.min_count = min_count
        self.empty = empty
        self.error: str | None = None
        self.message: str | None = None
        self.view = Viewport()
        self.rows: list[Row] = []
        self.summary: Text | None = None
        reserved = {" ", "a"} if multi else set()
        if reserved & set(self.hotkeys):
            raise ValueError("Hotkey kollidiert mit der Mehrfachauswahl")
        self.pending = initial
        self.loaded = False
        self.set_rows(rows, summary, prune=bool(rows))
        self.focus(initial)

    def focus(self, value: Any) -> None:
        if value is None:
            return
        self.view.cursor = next((index for index, row in enumerate(self.rows) if row.value == value), min(self.view.cursor, max(0, len(self.rows) - 1)))
        self.view.offset = self.view.fit()

    def set_rows(self, rows: Sequence[Row], summary: Text | None, prune: bool = True) -> None:
        self.rows = list(rows)
        self.summary = summary
        if prune:
            values = [row.value for row in self.rows]
            self.checked = [value for value in self.checked if value in values]
        self.view.resize(len(self.rows), self.view.size)

    def reload(self) -> None:
        if self.loader is None:
            return
        current = self.current_value()
        if current is None:
            current, self.pending = self.pending, None
        try:
            data = self.loader()
        except CubiError as exc:
            self.error = str(exc)
            self.set_rows([], None, prune=False)
            return
        self.error = None
        if not self.loaded and data.checked is not None:
            self.checked = list(data.checked)
        self.loaded = True
        self.set_rows(data.rows, data.summary)
        self.focus(current)

    def current_value(self) -> Any:
        return self.rows[self.view.cursor].value if self.rows else None

    def checked_values(self) -> list:
        return [row.value for row in self.rows if row.value in self.checked]

    def answer(self, value: Any) -> str:
        if self.multi:
            labels = [self.label_of(row) for row in self.rows if row.value in value]
            return f"{len(labels)} gewählt: {', '.join(labels)}" if labels else "keine"
        row = next((row for row in self.rows if row.value == value), None)
        return self.label_of(row) if row is not None else str(value)

    @staticmethod
    def label_of(row: Row) -> str:
        if row.label is not None:
            return row.label
        return cell_text(row.cells[0]).plain if row.cells else str(row.value)

    def toggle(self, value: Any) -> None:
        if value in self.checked:
            self.checked.remove(value)
        else:
            self.checked.append(value)

    def handle(self, key: str) -> tuple[str, Any] | None:
        if key in NAV_KEYS:
            self.view.move(key)
            self.message = None
            return None
        if key == "ctrl-c":
            raise KeyboardInterrupt
        if key == "esc":
            return "back", None
        if key == "enter":
            if not self.rows:
                return None
            if self.multi:
                if len(self.checked) < self.min_count:
                    self.message = f"Mindestens {self.min_count} Eintrag wählen" if self.min_count == 1 else f"Mindestens {self.min_count} Einträge wählen"
                    return None
                return "enter", self.checked_values()
            return "enter", self.current_value()
        if self.multi and self.rows:
            if key == " ":
                self.toggle(self.current_value())
                self.message = None
                return None
            if key == "a":
                values = [row.value for row in self.rows]
                self.checked = [] if len(self.checked) == len(values) else values
                self.message = None
                return None
        if key == "r" and self.loader is not None:
            return "reload", None
        if key in self.hotkeys:
            return key, self.checked_values() if self.multi else self.current_value()
        return None

    def footer_parts(self) -> list[str]:
        parts = []
        if self.rows and len(self.rows) > self.view.size:
            parts.append(f"{self.view.cursor + 1}/{len(self.rows)}")
        if self.multi:
            parts.append(f"{len(self.checked)} gewählt")
            parts.extend(["↑↓ Bewegen", "Space Wählen", "a Alle", "Enter Bestätigen"])
        else:
            parts.extend(["↑↓ Auswahl", "Enter Bestätigen"])
        parts.extend(f"{key} {label}" for key, label in self.hotkeys.items())
        if self.loader is not None:
            parts.append("r Neu laden")
        parts.append("Esc Abbrechen")
        if self.interval:
            parts.append(f"Auto {int(self.interval)}s")
        return parts

    def footer(self, width: int) -> list[Text]:
        lines: list[str] = []
        current = ""
        for part in self.footer_parts():
            candidate = f"{current} · {part}" if current else part
            if current and cell_len(candidate) + 2 > width:
                lines.append(current)
                current = part
            else:
                current = candidate
        if current:
            lines.append(current)
        return [Text("  " + line, style="dim") for line in lines]

    def header_lines(self) -> list[Text]:
        lines = [question_line(self.question)]
        if self.subtitle is not None:
            lines.extend(Text("  ") + part for part in self.subtitle.split("\n", allow_blank=True))
        if self.summary is not None:
            lines.extend(Text("  ") + part for part in self.summary.split("\n", allow_blank=True))
        return lines

    def lines(self, width: int | None = None) -> list[Text]:
        width = width or term.terminal_size()[0] - 1
        lines = self.header_lines()
        if self.error:
            lines.append(Text("  ✖ " + self.error.replace("\n", " "), style="red"))
            lines.extend(self.footer(width))
            return lines
        if not self.rows:
            lines.append(Text("  " + self.empty, style="dim"))
            lines.extend(self.footer(width))
            return lines
        prefix = 2 + (4 if self.multi else 0)
        if self.columns:
            names = [name for name, _ in self.columns]
            justifies = [justify for _, justify in self.columns]
        else:
            count = max(len(row.cells) for row in self.rows)
            names = [""] * count
            justifies = ["left"] * count
        naturals = [max([cell_len(name)] + [cell_width(row.cells[position]) if position < len(row.cells) else 0 for row in self.rows]) for position, name in enumerate(names)]
        widths = fit_widths(naturals, max(1, width - prefix - GAP * (len(naturals) - 1)))
        chrome = len(lines) + len(self.footer(width)) + (1 if self.columns else 0) + (1 if self.message else 0)
        self.view.resize(len(self.rows), window_size(len(self.rows), chrome))
        footer = self.footer(width)
        if self.columns:
            head = Text(" " * prefix)
            for position, name in enumerate(names):
                if position:
                    head.append(" " * GAP)
                head.append_text(fit_cell(Text(name, style="bold"), widths[position], justifies[position]))
            lines.append(head)
        for index in self.view.window():
            row = self.rows[index]
            selected = index == self.view.cursor
            line = Text()
            line.append("❯ " if selected else "  ", style="bold cyan")
            if self.multi:
                on = row.value in self.checked
                line.append("[x] " if on else "[ ] ", style="bold green" if on else "dim")
            for position in range(len(names)):
                if position:
                    line.append(" " * GAP)
                cell = row.cells[position] if position < len(row.cells) else ""
                text = fit_cell(cell, widths[position], justifies[position])
                if selected and position == 0:
                    text.stylize("bold" if text.spans or text.style else "bold cyan")
                line.append_text(text)
            lines.append(line)
        if self.message:
            lines.append(Text("  " + self.message, style="red"))
        lines.extend(footer)
        return lines


def items_to_rows(items: Sequence[Item]) -> list[Row]:
    with_hints = any(item.hint for item in items)
    rows = []
    for item in items:
        cells = [Text(item.label)]
        if with_hints:
            cells.append(Text(item.hint, style="dim"))
        rows.append(Row(cells, item.value, item.label))
    return rows


def run_list(prompt: ListPrompt) -> tuple[str, Any]:
    region = term.Region()
    with term.interactive():
        try:
            if prompt.loader is not None:
                region.draw([question_line(prompt.question), Text("  Lade …", style="dim")])
                prompt.reload()
            while True:
                region.draw(prompt.lines())
                key = term.read_key(prompt.interval)
                if key is None:
                    prompt.reload()
                    continue
                try:
                    result = prompt.handle(key)
                except KeyboardInterrupt:
                    region.finish(cancel_line(prompt.question))
                    raise
                if result is None:
                    continue
                action, value = result
                if action == "reload":
                    prompt.reload()
                elif action == "enter":
                    region.finish(answer_line(prompt.question, prompt.answer(value)))
                    return result
                elif action == "back":
                    region.finish(cancel_line(prompt.question))
                    return result
                else:
                    region.erase()
                    return result
        except BaseException:
            region.erase()
            raise


def select(question: str, items: Sequence[Item], initial: Any = None, subtitle: str | Text | None = None) -> Any:
    if not items:
        raise CubiError("Keine Einträge vorhanden")
    action, value = run_list(ListPrompt(question, items_to_rows(items), subtitle=subtitle, initial=initial))
    return value if action == "enter" else None


def multiselect(
    question: str,
    items: Sequence[Item],
    checked: Sequence = (),
    min_count: int = 0,
    subtitle: str | Text | None = None,
) -> list | None:
    if not items:
        raise CubiError("Keine Einträge vorhanden")
    prompt = ListPrompt(question, items_to_rows(items), multi=True, checked=checked, subtitle=subtitle, min_count=min_count)
    action, value = run_list(prompt)
    return value if action == "enter" else None


def table_select(
    title: str,
    columns: Sequence,
    loader: Callable[[], TableData],
    hotkeys: dict[str, str] | None = None,
    interval: float | None = None,
    initial: Any = None,
    multi: bool = False,
    checked: Sequence = (),
    min_count: int = 0,
) -> tuple[str, Any]:
    prompt = ListPrompt(
        title,
        columns=columns,
        multi=multi,
        checked=checked,
        hotkeys=hotkeys,
        loader=loader,
        interval=interval,
        initial=initial,
        min_count=min_count,
    )
    return run_list(prompt)
