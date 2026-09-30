import contextlib
import os
import re
import select
import shlex
import shutil
import subprocess
import sys
import tempfile
import termios
import tty
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Sequence

from rich import box
from rich.cells import cell_len
from rich.console import Console, Group
from rich.live import Live
from rich.panel import Panel
from rich.table import Table
from rich.text import Text

from . import shell
from .errors import CubiError

console = Console(highlight=False)

KEYS = {
    b"\x1b[A": "up",
    b"\x1bOA": "up",
    b"\x1b[B": "down",
    b"\x1bOB": "down",
    b"\x1b[C": "right",
    b"\x1bOC": "right",
    b"\x1b[D": "left",
    b"\x1bOD": "left",
    b"\x1b[5~": "pageup",
    b"\x1b[6~": "pagedown",
    b"\x1b[H": "home",
    b"\x1bOH": "home",
    b"\x1b[1~": "home",
    b"\x1b[F": "end",
    b"\x1bOF": "end",
    b"\x1b[4~": "end",
    b"\x1b": "esc",
    b"\r": "enter",
    b"\n": "enter",
    b"\x7f": "backspace",
}

BACK_KEYS = ("esc", "left", "q", "backspace")

LONG_SEQUENCES = sorted((sequence for sequence in KEYS if len(sequence) > 1), key=len, reverse=True)
SHORT_SEQUENCES = [sequence for sequence in KEYS if len(sequence) == 1]
UNKNOWN_ESCAPE = re.compile(rb"\x1b(?:\[[0-9;?]*[@-~]|O[@-~])")
PENDING: list[str] = []
MIN_COLUMN = 14


@dataclass
class Item:
    label: str
    value: Any
    hint: str = ""


@dataclass
class Row:
    cells: list
    value: Any = None


@dataclass
class TableData:
    rows: list = field(default_factory=list)
    summary: Text | None = None


@contextlib.contextmanager
def raw_terminal():
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    try:
        tty.setcbreak(fd)
        yield
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)


def tokenize(data: bytes) -> list[str]:
    keys: list[str] = []
    index = 0
    while index < len(data):
        matched = next((sequence for sequence in LONG_SEQUENCES if data.startswith(sequence, index)), None)
        if matched is not None:
            keys.append(KEYS[matched])
            index += len(matched)
            continue
        unknown = UNKNOWN_ESCAPE.match(data, index)
        if unknown is not None:
            keys.append("unknown")
            index = unknown.end()
            continue
        single = next((sequence for sequence in SHORT_SEQUENCES if data.startswith(sequence, index)), None)
        if single is not None:
            keys.append(KEYS[single])
            index += 1
            continue
        text = data[index:index + 4].decode(errors="ignore")
        if text:
            keys.append(text[0])
            index += len(text[0].encode())
        else:
            index += 1
    return keys


def read_key(timeout: float | None = None) -> str | None:
    if PENDING:
        return PENDING.pop(0)
    fd = sys.stdin.fileno()
    ready, _, _ = select.select([fd], [], [], timeout)
    if not ready:
        return None
    PENDING.extend(tokenize(os.read(fd, 64)))
    return PENDING.pop(0) if PENDING else "unknown"


def scroll(cursor: int, offset: int, size: int) -> int:
    if cursor < offset:
        return cursor
    if cursor >= offset + size:
        return cursor - size + 1
    return offset


def render_menu(title: str, items: Sequence[Item], cursor: int, offset: int, size: int, subtitle: str | None) -> Panel:
    grid = Table.grid(padding=(0, 1))
    grid.add_column(width=1)
    grid.add_column(no_wrap=True)
    grid.add_column(no_wrap=True, style="dim")
    for index in range(offset, min(len(items), offset + size)):
        item = items[index]
        selected = index == cursor
        grid.add_row(
            Text("❯" if selected else " ", style="bold cyan"),
            Text(item.label, style="bold cyan" if selected else ""),
            Text(item.hint),
        )
    parts: list = []
    if subtitle:
        parts.extend([Text(subtitle, style="dim"), Text("")])
    parts.append(grid)
    footer = "↑↓ Auswahl · Enter/→ Öffnen · Esc/← Zurück"
    if len(items) > size:
        footer = f"{cursor + 1}/{len(items)} · {footer}"
    return Panel(
        Group(*parts),
        title=Text(title, style="bold"),
        subtitle=Text(footer, style="dim"),
        box=box.ROUNDED,
        padding=(1, 2),
        width=min(console.width, 78),
    )


