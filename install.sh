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
  *) echo "${target_dir} ist nicht im PATH" >&2 ;;
esac
