import json
from pathlib import Path

from rich.text import Text

from .. import shell, ui
from ..errors import CubiError
from ..timeutil import age
from ..ui import Row, TableData


def ssh_aliases() -> set[str]:
    path = Path.home() / ".ssh" / "config"
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return set()
    aliases: set[str] = set()
    for line in lines:
        parts = line.split()
        if len(parts) > 1 and parts[0].lower() == "host":
            aliases.update(part for part in parts[1:] if "*" not in part and "?" not in part)
    return aliases


def fetch_status() -> dict:
    result = shell.capture(["tailscale", "status", "--json"], timeout=15)
    try:
        data = json.loads(result.stdout)
    except ValueError:
        raise CubiError(shell.first_line(result.stderr) or "Tailscale-Status nicht lesbar")
    state = data.get("BackendState", "")
    if state != "Running":
        raise CubiError(f"Tailscale ist nicht verbunden (Status: {state or 'unbekannt'}). Mit u starten oder unter Login anmelden.")
    return data


def device(entry: dict, is_self: bool) -> dict:
    addresses = entry.get("TailscaleIPs") or []
    return {
        "name": entry.get("HostName") or entry.get("DNSName", "?"),
        "ip": addresses[0] if addresses else "-",
        "os": entry.get("OS", "-"),
        "online": bool(entry.get("Online")) or is_self,
        "last_seen": entry.get("LastSeen"),
        "self": is_self,
    }


def status_cell(entry: dict) -> Text:
    if entry["self"]:
        return Text("dieses Gerät", style="cyan")
    if entry["online"]:
        return Text("online", style="green")
    return Text(f"offline · {age(entry['last_seen'])}", style="red")


def load() -> TableData:
    data = fetch_status()
    devices = [device(data.get("Self", {}), True)]
    peers = [device(peer, False) for peer in (data.get("Peer") or {}).values()]
    peers.sort(key=lambda entry: (not entry["online"], entry["name"].lower()))
    devices.extend(peers)
    rows = [
        Row(
            [Text(entry["name"]), Text(entry["ip"]), Text(entry["os"]), status_cell(entry)],
            (entry["name"], entry["ip"], entry["self"]),
        )
        for entry in devices
    ]
    online = sum(1 for entry in devices if entry["online"])
    return TableData(rows, Text(f"{len(devices)} Geräte · {online} online", style="dim"))


def connect(name: str, ip: str) -> None:
    target = name if name in ssh_aliases() else ip
    ui.console.clear()
    shell.replace(["ssh", target])


def device_menu(name: str, ip: str) -> None:
    ui.action_menu(
        f"Gerät · {name}",
        [
            ("SSH", lambda: connect(name, ip)),
            ("Ping", lambda: ui.run_command(["tailscale", "ping", "-c", "3", ip])),
        ],
        subtitle=ip,
    )


def up() -> None:
    if ui.run_command(["tailscale", "up"]) != 0:
        ui.info("Hinweis: Möglicherweise sind Root-Rechte nötig, dann sudo tailscale up ausführen.")


def down() -> None:
    if ui.confirm("Tailscale trennen?"):
        if ui.run_command(["tailscale", "down"]) != 0:
            ui.info("Hinweis: Möglicherweise sind Root-Rechte nötig, dann sudo tailscale down ausführen.")


def run() -> None:
    selected = None
    while True:
        key, value = ui.table_view(
            "Tailscale",
            ["Gerät", "IP", "OS", "Status"],
            load,
            hotkeys={"u": "Up", "d": "Down", "s": "Status roh"},
            interval=15,
            initial=selected,
        )
        if key == "back":
            return
        if key == "u":
            ui.guard(up)
        elif key == "d":
            ui.guard(down)
        elif key == "s":
            ui.guard(ui.show_command, ["tailscale", "status"])
        elif key == "enter":
            selected = value
            name, ip, is_self = value
            if not is_self:
                ui.guard(device_menu, name, ip)
