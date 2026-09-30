import sys
from typing import Callable

from . import ui
from .errors import CubiError
from .modules import argocd, auth, connect, github, helm, kube, tailscale
from .ui import Item

MODULES: dict[str, tuple[Callable[[], None], str, str]] = {
    "connect": (connect.run, "Connect", "SSH zu VMs und Hosts"),
    "argocd": (argocd.run, "ArgoCD", "Status aller Apps, Sync, Diff"),
    "tailscale": (tailscale.run, "Tailscale", "Geräte, Ping, SSH, Up/Down"),
    "github": (github.run, "GitHub", "Runs, Pull Requests, Issues, Kanban-Board"),
    "kubernetes": (kube.run, "Kubernetes", "Pods, Nodes, Namespaces"),
    "helm": (helm.run, "Helm", "Releases"),
    "login": (auth.run, "Login", "ArgoCD, GitHub, Tailscale"),
}

ALIASES = {
    "argo": "argocd",
    "ts": "tailscale",
    "gh": "github",
    "k8s": "kubernetes",
    "kubectl": "kubernetes",
    "auth": "login",
}


def usage() -> str:
    lines = ["Aufruf: cubi [bereich]", "", "Bereiche:"]
    for name, (_, _, hint) in MODULES.items():
        lines.append(f"  {name:<12}{hint}")
    lines.append("")
    lines.append("Ohne Bereich startet das interaktive Hauptmenü.")
    return "\n".join(lines)


def launch(name: str) -> None:
    ui.guard(MODULES[name][0])


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if args and args[0] in ("-h", "--help", "help"):
        print(usage())
        return 0
    if not (sys.stdin.isatty() and sys.stdout.isatty()):
        print("cubi benötigt ein interaktives Terminal", file=sys.stderr)
        return 1
    if args:
        name = ALIASES.get(args[0], args[0])
        if name not in MODULES:
            print(f"Unbekannter Bereich: {args[0]}\n\n{usage()}", file=sys.stderr)
            return 2
        launch(name)
        return 0
    items = [Item(label, name, hint) for name, (_, label, hint) in MODULES.items()]
    items.append(Item("Beenden", "exit"))
    choice = None
    while True:
        try:
            choice = ui.menu("cubi", items, initial=choice)
        except CubiError as exc:
            ui.error(str(exc))
            return 1
        if choice is None or choice == "exit":
            return 0
        launch(choice)
