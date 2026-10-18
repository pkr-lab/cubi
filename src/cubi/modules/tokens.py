import re
from datetime import date

from rich.table import Table
from rich.text import Text

from .. import shell, token_register, ui
from ..errors import CubiError
from ..token_register import Token
from ..ui import Item, Row, TableData

MODE_STYLE = {"auto": "green", "scripted": "cyan", "manual": ""}
RULES = (
    "Neues Token anlegen, das alte bleibt gültig (Überlappung).",
    "Alle Verbraucher auf das neue Token umstellen.",
    "Prüfen, dass alles funktioniert.",
    "Erst dann das alte Token widerrufen.",
)


def find(token_id: str) -> Token:
    token = next((item for item in token_register.load() if item.id == token_id), None)
    if token is None:
        raise CubiError(f"{token_id} steht nicht im Register")
    return token


def due_cell(token: Token) -> Text:
    left = token.days_left
    if left is None:
        return Text("unbekannt", style="yellow")
    stamp = token.due.isoformat()
    if left < 0:
        return Text(f"{stamp} · seit {-left} d abgelaufen", style="bold red")
    style = "red" if left <= token_register.URGENT else "yellow" if left <= token_register.SOON else "green"
    return Text(f"{stamp} · in {left} d", style=style)


def issued_cell(token: Token) -> Text:
    if token.issued is None:
        return Text("-", style="dim")
    return Text(f"{token.issued.isoformat()} · {(date.today() - token.issued).days} d", style="dim")


def load() -> TableData:
    tokens = sorted(
        token_register.load(),
        key=lambda item: (item.urgency, item.days_left if item.days_left is not None else 0, item.id),
    )
    rows = [
        Row(
            [Text(item.id), Text(item.service), Text(item.mode, style=MODE_STYLE.get(item.mode, "")), issued_cell(item), due_cell(item)],
            item.id,
        )
        for item in tokens
    ]
    overdue = sum(1 for item in tokens if item.urgency == 0)
    soon = sum(1 for item in tokens if item.urgency == 2)
    unknown = sum(1 for item in tokens if item.urgency == 1)
    summary = Text(f"{len(tokens)} Tokens · ", style="dim")
    summary.append(f"{overdue} dringend", style="red" if overdue else "dim")
    summary.append(" · ", style="dim")
    summary.append(f"{soon} bald fällig", style="yellow" if soon else "dim")
    summary.append(f" · {unknown} ohne Ablaufdatum", style="dim")
    return TableData(rows, summary)


def details(token_id: str) -> None:
    token = find(token_id)
    grid = Table.grid(padding=(0, 2))
    grid.add_column(style="bold", no_wrap=True)
    grid.add_column()
    grid.add_row("Token", token.id)
    grid.add_row("Dienst", f"{token.service} · {token.kind}")
    grid.add_row("Rotation", token.mode)
    grid.add_row("Ausgestellt", token.issued.isoformat() if token.issued else "unbekannt")
    grid.add_row("Läuft ab", due_cell(token))
    if token.live_check:
        grid.add_row("Live-Prüfung", f"{token.live_check} (token-watch)")
    grid.add_row("Runbook", token.runbook or "-")
    grid.add_row("Abgelegt in", "\n".join(token.stored_in) or "-")
    grid.add_row("Blast Radius", token.blast_radius or "-")
    ui.show(grid, f"Token · {token.id}")


def slug(heading: str) -> str:
    text = re.sub(r"[^\w\s-]", "", heading.strip().lower())
    return re.sub(r"\s", "-", text)


def runbook(token_id: str) -> None:
    token = find(token_id)
    path_text, _, anchor = token.runbook.partition("#")
    path = token_register.resolve_runbook(path_text)
    if not path_text or not path.is_file():
        raise CubiError(f"Runbook ist keine Datei im Repo: {token.runbook or '-'}")
    text = path.read_text(encoding="utf-8")
    if anchor:
        lines = text.splitlines()
        start = next((index for index, line in enumerate(lines) if line.startswith("#") and slug(line.lstrip("#")) == anchor), None)
        if start is not None:
            text = "\n".join(lines[start:])
    ui.show_text(text, f"Runbook · {token.id}")


