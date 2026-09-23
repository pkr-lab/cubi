import json
import re
from concurrent.futures import ThreadPoolExecutor

from rich.text import Text

from .. import argocd_config, shell, ui
from ..errors import CubiError
from ..ui import Item, Row, TableData


def good(message: str) -> Text:
    return Text(f"✔ {message}", style="green")


def bad(message: str) -> Text:
    return Text(f"✖ {message}", style="red")


def argocd_rows() -> list[Row]:
    rows = []
    for context in argocd_config.configured_contexts():
        if context["core"]:
            status = good("Core-Modus, kein Login nötig")
        else:
            state, when = argocd_config.token_status(context["name"])
            stamp = f"{when:%Y-%m-%d %H:%M}" if when else ""
            if state == "valid":
                status = good(f"gültig bis {stamp}" if stamp else "eingeloggt")
            elif state == "expired":
                status = bad(f"abgelaufen seit {stamp}")
            elif state == "invalid":
                status = bad("Token ungültig")
            else:
                status = bad("nicht eingeloggt")
        rows.append(Row([Text("ArgoCD"), Text(context["name"]), status], ("argocd", context["name"])))
    return rows


def github_row() -> Row:
    value = ("github", "github.com")
    try:
        result = shell.capture(["gh", "auth", "status", "-h", "github.com"], timeout=15)
    except CubiError as exc:
        return Row([Text("GitHub"), Text("github.com"), bad(str(exc))], value)
    if result.returncode != 0:
        return Row([Text("GitHub"), Text("github.com"), bad("nicht eingeloggt")], value)
    match = re.search(r"account (\S+)", result.stdout + result.stderr)
    account = match.group(1) if match else "unbekannt"
    return Row([Text("GitHub"), Text("github.com"), good(f"eingeloggt als {account}")], value)


def tailscale_row() -> Row:
    value = ("tailscale", "tailnet")
    try:
        result = shell.capture(["tailscale", "status", "--json"], timeout=15)
    except CubiError as exc:
        return Row([Text("Tailscale"), Text("Tailnet"), bad(str(exc))], value)
    try:
        data = json.loads(result.stdout)
    except ValueError:
        return Row([Text("Tailscale"), Text("Tailnet"), bad(shell.first_line(result.stderr) or "Status nicht lesbar")], value)
    state = data.get("BackendState", "")
    tailnet = (data.get("CurrentTailnet") or {}).get("Name") or "Tailnet"
    if state == "Running":
        status = good("verbunden")
    elif state == "NeedsLogin":
        status = bad("Login nötig")
    elif state == "Stopped":
        status = bad("gestoppt")
    else:
        status = bad(state or "unbekannt")
    return Row([Text("Tailscale"), Text(tailnet), status], value)


def load() -> TableData:
    with ThreadPoolExecutor(max_workers=2) as pool:
        github_future = pool.submit(github_row)
        tailscale_future = pool.submit(tailscale_row)
        rows = argocd_rows()
        rows.append(github_future.result())
        rows.append(tailscale_future.result())
    return TableData(rows)


def argocd_login(name: str, mode: str) -> None:
    context = next((item for item in argocd_config.configured_contexts() if item["name"] == name), None)
    if context is None:
        raise CubiError(f"Unbekannter ArgoCD-Kontext: {name}")
    cmd = ["argocd", "login", context["server"]]
    if context["plaintext"]:
        cmd.append("--plaintext")
    if context["insecure"]:
        cmd.append("--insecure")
    if context["server"] != name:
        cmd += ["--name", name]
    if mode == "sso":
        cmd.append("--sso")
    elif context["username"]:
        cmd += ["--username", str(context["username"])]
    ui.run_command(cmd, f"ArgoCD-Login · {name}")


def argocd_activate(name: str) -> None:
    argocd_config.activate(name)
    ui.info(f"ArgoCD-Kontext {name} ist aktiv.")


def argocd_menu(name: str) -> None:
    context = next((item for item in argocd_config.configured_contexts() if item["name"] == name), None)
    actions = []
    if context is not None and not context["core"]:
        actions.append(("Login mit Passwort", lambda: argocd_login(name, "password")))
        actions.append(("Login mit SSO", lambda: argocd_login(name, "sso")))
    actions.append(("Kontext aktivieren", lambda: argocd_activate(name)))
    ui.action_menu(f"ArgoCD · {name}", actions)


def github_menu() -> None:
    ui.action_menu(
        "GitHub",
        [
            ("Login im Browser", lambda: ui.run_command(["gh", "auth", "login", "-h", "github.com"])),
            ("Scopes erneuern", lambda: ui.run_command(["gh", "auth", "refresh", "-h", "github.com"])),
        ],
    )


def tailscale_menu() -> None:
    ui.action_menu(
        "Tailscale",
        [
            ("tailscale up", lambda: ui.run_command(["tailscale", "up"])),
            ("tailscale login", lambda: ui.run_command(["tailscale", "login"])),
        ],
        subtitle="Bei Berechtigungsfehlern mit sudo wiederholen",
    )


def run() -> None:
    selected = None
    while True:
        key, value = ui.table_view("Login", ["Dienst", "Ziel", "Status"], load, initial=selected)
        if key != "enter":
            return
        selected = value
        kind, target = value
        if kind == "argocd":
            ui.guard(argocd_menu, target)
        elif kind == "github":
            ui.guard(github_menu)
        elif kind == "tailscale":
            ui.guard(tailscale_menu)
