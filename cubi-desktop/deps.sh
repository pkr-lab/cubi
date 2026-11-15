#!/usr/bin/env bash
# Benötigte Programme für cubi-desktop prüfen und fehlende installieren.
# Linux: apt, dazu offizielle Release-Binaries mit SHA-256-Prüfung nach ~/.local/bin.
# macOS: Homebrew.
#
#   ./deps.sh          prüfen, nachfragen, installieren
#   ./deps.sh --yes    ohne Rückfrage installieren
#   ./deps.sh --check  nur anzeigen, was fehlt
set -euo pipefail

os="${CUBI_INSTALL_OS:-$(uname -s)}"
bin_dir="${CUBI_BIN_DIR:-$HOME/.local/bin}"
mode=ask
fetch_only=()

while [ $# -gt 0 ]; do
  case "$1" in
    --yes | -y) mode=yes ;;
    --check) mode=check ;;
    --fetch)
      shift
      fetch_only=("$@")
      break
      ;;
    -h | --help)
      sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unbekannte Option: $1" >&2
      exit 2
      ;;
  esac
  shift
done

case "$(uname -m)" in
  x86_64 | amd64) arch=amd64 ;;
  arm64 | aarch64) arch=arm64 ;;
  *)
    echo "Nicht unterstützte Architektur: $(uname -m)" >&2
    exit 1
    ;;
esac
os_l="$(printf '%s' "$os" | tr '[:upper:]' '[:lower:]')"

have() { command -v "$1" >/dev/null 2>&1; }

have_tool() {
  case "$1" in
    tailscale) have tailscale || [ -x /Applications/Tailscale.app/Contents/MacOS/Tailscale ] ;;
    pyyaml) python3 -c 'import yaml' >/dev/null 2>&1 ;;
    cargo) have cargo || [ -x "$HOME/.cargo/bin/cargo" ] ;;
    docker) have docker ;;
    *) have "$1" ;;
  esac
}

# Werkzeug|Zweck
TOOLS=(
  "cargo|Rust, zum Bauen der App"
  "git|Git-Status, Push"
  "gh|GitHub, PRs, Login"
  "kubectl|Kubernetes, ArgoCD-Apps"
  "argocd|ArgoCD-CLI: Sync, Diff, Login"
  "helm|Helm-Releases, Lint"
  "tailscale|Tailscale"
  "ssh|Connect, SSH"
  "curl|Downloads, CA-Check"
  "make|Pre-Push: make lint"
  "python3|Pre-Push: Token-Register"
  "pyyaml|Pre-Push: Python-Modul yaml"
  "npx|Pre-Push: commitlint"
  "yamllint|Pre-Push: Lint"
  "ansible-lint|Pre-Push: capulus-ansible"
  "gitleaks|Pre-Push: Secret-Scan"
  "docker|Pre-Push: act und Image-Builds"
  "act|Pre-Push: Workflow-Jobs"
)
[ "$os" = Darwin ] && TOOLS+=("xcodegen|Pre-Push: capulus-ios")

LINUX_BUILD_PKGS=(build-essential)

missing=()
for entry in "${TOOLS[@]}"; do
  tool="${entry%%|*}"
  have_tool "$tool" || missing+=("$entry")
done

missing_build=()
if [ "$os" = Linux ] && have dpkg-query; then
  for p in "${LINUX_BUILD_PKGS[@]}"; do
    dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing_build+=("$p")
  done
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

fetch() { curl -fsSL --retry 3 --proto '=https' -o "$2" "$1"; }

verify() {
  local file="$1" expected="$2" got
  got="$(sha256_of "$file")"
  if [ -z "$expected" ] || [ "$got" != "$expected" ]; then
    echo "Prüfsumme stimmt nicht für $(basename "$file") (erwartet ${expected:-?}, erhalten $got)" >&2
    return 1
  fi
}

sum_from() { grep -E "[ *]$2\$" "$1" | head -n 1 | cut -d' ' -f1; }

