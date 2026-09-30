import re
from typing import Callable, Sequence
from urllib.parse import quote

from rich.text import Text

from .. import config, shell, ui
from ..errors import CubiError
from ..timeutil import age
from ..ui import Item, Row, TableData

RUN_FIELDS = "databaseId,displayTitle,status,conclusion,workflowName,headBranch,event,createdAt"
PR_FIELDS = "number,title,author,headRefName,isDraft,updatedAt,reviewDecision"
ISSUE_FIELDS = "number,title,author,labels,updatedAt"

RUN_STYLES = {
    "success": "green",
    "failure": "red",
    "timed_out": "red",
    "startup_failure": "red",
    "cancelled": "dim",
    "skipped": "dim",
    "neutral": "dim",
    "stale": "dim",
    "in_progress": "yellow",
    "queued": "yellow",
    "waiting": "yellow",
    "pending": "yellow",
    "action_required": "yellow",
}

REVIEW_STYLES = {
    "approved": "green",
    "changes": "red",
    "review": "yellow",
    "draft": "dim",
    "-": "dim",
}

REVIEW_LABELS = {
    "APPROVED": "approved",
    "CHANGES_REQUESTED": "changes",
    "REVIEW_REQUIRED": "review",
}


def default_repo() -> str:
    return str(config.section("github").get("default_repo") or "")


def known_repos() -> list[str]:
    settings = config.section("github")
    repos = [str(repo) for repo in settings.get("repos") or []]
    default = default_repo()
    if default and default not in repos:
        repos.insert(0, default)
    return repos


def repo_from_url(url: str) -> str | None:
    match = re.search(r"github\.com[:/]([^/]+/[^/]+?)(?:\.git)?/?$", url or "")
    return match.group(1) if match else None


def pick_repo(current: str | None) -> str | None:
    repos = known_repos()
    try:
        listed = shell.capture_json(["gh", "repo", "list", "--limit", "100", "--json", "nameWithOwner"], timeout=20)
    except CubiError:
        listed = []
    for entry in listed:
        name = entry.get("nameWithOwner")
        if name and name not in repos:
            repos.append(name)
    if not repos:
        raise CubiError("Keine Repositories gefunden")
    return ui.menu("Repo wählen", [Item(repo, repo, "aktuell" if repo == current else "") for repo in repos])


def author_name(entry: dict) -> str:
    author = entry.get("author") or {}
    return author.get("login") or author.get("name") or "-"


def list_view(
    repo: str,
    title: str,
    columns: Sequence,
    loader: Callable[[], TableData],
    actions: Callable[[int], list],
) -> None:
    selected = None
    while True:
        key, value = ui.table_view(f"GitHub · {title} · {repo}", columns, loader, interval=30, initial=selected)
        if key != "enter":
            return
        selected = value
        ui.action_menu(f"{title} · {value}", actions(value))


def run_status(entry: dict) -> Text:
    completed = entry.get("status") == "completed" and entry.get("conclusion")
    value = entry["conclusion"] if completed else entry.get("status", "-")
    return Text(value, style=RUN_STYLES.get(value, ""))


def runs(repo: str) -> None:
    def loader() -> TableData:
        data = shell.capture_json(["gh", "run", "list", "-R", repo, "--limit", "40", "--json", RUN_FIELDS], timeout=30)
        rows = [
            Row(
                [
                    Text(entry.get("workflowName", "")),
                    Text(entry.get("displayTitle", "")),
                    Text(entry.get("headBranch", "")),
                    Text(entry.get("event", "")),
                    run_status(entry),
                    Text(age(entry.get("createdAt"))),
                ],
                entry["databaseId"],
            )
            for entry in data
        ]
        return TableData(rows, Text(f"{len(rows)} letzte Runs", style="dim"))

    def actions(run_id: int) -> list:
        ident = str(run_id)
        return [
            ("Details", lambda: ui.show_command(["gh", "run", "view", ident, "-R", repo])),
            ("Log fehlgeschlagener Schritte", lambda: ui.show_command(["gh", "run", "view", ident, "-R", repo, "--log-failed"], "Keine fehlgeschlagenen Schritte")),
            ("Neu starten", lambda: rerun(repo, ident)),
            ("Im Browser öffnen", lambda: ui.run_command(["gh", "run", "view", ident, "-R", repo, "--web"])),
        ]

    list_view(repo, "Workflow-Runs", ["Workflow", "Titel", "Branch", "Event", "Status", "Alter"], loader, actions)