def mark(token_id: str) -> bool:
    token = find(token_id)
    issued_text = ui.prompt(f"{token.id} · ausgestellt am (JJJJ-MM-TT)", date.today().isoformat())
    if issued_text is None:
        return False
    issued = token_register.validate_date(issued_text.strip())
    hint = "leer = ausgestellt + max_age_days" if token.max_age_days else "leer = unbekannt"
    expires_text = ui.prompt(f"{token.id} · läuft ab am (JJJJ-MM-TT, {hint})", "")
    if expires_text is None:
        return False
    expires = token_register.validate_date(expires_text.strip()) if expires_text.strip() else None
    token_register.set_dates(token.id, issued, expires)
    ui.info(
        f"Register aktualisiert: {token_register.register_path()}\n"
        "Änderung committen, ein PR pro Rotation. Danach prüft token-watch das neue Datum."
    )
    return True


def guide(token: Token) -> None:
    ui.begin(f"Rotation · {token.id}")
    ui.console.print(Text(token.blast_radius, style="dim"))
    ui.console.print()
    if token.hint:
        ui.console.print(Text(token.hint))
        ui.console.print()
    ui.console.print(Text("Abgelegt in:", style="bold"))
    for place in token.stored_in:
        ui.console.print(Text(f"  {place}"))
    ui.console.print()
    if token.overlap:
        ui.console.print(Text("Ablauf:", style="bold"))
        for number, rule in enumerate(RULES, 1):
            ui.console.print(Text(f"  {number}. {rule}"))
    else:
        ui.console.print(Text("Keine Überlappung möglich: Das Token wird in einem Schritt getauscht.", style="yellow"))
    ui.console.print()


def rotate(token_id: str) -> None:
    token = find(token_id)
    if token.mode == "auto":
        ui.info(f"{token.id} rotiert automatisch, siehe Runbook ({token.runbook}). Nichts zu tun.")
        return
    guide(token)
    if token.mode == "scripted":
        root = token_register.repo_root()
        if not token.command:
            raise CubiError(f"{token.id}: Im Register fehlt rotation.command")
        cmd = [str(root / token.command[0]), *token.command[1:]]
        if not ui.confirm(f"Skript ausführen? ({' '.join(token.command)})"):
            return
        if ui.run_command(cmd, f"Rotation · {token.id}", cwd=str(root), wait=False) != 0:
            ui.pause()
            return
        if token.overlap:
            ui.console.print(Text("\nDas alte Token ist noch gültig. Erst nach der Prüfung widerrufen.", style="yellow"))
        if ui.confirm("Rotation im Register eintragen?", default=True):
            mark(token.id)
        return
    if ui.confirm("Runbook jetzt lesen?", default=True):
        runbook(token.id)
    if ui.confirm("Rotation durchgeführt und im Register eintragen?"):
        mark(token.id)


def token_menu(token_id: str) -> None:
    ui.action_menu(
        f"Token · {token_id}",
        [
            ("Rotieren", lambda: rotate(token_id)),
            ("Details", lambda: details(token_id)),
            ("Runbook lesen", lambda: runbook(token_id)),
            ("Als rotiert im Register eintragen", lambda: mark(token_id)),
        ],
        subtitle=find(token_id).blast_radius,
    )


def check_register() -> None:
    root = token_register.repo_root()
    result = shell.capture(["python3", "scripts/check-token-register.py"], cwd=str(root))
    ui.show_text(shell.merge_output(result) or "(keine Ausgabe)", "Register prüfen")


def run() -> None:
    selected = None
    while True:
        key, value = ui.table_view(
            "Tokens",
            ["Token", "Dienst", "Rotation", "Ausgestellt", "Läuft ab"],
            load,
            hotkeys={"c": "Register prüfen"},
            initial=selected,
        )
        if key == "back":
            return
        if key == "c":
            ui.guard(check_register)
        elif key == "enter":
            selected = value
            ui.guard(token_menu, value)
