from typing import Any

from rich.text import Text

from .. import shell, ui
from ..errors import CubiError
from ..timeutil import age
from ..ui import Item, Row, TableData

PENDING_STATES = {"Pending", "ContainerCreating", "Terminating", "PodInitializing"}
DONE_STATES = {"Completed", "Succeeded"}


def kubectl_json(args: list[str], timeout: float = 30) -> Any:
    return shell.capture_json(["kubectl", *args, "-o", "json"], timeout=timeout)


def query(cmd: list[str]) -> str:
    try:
        result = shell.capture(cmd, timeout=10)
    except CubiError:
        return ""
    return result.stdout.strip() if result.returncode == 0 else ""


def current_namespace() -> str:
    return query(["kubectl", "config", "view", "--minify", "-o", "jsonpath={..namespace}"]) or "default"


def current_context() -> str:
    return query(["kubectl", "config", "current-context"]) or "-"


def pod_status(pod: dict) -> str:
    if pod["metadata"].get("deletionTimestamp"):
        return "Terminating"
    status = pod.get("status", {})
    reason = status.get("reason") or status.get("phase", "Unknown")
    for container in status.get("initContainerStatuses") or []:
        state = container.get("state", {})
        waiting = state.get("waiting")
        terminated = state.get("terminated")
        if waiting and waiting.get("reason"):
            return f"Init:{waiting['reason']}"
        if terminated and terminated.get("exitCode", 0) != 0:
            return f"Init:{terminated.get('reason') or 'Error'}"
    for container in status.get("containerStatuses") or []:
        state = container.get("state", {})
        waiting = state.get("waiting")
        terminated = state.get("terminated")
        if waiting and waiting.get("reason"):
            reason = waiting["reason"]
        elif terminated and terminated.get("reason") and status.get("phase") != "Succeeded":
            reason = terminated["reason"]
    return reason


def status_style(status: str, ready: bool) -> str:
    if status == "Running":
        return "green" if ready else "yellow"
    if status in DONE_STATES:
        return "dim"
    if status in PENDING_STATES or status.startswith("Init:"):
        return "yellow"
    return "red"


def load_pods(namespace: str | None) -> TableData:
    args = ["get", "pods", "-n", namespace] if namespace else ["get", "pods", "-A"]
    items = kubectl_json(args)["items"]
    items.sort(key=lambda pod: (pod["metadata"].get("namespace", ""), pod["metadata"]["name"]))
    rows = []
    problems = 0
    for pod in items:
        metadata = pod["metadata"]
        statuses = pod.get("status", {}).get("containerStatuses") or []
        total = len(pod.get("spec", {}).get("containers") or [])
        ready_count = sum(1 for container in statuses if container.get("ready"))
        restarts = sum(container.get("restartCount", 0) for container in statuses)
        status = pod_status(pod)
        style = status_style(status, ready_count == total)
        if style in ("red", "yellow"):
            problems += 1
        cells = []
        if namespace is None:
            cells.append(Text(metadata.get("namespace", "")))
        cells.extend(
            [
                Text(metadata["name"]),
                Text(f"{ready_count}/{total}"),
                Text(status, style=style),
                Text(str(restarts)),
                Text(age(metadata.get("creationTimestamp"))),
                Text(pod.get("spec", {}).get("nodeName", "-")),
            ]
        )
        rows.append(Row(cells, (metadata.get("namespace", ""), metadata["name"])))
    return TableData(rows, Text(f"{len(rows)} Pods · {problems} mit Auffälligkeiten", style="dim"))


def pod_menu(namespace: str, name: str) -> None:
    ui.action_menu(
        f"Pod · {name}",
        [
            ("Logs", lambda: ui.show_command(["kubectl", "logs", "-n", namespace, name, "--all-containers=true", "--tail=300"], "Keine Logs")),
            ("Logs folgen", lambda: ui.run_command(["kubectl", "logs", "-f", "-n", namespace, name, "--all-containers=true", "--tail=100"])),
            ("Describe", lambda: ui.show_command(["kubectl", "describe", "pod", "-n", namespace, name])),
            (
                "Shell",
                lambda: ui.run_command(["kubectl", "exec", "-it", "-n", namespace, name, "--", "sh", "-c", "command -v bash >/dev/null 2>&1 && exec bash || exec sh"]),
            ),
            ("Löschen", lambda: delete_pod(namespace, name)),
        ],
        subtitle=namespace,
    )


def delete_pod(namespace: str, name: str) -> None:
    if ui.confirm(f"Pod {name} löschen?"):
        ui.run_command(["kubectl", "delete", "pod", "-n", namespace, name])


