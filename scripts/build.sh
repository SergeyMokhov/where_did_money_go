#!/usr/bin/env bash
# Build the wdmg release binary once; reuse if already current.
# Prints the absolute path to the binary on stdout (last line).
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE_DIR="$SKILL_DIR/wdmg"
BIN="$CRATE_DIR/target/release/wdmg"

# Rebuild only if the binary is missing or any source/manifest is newer than it.
needs_build=0
if [[ ! -x "$BIN" ]]; then
  needs_build=1
else
  if [[ -n "$(find "$CRATE_DIR/src" "$CRATE_DIR/Cargo.toml" "$SKILL_DIR/assets" -newer "$BIN" -print -quit 2>/dev/null)" ]]; then
    needs_build=1
  fi
fi

if [[ "$needs_build" -eq 1 ]]; then
  echo "Building wdmg (release)..." >&2
  ( cd "$CRATE_DIR" && cargo build --release ) >&2
fi

echo "$BIN"
