#!/usr/bin/env bash
# Generate a full HTML report from a folder of statement exports — NO LLM / no
# Claude involved. This is the pure, deterministic pipeline only:
#
#     build  ->  wdmg analyze  ->  wdmg report
#
# Usage:
#   scripts/report.sh --input <data-dir> [--config <cfg-dir>] [--out <out-dir>]
#
#   --input   (required) folder containing your CSV statement exports.
#   --config  (optional) config dir holding profiles/categories/people.toml.
#             If missing or empty, shipped defaults are written there (and any
#             individual file you leave out falls back to the embedded default).
#             Edit categories.toml / people.toml to tune, then just re-run.
#   --out     (optional) where analysis.json + report.html are written.
#             Defaults to "<input>/wdmg-report".
#
# Prints the path to report.html on the last line of stdout. The report opens
# fully offline in any browser. The only thing missing vs. the LLM-assisted flow
# is the "Recommendations" narrative — every chart, table and number is complete.
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() { sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; }

INPUT="" CONFIG="" OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --input)  INPUT="${2:-}"; shift 2;;
    --config) CONFIG="${2:-}"; shift 2;;
    --out)    OUT="${2:-}"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2;;
  esac
done

if [[ -z "$INPUT" ]]; then echo "error: --input <data-dir> is required" >&2; usage >&2; exit 2; fi
if [[ ! -d "$INPUT" ]]; then echo "error: input directory not found: $INPUT" >&2; exit 2; fi

INPUT="$(cd "$INPUT" && pwd)"
OUT="${OUT:-$INPUT/wdmg-report}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
CONFIG="${CONFIG:-$OUT/config}"
mkdir -p "$CONFIG"; CONFIG="$(cd "$CONFIG" && pwd)"

# Build (or reuse) the release binary; build.sh prints its path on the last line.
BIN="$("$SKILL_DIR/scripts/build.sh")"

# Seed editable default config the first time (never overwrites existing files).
if ! ls "$CONFIG"/*.toml >/dev/null 2>&1; then
  echo "Writing default config into $CONFIG (edit categories.toml to tune, then re-run)." >&2
  "$BIN" init "$CONFIG" >&2
fi

echo "Analyzing exports in: $INPUT" >&2
"$BIN" analyze --input "$INPUT" --config "$CONFIG" --out "$OUT" >&2

echo "Rendering report..." >&2
"$BIN" report --analysis "$OUT/analysis.json" --out "$OUT/report.html" >&2

echo "Done. Open this file in a browser (works offline):" >&2
echo "$OUT/report.html"
