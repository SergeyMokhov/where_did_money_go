#!/usr/bin/env bash
# Regenerate the playground (demo) data and check it analyzes cleanly.
#
#   ./playground/generate.sh
#
# Produces ./playground/data/*.csv and runs the analyzer with ./playground/config
# (writing analysis.json + review_queue.json here). The browser app bundles this
# data + config as its demo project: rebuild it with `cd web && yarn build`.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
BIN="$ROOT/wdmg/target/release/wdmg"

if [[ ! -x "$BIN" ]]; then
  echo "building wdmg…"
  ( cd "$ROOT/wdmg" && cargo build --release )
fi

echo "generating themed data…"
python3 "$HERE/generate_data.py"

echo "analyzing…"
"$BIN" analyze --input "$HERE/data" --config "$HERE/config" --out "$HERE"

echo "done — rebuild the app (cd web && yarn build) to refresh its demo project"
