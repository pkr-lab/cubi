import json
from dataclasses import dataclass, field

from rich.text import Text

from .. import argocd_apps, shell, ui
from ..argocd_apps import appset_note, policy_cell, policy_flags
from ..errors import CubiError
from ..ui import Item

DOWN = "down"
UP = "up"
KINDS = {"Deployment": "deployments.apps", "StatefulSet": "statefulsets.apps"}
COLUMNS = ["App", "Workloads", "Pods", "Auto-Sync"]


@dataclass(frozen=True)
class Workload:
    kind: str
    namespace: str
    name: str

    @property
    def key(self) -> str:
        return f"{self.kind}/{self.namespace}/{self.name}"

    @property
    def target(self) -> str:
        return f"{KINDS[self.kind]}/{self.name}"


@dataclass
class Change:
    app: dict
    action: str
    state_value: str | None
    pause: bool
    resume: dict | None
    ops: list[tuple[Workload, int]]
    before: str
    after: str
    notes: list[str] = field(default_factory=list)

    @property
    def state_key(self) -> str:
        return f"{self.app['name']}.scale"


def from_key(key: str) -> Workload:
    kind, namespace, name = key.split("/", 2)
    return Workload(kind, namespace, name)


def workloads(app: dict) -> list[Workload]:
    spec = app["raw"].get("spec", {})
    fallback = str((spec.get("destination") or {}).get("namespace") or "default")
    found: list[Workload] = []
    for resource in app["raw"].get("status", {}).get("resources") or []:
        if resource.get("group") == "apps" and resource.get("kind") in KINDS and resource.get("name"):
            workload = Workload(resource["kind"], resource.get("namespace") or fallback, resource["name"])
            if workload not in found:
                found.append(workload)
    return found


def fetch_live(items: list[Workload]) -> dict[str, int]:
    live: dict[str, int] = {}
    for namespace in sorted({item.namespace for item in items}):
        data = shell.capture_json(["kubectl", "get", ",".join(KINDS.values()), "-n", namespace, "-o", "json"], timeout=30)
        for entry in data.get("items", []):
            workload = Workload(entry.get("kind", ""), namespace, entry["metadata"]["name"])
            live[workload.key] = int(entry.get("spec", {}).get("replicas", 1))
    return live


def plan_down(app: dict, live: dict[str, int]) -> Change | str:
    items = workloads(app)
    if not items:
        return "keine Deployments/StatefulSets"
    running = {item.key: live[item.key] for item in items if live.get(item.key, 0) > 0}
    if not running:
        return "bereits 0 Pods"
    memory = app["scale"] or {"replicas": {}, "autoSync": None}
    replicas = {**memory["replicas"], **running}
    automated = app["automated"] if app["automated"] is not None else memory["autoSync"]
    state_value = json.dumps({"replicas": replicas, "autoSync": automated}, sort_keys=True)
    pause = app["automated"] is not None
    notes = []
    if pause:
        flags = policy_flags(app["automated"])
        notes.append(f"Auto-Sync wird deaktiviert ({flags or 'ohne prune/selfHeal'}), sonst stellt ArgoCD die Pods wieder her")
        notes.extend(appset_note(app, "die Sync-Policy"))
    ops = [(from_key(key), 0) for key in running]
    return Change(app, DOWN, state_value, pause, None, ops, f"{sum(running.values())} Pods", "0 Pods", notes)


def plan_up(app: dict) -> Change | str:
    memory = app["scale"]
    if memory is None:
        return "keine gemerkten Replikas"
    resume = memory["autoSync"] if app["automated"] is None else None
    notes = []
    if resume is not None:
        notes.append(f"Auto-Sync wird wieder aktiviert ({policy_flags(resume) or 'ohne prune/selfHeal'})")
        if app["appset"]:
            notes.append(f"ApplicationSet {app['appset']}: Regel wird entfernt, der Controller setzt die Policy aus dem Template")
    ops = [(from_key(key), count) for key, count in memory["replicas"].items()]
    return Change(app, UP, None, False, resume, ops, "0 Pods", f"{sum(memory['replicas'].values())} Pods", notes)


def cells_for(action: str):
    def cells(app: dict) -> list:
        applicable = (bool(workloads(app)) and app["scale"] is None) if action == DOWN else app["scale"] is not None
        style = "" if applicable else "dim"
        if app["scale"] is not None:
            status = Text(f"auf 0 (vorher {sum(app['scale']['replicas'].values())})", style="yellow" if applicable else "dim")
        else:
            status = Text("normal", style=style or "dim")
        mode = policy_cell(app["automated"])
        if not applicable:
            mode.stylize("dim")
        return [Text(app["name"], style=style), Text(str(len(workloads(app))), style=style), status, mode]

    return cells