latest_tag() {
  curl -fsSL --retry 3 "https://api.github.com/repos/$1/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

put_bin() {
  mkdir -p "$bin_dir"
  install -m 755 "$1" "$bin_dir/$2"
  echo "    $2 -> $bin_dir/$2"
}

fetch_kubectl() {
  local v url
  v="$(curl -fsSL --retry 3 https://dl.k8s.io/release/stable.txt)"
  url="https://dl.k8s.io/release/$v/bin/$os_l/$arch/kubectl"
  fetch "$url" "$tmp/kubectl"
  verify "$tmp/kubectl" "$(curl -fsSL --retry 3 "$url.sha256" | cut -d' ' -f1)"
  put_bin "$tmp/kubectl" kubectl
}

fetch_helm() {
  local v f
  v="$(latest_tag helm/helm)"
  f="helm-$v-$os_l-$arch.tar.gz"
  fetch "https://get.helm.sh/$f" "$tmp/$f"
  fetch "https://get.helm.sh/$f.sha256sum" "$tmp/$f.sha256sum"
  verify "$tmp/$f" "$(cut -d' ' -f1 "$tmp/$f.sha256sum")"
  tar -xzf "$tmp/$f" -C "$tmp"
  put_bin "$tmp/$os_l-$arch/helm" helm
}

fetch_argocd() {
  local v base f
  v="$(latest_tag argoproj/argo-cd)"
  base="https://github.com/argoproj/argo-cd/releases/download/$v"
  f="argocd-$os_l-$arch"
  fetch "$base/$f" "$tmp/$f"
  fetch "$base/cli_checksums.txt" "$tmp/argocd.sums"
  verify "$tmp/$f" "$(sum_from "$tmp/argocd.sums" "$f")"
  put_bin "$tmp/$f" argocd
}

fetch_act() {
  local v base a f
  v="$(latest_tag nektos/act)"
  base="https://github.com/nektos/act/releases/download/$v"
  [ "$arch" = amd64 ] && a=x86_64 || a=arm64
  f="act_${os}_${a}.tar.gz"
  fetch "$base/$f" "$tmp/$f"
  fetch "$base/checksums.txt" "$tmp/act.sums"
  verify "$tmp/$f" "$(sum_from "$tmp/act.sums" "$f")"
  tar -xzf "$tmp/$f" -C "$tmp" act
  put_bin "$tmp/act" act
}

fetch_gitleaks() {
  local v n base a f
  v="$(latest_tag gitleaks/gitleaks)"
  n="${v#v}"
  base="https://github.com/gitleaks/gitleaks/releases/download/$v"
  [ "$arch" = amd64 ] && a=x64 || a=arm64
  f="gitleaks_${n}_${os_l}_${a}.tar.gz"
  fetch "$base/$f" "$tmp/$f"
  fetch "$base/gitleaks_${n}_checksums.txt" "$tmp/gitleaks.sums"
  verify "$tmp/$f" "$(sum_from "$tmp/gitleaks.sums" "$f")"
  tar -xzf "$tmp/$f" -C "$tmp" gitleaks
  put_bin "$tmp/gitleaks" gitleaks
}

if [ "${#fetch_only[@]}" -gt 0 ]; then
  for t in "${fetch_only[@]}"; do "fetch_$t"; done
  exit 0
fi

if [ "${#missing[@]}" -eq 0 ] && [ "${#missing_build[@]}" -eq 0 ]; then
  echo "==> Alle benötigten Programme sind vorhanden."
  exit 0
fi

echo "==> Fehlende Programme:"
for entry in "${missing[@]}"; do
  printf '    %-14s %s\n' "${entry%%|*}" "${entry#*|}"
done
for p in "${missing_build[@]}"; do
  printf '    %-14s %s\n' "$p" "Build-Paket"
done
echo "    (optional, nicht automatisch: codeql für die CodeQL-Checks)"

[ "$mode" = check ] && exit 0

if [ "$mode" = ask ]; then
  if { : </dev/tty; } 2>/dev/null; then
    printf 'Fehlende Programme jetzt installieren? [J/n] '
    read -r answer </dev/tty || answer=n
    case "$answer" in
      "" | j | J | ja | Ja | y | Y | yes) ;;
      *)
        echo "Übersprungen."
        exit 0
        ;;
    esac
  else
    echo "Kein Terminal für die Rückfrage: mit --yes erneut aufrufen." >&2
    exit 0
  fi
fi

is_missing() {
  local entry
  for entry in "${missing[@]}"; do
    [ "${entry%%|*}" = "$1" ] && return 0
  done
  return 1
}

hints=()

