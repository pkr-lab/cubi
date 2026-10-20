import json
from dataclasses import dataclass, field

from rich.text import Text

from .. import argocd_apps, ui
from ..argocd_apps import appset_note, policy_cell, policy_flags, policy_label
from ..errors import CubiError
from ..ui import Item

DISABLE = "disable"
ENABLE = "enable"
COLUMNS = ["App", "Projekt", "Auto-Sync", "Gemerkt", "Verwaltet von"]


@dataclass
class Change:
    app: dict
    action: str
    state_value: str | None
    restore: dict | None
    before: str
    after: str
    notes: list[str] = field(default_factory=list)

    @property
    def state_key(self) -> str:
        return f"{self.app['name']}.sync"


def plan(app: dict, action: str) -> Change | str:
    automated = app["automated"]
    if action == DISABLE:
        if automated is None:
            return "bereits manual"
        notes = [f"Einstellungen werden gemerkt ({policy_flags(automated) or 'ohne prune/selfHeal'})", *appset_note(app, "die Sync-Policy")]
        return Change(app, action, json.dumps(automated, sort_keys=True), None, policy_label(automated), "manual", notes)
    if automated is not None:
        return "bereits auto"
    stored = app["stored"]
    restored = stored if stored is not None else {}
    note = (
        f"gemerkte Einstellungen werden wiederhergestellt ({policy_flags(restored) or 'ohne prune/selfHeal'})"
        if stored is not None
        else "keine gemerkten Einstellungen, automated ohne prune/selfHeal"
    )
    notes = [note]
    if app["appset"]:
        notes.append(f"ApplicationSet {app['appset']}: Regel wird entfernt, der Controller setzt die Policy aus dem Template")
    elif app["managed_by"]:
        notes.append(f"wird von {app['managed_by']} verwaltet, die Änderung kann dort zurückgesetzt werden")
    return Change(app, action, None, restored, "manual", policy_label(restored), notes)


def owner_cell(app: dict) -> Text:
    if app["appset"]:
        return Text(f"AppSet {app['appset']}", style="dim")
    return Text(app["managed_by"] or "-", style="dim")


def cells_for(action: str):
    def cells(app: dict) -> list:
        applicable = (app["automated"] is not None) if action == DISABLE else (app["automated"] is None)
        style = "" if applicable else "dim"
        stored = policy_label(app["stored"]) if app["stored"] is not None else "-"
        mode = policy_cell(app["automated"])
        if not applicable:
            mode.stylize("dim")
        return [Text(app["name"], style=style), Text(app["project"], style=style), mode, Text(stored, style="dim"), owner_cell(app)]

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


def apply(changes: list[Change]) -> list[Change]:
    done = []
    for change in changes:
        name = change.app["name"]
        try:
            if change.action == DISABLE:
                argocd_apps.save_state(change.state_key, change.state_value)
                argocd_apps.pause_auto_sync(change.app)
            else:
                argocd_apps.resume_auto_sync(change.app, change.restore or {})
                argocd_apps.save_state(change.state_key, None)
        except CubiError as exc:
            ui.console.print(Text(f"✖ {name}: {exc}", style="red"))
            continue
        verb = "deaktiviert" if change.action == DISABLE else "aktiviert"
        ui.console.print(Text(f"✔ {name}: Auto-Sync {verb} ({change.after})", style="green"))
        done.append(change)
    return done


def verify(changes: list[Change]) -> None:
    if not changes:
        return
    current = argocd_apps.recheck([change.app["name"] for change in changes])
    checks = []
    for change in changes:
        app = current.get(change.app["name"])
        wanted_manual = change.action == DISABLE
        checks.append((change.app["name"], app is not None and (app["automated"] is None) == wanted_manual))
    argocd_apps.report_checks(checks, "wurde zurückgesetzt, ein Controller überschreibt die Sync-Policy")


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
    selection = argocd_apps.choose_apps(title, COLUMNS, cells_for(action), with_state=True)
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
        verify(apply(changes))