def render_plan(changes: list[Change], skipped: list[tuple[dict, str]]) -> None:
    for change in changes:
        line = Text("  ")
        line.append(change.app["name"], style="bold")
        line.append(f"  {change.before} → {change.after}", style="cyan")
        line.append(f"  ({len(change.ops)} Workloads)", style="dim")
        ui.console.print(line)
        for workload, count in change.ops:
            ui.console.print(Text(f"    {workload.kind}/{workload.name} ({workload.namespace}) → {count}", style="dim"))
        for note in change.notes:
            ui.console.print(Text(f"    ⚠ {note}", style="yellow"))
    for app, reason in skipped:
        ui.console.print(Text(f"  {app['name']}  übersprungen ({reason})", style="dim"))


def scale_workload(workload: Workload, count: int) -> None:
    shell.ensure(
        shell.capture(["kubectl", "scale", workload.target, "-n", workload.namespace, f"--replicas={count}"], timeout=30),
        f"{workload.kind}/{workload.name}: Skalieren fehlgeschlagen",
    )


def scale_all(change: Change) -> bool:
    failed = False
    for workload, count in change.ops:
        try:
            scale_workload(workload, count)
        except CubiError as exc:
            failed = True
            ui.console.print(Text(f"✖ {change.app['name']}: {exc}", style="red"))
    return not failed


def apply(changes: list[Change]) -> list[Change]:
    done = []
    for change in changes:
        name = change.app["name"]
        try:
            if change.action == DOWN:
                argocd_apps.save_state(change.state_key, change.state_value)
                if change.pause:
                    argocd_apps.pause_auto_sync(change.app)
                finished = scale_all(change)
            else:
                finished = scale_all(change)
                if finished:
                    if change.resume is not None:
                        argocd_apps.resume_auto_sync(change.app, change.resume)
                    argocd_apps.save_state(change.state_key, None)
        except CubiError as exc:
            ui.console.print(Text(f"✖ {name}: {exc}", style="red"))
            continue
        if finished:
            ui.console.print(Text(f"✔ {name}: {change.before} → {change.after}", style="green"))
            done.append(change)
        elif change.action == UP:
            ui.console.print(Text(f"  {name}: gemerkte Replikas bleiben erhalten, erneut ausführen", style="yellow"))
    return done


def verify(changes: list[Change]) -> None:
    if not changes:
        return
    current = argocd_apps.recheck([change.app["name"] for change in changes])
    checks = []
    for change in changes:
        app = current.get(change.app["name"])
        if app is None:
            checks.append((change.app["name"], False))
            continue
        try:
            live = fetch_live([workload for workload, _ in change.ops])
        except CubiError:
            checks.append((change.app["name"], False))
            continue
        if change.action == DOWN:
            ok = all(live.get(workload.key, 0) == 0 for workload, _ in change.ops)
        else:
            ok = all(live.get(workload.key) == count for workload, count in change.ops)
        checks.append((change.app["name"], ok))
    argocd_apps.report_checks(checks, "Replikas weichen ab, ein Controller überschreibt die Änderung")


def run() -> None:
    action = ui.select(
        "Pods skalieren",
        [
            Item("Auf 0 Pods stellen", DOWN, "Deployments/StatefulSets, merkt die Replikas, deaktiviert Auto-Sync"),
            Item("Pods wiederherstellen", UP, "stellt die gemerkten Replikas und Auto-Sync wieder her"),
        ],
    )
    if action is None:
        return
    title = "Apps: auf 0 Pods stellen" if action == DOWN else "Apps: Pods wiederherstellen"
    selection = argocd_apps.choose_apps(title, COLUMNS, cells_for(action), with_state=True)
    if not selection:
        return
    changes: list[Change] = []
    skipped: list[tuple[dict, str]] = []
    for app in selection:
        if action == DOWN:
            items = workloads(app)
            result = plan_down(app, fetch_live(items) if items else {})
        else:
            result = plan_up(app)
        if isinstance(result, Change):
            changes.append(result)
        else:
            skipped.append((app, result))
    ui.begin("Geplante Änderungen")
    render_plan(changes, skipped)
    if not changes:
        ui.info("Nichts zu tun.")
        return
    question = f"{len(changes)} App(s) auf 0 Pods stellen?" if action == DOWN else f"{len(changes)} App(s) wiederherstellen?"
    if ui.confirm(question):
        verify(apply(changes))
