#!/usr/bin/env bash
# Regenerate the playground data and report end-to-end.
#
#   ./playground/generate.sh
#
# Produces ./playground/data/*.csv, runs the analyzer with ./playground/config,
# and renders ./playground/report.html — open it in any browser (fully offline).
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

echo "rendering report…"
"$BIN" report --analysis "$HERE/analysis.json" --out "$HERE/report.html"

echo "done -> $HERE/report.html"
