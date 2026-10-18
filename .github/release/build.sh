#!/usr/bin/env bash
set -euo pipefail

version="$1"
mkdir -p dist
git archive --format=tar.gz --prefix="cubi-v${version}/" -o "dist/cubi-v${version}.tar.gz" HEAD bin src config install.sh README.md
