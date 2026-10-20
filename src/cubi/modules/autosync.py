import json
from dataclasses import dataclass, field

from rich.text import Text

from .. import argocd_apps, ui
from ..argocd_apps import PREVIOUS_SYNC_ANNOTATION, policy_cell, policy_flags, policy_label
from ..errors import CubiError
from ..ui import Item

DISABLE = "disable"
ENABLE = "enable"
COLUMNS = ["App", "Projekt", "Auto-Sync", "Gemerkt"]


@dataclass
class Change:
    app: dict
    action: str
    patch: dict
    before: str
    after: str
    notes: list[str] = field(default_factory=list)


def disable_patch(automated: dict) -> dict:
    return {
        "metadata": {"annotations": {PREVIOUS_SYNC_ANNOTATION: json.dumps(automated, sort_keys=True)}},
        "spec": {"syncPolicy": {"automated": None}},
    }


def enable_patch(automated: dict) -> dict:
    return {
        "metadata": {"annotations": {PREVIOUS_SYNC_ANNOTATION: None}},
        "spec": {"syncPolicy": {"automated": automated}},
    }


def owner_note(app: dict) -> list[str]:
    if app["managed_by"]:
        return [f"wird von {app['managed_by']} verwaltet, die Änderung kann dort zurückgesetzt werden"]
    return []


def plan(app: dict, action: str) -> Change | str:
    automated = app["automated"]
    if action == DISABLE:
        if automated is None:
            return "bereits manual"
        return Change(
            app,
            action,
            disable_patch(automated),
            policy_label(automated),
            "manual",
            [f"Einstellungen werden gemerkt ({policy_flags(automated) or 'ohne prune/selfHeal'})", *owner_note(app)],
        )
    if automated is not None:
        return "bereits auto"
    stored = app["stored"]
    restored = stored if stored is not None else {}
    note = (
        f"gemerkte Einstellungen werden wiederhergestellt ({policy_flags(restored) or 'ohne prune/selfHeal'})"
        if stored is not None
        else "keine gemerkten Einstellungen, automated ohne prune/selfHeal"
    )
    return Change(app, action, enable_patch(restored), "manual", policy_label(restored), [note, *owner_note(app)])


def cells_for(action: str):
    def cells(app: dict) -> list:
        applicable = (app["automated"] is not None) if action == DISABLE else (app["automated"] is None)
        style = "" if applicable else "dim"
        stored = policy_label(app["stored"]) if app["stored"] is not None else "-"
        mode = policy_cell(app["automated"])
        if not applicable:
            mode.stylize("dim")
        return [Text(app["name"], style=style), Text(app["project"], style=style), mode, Text(stored, style="dim")]

    return cells


def render_plan(changes: list[Change], skipped: list[tuple[dict, str]]) -> None:
    for change in changes:
        line = Text("  ")
        line.append(change.app["name"], style="bold")
        line.append(f"  {change.before} → {change.after}", style="cyan")
        ui.console.print(line)
        for note in change.notes:
            ui.console.print(Text(f"    {note}", style="yellow" if "verwaltet" in note else "dim"))
    for app, reason in skipped:
        ui.console.print(Text(f"  {app['name']}  übersprungen ({reason})", style="dim"))


def apply(changes: list[Change]) -> None:
    for change in changes:
        name = change.app["name"]
        try:
            argocd_apps.patch_app(name, change.patch, "merge")
        except CubiError as exc:
            ui.console.print(Text(f"✖ {name}: {exc}", style="red"))
            continue
        verb = "deaktiviert" if change.action == DISABLE else "aktiviert"
        ui.console.print(Text(f"✔ {name}: Auto-Sync {verb} ({change.after})", style="green"))


def run() -> None:
    action = ui.select(
        "Auto-Sync",
        [
            Item("Auto-Sync deaktivieren", DISABLE, "entfernt spec.syncPolicy.automated, merkt prune/selfHeal"),
            Item("Auto-Sync aktivieren", ENABLE, "stellt die gemerkten Einstellungen wieder her"),
        ],
    )
    if action is None:
        return
    title = "Apps: Auto-Sync deaktivieren" if action == DISABLE else "Apps: Auto-Sync aktivieren"
    selection = argocd_apps.choose_apps(title, COLUMNS, cells_for(action))
    if not selection:
        return
    changes: list[Change] = []
    skipped: list[tuple[dict, str]] = []
    for app in selection:
        result = plan(app, action)
        if isinstance(result, Change):
            changes.append(result)
        else:
            skipped.append((app, result))
    ui.begin("Geplante Änderungen")
    render_plan(changes, skipped)
    if not changes:
        ui.info("Nichts zu tun.")
        return
    if ui.confirm(f"{len(changes)} App(s) ändern?"):
        apply(changes)
