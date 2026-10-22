from __future__ import annotations

import contextlib
import io
import os
import re
import select
import shutil
import sys
import termios
from typing import Sequence

from rich.console import Console
from rich.text import Text

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
    b"\x1b[3~": "delete",
    b"\x1b": "esc",
    b"\x03": "ctrl-c",
    b"\r": "enter",
    b"\n": "enter",
    b"\x7f": "backspace",
    b"\x08": "backspace",
}

LONG_SEQUENCES = sorted((sequence for sequence in KEYS if len(sequence) > 1), key=len, reverse=True)
SHORT_SEQUENCES = [sequence for sequence in KEYS if len(sequence) == 1]
UNKNOWN_ESCAPE = re.compile(rb"\x1b(?:\[[0-9;?]*[@-~]|O[@-~])")
PENDING: list[str] = []

HIDE_CURSOR = "\x1b[?25l"
SHOW_CURSOR = "\x1b[?25h"


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


@contextlib.contextmanager
def interactive():
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    attrs = termios.tcgetattr(fd)
    attrs[3] &= ~(termios.ICANON | termios.ECHO | termios.ISIG | termios.IEXTEN)
    attrs[6][termios.VMIN] = 1
    attrs[6][termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSADRAIN, attrs)
    write(HIDE_CURSOR)
    try:
        yield
    finally:
        write(SHOW_CURSOR)
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)
        PENDING.clear()


def write(data: str) -> None:
    sys.stdout.write(data)
    sys.stdout.flush()


def terminal_size() -> tuple[int, int]:
    size = shutil.get_terminal_size((80, 24))
    return max(20, size.columns), max(8, size.lines)


def render_ansi(lines: Sequence[Text], width: int) -> tuple[str, int]:
    buffer = io.StringIO()
    out = Console(file=buffer, force_terminal=True, color_system=Console().color_system, width=width, highlight=False)
    count = 0
    for line in lines:
        for part in line.split("\n", allow_blank=True):
            out.print(part, no_wrap=True, overflow="ellipsis", crop=True)
            count += 1
    return buffer.getvalue(), count


class Region:
    def __init__(self) -> None:
        self.height = 0

    def erase(self) -> None:
        if self.height:
            write(f"\x1b[{self.height}A\r\x1b[J")
            self.height = 0

    def draw(self, lines: Sequence[Text]) -> None:
        width, _ = terminal_size()
        data, count = render_ansi(lines, width - 1)
        if self.height:
            data = f"\x1b[{self.height}A\r\x1b[J" + data
        write(data)
        self.height = count

    def finish(self, line: Text) -> None:
        self.erase()
        self.draw([line])
        self.height = 0
