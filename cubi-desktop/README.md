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

Läuft unter Linux und macOS (Apple Silicon und Intel). Voraussetzung ist Rust stable (`rustup`).

```bash
cd cubi-desktop
cargo build --release
```

**macOS:**
- Rust installieren: `brew install rustup && rustup-init`. Ohne Homebrew geht es mit dem Installer von rustup.rs.
- Die Werkzeuge, die cubi aufruft, kommen ebenfalls per Homebrew: `brew install kubectl argocd helm gh act`.
- Tailscale findet die App auch ohne CLI im `PATH`, nämlich in `/Applications/Tailscale.app`.
- Interaktive Befehle öffnen Terminal.app. Für iTerm in `desktop.toml` `terminal = ["open", "-a", "iTerm"]` setzen.
- Die App übernimmt beim Start den `PATH` der Login-Shell. So findet sie Homebrew-Tools auch dann, wenn sie nicht aus einem Terminal gestartet wurde.

**Linux:** Für den Build braucht es die Pakete `libxkbcommon-dev`, `libwayland-dev`, `libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev` und `libgl1-mesa-dev`.

Die CI (`.github/workflows/cubi-desktop.yml`) baut und testet auf `ubuntu-latest` und `macos-latest`.

Entwicklung: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Installation als Desktop-App

```bash
./install.sh
```

Das Skript baut die App im Release-Modus und kopiert die Konfiguration nach `~/.config/cubi-desktop/`. Vorhandene Dateien dort bleiben unverändert. Danach installiert es die App mit Icon:

| System | Ergebnis |
|---|---|
| Linux | `~/.local/bin/cubi-desktop`, Eintrag „cubi“ im Anwendungsmenü (`~/.local/share/applications/cubi-desktop.desktop`), Icons in `~/.local/share/icons/hicolor`, Symbol auf dem Schreibtisch |
| macOS | `~/Applications/cubi.app` mit Icon, startbar über Launchpad und Spotlight, Verknüpfung auf dem Schreibtisch. Fürs Dock: App starten, Rechtsklick aufs Dock-Symbol, „Im Dock behalten“. |

- `./install.sh --no-desktop` installiert ohne Symbol auf dem Schreibtisch.
- `./install.sh --uninstall` entfernt App, Starter und Icons. Die Konfiguration bleibt erhalten.
- Nach einem `git pull` installiert ein erneutes `./install.sh` die neue Version.

Unter GNOME muss das Schreibtisch-Symbol beim ersten Start eventuell per Rechtsklick und „Start erlauben“ freigegeben werden.

## Start aus dem Terminal

```bash
cubi-desktop                    # bzw. ./target/release/cubi-desktop
```

- `--bereich <name>` öffnet einen Bereich direkt. Die Namen und Aliase sind dieselben wie bei cubi, z. B. `argocd`, `sync`, `ft`, `scale`, `k8s`, `gh`, `tokens`, `login`, `precheck`.
- Ein Repo-Pfad als Argument, oder der Start in einem Git-Repo, öffnet die Pre-Push-Checks für dieses Repo.
- `--cli` führt die Pre-Push-Checks ohne Oberfläche aus. Exit-Code 0 heißt: alle Pflicht-Checks sind grün.
- `--config <ordner>` nimmt einen anderen Konfigurationsordner, alternativ `CUBI_DESKTOP_CONFIG`. Standard ist auf Linux und macOS `~/.config/cubi-desktop/`, bzw. `$XDG_CONFIG_HOME/cubi-desktop/`, wenn die Variable gesetzt ist.

Tokens kommen aus `gh auth`, der kubeconfig und der ArgoCD-Config. Die App speichert keine Secrets.

## Konfiguration

```
~/.config/cubi-desktop/
├── desktop.toml                          # terminal = ["gnome-terminal", "--"] oder ["open", "-a", "iTerm"] (leer = automatisch)
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
