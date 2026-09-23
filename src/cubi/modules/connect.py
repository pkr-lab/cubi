from .. import config, shell, ui
from ..errors import CubiError
from ..ui import Item


def group_title(key: str) -> str:
    return key.upper() if len(key) <= 3 else key.capitalize()


def describe(alias: str) -> str:
    try:
        result = shell.capture(["ssh", "-G", alias], timeout=5)
    except CubiError:
        return ""
    if result.returncode != 0:
        return ""
    values: dict[str, str] = {}
    for line in result.stdout.splitlines():
        key, _, value = line.partition(" ")
        values.setdefault(key, value)
    host = values.get("hostname", "")
    user = values.get("user", "")
    return f"{user}@{host}" if user and host else host


def targets(group: str, entries: object) -> list[dict]:
    if not isinstance(entries, list) or not entries:
        raise CubiError(f"Keine Ziele in connect.{group}")
    result = []
    for entry in entries:
        if not isinstance(entry, dict) or "name" not in entry or "ssh" not in entry:
            raise CubiError(f"Ungültiger Eintrag in connect.{group}: name und ssh sind Pflicht")
        result.append(entry)
    return result


def choose_target(group: str, entries: object) -> None:
    items = [Item(entry["name"], entry["ssh"], describe(entry["ssh"])) for entry in targets(group, entries)]
    alias = ui.menu(f"Connect · {group_title(group)}", items)
    if alias is None:
        return
    ui.console.clear()
    shell.replace(["ssh", alias])


def run() -> None:
    groups = config.section("connect")
    if not groups:
        raise CubiError("Keine Connect-Ziele in targets.yaml")
    items = [Item(group_title(key), key, f"{len(entries or [])} Ziele") for key, entries in groups.items()]
    group = None
    while True:
        group = ui.menu("Connect", items, initial=group)
        if group is None:
            return
        ui.guard(choose_target, group, groups[group])