def rerun(repo: str, ident: str) -> None:
    if ui.confirm(f"Run {ident} neu starten?"):
        ui.run_command(["gh", "run", "rerun", ident, "-R", repo])


def review_cell(entry: dict) -> Text:
    if entry.get("isDraft"):
        label = "draft"
    else:
        label = REVIEW_LABELS.get(entry.get("reviewDecision") or "", "-")
    return Text(label, style=REVIEW_STYLES[label])


def pr_detail(repo: str, number: int) -> dict:
    return shell.capture_json(["gh", "pr", "view", str(number), "-R", repo, "--json", "number,isDraft"], timeout=20)


def toggle_draft(repo: str, number: int) -> None:
    current = pr_detail(repo, number)
    if current.get("isDraft"):
        if ui.confirm(f"PR #{number} als bereit für Review markieren?"):
            shell.ensure(shell.capture(["gh", "pr", "ready", str(number), "-R", repo], timeout=20), "Konnte nicht als bereit markiert werden")
    elif ui.confirm(f"PR #{number} als Entwurf markieren?"):
        shell.ensure(shell.capture(["gh", "pr", "ready", str(number), "-R", repo, "--undo"], timeout=20), "Konnte nicht als Entwurf markiert werden")


def pulls(repo: str) -> None:
    def loader() -> TableData:
        data = shell.capture_json(["gh", "pr", "list", "-R", repo, "--limit", "40", "--json", PR_FIELDS], timeout=30)
        rows = [
            Row(
                [
                    Text(f"#{entry['number']}"),
                    Text(entry.get("title", "")),
                    Text(author_name(entry)),
                    Text(entry.get("headRefName", "")),
                    review_cell(entry),
                    Text(age(entry.get("updatedAt"))),
                ],
                entry["number"],
            )
            for entry in data
        ]
        return TableData(rows, Text(f"{len(rows)} offene Pull Requests", style="dim"))

    def actions(number: int) -> list:
        ident = str(number)
        return [
            ("Details", lambda: ui.show_command(["gh", "pr", "view", ident, "-R", repo])),
            ("Checks", lambda: ui.show_command(["gh", "pr", "checks", ident, "-R", repo], "Keine Checks")),
            ("Diff", lambda: ui.show_command(["gh", "pr", "diff", ident, "-R", repo], "Kein Diff")),
            ("Entwurf umschalten", lambda: toggle_draft(repo, number)),
            ("Im Browser öffnen", lambda: ui.run_command(["gh", "pr", "view", ident, "-R", repo, "--web"])),
        ]

    list_view(repo, "Pull Requests", ["#", "Titel", "Autor", "Branch", "Review", "Alter"], loader, actions)


def issue_detail(repo: str, number: int) -> dict:
    return shell.capture_json(
        ["gh", "issue", "view", str(number), "-R", repo, "--json", "number,title,body,state,labels,assignees"],
        timeout=20,
    )


def all_labels(repo: str) -> list[str]:
    data = shell.capture_json(["gh", "label", "list", "-R", repo, "--json", "name", "--limit", "100"], timeout=20)
    return sorted(entry["name"] for entry in data)


def assignable_users(repo: str) -> list[str]:
    data = shell.capture_json(["gh", "api", f"repos/{repo}/assignees", "--paginate"], timeout=20)
    return sorted(entry["login"] for entry in data)


def edit_title(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    title = ui.prompt(f"Titel · #{number}", current.get("title", ""))
    if title is None or title == current.get("title") or not title.strip():
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--title", title], timeout=20),
        "Titel konnte nicht geändert werden",
    )


def edit_body(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    body = ui.edit_text(current.get("body") or "")
    if body == (current.get("body") or ""):
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--body", body], timeout=30),
        "Beschreibung konnte nicht geändert werden",
    )


def add_label(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    have = {label["name"] for label in current.get("labels") or []}
    choices = [name for name in all_labels(repo) if name not in have]
    if not choices:
        raise CubiError("Keine weiteren Labels verfügbar")
    label = ui.menu(f"Label hinzufügen · #{number}", [Item(name, name) for name in choices])
    if label is None:
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--add-label", label], timeout=20),
        "Label konnte nicht hinzugefügt werden",
    )


