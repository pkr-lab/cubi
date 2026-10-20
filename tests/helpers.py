import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time
from pathlib import Path

SRC = Path(__file__).resolve().parents[1] / "src"
sys.path.insert(0, str(SRC))

PRELUDE = f"import sys; sys.path.insert(0, {str(SRC)!r})\n"

KEY_UP = b"\x1b[A"
KEY_DOWN = b"\x1b[B"
KEY_ENTER = b"\r"
KEY_ESC = b"\x1b"
KEY_CTRL_C = b"\x03"
KEY_SPACE = b" "


def run_in_pty(script, keys, rows=24, cols=80, settle=0.12):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp(sys.executable, [sys.executable, "-c", PRELUDE + script])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    chunks = []

    def drain(duration):
        end = time.time() + duration
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.03)
            if not ready:
                continue
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            chunks.append(data)

    drain(0.8)
    for key in keys:
        try:
            os.write(fd, key)
        except OSError:
            break
        drain(settle)
    drain(0.4)
    try:
        _, status = os.waitpid(pid, 0)
    except ChildProcessError:
        status = 0
    os.close(fd)
    return b"".join(chunks).decode(errors="replace"), os.waitstatus_to_exitcode(status)
