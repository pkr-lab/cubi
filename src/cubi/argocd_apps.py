import json
from typing import Any, Callable, Sequence

from rich.text import Text

from . import argocd_config, shell, ui
from .errors import CubiError
from .ui import Row, TableData

APP_RESOURCE = "applications.argoproj.io"
PREVIOUS_SYNC_ANNOTATION = "cubi.pkr-lab.io/previous-automated-sync"
TRACKING_ANNOTATION = "argocd.argoproj.io/tracking-id"
DEFAULT_BRANCH = "main"


def sources_of(spec: dict) -> list[dict]:
    sources = spec.get("sources")
    if sources:
        return [source for source in sources if isinstance(source, dict)]
    source = spec.get("source")
    return [source] if isinstance(source, dict) else []


def stored_sync(annotations: dict) -> dict | None:
    value = annotations.get(PREVIOUS_SYNC_ANNOTATION)
    if not value:
        return None
    try:
        data = json.loads(value)
    except ValueError:
        return None
    return data if isinstance(data, dict) else None


def managed_by(metadata: dict) -> str | None:
    owners = [owner for owner in metadata.get("ownerReferences") or [] if isinstance(owner, dict)]
    if owners:
        return f"{owners[0].get('kind', 'Owner')} {owners[0].get('name', '')}".strip()
    if TRACKING_ANNOTATION in (metadata.get("annotations") or {}):
        return "einer übergeordneten App"
    return None


def normalize(item: dict) -> dict[str, Any]:
    metadata = item.get("metadata", {})
    spec = item.get("spec", {})
    status = item.get("status", {})
    sources = sources_of(spec)
    source = sources[0] if sources else {}
    automated = (spec.get("syncPolicy") or {}).get("automated")
    return {
        "name": metadata.get("name", "?"),
        "project": spec.get("project", "default"),
        "sync": status.get("sync", {}).get("status", "Unknown"),
        "health": status.get("health", {}).get("status", "Unknown"),
        "repo": source.get("repoURL", ""),
        "path": source.get("path", ""),
        "revision": source.get("targetRevision", ""),
        "sources": sources,
        "multi": bool(spec.get("sources")),
        "automated": automated if isinstance(automated, dict) else None,
        "stored": stored_sync(metadata.get("annotations") or {}),
        "managed_by": managed_by(metadata),
        "raw": item,
    }


def fetch_apps() -> list[dict]:
    namespace = argocd_config.namespace()
    try:
        data = shell.capture_json(["kubectl", "get", APP_RESOURCE, "-n", namespace, "-o", "json"], timeout=30)
    except CubiError as exc:
        raise CubiError(f"ArgoCD-Apps konnten nicht gelesen werden: {exc}")
    return [normalize(item) for item in data.get("items", [])]


def patch_app(name: str, patch: Any, kind: str) -> None:
    shell.ensure(
        shell.capture(
            ["kubectl", "patch", APP_RESOURCE, name, "-n", argocd_config.namespace(), f"--type={kind}", "-p", json.dumps(patch)],
            timeout=30,
        ),
        f"{name}: Änderung fehlgeschlagen",
    )


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
) -> list[dict] | None:
    state: dict[str, dict] = {}

    def loader() -> TableData:
        apps = sorted(fetch_apps(), key=lambda app: app["name"])
        state.clear()
        state.update({app["name"]: app for app in apps})
        rows = [Row(cells(app), app["name"], app["name"]) for app in apps]
        checked = [app["name"] for app in apps if preselect and preselect(app)]
        return TableData(rows, Text(f"{len(apps)} Apps", style="dim"), checked)

    key, names = ui.table_select(title, columns, loader, multi=True, min_count=1)
    if key != "enter":
        return None
    return [state[name] for name in names if name in state]