def remove_label(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    have = [label["name"] for label in current.get("labels") or []]
    if not have:
        raise CubiError("Issue hat keine Labels")
    label = ui.menu(f"Label entfernen · #{number}", [Item(name, name) for name in have])
    if label is None:
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--remove-label", label], timeout=20),
        "Label konnte nicht entfernt werden",
    )


def add_assignee(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    have = {entry["login"] for entry in current.get("assignees") or []}
    choices = [login for login in assignable_users(repo) if login not in have]
    if not choices:
        raise CubiError("Keine weiteren Personen verfügbar")
    login = ui.menu(f"Zuweisen · #{number}", [Item(login, login) for login in choices])
    if login is None:
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--add-assignee", login], timeout=20),
        "Zuweisung fehlgeschlagen",
    )


def remove_assignee(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    have = [entry["login"] for entry in current.get("assignees") or []]
    if not have:
        raise CubiError("Issue ist niemandem zugewiesen")
    login = ui.menu(f"Zuweisung entfernen · #{number}", [Item(login, login) for login in have])
    if login is None:
        return
    shell.ensure(
        shell.capture(["gh", "issue", "edit", str(number), "-R", repo, "--remove-assignee", login], timeout=20),
        "Zuweisung konnte nicht entfernt werden",
    )


def toggle_state(repo: str, number: int) -> None:
    current = issue_detail(repo, number)
    if current.get("state") == "CLOSED":
        if ui.confirm(f"Issue #{number} wieder öffnen?"):
            shell.ensure(shell.capture(["gh", "issue", "reopen", str(number), "-R", repo], timeout=20), "Öffnen fehlgeschlagen")
    elif ui.confirm(f"Issue #{number} schließen?"):
        shell.ensure(shell.capture(["gh", "issue", "close", str(number), "-R", repo], timeout=20), "Schließen fehlgeschlagen")


def comment_issue(repo: str, number: int) -> None:
    body = ui.edit_text("")
    if not body.strip():
        return
    shell.ensure(
        shell.capture(["gh", "issue", "comment", str(number), "-R", repo, "--body", body], timeout=20),
        "Kommentar konnte nicht hinzugefügt werden",
    )


def edit_menu(repo: str, number: int) -> None:
    ui.action_menu(
        f"Bearbeiten · #{number}",
        [
            ("Titel ändern", lambda: edit_title(repo, number)),
            ("Beschreibung bearbeiten", lambda: edit_body(repo, number)),
            ("Label hinzufügen", lambda: add_label(repo, number)),
            ("Label entfernen", lambda: remove_label(repo, number)),
            ("Zuweisen", lambda: add_assignee(repo, number)),
            ("Zuweisung entfernen", lambda: remove_assignee(repo, number)),
            ("Schließen/Öffnen", lambda: toggle_state(repo, number)),
        ],
    )


def issues(repo: str) -> None:
    def loader() -> TableData:
        data = shell.capture_json(["gh", "issue", "list", "-R", repo, "--limit", "40", "--json", ISSUE_FIELDS], timeout=30)
        rows = [
            Row(
                [
                    Text(f"#{entry['number']}"),
                    Text(entry.get("title", "")),
                    Text(author_name(entry)),
                    Text(", ".join(label["name"] for label in entry.get("labels") or []), style="cyan"),
                    Text(age(entry.get("updatedAt"))),
                ],
                entry["number"],
            )
            for entry in data
        ]
        return TableData(rows, Text(f"{len(rows)} offene Issues", style="dim"))

    def actions(number: int) -> list:
        ident = str(number)
        return [
            ("Details", lambda: ui.show_command(["gh", "issue", "view", ident, "-R", repo])),
            ("Bearbeiten", lambda: edit_menu(repo, number)),
            ("Kommentieren", lambda: comment_issue(repo, number)),
            ("Im Browser öffnen", lambda: ui.run_command(["gh", "issue", "view", ident, "-R", repo, "--web"])),
        ]

    list_view(repo, "Issues", ["#", "Titel", "Autor", "Labels", "Alter"], loader, actions)


BOARD_STYLES = ["cyan", "yellow", "green", "magenta", "blue", "red", "white"]

_board_cache: dict[str, tuple[str, int]] = {}


def configured_board() -> tuple[str, int] | None:
    board = config.section("github").get("board") or {}
    owner, number = board.get("owner"), board.get("number")
    if owner and number:
        return str(owner), int(number)
    return None


def pick_board(owner: str) -> tuple[str, int]:
    data = shell.capture_json(["gh", "project", "list", "--owner", owner, "--format", "json", "--limit", "100"], timeout=20)
    projects = data.get("projects") or []
    if not projects:
        raise CubiError(f"Keine Projekte für {owner} gefunden")
    items = [Item(str(project.get("title", "?")), (owner, int(project["number"])), f"#{project['number']}") for project in projects]
    picked = ui.menu(f"Board wählen · {owner}", items)
    if picked is None:
        raise CubiError("Kein Board ausgewählt")
    return picked


def resolve_board(repo: str) -> tuple[str, int]:
    configured = configured_board()
    if configured:
        return configured
    if repo not in _board_cache:
        _board_cache[repo] = pick_board(repo.split("/")[0])
    return _board_cache[repo]


def project_id(owner: str, number: int) -> str:
    data = shell.capture_json(["gh", "project", "view", str(number), "--owner", owner, "--format", "json"], timeout=20)
    ident = data.get("id")
    if not ident:
        raise CubiError("Projekt-ID nicht gefunden")
    return str(ident)


def status_field(owner: str, number: int) -> dict:
    data = shell.capture_json(["gh", "project", "field-list", str(number), "--owner", owner, "--format", "json", "--limit", "100"], timeout=20)
    fields = data.get("fields") or []
    named = next((f for f in fields if str(f.get("name", "")).strip().lower() == "status" and f.get("options")), None)
    field = named or next((f for f in fields if f.get("options")), None)
    if field is None:
        raise CubiError("Kein Status-Feld mit Spalten im Board gefunden")
    return field


def field_value(entry: dict, name: str) -> str | None:
    for key in (name.lower(), name, name[:1].lower() + name[1:]):
        if key in entry:
            return entry[key]
    return None


def status_style(name: str, order: Sequence[str]) -> str:
    index = order.index(name) if name in order else len(order)
    return BOARD_STYLES[index % len(BOARD_STYLES)]


def board(repo: str) -> None:
    owner, number = resolve_board(repo)
    field = status_field(owner, number)
    field_name = str(field.get("name", "Status"))
    field_id = str(field["id"])
    options = field.get("options") or []
    option_names = [str(option["name"]) for option in options]
    pid = project_id(owner, number)

    items_by_id: dict[str, dict] = {}

    def loader() -> TableData:
        data = shell.capture_json(["gh", "project", "item-list", str(number), "--owner", owner, "--format", "json", "--limit", "200"], timeout=30)
        entries = [entry for entry in data.get("items") or [] if entry.get("id") and (entry.get("content") or {}).get("type") == "Issue"]
        entries.sort(key=lambda entry: option_names.index(field_value(entry, field_name)) if field_value(entry, field_name) in option_names else len(option_names))
        items_by_id.clear()
        rows = []
        for entry in entries:
            items_by_id[entry["id"]] = entry
            content = entry.get("content") or {}
            status = str(field_value(entry, field_name) or "-")
            rows.append(
                Row(
                    [
                        Text(status, style=status_style(status, option_names)),
                        Text(f"#{content.get('number', '?')}"),
                        Text(content.get("title", entry.get("title", ""))),
                        Text(content.get("repository", "")),
                        Text(", ".join(entry.get("labels") or []), style="cyan"),
                    ],
                    entry["id"],
                )
            )
        return TableData(rows, Text(f"{len(rows)} Einträge · Spalten: {', '.join(option_names)}", style="dim"))

    def move(item_id: str) -> None:
        entry = items_by_id.get(item_id) or {}
        current = field_value(entry, field_name)
        choice = ui.menu("Spalte wählen", [Item(name, name, "aktuell" if name == current else "") for name in option_names], initial=current)
        if choice is None or choice == current:
            return
        target = next(option for option in options if str(option["name"]) == choice)
        shell.ensure(
            shell.capture(
                [
                    "gh", "project", "item-edit",
                    "--id", item_id,
                    "--field-id", field_id,
                    "--project-id", pid,
                    "--single-select-option-id", str(target["id"]),
                ],
                timeout=20,
            ),
            "Verschieben fehlgeschlagen",
        )

    def actions(item_id: str) -> list:
        entry = items_by_id.get(item_id) or {}
        content = entry.get("content") or {}
        issue_repo = content.get("repository") or repo
        number_ = content.get("number")
        actions_list = [("Verschieben", lambda: move(item_id))]
        if number_:
            actions_list.append(("Details", lambda: ui.show_command(["gh", "issue", "view", str(number_), "-R", issue_repo])))
            actions_list.append(("Kommentieren", lambda: comment_issue(issue_repo, number_)))
            actions_list.append(("Im Browser öffnen", lambda: ui.run_command(["gh", "issue", "view", str(number_), "-R", issue_repo, "--web"])))
        return actions_list

    selected = None
    while True:
        key, value = ui.table_view(f"GitHub · Kanban-Board · {owner}", ["Status", "#", "Titel", "Repo", "Labels"], loader, interval=30, initial=selected)
        if key != "enter":
            return
        selected = value
        entry = items_by_id.get(value) or {}
        title = (entry.get("content") or {}).get("title") or entry.get("title", "")
        ui.action_menu(f"Board-Eintrag · {title}", actions(value))


def commits(repo: str, path: str, revision: str) -> None:
    def loader() -> TableData:
        query = f"repos/{repo}/commits?per_page=30"
        if path:
            query += "&path=" + quote(path)
        if revision and revision != "HEAD":
            query += "&sha=" + quote(revision)
        data = shell.capture_json(["gh", "api", query], timeout=30)
        rows = []
        for entry in data:
            commit = entry.get("commit", {})
            author = commit.get("author", {})
            message = (commit.get("message") or "").splitlines()
            rows.append(
                Row(
                    [
                        Text(entry["sha"][:7], style="yellow"),
                        Text(age(author.get("date"))),
                        Text(author.get("name", "-")),
                        Text(message[0] if message else ""),
                    ],
                    entry["sha"],
                )
            )
        return TableData(rows, Text(f"{path or '/'} @ {revision or 'HEAD'}", style="dim"))

    selected = None
    while True:
        key, value = ui.table_view(f"GitHub · Commits · {repo}", ["Commit", "Alter", "Autor", "Nachricht"], loader, interval=60, initial=selected)
        if key != "enter":
            return
        selected = value
        ui.run_command(["gh", "browse", value, "-R", repo])


def browse_path(repo: str, path: str, revision: str) -> None:
    cmd = ["gh", "browse", path or "", "-R", repo]
    if revision and revision != "HEAD":
        cmd += ["-b", revision]
    ui.run_command([part for part in cmd if part])


def source_menu(repo_url: str, path: str, revision: str) -> None:
    repo = repo_from_url(repo_url)
    if repo is None:
        raise CubiError(f"Quelle ist kein GitHub-Repository: {repo_url or 'unbekannt'}")
    ui.action_menu(
        f"Quelle · {repo}",
        [
            ("Letzte Commits für den Pfad", lambda: commits(repo, path, revision)),
            ("Pfad im Browser öffnen", lambda: browse_path(repo, path, revision)),
        ],
        subtitle=f"{path or '/'} @ {revision or 'HEAD'}",
    )


def run() -> None:
    repo = default_repo() or ui.guard(pick_repo, None)
    if not repo:
        return
    choice = None
    while True:
        choice = ui.menu(
            f"GitHub · {repo}",
            [
                Item("Workflow-Runs", "runs", "gh run list"),
                Item("Pull Requests", "pulls", "gh pr list"),
                Item("Issues", "issues", "gh issue list"),
                Item("Kanban-Board", "board", "gh project item-list"),
                Item("Repo im Browser öffnen", "browse", "gh repo view --web"),
                Item("Repo wechseln", "switch", "aus Config und gh repo list"),
            ],
            initial=choice,
        )
        if choice is None:
            return
        if choice == "runs":
            ui.guard(runs, repo)
        elif choice == "pulls":
            ui.guard(pulls, repo)
        elif choice == "issues":
            ui.guard(issues, repo)
        elif choice == "board":
            ui.guard(board, repo)
        elif choice == "browse":
            ui.guard(ui.run_command, ["gh", "repo", "view", "-R", repo, "--web"])
        elif choice == "switch":
            repo = ui.guard(pick_repo, repo) or repo
