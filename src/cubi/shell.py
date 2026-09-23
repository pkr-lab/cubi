import json
import os
import re
import shutil
import subprocess
from typing import Any, Sequence

from .errors import CubiError

LOG_NOISE = re.compile(r"^[EWIF]\d{4} \d{2}:\d{2}:\d{2}")


def require(tool: str) -> None:
    if shutil.which(tool) is None:
        raise CubiError(f"{tool} ist nicht installiert oder nicht im PATH")


def clean_line(line: str) -> str:
    stripped = line.strip()
    if stripped.startswith("{") and stripped.endswith("}"):
        try:
            payload = json.loads(stripped)
        except json.JSONDecodeError:
            return line
        if isinstance(payload, dict) and "msg" in payload:
            return str(payload["msg"])
    return line


def clean_text(text: str) -> str:
    return "\n".join(clean_line(line) for line in text.splitlines())


def first_line(text: str) -> str:
    lines = [line.strip() for line in clean_text(text).splitlines() if line.strip()]
    useful = [line for line in lines if not LOG_NOISE.match(line)]
    if useful:
        return useful[0]
    return lines[0] if lines else ""


def capture(cmd: Sequence[str], timeout: float | None = 30) -> subprocess.CompletedProcess:
    require(cmd[0])
    try:
        return subprocess.run(list(cmd), capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise CubiError(f"Zeitüberschreitung bei: {' '.join(cmd)}")


def ensure(result: subprocess.CompletedProcess, fallback: str) -> subprocess.CompletedProcess:
    if result.returncode != 0:
        raise CubiError(first_line(result.stderr) or fallback)
    return result


def capture_json(cmd: Sequence[str], timeout: float | None = 30) -> Any:
    result = ensure(capture(cmd, timeout), f"Befehl fehlgeschlagen: {' '.join(cmd)}")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        raise CubiError(f"Ungültige Ausgabe von: {' '.join(cmd)}")


def merge_output(result: subprocess.CompletedProcess) -> str:
    parts = [result.stdout.rstrip()]
    errors = "\n".join(line for line in clean_text(result.stderr).splitlines() if not LOG_NOISE.match(line.strip())).rstrip()
    if errors:
        parts.append(errors)
    return "\n".join(part for part in parts if part)


def run(cmd: Sequence[str]) -> int:
    require(cmd[0])
    try:
        return subprocess.run(list(cmd)).returncode
    except KeyboardInterrupt:
        return 130


def replace(cmd: Sequence[str]) -> None:
    require(cmd[0])
    os.execvp(cmd[0], list(cmd))