install_linux() {
  if ! have apt-get; then
    echo "Kein apt-get gefunden. Die Liste oben bitte mit dem Paketmanager der Distribution installieren." >&2
    exit 1
  fi
  local sudo=""
  [ "$(id -u)" -ne 0 ] && sudo="sudo"
  local -A apt_for=(
    [git]=git [gh]=gh [ssh]=openssh-client [curl]=curl [make]=make [python3]=python3
    [pyyaml]=python3-yaml [npx]=npm [yamllint]=yamllint [ansible-lint]=ansible-lint
    [gitleaks]=gitleaks [docker]=docker.io
  )
  local pkgs=("${missing_build[@]}") t p cand
  if is_missing cargo; then pkgs+=(curl ca-certificates build-essential); fi
  for t in "${!apt_for[@]}"; do
    is_missing "$t" || continue
    p="${apt_for[$t]}"
    cand="$(LC_ALL=C apt-cache policy "$p" 2>/dev/null | sed -n 's/^ *Candidate: //p')"
    if [ -n "$cand" ] && [ "$cand" != "(none)" ]; then
      pkgs+=("$p")
    elif [ "$t" = gitleaks ]; then
      :
    else
      hints+=("$t: kein apt-Paket $p gefunden, bitte manuell installieren")
    fi
  done
  if [ "${#pkgs[@]}" -gt 0 ]; then
    mapfile -t pkgs < <(printf '%s\n' "${pkgs[@]}" | sort -u)
    echo "==> apt: ${pkgs[*]}"
    $sudo apt-get update
    $sudo apt-get install -y --no-install-recommends "${pkgs[@]}"
  fi
  for t in kubectl helm argocd act gitleaks; do
    if is_missing "$t" && ! have "$t"; then
      echo "==> $t (offizielles Release, SHA-256 geprüft)"
      "fetch_$t"
    fi
  done
  if is_missing tailscale; then
    echo "==> tailscale (offizieller Installer von tailscale.com)"
    fetch https://tailscale.com/install.sh "$tmp/tailscale-install.sh"
    sh "$tmp/tailscale-install.sh"
    hints+=("Tailscale verbinden: sudo tailscale up")
  fi
  if is_missing docker && have docker && ! id -nG | grep -qw docker; then
    hints+=("Docker ohne sudo: sudo usermod -aG docker $USER, danach neu anmelden")
  fi
}

install_mac() {
  if ! have brew; then
    echo "Homebrew fehlt. Installation: https://brew.sh, danach ./deps.sh erneut aufrufen." >&2
    exit 1
  fi
  local -A brew_for=(
    [cargo]=rustup [git]=git [gh]=gh [kubectl]=kubernetes-cli [argocd]=argocd [helm]=helm
    [npx]=node [yamllint]=yamllint [ansible-lint]=ansible-lint [gitleaks]=gitleaks [act]=act
    [xcodegen]=xcodegen [python3]=python
  )
  local formulas=() t
  for t in "${!brew_for[@]}"; do
    is_missing "$t" && formulas+=("${brew_for[$t]}")
  done
  if [ "${#formulas[@]}" -gt 0 ]; then
    echo "==> brew: ${formulas[*]}"
    brew install "${formulas[@]}"
  fi
  if is_missing cargo; then
    rustup-init -y --profile minimal
  fi
  if is_missing pyyaml; then
    python3 -m pip install --user pyyaml || hints+=("PyYAML: python3 -m pip install --user pyyaml")
  fi
  if is_missing tailscale; then
    brew install --cask tailscale-app || brew install --cask tailscale || hints+=("Tailscale: https://tailscale.com/download/mac")
  fi
  if is_missing docker; then
    brew install --cask docker-desktop || brew install --cask docker || hints+=("Docker Desktop: https://www.docker.com/products/docker-desktop/")
    hints+=("Docker Desktop einmal starten, damit der Docker-Dienst läuft")
  fi
  if is_missing ssh || is_missing make || is_missing curl; then
    hints+=("ssh, make, curl: xcode-select --install")
  fi
}

case "$os" in
  Linux) install_linux ;;
  Darwin) install_mac ;;
  *)
    echo "Nicht unterstütztes System: $os" >&2
    exit 1
    ;;
esac

if is_missing cargo && [ "$os" = Linux ] && ! have_tool cargo; then
  echo "==> Rust (rustup, offizieller Installer)"
  fetch https://sh.rustup.rs "$tmp/rustup-init.sh"
  sh "$tmp/rustup-init.sh" -y --profile minimal
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) hints+=("$bin_dir in den PATH aufnehmen (z. B. in ~/.profile)") ;;
esac

echo "==> Pakete installiert."
for h in "${hints[@]}"; do
  echo "    Hinweis: $h"
done
