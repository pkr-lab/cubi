import re
from datetime import datetime

from rich.text import Text

from .. import shell, ui
from ..timeutil import age
from ..ui import Row, TableData

STATUS_STYLES = {
    "deployed": "green",
    "failed": "red",
    "unknown": "red",
    "superseded": "dim",
    "uninstalled": "dim",
    "uninstalling": "yellow",
    "pending-install": "yellow",
    "pending-upgrade": "yellow",
    "pending-rollback": "yellow",
}


def updated_age(value: str) -> str:
    match = re.match(r"(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2})\S* ([+-]\d{4})", value or "")
    if not match:
        return "-"
    try:
        parsed = datetime.strptime(" ".join(match.groups()), "%Y-%m-%d %H:%M:%S %z")
    except ValueError:
        return "-"
    return age(parsed)


def load() -> TableData:
    data = shell.capture_json(["helm", "list", "-A", "-o", "json", "--max", "500"], timeout=30)
    data.sort(key=lambda release: (release["namespace"], release["name"]))
    rows = [
        Row(
            [
                Text(release["name"]),
                Text(release["namespace"]),
                Text(str(release.get("revision", "-"))),
                Text(release.get("status", "-"), style=STATUS_STYLES.get(release.get("status", ""), "")),
                Text(release.get("chart", "-")),
                Text(release.get("app_version", "-")),
                Text(updated_age(release.get("updated", ""))),
            ],
            (release["namespace"], release["name"]),
        )
        for release in data
    ]
    return TableData(rows, Text(f"{len(rows)} Releases", style="dim"))


def release_menu(namespace: str, name: str) -> None:
    ui.action_menu(
        f"Release · {name}",
        [
            ("Status", lambda: ui.show_command(["helm", "status", name, "-n", namespace])),
            ("Values", lambda: ui.show_command(["helm", "get", "values", name, "-n", namespace], "Keine eigenen Values")),
            ("Historie", lambda: ui.show_command(["helm", "history", name, "-n", namespace])),
        ],
        subtitle=namespace,
    )


def run() -> None:
    selected = None
    while True:
        key, value = ui.table_view(
            "Helm · Releases",
            ["Release", "Namespace", "Rev", "Status", "Chart", "App-Version", "Alter"],
            load,
            interval=30,
            initial=selected,
        )
        if key != "enter":
            return
        selected = value
        ui.guard(release_menu, *value)
