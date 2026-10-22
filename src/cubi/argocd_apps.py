from __future__ import annotations

import json
import time
from typing import Any, Callable, Sequence

from rich.text import Text

from . import argocd_config, shell, ui
from .errors import CubiError
from .ui import Row, TableData

APP_RESOURCE = "applications.argoproj.io"
APPSET_RESOURCE = "applicationsets.argoproj.io"
STATE_CONFIGMAP = "cubi-state"
SYNC_POINTER = "/spec/syncPolicy"
TRACKING_ANNOTATION = "argocd.argoproj.io/tracking-id"
DEFAULT_BRANCH = "main"
SETTLE_SECONDS = 8


def sources_of(spec: dict) -> list[dict]:
    sources = spec.get("sources")
    if sources:
        return [source for source in sources if isinstance(source, dict)]
    source = spec.get("source")
    return [source] if isinstance(source, dict) else []


def parse_sync(value: str | None) -> dict | None:
    if not value:
        return None
    try:
        data = json.loads(value)
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


def parse_scale(value: str | None) -> dict | None:
    data = parse_sync(value)
    replicas = data.get("replicas") if data else None
    if not isinstance(replicas, dict) or not replicas:
        return None
    try:
        counts = {str(key): int(count) for key, count in replicas.items()}
    except (TypeError, ValueError):
        return None
    auto = data.get("autoSync")
    return {"replicas": counts, "autoSync": auto if isinstance(auto, dict) else None}


def appset_of(metadata: dict) -> str | None:
    for owner in metadata.get("ownerReferences") or []:
        if isinstance(owner, dict) and owner.get("kind") == "ApplicationSet" and owner.get("name"):
            return str(owner["name"])
    return None


def managed_by(metadata: dict) -> str | None:
    owners = [owner for owner in metadata.get("ownerReferences") or [] if isinstance(owner, dict) and owner.get("kind") != "ApplicationSet"]
    if owners:
        return f"{owners[0].get('kind', 'Owner')} {owners[0].get('name', '')}".strip()
    if TRACKING_ANNOTATION in (metadata.get("annotations") or {}):
        return "einer übergeordneten App"
    return None


def normalize(item: dict, state: dict[str, str] | None = None) -> dict[str, Any]:
    state = state or {}
    metadata = item.get("metadata", {})
    spec = item.get("spec", {})
    status = item.get("status", {})
    name = metadata.get("name", "?")
    sources = sources_of(spec)
    source = sources[0] if sources else {}
    automated = (spec.get("syncPolicy") or {}).get("automated")
    return {
        "name": name,
        "project": spec.get("project", "default"),
        "sync": status.get("sync", {}).get("status", "Unknown"),
        "health": status.get("health", {}).get("status", "Unknown"),
        "repo": source.get("repoURL", ""),
        "path": source.get("path", ""),
        "revision": source.get("targetRevision", ""),
        "sources": sources,
        "multi": bool(spec.get("sources")),
        "automated": automated if isinstance(automated, dict) else None,
        "stored": parse_sync(state.get(f"{name}.sync")),
        "scale": parse_scale(state.get(f"{name}.scale")),
        "appset": appset_of(metadata),
        "managed_by": managed_by(metadata),
        "raw": item,
    }


def not_found(result: Any) -> bool:
    return "not found" in result.stderr.lower() or "notfound" in result.stderr.lower()


def load_state() -> dict[str, str]:
    result = shell.capture(["kubectl", "get", "configmap", STATE_CONFIGMAP, "-n", argocd_config.namespace(), "-o", "json"], timeout=30)
    if result.returncode != 0:
        if not_found(result):
            return {}
        raise CubiError(f"Zustand ({STATE_CONFIGMAP}) konnte nicht gelesen werden: {shell.first_line(result.stderr)}")
    try:
        data = json.loads(result.stdout).get("data") or {}
    except ValueError:
        return {}
    return {str(key): str(value) for key, value in data.items()}


def save_state(key: str, value: str | None) -> None:
    namespace = argocd_config.namespace()
    if value is not None:
        created = shell.capture(["kubectl", "create", "configmap", STATE_CONFIGMAP, "-n", namespace], timeout=30)
        if created.returncode != 0 and "alreadyexists" not in created.stderr.lower().replace(" ", ""):
            raise CubiError(f"Zustand ({STATE_CONFIGMAP}) konnte nicht angelegt werden: {shell.first_line(created.stderr)}")
    result = shell.capture(
        ["kubectl", "patch", "configmap", STATE_CONFIGMAP, "-n", namespace, "--type=merge", "-p", json.dumps({"data": {key: value}})],
        timeout=30,
    )
    if result.returncode != 0 and not (value is None and not_found(result)):
        raise CubiError(f"Zustand ({STATE_CONFIGMAP}) konnte nicht geschrieben werden: {shell.first_line(result.stderr)}")


def fetch_apps(with_state: bool = False) -> list[dict]:
    namespace = argocd_config.namespace()
    try:
        data = shell.capture_json(["kubectl", "get", APP_RESOURCE, "-n", namespace, "-o", "json"], timeout=30)
    except CubiError as exc:
        raise CubiError(f"ArgoCD-Apps konnten nicht gelesen werden: {exc}")
    state = load_state() if with_state else None
    return [normalize(item, state) for item in data.get("items", [])]


