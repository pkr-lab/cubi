from __future__ import annotations

import os
import re
import shlex
import tempfile
from dataclasses import dataclass
from datetime import date, timedelta
from pathlib import Path
from typing import Any

import yaml

from . import config
from .errors import CubiError

REGISTER = Path("ops") / "token-register.yaml"
DOCS_PREFIX = "pkr-lab-docs:"
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SOON = 60
URGENT = 7


@dataclass
class Token:
    id: str
    service: str
    kind: str
    stored_in: list[str]
    mode: str
    runbook: str
    command: list[str]
    hint: str
    overlap: bool
    blast_radius: str
    issued: date | None
    expires: date | None
    max_age_days: int | None
    live_check: str | None

    @property
    def due(self) -> date | None:
        if self.expires:
            return self.expires
        if self.issued and self.max_age_days:
            return self.issued + timedelta(days=self.max_age_days)
        return None

    @property
    def days_left(self) -> int | None:
        due = self.due
        return None if due is None else (due - date.today()).days

    @property
    def urgency(self) -> int:
        left = self.days_left
        if left is None:
            return 1
        if left <= URGENT:
            return 0
        if left <= SOON:
            return 2
        return 3


def repo_root() -> Path:
    configured = config.section("tokens").get("repo")
    root = Path(str(configured)).expanduser() if configured else config.ROOT.parent / "capulus-core"
    if not (root / REGISTER).is_file():
        raise CubiError(f"Token-Register nicht gefunden: {root / REGISTER} (tokens.repo in der Config setzen)")
    return root


def docs_root() -> Path:
    configured = config.section("tokens").get("docs_repo")
    root = Path(str(configured)).expanduser() if configured else config.ROOT.parent / "pkr-lab-docs"
    if not root.is_dir():
        raise CubiError(f"Docs-Repo nicht gefunden: {root} (tokens.docs_repo in der Config setzen)")
    return root


def resolve_runbook(value: str) -> Path:
    if value.startswith(DOCS_PREFIX):
        return docs_root() / value[len(DOCS_PREFIX):]
    return repo_root() / value


def register_path() -> Path:
    return repo_root() / REGISTER


def parse_date(value: Any) -> date | None:
    if value is None:
        return None
    if isinstance(value, date):
        return value
    try:
        return date.fromisoformat(str(value))
    except ValueError:
        raise CubiError(f"Ungültiges Datum im Register: {value!r}")


def load() -> list[Token]:
    path = register_path()
    try:
        entries = yaml.safe_load(path.read_text(encoding="utf-8")) or []
    except yaml.YAMLError as exc:
        raise CubiError(f"Ungültiges Register {path}: {exc}")
    tokens = []
    for entry in entries:
        rotation = entry.get("rotation") or {}
        tokens.append(
            Token(
                id=entry["id"],
                service=entry.get("service", "-"),
                kind=entry.get("kind", "-"),
                stored_in=list(entry.get("stored_in") or []),
                mode=rotation.get("mode", "manual"),
                runbook=rotation.get("runbook", ""),
                command=shlex.split(rotation.get("command") or ""),
                hint=rotation.get("hint", ""),
                overlap=bool(rotation.get("overlap", True)),
                blast_radius=entry.get("blast_radius", ""),
                issued=parse_date(entry.get("issued")),
                expires=parse_date(entry.get("expires")),
                max_age_days=entry.get("max_age_days"),
                live_check=entry.get("live_check"),
            )
        )
    return tokens


def validate_date(text: str) -> date:
    if not DATE.match(text):
        raise CubiError(f"Datum muss das Format JJJJ-MM-TT haben: {text!r}")
    return parse_date(text)


def set_dates(token_id: str, issued: date, expires: date | None) -> None:
    path = register_path()
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    start = next((index for index, line in enumerate(lines) if line.rstrip() == f"- id: {token_id}"), None)
    if start is None:
        raise CubiError(f"{token_id} steht nicht im Register")
    end = next((index for index in range(start + 1, len(lines)) if lines[index].startswith("- id:")), len(lines))
    values = {"issued": issued.isoformat(), "expires": expires.isoformat() if expires else "null"}
    for key, value in values.items():
        pattern = re.compile(rf"^(  {key}:)\s.*$")
        position = next((index for index in range(start, end) if pattern.match(lines[index])), None)
        if position is None:
            raise CubiError(f"{token_id}: Feld {key} fehlt im Register")
        lines[position] = pattern.sub(rf"\1 {value}", lines[position].rstrip("\n")) + "\n"
    handle, temp = tempfile.mkstemp(dir=path.parent, prefix=".token-register-")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as out:
            out.writelines(lines)
        os.chmod(temp, path.stat().st_mode & 0o777)
        os.replace(temp, path)
    except BaseException:
        os.unlink(temp)
        raise
