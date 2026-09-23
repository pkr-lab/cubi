import base64
import json
from datetime import datetime
from pathlib import Path
from typing import Any

import yaml

from . import config, shell
from .errors import CubiError

DEFAULT_NAMESPACE = "argocd"


def namespace() -> str:
    return str(config.section("argocd").get("namespace") or DEFAULT_NAMESPACE)


def config_file() -> Path:
    return Path.home() / ".config" / "argocd" / "config"


def read() -> dict[str, Any]:
    try:
        data = yaml.safe_load(config_file().read_text())
    except (OSError, yaml.YAMLError):
        return {}
    return data if isinstance(data, dict) else {}


def current_context() -> str:
    return str(read().get("current-context") or "-")


def configured_contexts() -> list[dict[str, Any]]:
    entries = config.section("argocd").get("contexts") or []
    contexts = []
    for entry in entries:
        if isinstance(entry, str):
            entry = {"name": entry}
        if not isinstance(entry, dict) or not entry.get("name"):
            raise CubiError("Ungültiger Eintrag in argocd.contexts: name ist Pflicht")
        name = str(entry["name"])
        contexts.append(
            {
                "name": name,
                "server": str(entry.get("server", name)),
                "core": bool(entry.get("core", False)),
                "plaintext": bool(entry.get("plaintext", False)),
                "insecure": bool(entry.get("insecure", False)),
                "username": entry.get("username"),
            }
        )
    return contexts


def token_status(name: str) -> tuple[str, datetime | None]:
    data = read()
    context = next((item for item in data.get("contexts") or [] if item.get("name") == name), None)
    if context is None:
        return "none", None
    user = next((item for item in data.get("users") or [] if item.get("name") == context.get("user")), None)
    token = user.get("auth-token") if user else None
    if not token:
        return "none", None
    try:
        payload = token.split(".")[1]
        payload += "=" * (-len(payload) % 4)
        expiry = json.loads(base64.urlsafe_b64decode(payload)).get("exp")
    except (IndexError, ValueError, TypeError):
        return "invalid", None
    if expiry is None:
        return "valid", None
    when = datetime.fromtimestamp(expiry)
    return ("valid" if when > datetime.now() else "expired"), when


def activate(name: str) -> None:
    shell.ensure(shell.capture(["argocd", "context", name], timeout=15), f"Kontext {name} konnte nicht aktiviert werden")
