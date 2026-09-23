import os
from functools import lru_cache
from pathlib import Path
from typing import Any

import yaml

from .errors import CubiError

ROOT = Path(__file__).resolve().parents[2]


def config_path() -> Path:
    candidates = []
    override = os.environ.get("CUBI_CONFIG")
    if override:
        candidates.append(Path(override).expanduser())
    candidates.append(Path.home() / ".config" / "cubi" / "targets.yaml")
    candidates.append(ROOT / "config" / "targets.yaml")
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise CubiError("Keine targets.yaml gefunden")


@lru_cache(maxsize=1)
def load() -> dict[str, Any]:
    path = config_path()
    try:
        data = yaml.safe_load(path.read_text())
    except yaml.YAMLError as exc:
        raise CubiError(f"Ungültige Config {path}: {exc}")
    if data is None:
        return {}
    if not isinstance(data, dict):
        raise CubiError(f"Ungültige Config {path}: oberste Ebene muss ein Mapping sein")
    return data


def section(name: str) -> dict[str, Any]:
    value = load().get(name)
    return value if isinstance(value, dict) else {}
