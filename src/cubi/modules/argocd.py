from collections import Counter
from typing import Any

from rich.console import Group
from rich.panel import Panel
from rich.table import Table
from rich.text import Text
from rich import box

from .. import argocd_config, shell, ui
from ..errors import CubiError
from ..timeutil import age, parse_iso
from ..ui import Item, Row, TableData
from . import github

SYNC_STYLES = {
    "Synced": "green",
    "OutOfSync": "yellow",
    "Unknown": "magenta",
}

HEALTH_STYLES = {
    "Healthy": "green",
    "Progressing": "yellow",
    "Degraded": "red",
    "Missing": "red",
    "Suspended": "cyan",
    "Unknown": "magenta",
}

SYNC_ORDER = ["Synced", "OutOfSync", "Unknown"]
HEALTH_ORDER = ["Healthy", "Progressing", "Degraded", "Missing", "Suspended", "Unknown"]
ALWAYS_SHOWN = {"OutOfSync", "Degraded"}

HEALTH_RANK = {
    "Degraded": 0,
    "Missing": 2,
    "Progressing": 3,
    "Unknown": 4,
    "Suspended": 5,
    "Healthy": 9,
}

SYNC_RANK = {
    "OutOfSync": 1,
    "Unknown": 4,
    "Synced": 9,
}

AUTH_HINTS = ("unauthenticated", "token is expired", "expired", "please log in", "login")


def normalize(item: dict) -> dict[str, Any]:
    metadata = item.get("metadata", {})
    spec = item.get("spec", {})
    status = item.get("status", {})
    source = spec.get("source") or next(iter(spec.get("sources") or []), {})
    return {
        "name": metadata.get("name", "?"),
        "project": spec.get("project", "default"),
        "sync": status.get("sync", {}).get("status", "Unknown"),
        "health": status.get("health", {}).get("status", "Unknown"),
        "repo": source.get("repoURL", ""),
        "path": source.get("path", ""),
        "revision": source.get("targetRevision", ""),
        "raw": item,
    }


def rank(app: dict) -> tuple[int, str]:
    return min(HEALTH_RANK.get(app["health"], 4), SYNC_RANK.get(app["sync"], 4)), app["name"]


def is_problem(app: dict) -> bool:
    return app["sync"] != "Synced" or app["health"] != "Healthy"


def matches(app: dict, state: dict) -> bool:
    if state["mode"] == "problems":
        return is_problem(app)
    if state["mode"] == "project":
        return app["project"] == state["project"]
    return True


def filter_label(state: dict) -> str:
    if state["mode"] == "problems":
        return "nur Probleme"
    if state["mode"] == "project":
        return f"Projekt {state['project']}"
    return ""


def fetch_apps() -> list[dict]:
    namespace = argocd_config.namespace()
    try:
        data = shell.capture_json(["kubectl", "get", "applications.argoproj.io", "-n", namespace, "-o", "json"], timeout=30)
    except CubiError as exc:
        raise CubiError(f"ArgoCD-Apps konnten nicht gelesen werden: {exc}")
    return [normalize(item) for item in data.get("items", [])]


def count_line(label: str, counts: Counter, order: list[str], styles: dict[str, str]) -> Text:
    line = Text(f"{label:<8}", style="dim")
    first = True
    for status in order:
        number = counts.get(status, 0)
        if number == 0 and status not in ALWAYS_SHOWN:
            continue
        if not first:
            line.append(" · ", style="dim")
        line.append(f"{status} {number}", style=styles.get(status, "") if number else "dim")
        first = False
    return line


def summary(apps: list[dict], visible: list[dict], state: dict) -> Text:
    head = Text(f"{len(apps)} Apps", style="bold")
    label = filter_label(state)
    if label:
        head.append(f"  ·  Filter: {label} ({len(visible)})", style="cyan")
    text = Text()
    text.append_text(head)
    text.append("\n")
    text.append_text(count_line("Sync", Counter(app["sync"] for app in apps), SYNC_ORDER, SYNC_STYLES))
    text.append("\n")
    text.append_text(count_line("Health", Counter(app["health"] for app in apps), HEALTH_ORDER, HEALTH_STYLES))
    return text


def make_loader(state: dict):
    def loader() -> TableData:
        apps = fetch_apps()
        state["apps"] = {app["name"]: app for app in apps}
        state["projects"] = sorted({app["project"] for app in apps})
        visible = sorted((app for app in apps if matches(app, state)), key=rank)
        rows = [
            Row(
                [
                    Text(app["name"]),
                    Text(app["project"]),
                    Text(app["sync"], style=SYNC_STYLES.get(app["sync"], "")),
                    Text(app["health"], style=HEALTH_STYLES.get(app["health"], "")),
                ],
                app["name"],
            )
            for app in visible
        ]
        return TableData(rows, summary(apps, visible, state))

    return loader


def choose_filter(state: dict) -> None:
    items = [Item("Alle Apps", ("all", None)), Item("Nur Probleme", ("problems", None))]
    items.extend(Item(f"Projekt: {project}", ("project", project)) for project in state.get("projects", []))
    choice = ui.menu("Filter", items)
    if choice is not None:
        state["mode"], state["project"] = choice


