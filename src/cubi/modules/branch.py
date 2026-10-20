import re
from dataclasses import dataclass, field
from urllib.parse import quote

from rich.text import Text

from .. import argocd_apps, shell, ui
from ..argocd_apps import DEFAULT_BRANCH, policy_cell, policy_flags, revision_label, short_repo
from ..errors import CubiError
from ..ui import Item
from . import github

SWITCH = "switch"
RESET = "reset"
COLUMNS = ["App", "Branch", "Auto-Sync", "Quelle"]
COMMIT_ID = re.compile(r"^[0-9a-f]{7,40}$")
NO_PROMPT = {"GIT_TERMINAL_PROMPT": "0", "GIT_ASKPASS": "true", "SSH_ASKPASS": "true"}


@dataclass
class Change:
    app: dict
    index: int
    source: dict
    previous: str
    target: str
    patch: list
    notes: list[str] = field(default_factory=list)


def source_pointer(app: dict, index: int) -> str:
    return f"/spec/sources/{index}" if app["multi"] else "/spec/source"


def build_patch(app: dict, index: int, target: str) -> list:
    pointer = source_pointer(app, index)
    source = app["sources"][index]
    return [
        {"op": "test", "path": f"{pointer}/repoURL", "value": source.get("repoURL", "")},
        {"op": "add", "path": f"{pointer}/targetRevision", "value": target},
    ]


def source_title(source: dict) -> str:
    detail = source.get("path") or (f"ref {source['ref']}" if source.get("ref") else "")
    revision = source.get("targetRevision") or "HEAD"
    return f"{short_repo(source.get('repoURL', ''))} {detail} @ {revision}".replace("  ", " ")


def choose_source(app: dict) -> int | None:
    if not app["multi"] or len(app["sources"]) == 1:
        return 0
    items = [Item(source_title(source), index) for index, source in enumerate(app["sources"])]
    return ui.select(f"Source von {app['name']}", items)


def ref_exists(repo_url: str, ref: str) -> bool | None:
    repo = github.repo_from_url(repo_url)
    try:
        if repo:
            quoted = quote(ref, safe="/")
            for kind in ("heads", "tags"):
                data = shell.capture_json(["gh", "api", f"repos/{repo}/git/matching-refs/{kind}/{quoted}"], timeout=20)
                if any(entry.get("ref") == f"refs/{kind}/{ref}" for entry in data):
                    return True
            return None if COMMIT_ID.match(ref) else False
        result = shell.capture(
            ["git", "ls-remote", "--exit-code", repo_url, f"refs/heads/{ref}", f"refs/tags/{ref}"],
            timeout=20,
            env=NO_PROMPT,
        )
    except CubiError:
        return None
    if result.returncode == 0:
        return True
    if result.returncode == 2:
        return None if COMMIT_ID.match(ref) else False
    return None


def auto_sync_notes(app: dict) -> list[str]:
    automated = app["automated"]
    if automated is None:
        return []
    flags = policy_flags(automated)
    notes = [f"Auto-Sync aktiv{f' ({flags})' if flags else ''}: ArgoCD synct sofort auf den neuen Branch"]
    if automated.get("prune"):
        notes.append("prune aktiv: Ressourcen, die im neuen Branch fehlen, werden gelöscht")
    return notes


def plan(app: dict, index: int, target: str) -> Change | str:
    source = app["sources"][index]
    previous = str(source.get("targetRevision") or "HEAD")
    if previous == target:
        return f"steht bereits auf {target}"
    return Change(app, index, source, previous, target, build_patch(app, index, target), auto_sync_notes(app))


def cells(app: dict) -> list:
    return [
        Text(app["name"]),
        Text(revision_label(app)),
        policy_cell(app["automated"]),
        Text(short_repo(app["repo"]) + (f" (+{len(app['sources']) - 1})" if len(app["sources"]) > 1 else ""), style="dim"),
    ]


def off_target(app: dict) -> bool:
    return any(str(source.get("targetRevision") or "HEAD") != DEFAULT_BRANCH for source in app["sources"])


def check_remote(changes: list[Change]) -> dict[tuple[str, str], bool | None]:
    results: dict[tuple[str, str], bool | None] = {}
    for change in changes:
        key = (change.source.get("repoURL", ""), change.target)
        if key not in results:
            results[key] = ref_exists(*key)
    return results


def render_plan(changes: list[Change], skipped: list[tuple[dict, str]], remote: dict[tuple[str, str], bool | None]) -> None:
    for change in changes:
        line = Text("  ")
        line.append(change.app["name"], style="bold")
        line.append(f"  {change.previous} → {change.target}", style="cyan")
        if change.app["multi"]:
            line.append(f"  ({source_title(change.source)})", style="dim")
        ui.console.print(line)
        found = remote.get((change.source.get("repoURL", ""), change.target))
        if found is False:
            ui.console.print(Text(f"    ⚠ Branch {change.target} existiert nicht in {short_repo(change.source.get('repoURL', ''))}", style="bold red"))
        elif found is None:
            ui.console.print(Text("    Existenz im Remote-Repo nicht prüfbar", style="dim"))
        for note in change.notes:
            ui.console.print(Text(f"    ⚠ {note}", style="yellow"))
    for app, reason in skipped:
        ui.console.print(Text(f"  {app['name']}  übersprungen ({reason})", style="dim"))


def apply(changes: list[Change]) -> None:
    for change in changes:
        name = change.app["name"]
        try:
            argocd_apps.patch_app(name, change.patch, "json")
        except CubiError as exc:
            ui.console.print(Text(f"✖ {name}: {exc}", style="red"))
            continue
        ui.console.print(Text(f"✔ {name}: {change.previous} → {change.target}", style="green"))


def ask_target() -> str | None:
    value = ui.prompt("Ziel-Branch", DEFAULT_BRANCH)
    if value is None:
        return None
    target = value.strip()
    if not target:
        return None
    if re.search(r"\s", target):
        raise CubiError("Ein Branch-Name darf keine Leerzeichen enthalten")
    return target


def run() -> None:
    mode = ui.select(
        "Branch",
        [
            Item("Branch umstellen", SWITCH, "spec.source.targetRevision ändern"),
            Item(f"Auf {DEFAULT_BRANCH} zurücksetzen", RESET, f"Apps abseits von {DEFAULT_BRANCH} sind vorgewählt"),
        ],
    )
    if mode is None:
        return
    title = "Apps: Branch umstellen" if mode == SWITCH else f"Apps: auf {DEFAULT_BRANCH} zurücksetzen"
    selection = argocd_apps.choose_apps(title, COLUMNS, cells, off_target if mode == RESET else None)
    if not selection:
        return
    if mode == RESET:
        target = DEFAULT_BRANCH
    else:
        target = ask_target()
        if target is None:
            return
    changes: list[Change] = []
    skipped: list[tuple[dict, str]] = []
    for app in selection:
        if not app["sources"]:
            skipped.append((app, "keine Source"))
            continue
        index = choose_source(app)
        if index is None:
            return
        result = plan(app, index, target)
        if isinstance(result, Change):
            changes.append(result)
        else:
            skipped.append((app, result))
    remote = check_remote(changes)
    ui.begin("Geplante Änderungen")
    render_plan(changes, skipped, remote)
    if not changes:
        ui.info("Nichts zu tun.")
        return
    if ui.confirm(f"{len(changes)} App(s) auf {target} umstellen?"):
        apply(changes)
