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
            ("Im Browser öffnen", lambda: ui.run_command(["gh", "pr", "view", ident, "-R", repo, "--web"])),
        ]

    list_view(repo, "Pull Requests", ["#", "Titel", "Autor", "Branch", "Review", "Alter"], loader, actions)


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
            ("Im Browser öffnen", lambda: ui.run_command(["gh", "issue", "view", ident, "-R", repo, "--web"])),
        ]

    list_view(repo, "Issues", ["#", "Titel", "Autor", "Labels", "Alter"], loader, actions)


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
        elif choice == "browse":
            ui.guard(ui.run_command, ["gh", "repo", "view", "-R", repo, "--web"])
        elif choice == "switch":
            repo = ui.guard(pick_repo, repo) or repo
