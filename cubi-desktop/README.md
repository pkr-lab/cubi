# cubi-desktop

Desktop-Oberfläche für cubi (Rust, egui). Sie enthält alle Bereiche des Terminal-cubi und zusätzlich die Pre-Push-Checks:

| Bereich | Inhalt |
|---|---|
| Pre-Push-Checks | CI-Checks des Repos lokal ausführen, dann Push oder PR per Button |
| GitHub | Workflow-Runs, Pull Requests, Issues (bearbeiten, kommentieren), Kanban-Board |
| ArgoCD | Apps mit Filter und Details, Sync (auch mehrere), Diff, Historie, Quelle, Auto-Sync, Feature-Testing, Pods skalieren |
| Kubernetes | Pods (Logs, Shell, Describe, Löschen), Nodes, Warnungen, Kontext und Namespace |
| Helm | Releases mit Status, Values, Historie |
| Connect, Tailscale, Login | SSH-Ziele, Geräte im Tailnet, Anmeldestatus |
| Tokens | Token-Register: Ablauf, Runbook, Rotation, Datum eintragen |

Interaktive Befehle (SSH, Pod-Shell, Logs folgen, Logins, Rotationsskripte) öffnen ein Terminalfenster.

## Build

Voraussetzung: Rust stable (`rustup`).

```bash
cd cubi-desktop
cargo build --release
```

Entwicklung: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Start

```bash
mkdir -p ~/.config/cubi-desktop
cp -r config/* ~/.config/cubi-desktop/
./target/release/cubi-desktop
```

- `--bereich <name>` öffnet einen Bereich direkt. Die Namen und Aliase sind dieselben wie bei cubi, z. B. `argocd`, `sync`, `ft`, `scale`, `k8s`, `gh`, `tokens`, `login`, `precheck`.
- Ein Repo-Pfad als Argument, oder der Start in einem Git-Repo, öffnet die Pre-Push-Checks für dieses Repo.
- `--cli` führt die Pre-Push-Checks ohne Oberfläche aus. Exit-Code 0 heißt: alle Pflicht-Checks sind grün.
- `--config <ordner>` nimmt einen anderen Konfigurationsordner, alternativ `CUBI_DESKTOP_CONFIG`.

Tokens kommen aus `gh auth`, der kubeconfig und der ArgoCD-Config. Die App speichert keine Secrets.

## Konfiguration

```
~/.config/cubi-desktop/
├── desktop.toml                          # terminal = ["gnome-terminal", "--"] (leer = automatisch)
└── precheck/
    ├── config.toml                       # parallel, repos
    ├── profiles/<repo>.toml              # ein Profil pro Repo
    └── shared/commitlint.config.cjs      # Regeln aus pkr-lab/.github
```

Die cubi-Bereiche lesen dieselbe `targets.yaml` wie das Terminal-cubi. Gesucht wird in dieser Reihenfolge: `CUBI_CONFIG`, `~/.config/cubi/targets.yaml`, `config/targets.yaml` im cubi-Repo.

Ein Precheck-Profil ordnet sich über den Remote `origin` (`owner/repo`) zu. Neue Repos oder Checks brauchen nur TOML, keine Codeänderung:

```toml
[[checks]]
id = "lint"
name = "make lint (yamllint, helm lint)"
source = ".github/workflows/ci.yml#lint"
needs = ["make", "yamllint", "helm"]
steps = [["make", "lint"]]
```

| Schlüssel | Bedeutung |
|---|---|
| `steps` | Kommandos als Argumentlisten, laufen nacheinander im Repo, ohne Shell |
| `required` | Pflicht-Check für den Push (Standard `true`) |
| `needs`, `needs_docker`, `os`, `probes` | Voraussetzungen. Fehlt eine, steht der Check auf „Voraussetzung fehlt“, nie auf grün. |
| `paths`, `paths_ignore` | Glob-Filter auf geänderte Dateien, wie in GitHub Actions |
| `skip_if_no_commits`, `env`, `timeout_secs`, `source` | überspringen ohne Commits, Umgebung, Zeitlimit, Herkunft |

In Argumenten sind diese Platzhalter erlaubt: `{repo}`, `{base}`, `{head}`, `{tmp}`, `{config_dir}`.

Screenshot für die Entwicklung: `cargo build --features screenshot`, dann `CUBI_DESKTOP_SCREENSHOT=bild.png cubi-desktop --bereich helm`.

Doku: [cubi-desktop](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/desktop.md)