def menu(title: str, items: Sequence[Item], subtitle: str | None = None, initial: Any = None) -> Any:
    if not items:
        raise CubiError("Keine Einträge vorhanden")
    cursor = next((index for index, item in enumerate(items) if initial is not None and item.value == initial), 0)
    offset = 0
    with raw_terminal(), Live(console=console, screen=True, auto_refresh=False) as live:
        while True:
            size = max(3, console.height - 9)
            offset = scroll(cursor, offset, size)
            live.update(render_menu(title, items, cursor, offset, size, subtitle), refresh=True)
            key = read_key()
            if key == "up":
                cursor = (cursor - 1) % len(items)
            elif key == "down":
                cursor = (cursor + 1) % len(items)
            elif key == "pageup":
                cursor = max(0, cursor - size)
            elif key == "pagedown":
                cursor = min(len(items) - 1, cursor + size)
            elif key == "home":
                cursor = 0
            elif key == "end":
                cursor = len(items) - 1
            elif key in ("enter", "right"):
                return items[cursor].value
            elif key in BACK_KEYS:
                return None


def render_prompt(title: str, text: str, cursor: int) -> Panel:
    line = Text()
    line.append(text[:cursor])
    line.append(text[cursor] if cursor < len(text) else " ", style="reverse")
    line.append(text[cursor + 1:])
    return Panel(
        line,
        title=Text(title, style="bold"),
        subtitle=Text("Enter Bestätigen · Esc Abbrechen", style="dim"),
        box=box.ROUNDED,
        padding=(1, 2),
        width=min(console.width, 78),
    )


def prompt(title: str, initial: str = "") -> str | None:
    text = initial
    cursor = len(text)
    with raw_terminal(), Live(console=console, screen=True, auto_refresh=False) as live:
        while True:
            live.update(render_prompt(title, text, cursor), refresh=True)
            key = read_key()
            if key == "enter":
                return text
            if key == "esc":
                return None
            if key == "backspace":
                if cursor > 0:
                    text = text[:cursor - 1] + text[cursor:]
                    cursor -= 1
            elif key == "left":
                cursor = max(0, cursor - 1)
            elif key == "right":
                cursor = min(len(text), cursor + 1)
            elif key == "home":
                cursor = 0
            elif key == "end":
                cursor = len(text)
            elif key and len(key) == 1 and key.isprintable():
                text = text[:cursor] + key + text[cursor:]
                cursor += 1


def edit_text(initial: str, suffix: str = ".md") -> str:
    editor = shlex.split(os.environ.get("EDITOR") or os.environ.get("VISUAL") or "vi")
    with tempfile.NamedTemporaryFile("w", suffix=suffix, delete=False) as handle:
        handle.write(initial)
        path = handle.name
    try:
        begin("Bearbeiten")
        shell.run(editor + [path])
        return Path(path).read_text()
    finally:
        os.unlink(path)


def cell_width(cell: Any) -> int:
    return cell_len(cell.plain if isinstance(cell, Text) else str(cell))


def fit_widths(naturals: list[int], available: int) -> list[int] | None:
    if sum(naturals) <= available:
        return None
    for floor in (MIN_COLUMN, 6):
        floors = [min(natural, floor) for natural in naturals]
        if sum(floors) > available:
            continue
        widths = list(naturals)
        while sum(widths) > available:
            index = max((position for position in range(len(widths)) if widths[position] > floors[position]), key=lambda position: widths[position])
            widths[index] -= 1
        return widths
    return None


def render_table(
    title: str,
    columns: Sequence,
    data: TableData | None,
    error: str | None,
    cursor: int,
    offset: int,
    size: int,
    hotkeys: dict[str, str],
    interval: float | None,
) -> Group:
    parts: list = [Text(title, style="bold cyan")]
    if data is not None and data.summary is not None:
        parts.extend([Text(""), data.summary])
    parts.append(Text(""))
    rows = data.rows if data is not None else []
    if error:
        parts.append(Panel(Text(error, style="red"), title=Text("Fehler", style="bold red"), title_align="left", border_style="red"))
    elif not rows:
        parts.append(Text("Keine Einträge", style="dim"))
    else:
        table = Table(box=box.HEAVY_HEAD, header_style="bold")
        headers = [column if isinstance(column, tuple) else (column, "left") for column in columns]
        naturals = [1] + [max([cell_len(name)] + [cell_width(row.cells[position]) for row in rows]) for position, (name, _) in enumerate(headers)]
        widths = fit_widths(naturals, console.width - (3 * len(naturals) + 1))
        table.add_column("", width=1)
        for position, (name, justify) in enumerate(headers):
            table.add_column(name, justify=justify, no_wrap=True, overflow="ellipsis", width=None if widths is None else widths[position + 1])
        for index in range(offset, min(len(rows), offset + size)):
            selected = index == cursor
            cells = [Text("❯" if selected else " ", style="bold cyan")]
            cells.extend(Text(cell) if isinstance(cell, str) else cell for cell in rows[index].cells)
            table.add_row(*cells, style="on grey27" if selected else None)
        parts.append(table)
    hints = ["↑↓ Auswahl", "Enter Öffnen"]
    hints.extend(f"{key} {label}" for key, label in hotkeys.items())
    hints.extend(["r Neu laden", "Esc Zurück"])
    if interval:
        hints.append(f"Auto {int(interval)}s")
    footer = " · ".join(hints)
    if len(rows) > size:
        footer = f"{cursor + 1}/{len(rows)} · {footer}"
    parts.append(Text(footer, style="dim"))
    return Group(*parts)


