#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target_dir="${HOME}/.local/bin"

python3 - <<'PY'
import importlib.util
import sys

missing = [name for name in ("rich", "yaml") if importlib.util.find_spec(name) is None]
if missing:
    print("Fehlende Python-Module: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
PY

chmod +x "${root}/bin/cubi"
mkdir -p "${target_dir}"
ln -sf "${root}/bin/cubi" "${target_dir}/cubi"
echo "cubi installiert: ${target_dir}/cubi"

case ":${PATH}:" in
  *":${target_dir}:"*) ;;
  *)
    echo "${target_dir} ist nicht im PATH" >&2

    path_line="export PATH=\"${target_dir}:\$PATH\""
    marker="# Added by cubi install.sh"
    updated_files=()

    for rc in "${HOME}/.zshrc" "${HOME}/.bashrc" "${HOME}/.bash_profile"; do
      if [[ -f "${rc}" ]] && grep -Fq "${target_dir}" "${rc}" 2>/dev/null; then
        continue
      fi
      {
        echo ""
        echo "${marker}"
        echo "${path_line}"
      } >> "${rc}"
      updated_files+=("${rc}")
    done

    if [[ ${#updated_files[@]} -gt 0 ]]; then
      echo "PATH-Eintrag ergänzt in: ${updated_files[*]}" >&2
      echo "Bitte Terminal neu starten oder 'source' auf die jeweilige Datei anwenden." >&2
    fi
    ;;
esac
