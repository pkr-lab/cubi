import re
import unittest
from pathlib import Path

import helpers

SOURCES = sorted((helpers.SRC / "cubi").rglob("*.py")) + [helpers.SRC.parent / "bin" / "cubi"]

FORBIDDEN = {
    r"screen\s*=\s*True": "Alternate Screen",
    r"console\.clear\(": "Terminal leeren",
    r"\\x1b\[2J|\\033\[2J|\\x1b\[3J|\\x1bc": "Screen-Clear-Sequenz",
    r"1049|\?47h": "Alternate-Screen-Sequenz",
    r"\"less\"|'less'": "Pager",
    r"EDITOR|VISUAL|\"vi\"": "Editor",
    r"--web\b|\"gh\", \"browse\"|webbrowser|xdg-open": "Browser",
    r"Live\(": "rich.Live",
    r"termios\.tcsetattr.*TCSAFLUSH": "unerwarteter Terminal-Flush",
}


class NoFullscreenTest(unittest.TestCase):
    def test_no_forbidden_constructs(self):
        problems = []
        for path in SOURCES:
            for number, line in enumerate(path.read_text().splitlines(), 1):
                for pattern, reason in FORBIDDEN.items():
                    if re.search(pattern, line) and not (path.name == "test_no_fullscreen.py"):
                        problems.append(f"{path.name}:{number}: {reason}: {line.strip()}")
        self.assertEqual(problems, [])

    def test_sources_have_no_comments(self):
        problems = []
        for path in SOURCES:
            for number, line in enumerate(path.read_text().splitlines(), 1):
                stripped = line.strip()
                if stripped.startswith("#") and not stripped.startswith("#!"):
                    problems.append(f"{path.name}:{number}")
        self.assertEqual(problems, [])


if __name__ == "__main__":
    unittest.main()
