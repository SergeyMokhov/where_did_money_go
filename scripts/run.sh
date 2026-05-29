#!/usr/bin/env bash
# Thin wrapper: ensure the binary is built, then forward all args to it.
#   scripts/run.sh analyze --input <dir> --config <dir> --out <dir>
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$("$SKILL_DIR/scripts/build.sh")"
exec "$BIN" "$@"
