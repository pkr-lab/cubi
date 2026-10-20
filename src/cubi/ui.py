from typing import Any, Callable, Sequence

from rich import box
from rich.console import Console
from rich.panel import Panel
from rich.text import Text

from . import shell, term
from .choice import Item, Row, TableData, answer_line, cancel_line, multiselect, question_line, select, table_select
from .errors import CubiError

__all__ = [
    "Item",
    "Row",
    "TableData",
    "console",
    "select",
    "multiselect",
    "table_select",
    "prompt",
    "text_block",
    "confirm",
    "begin",
    "error",
    "info",
    "show",
    "show_text",
    "show_command",
    "run_command",
    "guard",
    "action_menu",
]

console = Console(highlight=False)


def prompt(question: str, initial: str = "") -> str | None:
    text = initial
    cursor = len(text)
    region = term.Region()
    with term.interactive():
        try:
            while True:
                line = question_line(question)
                line.append(" › ", style="dim")
                line.append(text[:cursor], style="cyan")
                line.append(text[cursor] if cursor < len(text) else " ", style="reverse")
                line.append(text[cursor + 1:], style="cyan")
                region.draw([line, Text("  Enter Bestätigen · Esc Abbrechen", style="dim")])
                key = term.read_key()
                if key == "enter":
                    region.finish(answer_line(question, text))
                    return text
                if key == "esc":
                    region.finish(cancel_line(question))
                    return None
                if key == "ctrl-c":
                    region.finish(cancel_line(question))
                    raise KeyboardInterrupt
                if key == "backspace":
                    if cursor > 0:
                        text = text[:cursor - 1] + text[cursor:]
                        cursor -= 1
                elif key == "delete":
                    text = text[:cursor] + text[cursor + 1:]
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
        except BaseException:
            region.erase()
            raise


def text_block(question: str) -> str | None:
    console.print(question_line(question).append(" (leere Zeile beendet die Eingabe)", style="dim"))
    lines: list[str] = []
    try:
        while True:
            line = input("  ")
            if not line.strip():
                break
            lines.append(line)
    except EOFError:
        pass
    return "\n".join(lines) if lines else None


def confirm(question: str, default: bool = False) -> bool:
    region = term.Region()
    hint = "(Y/n)" if default else "(y/N)"
    with term.interactive():
        try:
            line = question_line(question)
            line.append(f" {hint}", style="dim")
            region.draw([line])
            while True:
                key = term.read_key()
                if key in ("y", "Y", "j", "J"):
                    answer = True
                elif key in ("n", "N", "esc"):
                    answer = False
                elif key == "enter":
                    answer = default
                elif key == "ctrl-c":
                    region.finish(cancel_line(question))
                    raise KeyboardInterrupt
                else:
                    continue
                region.finish(answer_line(question, "Ja" if answer else "Nein"))
                return answer
        except BaseException:
            region.erase()
            raise


def begin(title: str) -> None:
    console.print()
    console.rule(Text(title, style="bold cyan"))


def error(message: str) -> None:
    console.print(Panel(Text(message, style="red"), title=Text("Fehler", style="bold red"), title_align="left", border_style="red", box=box.ROUNDED))


def info(message: str) -> None:
    console.print(Text(message))


def show(renderable: Any, title: str | None = None) -> None:
    begin(title or "Details")
    console.print(renderable)


def show_text(text: str, title: str | None = None) -> None:
    begin(title or "Ausgabe")
    console.print(Text.from_ansi(text))


def show_command(cmd: Sequence[str], empty: str = "(keine Ausgabe)", title: str | None = None) -> None:
    result = shell.capture(cmd, timeout=90)
    show_text(shell.merge_output(result) or empty, title)


def run_command(cmd: Sequence[str], title: str | None = None, cwd: str | None = None, env: dict[str, str] | None = None) -> int:
    if title:
        begin(title)
    console.print(Text("$ " + " ".join(cmd), style="dim"))
    code = shell.run(cmd, cwd=cwd, env=env)
    if code != 0:
        console.print(Text(f"Beendet mit Code {code}", style="red"))
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
        choice = select(title, items, initial=choice, subtitle=subtitle)
        if choice is None:
            return
        guard(actions[choice][1])