def patch_app(name: str, patch: Any, kind: str) -> None:
    shell.ensure(
        shell.capture(
            ["kubectl", "patch", APP_RESOURCE, name, "-n", argocd_config.namespace(), f"--type={kind}", "-p", json.dumps(patch)],
            timeout=30,
        ),
        f"{name}: Änderung fehlgeschlagen",
    )


def read_appset(appset: str) -> dict:
    return shell.capture_json(["kubectl", "get", APPSET_RESOURCE, appset, "-n", argocd_config.namespace(), "-o", "json"], timeout=30)


def hold_rule(app: str, pointer: str) -> dict:
    return {"name": app, "jsonPointers": [pointer]}


def set_hold(appset: str, app: str, pointer: str, hold: bool) -> bool:
    data = read_appset(appset)
    rules = list((data.get("spec") or {}).get("ignoreApplicationDifferences") or [])
    rule = hold_rule(app, pointer)
    if hold == (rule in rules):
        return False
    rules = [*rules, rule] if hold else [item for item in rules if item != rule]
    operations: list[dict] = [{"op": "test", "path": "/metadata/resourceVersion", "value": data["metadata"]["resourceVersion"]}]
    if rules:
        operations.append({"op": "add", "path": "/spec/ignoreApplicationDifferences", "value": rules})
    else:
        operations.append({"op": "remove", "path": "/spec/ignoreApplicationDifferences"})
    shell.ensure(
        shell.capture(
            ["kubectl", "patch", APPSET_RESOURCE, appset, "-n", argocd_config.namespace(), "--type=json", "-p", json.dumps(operations)],
            timeout=30,
        ),
        f"ApplicationSet {appset}: Regel für {app} konnte nicht geändert werden (parallel geändert?)",
    )
    return True


def template_revision(appset: str, multi: bool, index: int) -> str | None:
    template = ((read_appset(appset).get("spec") or {}).get("template") or {}).get("spec") or {}
    sources = template.get("sources") if multi else [template.get("source")]
    try:
        value = (sources or [])[index].get("targetRevision")
    except (IndexError, AttributeError):
        return None
    return value if isinstance(value, str) and "{{" not in value else None


def pause_auto_sync(app: dict) -> None:
    if app["appset"]:
        set_hold(app["appset"], app["name"], SYNC_POINTER, True)
    patch_app(app["name"], {"spec": {"syncPolicy": {"automated": None}}}, "merge")


def resume_auto_sync(app: dict, automated: dict) -> None:
    patch_app(app["name"], {"spec": {"syncPolicy": {"automated": automated}}}, "merge")
    if app["appset"]:
        set_hold(app["appset"], app["name"], SYNC_POINTER, False)


def appset_note(app: dict, what: str) -> list[str]:
    if app["appset"]:
        return [f"ApplicationSet {app['appset']}: {what} wird per ignoreApplicationDifferences festgehalten"]
    if app["managed_by"]:
        return [f"wird von {app['managed_by']} verwaltet, die Änderung kann dort zurückgesetzt werden"]
    return []


def recheck(names: Sequence[str]) -> dict[str, dict]:
    ui.console.print(Text("Prüfe, ob die Änderung hält …", style="dim"))
    time.sleep(SETTLE_SECONDS)
    wanted = set(names)
    return {app["name"]: app for app in fetch_apps(with_state=True) if app["name"] in wanted}


def report_checks(checks: Sequence[tuple[str, bool]], hint: str) -> None:
    for name, ok in checks:
        if ok:
            ui.console.print(Text(f"✔ {name}: Änderung hält", style="green"))
        else:
            ui.console.print(Text(f"⚠ {name}: {hint}", style="bold yellow"))


def policy_flags(automated: dict | None) -> str:
    if automated is None:
        return ""
    flags = [key for key in ("prune", "selfHeal", "allowEmpty") if automated.get(key)]
    return ", ".join(flags)


def policy_label(automated: dict | None) -> str:
    if automated is None:
        return "manual"
    flags = policy_flags(automated)
    return f"auto ({flags})" if flags else "auto"


def policy_cell(automated: dict | None) -> Text:
    return Text(policy_label(automated), style="green" if automated is not None else "yellow")


def revision_label(app: dict) -> str:
    revisions = [str(source.get("targetRevision") or "HEAD") for source in app["sources"]]
    return " | ".join(revisions) if revisions else "-"


def short_repo(url: str) -> str:
    text = url.rstrip("/")
    if text.endswith(".git"):
        text = text[:-4]
    parts = text.replace(":", "/").split("/")
    return "/".join(parts[-2:]) if len(parts) >= 2 else text or "-"


def choose_apps(
    title: str,
    columns: Sequence,
    cells: Callable[[dict], list],
    preselect: Callable[[dict], bool] | None = None,
    with_state: bool = False,
) -> list[dict] | None:
    state: dict[str, dict] = {}

    def loader() -> TableData:
        apps = sorted(fetch_apps(with_state), key=lambda app: app["name"])
        state.clear()
        state.update({app["name"]: app for app in apps})
        rows = [Row(cells(app), app["name"], app["name"]) for app in apps]
        checked = [app["name"] for app in apps if preselect and preselect(app)]
        return TableData(rows, Text(f"{len(apps)} Apps", style="dim"), checked)

    key, names = ui.table_select(title, columns, loader, multi=True, min_count=1)
    if key != "enter":
        return None
    return [state[name] for name in names if name in state]
