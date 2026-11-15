#!/usr/bin/env bash
# cubi-desktop bauen und als Desktop-App installieren (Linux und macOS).
#
#   ./install.sh               bauen, installieren, Verknüpfung auf dem Schreibtisch
#   ./install.sh --no-desktop  ohne Verknüpfung auf dem Schreibtisch
#   ./install.sh --uninstall   App, Starter und Icons entfernen (Konfiguration bleibt)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cfg="${XDG_CONFIG_HOME:-$HOME/.config}/cubi-desktop"
os="${CUBI_INSTALL_OS:-$(uname -s)}"
desktop_link=1
uninstall=0

for arg in "$@"; do
  case "$arg" in
    --no-desktop) desktop_link=0 ;;
    --uninstall) uninstall=1 ;;
    -h | --help)
      sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unbekannte Option: $arg" >&2
      exit 2
      ;;
  esac
done

desktop_dir() {
  local d=""
  if command -v xdg-user-dir >/dev/null 2>&1; then
    d="$(xdg-user-dir DESKTOP 2>/dev/null || true)"
  fi
  if [ -z "$d" ] || [ "$d" = "$HOME" ]; then
    d="$HOME/Desktop"
  fi
  printf '%s' "$d"
}

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -n 1)"

linux_paths() {
  bin_dir="$HOME/.local/bin"
  app_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  icon_root="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
  launcher="$app_dir/cubi-desktop.desktop"
}

mac_paths() {
  app="$HOME/Applications/cubi.app"
}

if [ "$uninstall" = 1 ]; then
  case "$os" in
    Linux)
      linux_paths
      rm -f "$bin_dir/cubi-desktop" "$launcher" "$(desktop_dir)/cubi-desktop.desktop"
      for s in 16 32 48 64 128 256 512; do
        rm -f "$icon_root/${s}x${s}/apps/cubi-desktop.png"
      done
      ;;
    Darwin)
      mac_paths
      if [ -f "$app/Contents/Info.plist" ] && grep -q "lab.pkr.cubi-desktop" "$app/Contents/Info.plist"; then
        rm -rf "$app"
      fi
      if [ -L "$HOME/Desktop/cubi.app" ]; then
        rm -f "$HOME/Desktop/cubi.app"
      fi
      ;;
  esac
  echo "cubi-desktop entfernt. Die Konfiguration in $cfg bleibt erhalten."
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo fehlt. Rust installieren: https://rustup.rs (macOS: brew install rustup && rustup-init)" >&2
  exit 1
fi

echo "==> Baue cubi-desktop $version"
cargo build --release --manifest-path "$here/Cargo.toml"
bin="$here/target/release/cubi-desktop"

echo "==> Konfiguration nach $cfg (vorhandene Dateien bleiben unverändert)"
mkdir -p "$cfg"
cp -R -n "$here/config/." "$cfg/" 2>/dev/null || true

case "$os" in
  Linux)
    linux_paths
    mkdir -p "$bin_dir" "$app_dir"
    cp "$bin" "$bin_dir/cubi-desktop"
    chmod 755 "$bin_dir/cubi-desktop"
    for s in 16 32 48 64 128 256 512; do
      mkdir -p "$icon_root/${s}x${s}/apps"
      cp "$here/assets/icon-$s.png" "$icon_root/${s}x${s}/apps/cubi-desktop.png"
    done
    cat >"$launcher" <<EOF
[Desktop Entry]
Type=Application
Version=1.5
Name=cubi
GenericName=Kubernetes- und CI-Werkzeug
Comment=ArgoCD, Kubernetes, GitHub und CI-Checks vor dem Push
Exec="$bin_dir/cubi-desktop"
Icon=cubi-desktop
Terminal=false
Categories=Development;
Keywords=kubernetes;argocd;github;ci;cubi;
StartupWMClass=cubi-desktop
StartupNotify=true
EOF
    chmod 644 "$launcher"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$app_dir" >/dev/null 2>&1 || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icon_root" >/dev/null 2>&1 || true
    echo "==> Startmenü-Eintrag: $launcher"
    if [ "$desktop_link" = 1 ]; then
      dd="$(desktop_dir)"
      if [ -d "$dd" ]; then
        cp "$launcher" "$dd/cubi-desktop.desktop"
        chmod 755 "$dd/cubi-desktop.desktop"
        command -v gio >/dev/null 2>&1 && gio set "$dd/cubi-desktop.desktop" metadata::trusted true >/dev/null 2>&1 || true
        echo "==> Schreibtisch-Symbol: $dd/cubi-desktop.desktop"
      fi
    fi
    ;;
  Darwin)
    mac_paths
    if [ -e "$app" ] && ! grep -q "lab.pkr.cubi-desktop" "$app/Contents/Info.plist" 2>/dev/null; then
      echo "$app existiert und ist keine cubi-desktop-App. Abbruch." >&2
      exit 1
    fi
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin" "$app/Contents/MacOS/cubi-desktop"
    chmod 755 "$app/Contents/MacOS/cubi-desktop"
    cp "$here/assets/icon.icns" "$app/Contents/Resources/AppIcon.icns"
    cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>cubi</string>
  <key>CFBundleDisplayName</key><string>cubi</string>
  <key>CFBundleIdentifier</key><string>lab.pkr.cubi-desktop</string>
  <key>CFBundleExecutable</key><string>cubi-desktop</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
    command -v codesign >/dev/null 2>&1 && codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
    touch "$app"
    echo "==> App: $app (im Launchpad und per Spotlight als „cubi“)"
    if [ "$desktop_link" = 1 ] && [ -d "$HOME/Desktop" ]; then
      ln -sfn "$app" "$HOME/Desktop/cubi.app"
      echo "==> Schreibtisch-Symbol: $HOME/Desktop/cubi.app"
    fi
    echo "    Fürs Dock: App starten, dann Rechtsklick aufs Dock-Symbol, Optionen, Im Dock behalten."
    ;;
  *)
    echo "Nicht unterstütztes System: $os (nur Linux und macOS)" >&2
    exit 1
    ;;
esac

echo "Fertig."