def choose_context() -> None:
    current = argocd_config.current_context()
    items = [Item(context["name"], context["name"], "aktiv" if context["name"] == current else "") for context in argocd_config.configured_contexts()]
    choice = ui.menu("ArgoCD-Kontext", items)
    if choice is not None:
        argocd_config.activate(choice)


def cli_output(args: list[str], empty: str) -> None:
    result = shell.capture(["argocd", *args], timeout=90)
    text = shell.merge_output(result)
    if result.returncode != 0 and any(hint in result.stderr.lower() for hint in AUTH_HINTS):
        text += "\n\nHinweis: Login abgelaufen oder fehlt. Im Hauptmenü unter Login erneuern."
    ui.show_text(text or empty)


def sync(name: str) -> None:
    if ui.confirm(f"{name} synchronisieren?"):
        ui.run_command(["argocd", "app", "sync", name])


def details(app: dict) -> None:
    raw = app["raw"]
    spec = raw.get("spec", {})
    status = raw.get("status", {})
    operation = status.get("operationState", {})
    policy = spec.get("syncPolicy", {}).get("automated")
    destination = spec.get("destination", {})
    info = Table.grid(padding=(0, 2))
    info.add_column(style="dim")
    info.add_column()
    info.add_row("Projekt", app["project"])
    info.add_row("Namespace", str(destination.get("namespace", "-")))
    info.add_row("Sync", Text(app["sync"], style=SYNC_STYLES.get(app["sync"], "")))
    info.add_row("Revision", str(status.get("sync", {}).get("revision", "-"))[:10])
    info.add_row("Health", Text(app["health"], style=HEALTH_STYLES.get(app["health"], "")))
    if status.get("health", {}).get("message"):
        info.add_row("Meldung", status["health"]["message"])
    info.add_row("Repo", app["repo"] or "-")
    info.add_row("Pfad", app["path"] or "-")
    info.add_row("Ziel", app["revision"] or "-")
    if policy is None:
        info.add_row("Sync-Policy", "manuell")
    else:
        flags = [name for name, key in (("prune", "prune"), ("selfHeal", "selfHeal")) if policy.get(key)]
        info.add_row("Sync-Policy", "automated" + (f" ({', '.join(flags)})" if flags else ""))
    if operation:
        finished = age(parse_iso(operation.get("finishedAt")))
        info.add_row("Letzter Sync", f"{operation.get('phase', '-')} vor {finished}")
        if operation.get("message"):
            info.add_row("", Text(operation["message"], style="dim"))
    parts: list = [Panel(info, title=Text(app["name"], style="bold"), title_align="left", box=box.ROUNDED)]
    conditions = status.get("conditions") or []
    if conditions:
        table = Table(box=box.SIMPLE_HEAD, header_style="bold", title="Bedingungen", title_justify="left")
        table.add_column("Typ")
        table.add_column("Meldung")
        for condition in conditions:
            table.add_row(condition.get("type", "-"), condition.get("message", ""))
        parts.append(table)
    affected = [
        resource
        for resource in status.get("resources") or []
        if resource.get("status") == "OutOfSync" or resource.get("health", {}).get("status") in ("Degraded", "Missing", "Progressing", "Unknown", "Suspended")
    ]
    if affected:
        table = Table(box=box.SIMPLE_HEAD, header_style="bold", title="Betroffene Ressourcen", title_justify="left")
        for column in ("Kind", "Name", "Namespace", "Sync", "Health"):
            table.add_column(column)
        for resource in affected:
            health = resource.get("health", {}).get("status", "-")
            sync_status = resource.get("status", "-")
            table.add_row(
                resource.get("kind", "-"),
                resource.get("name", "-"),
                resource.get("namespace", "-"),
                Text(sync_status, style=SYNC_STYLES.get(sync_status, "")),
                Text(health, style=HEALTH_STYLES.get(health, "")),
            )
        parts.append(table)
    ui.show(Group(*parts), f"App · {app['name']}")


def app_menu(app: dict) -> None:
    name = app["name"]
    ui.action_menu(
        f"App · {name}",
        [
            ("Details", lambda: details(app)),
            ("Sync", lambda: sync(name)),
            ("Refresh", lambda: cli_output(["app", "get", name, "--refresh"], "(keine Ausgabe)")),
            ("Hard Refresh", lambda: cli_output(["app", "get", name, "--hard-refresh"], "(keine Ausgabe)")),
            ("Diff", lambda: cli_output(["app", "diff", name], "Keine Unterschiede")),
            ("Historie", lambda: cli_output(["app", "history", name], "Keine Historie")),
            ("Quelle auf GitHub", lambda: github.source_menu(app["repo"], app["path"], app["revision"])),
        ],
        subtitle=f"{app['sync']} · {app['health']}",
    )


def run() -> None:
    state: dict[str, Any] = {"mode": "all", "project": None, "apps": {}, "projects": []}
    selected = None
    while True:
        key, value = ui.table_view(
            f"ArgoCD · Kontext {argocd_config.current_context()}",
            ["App", "Projekt", "Sync", "Health"],
            make_loader(state),
            hotkeys={"f": "Filter", "c": "Kontext"},
            interval=10,
            initial=selected,
        )
        if key == "back":
            return
        if key == "f":
            ui.guard(choose_filter, state)
        elif key == "c":
            ui.guard(choose_context)
        elif key == "enter":
            selected = value
            app = state["apps"].get(value)
            if app is not None:
                ui.guard(app_menu, app)
