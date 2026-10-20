import sys
from typing import Callable

from . import ui
from .errors import CubiError, Finished
from .modules import argocd, auth, autosync, connect, feature_testing, github, helm, kube, scale, tailscale, tokens
from .ui import Item

MODULES: dict[str, tuple[Callable[[], None], str, str]] = {
    "connect": (connect.run, "Connect", "SSH zu VMs und Hosts"),
    "argocd": (argocd.run, "ArgoCD", "Status aller Apps, Sync, Diff"),
    "autosync": (autosync.run, "Auto-Sync", "ArgoCD Auto-Sync deaktivieren/aktivieren"),
    "feature-testing": (feature_testing.run, "Feature-Testing", "ArgoCD-Apps auf einen Feature-Branch umstellen"),
    "scale": (scale.run, "Pods skalieren", "ArgoCD-Apps auf 0 oder eine eigene Anzahl Pods stellen, wiederherstellen"),
    "tailscale": (tailscale.run, "Tailscale", "Geräte, Ping, SSH, Up/Down"),
    "github": (github.run, "GitHub", "Runs, Pull Requests, Issues, Kanban-Board"),
    "kubernetes": (kube.run, "Kubernetes", "Pods, Nodes, Namespaces"),
    "helm": (helm.run, "Helm", "Releases"),
    "tokens": (tokens.run, "Tokens", "Ablauf und Rotation aus dem Token-Register"),
    "login": (auth.run, "Login", "ArgoCD, GitHub, Tailscale"),
}

ALIASES = {
    "argo": "argocd",
    "sync": "autosync",
    "branch": "feature-testing",
    "ft": "feature-testing",
    "ts": "tailscale",
    "gh": "github",
    "k8s": "kubernetes",
    "kubectl": "kubernetes",
    "auth": "login",
    "token": "tokens",
}


def usage() -> str:
    lines = ["Aufruf: cubi [bereich]", "", "Bereiche:"]
    width = max(len(name) for name in MODULES) + 2
    for name, (_, _, hint) in MODULES.items():
        lines.append(f"  {name:<{width}}{hint}")
    lines.append("")
    lines.append("Ohne Bereich startet das interaktive Hauptmenü.")
    return "\n".join(lines)


def launch(name: str) -> bool:
    try:
        ui.guard(MODULES[name][0])
    except Finished:
        return True
    return False


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
            choice = ui.select("cubi · Bereich wählen", items, initial=choice)
        except CubiError as exc:
            ui.error(str(exc))
            return 1
        if choice is None or choice == "exit":
            return 0
        if launch(choice):
            return 0
