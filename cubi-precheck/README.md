# cubi-precheck

Desktop-App (Rust, egui), die vor dem `git push` die CI-Checks eines Repos lokal ausführt. Wenn alle Pflicht-Checks grün sind, kann man per Button pushen oder einen PR öffnen.

## Build

Voraussetzung: Rust stable (`rustup`).

```bash
cd cubi-precheck
cargo build --release
```

Entwicklung: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.

## Start

```bash
mkdir -p ~/.config/cubi-precheck
cp -r config/* ~/.config/cubi-precheck/
./target/release/cubi-precheck ~/git/github/capulus/capulus-core
```

- Ohne Pfad öffnet die App das aktuelle Verzeichnis, wenn es ein Git-Repo ist, sonst das erste Repo aus `config.toml`.
- `--config <ordner>` nimmt einen anderen Konfigurationsordner. Alternativ geht die Umgebungsvariable `CUBI_PRECHECK_CONFIG`.
- `--cli` führt die Checks ohne Oberfläche aus. Der Exit-Code ist 0, wenn alle Pflicht-Checks grün sind, und 1, wenn nicht.

Für Push und PR werden `git` und `gh` genutzt. Die Anmeldung kommt aus `gh auth login` und der normalen Git-Konfiguration, die App speichert keine Tokens.

## Konfiguration

```
~/.config/cubi-precheck/
├── config.toml                  # parallel, repos
├── profiles/<repo>.toml         # ein Profil pro Repo
└── shared/commitlint.config.cjs # Regeln aus pkr-lab/.github
```

Das Profil wird über den Remote `origin` (`owner/repo`) gewählt. Ein neues Repo oder ein neuer Check braucht nur eine neue oder erweiterte TOML-Datei.

```toml
name = "capulus-core"
remote = "pkr-lab/capulus-core"
base_branch = "main"
protected_branches = ["main"]

[[checks]]
id = "lint"
name = "make lint (yamllint, helm lint)"
source = ".github/workflows/ci.yml#lint"
required = true
needs = ["make", "yamllint", "helm"]
steps = [["make", "lint"]]
```

| Schlüssel | Bedeutung |
|---|---|
| `steps` | Kommandos als Argumentlisten. Sie laufen nacheinander im Repo-Wurzelverzeichnis, ohne Shell. |
| `required` | Pflicht-Check für den Push (Standard `true`) |
| `needs` | Programme, die im `PATH` liegen müssen |
| `needs_docker` | `docker info` muss funktionieren |
| `os` | erlaubte Betriebssysteme (`linux`, `macos`, `windows`) |
| `probes` | Zusatzprüfungen `{ cmd = [...], hint = "..." }` |
| `paths` / `paths_ignore` | Glob-Filter auf geänderte Dateien, wie `paths` in GitHub Actions |
| `skip_if_no_commits` | überspringen, wenn es keine Commits gegenüber der Basis gibt |
| `env`, `timeout_secs`, `source` | Umgebung, Zeitlimit (Standard 3600 s), Herkunft in der CI |

In Argumenten sind diese Platzhalter erlaubt: `{repo}`, `{base}` (merge-base mit `origin/<base_branch>`), `{head}`, `{tmp}` (Temp-Ordner je Check) und `{config_dir}`. Unbekannte Platzhalter oder Felder lehnt die App beim Laden ab.

Fehlt eine Voraussetzung, steht der Check auf „Voraussetzung fehlt“. Das zählt nicht als grün. Ein Check, den ein Pfadfilter überspringt, zählt als grün, genau wie in der CI.

Doku: [cubi-precheck](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/precheck.md)
