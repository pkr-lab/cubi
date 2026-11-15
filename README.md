# cubi

Interaktives Terminal-Werkzeug für den Kubernetes-Alltag: ArgoCD, Kubernetes, Helm, Tailscale, GitHub, SSH-Verbindungen und das Token-Register von capulus-core in einem Menü.

## Quickstart

```bash
git clone https://github.com/pkr-lab/cubi.git
cd cubi
./install.sh
cubi
```

Voraussetzung: Python 3 mit `rich` und `pyyaml`.

## cubi-precheck

Desktop-App in Rust, die vor dem `git push` die CI-Checks der capulus-Repos lokal ausführt und danach Push oder PR per Button anbietet: [cubi-precheck/README.md](cubi-precheck/README.md).

## Dokumentation

[Überblick](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/index.md) · [Installation](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/installation.md) · [Konfiguration](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/konfiguration.md) · [Nutzung](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/nutzung.md) · [Bedienung](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/bedienung.md) · [ArgoCD-Features](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/argocd-features.md) · [Releases](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/releases.md) · [cubi-precheck](https://github.com/pkr-lab/pkr-lab-docs/blob/main/cubi/precheck.md)