def pods() -> None:
    show_all = False
    selected = None
    while True:
        namespace = None if show_all else current_namespace()
        scope = "alle Namespaces" if show_all else namespace
        columns = ["Pod", "Ready", "Status", "Restarts", "Alter", "Node"]
        if show_all:
            columns.insert(0, "Namespace")
        key, value = ui.table_view(
            f"Kubernetes · Pods · {scope}",
            columns,
            lambda: load_pods(namespace),
            hotkeys={"a": "Namespaces umschalten"},
            interval=10,
            initial=selected,
        )
        if key == "back":
            return
        if key == "a":
            show_all = not show_all
            selected = None
        elif key == "enter":
            selected = value
            ui.guard(pod_menu, *value)


def node_roles(node: dict) -> str:
    prefix = "node-role.kubernetes.io/"
    roles = [label[len(prefix):] for label in node["metadata"].get("labels", {}) if label.startswith(prefix)]
    return ", ".join(roles) or "-"


def load_nodes() -> TableData:
    items = kubectl_json(["get", "nodes"])["items"]
    rows = []
    for node in items:
        conditions = {condition["type"]: condition["status"] for condition in node.get("status", {}).get("conditions", [])}
        ready = conditions.get("Ready") == "True"
        addresses: dict[str, str] = {}
        for address in node.get("status", {}).get("addresses", []):
            addresses.setdefault(address["type"], address["address"])
        info = node.get("status", {}).get("nodeInfo", {})
        rows.append(
            Row(
                [
                    Text(node["metadata"]["name"]),
                    Text("Ready" if ready else "NotReady", style="green" if ready else "red"),
                    Text(node_roles(node)),
                    Text(info.get("kubeletVersion", "-")),
                    Text(addresses.get("InternalIP", "-")),
                    Text(age(node["metadata"].get("creationTimestamp"))),
                ],
                node["metadata"]["name"],
            )
        )
    return TableData(rows, Text(f"{len(rows)} Nodes", style="dim"))


def nodes() -> None:
    selected = None
    while True:
        key, value = ui.table_view(
            "Kubernetes · Nodes",
            ["Node", "Status", "Rollen", "Version", "IP", "Alter"],
            load_nodes,
            interval=15,
            initial=selected,
        )
        if key != "enter":
            return
        selected = value
        ui.guard(ui.show_command, ["kubectl", "describe", "node", value])


def choose_namespace() -> None:
    names = sorted(item["metadata"]["name"] for item in kubectl_json(["get", "namespaces"])["items"])
    current = current_namespace()
    choice = ui.menu("Namespace wählen", [Item(name, name, "aktuell" if name == current else "") for name in names])
    if choice is not None:
        shell.ensure(
            shell.capture(["kubectl", "config", "set-context", "--current", f"--namespace={choice}"], timeout=10),
            "Namespace konnte nicht gesetzt werden",
        )


def choose_context() -> None:
    result = shell.ensure(shell.capture(["kubectl", "config", "get-contexts", "-o", "name"], timeout=10), "Keine Kontexte gefunden")
    current = current_context()
    names = [line for line in result.stdout.splitlines() if line.strip()]
    choice = ui.menu("Kontext wählen", [Item(name, name, "aktuell" if name == current else "") for name in names])
    if choice is not None:
        shell.ensure(shell.capture(["kubectl", "config", "use-context", choice], timeout=10), "Kontext konnte nicht gesetzt werden")


def run() -> None:
    choice = None
    while True:
        choice = ui.menu(
            "Kubernetes",
            [
                Item("Pods", "pods", "Status, Logs, Shell"),
                Item("Nodes", "nodes", "Zustand und Versionen"),
                Item("Warnungen", "events", "Events vom Typ Warning"),
                Item("Namespace wechseln", "namespace", f"aktuell: {current_namespace()}"),
                Item("Kontext wechseln", "context", f"aktuell: {current_context()}"),
            ],
            initial=choice,
        )
        if choice is None:
            return
        if choice == "pods":
            ui.guard(pods)
        elif choice == "nodes":
            ui.guard(nodes)
        elif choice == "events":
            ui.guard(
                ui.show_command,
                ["kubectl", "get", "events", "-A", "--field-selector", "type=Warning", "--sort-by=.lastTimestamp"],
                "Keine Warnungen",
            )
        elif choice == "namespace":
            ui.guard(choose_namespace)
        elif choice == "context":
            ui.guard(choose_context)