def load_table(loader: Callable[[], TableData]) -> tuple[TableData | None, str | None]:
    try:
        return loader(), None
    except CubiError as exc:
        return None, str(exc)


def index_of(rows: Sequence[Row], value: Any, default: int) -> int:
    if value is not None:
        for index, row in enumerate(rows):
            if row.value == value:
                return index
    return default


def table_view(
    title: str,
    columns: Sequence,
    loader: Callable[[], TableData],
    hotkeys: dict[str, str] | None = None,
    interval: float | None = None,
    initial: Any = None,
) -> tuple[str, Any]:
    hotkeys = hotkeys or {}
    cursor = 0
    offset = 0
    with raw_terminal(), Live(console=console, screen=True, auto_refresh=False) as live:
        live.update(Text("Lade …", style="dim"), refresh=True)
        data, error = load_table(loader)
        if data is not None:
            cursor = index_of(data.rows, initial, 0)
        while True:
            rows = data.rows if data is not None else []
            cursor = min(cursor, max(0, len(rows) - 1))
            summary_lines = len(data.summary.plain.splitlines()) + 1 if data is not None and data.summary is not None else 0
            size = max(3, console.height - 9 - summary_lines)
            offset = scroll(cursor, offset, size)
            offset = min(offset, max(0, len(rows) - size))
            live.update(render_table(title, columns, data, error, cursor, offset, size, hotkeys, interval), refresh=True)
            key = read_key(interval)
            if key is None or key == "r":
                current = rows[cursor].value if rows else None
                data, error = load_table(loader)
                cursor = index_of(data.rows if data is not None else [], current, cursor)
                continue
            value = rows[cursor].value if rows else None
            if key == "up":
                cursor = max(0, cursor - 1)
            elif key == "down":
                cursor = min(max(0, len(rows) - 1), cursor + 1)
            elif key == "pageup":
                cursor = max(0, cursor - size)
            elif key == "pagedown":
                cursor = min(max(0, len(rows) - 1), cursor + size)
            elif key == "home":
                cursor = 0
            elif key == "end":
                cursor = max(0, len(rows) - 1)
            elif key in ("enter", "right"):
                if rows:
                    return "enter", value
            elif key in BACK_KEYS:
                return "back", None
            elif key in hotkeys:
                return key, value


def pause(message: str = "Enter drücken zum Fortfahren") -> None:
    console.print(Text(f"\n{message}", style="dim"))
    with raw_terminal():
        while read_key() not in ("enter", "esc", "q", " "):
            pass


def confirm(question: str, default: bool = False) -> bool:
    console.print(Text(f"{question} [{'J/n' if default else 'j/N'}] ", style="bold yellow"), end="")
    answer = default
    with raw_terminal():
        while True:
            key = read_key()
            if key in ("j", "J", "y", "Y"):
                answer = True
                break
            if key in ("n", "N", "esc", "q"):
                answer = False
                break
            if key == "enter":
                break
    console.print("Ja" if answer else "Nein")
    return answer


def begin(title: str) -> None:
    console.clear()
    console.rule(Text(title, style="bold cyan"))


def error(message: str) -> None:
    console.print(Panel(Text(message, style="red"), title=Text("Fehler", style="bold red"), title_align="left", border_style="red"))
    pause()


def info(message: str) -> None:
    console.print(Text(message))
    pause()


def show(renderable: Any, title: str | None = None) -> None:
    begin(title or "Details")
    console.print(renderable)
    pause()


def show_text(text: str, title: str | None = None) -> None:
    lines = text.splitlines()
    if len(lines) >= console.height - 4 and shutil.which("less"):
        subprocess.run(["less", "-R"], input=text, text=True)
        return
    begin(title or "Ausgabe")
    console.print(Text.from_ansi(text))
    pause()


def show_command(cmd: Sequence[str], empty: str = "(keine Ausgabe)", title: str | None = None) -> None:
    result = shell.capture(cmd, timeout=90)
    show_text(shell.merge_output(result) or empty, title)


def run_command(cmd: Sequence[str], title: str | None = None) -> int:
    begin(title or "Ausführung")
    console.print(Text("$ " + " ".join(cmd), style="dim"))
    code = shell.run(cmd)
    if code != 0:
        console.print(Text(f"Beendet mit Code {code}", style="red"))
    pause()
    return code


def guard(function: Callable, *args: Any) -> Any:
    try:
        return function(*args)
    except CubiError as exc:
        error(str(exc))
        return None


def action_menu(title: str, actions: Sequence[tuple[str, Callable[[], Any]]], subtitle: str | None = None) -> None:
    items = [Item(label, index) for index, (label, _) in enumerate(actions)]
    choice = None
    while True:
        choice = menu(title, items, subtitle, choice)
        if choice is None:
            return
        guard(actions[choice][1])
