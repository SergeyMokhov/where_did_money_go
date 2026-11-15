#!/usr/bin/env bash
# Build the wdmg WebAssembly module and its JS glue for the browser app.
#   scripts/build-wasm.sh
# Output (generated, git-ignored): web/src/wasm/{wdmg.js, wdmg_bg.wasm, *.d.ts}
# Requires: rustup target wasm32-unknown-unknown, and the wasm-bindgen CLI at the
# exact version pinned in wdmg-wasm/Cargo.toml (cargo install wasm-bindgen-cli
# --version <that version>).
set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATE_DIR="$SKILL_DIR/wdmg-wasm"
OUT_DIR="$SKILL_DIR/web/src/wasm"
WASM="$CRATE_DIR/target/wasm32-unknown-unknown/release/wdmg_wasm.wasm"

BINDGEN="$(command -v wasm-bindgen || true)"
if [[ -z "$BINDGEN" && -x "$HOME/.cargo/bin/wasm-bindgen" ]]; then
  BINDGEN="$HOME/.cargo/bin/wasm-bindgen"
fi
if [[ -z "$BINDGEN" ]]; then
  echo "error: wasm-bindgen CLI not found (cargo install wasm-bindgen-cli)" >&2
  exit 1
fi

# The CLI and the crate's wasm-bindgen must be the same version, or the glue and
# the module disagree at runtime.
want="$(sed -n 's/^wasm-bindgen = "=\(.*\)"$/\1/p' "$CRATE_DIR/Cargo.toml")"
have="$("$BINDGEN" --version | awk '{print $2}')"
if [[ -n "$want" && "$want" != "$have" ]]; then
  echo "error: wasm-bindgen CLI is $have but wdmg-wasm pins $want" >&2
  exit 1
fi

echo "Building wdmg-wasm (release, wasm32-unknown-unknown)..." >&2
( cd "$CRATE_DIR" && cargo build -p wdmg-wasm --release --target wasm32-unknown-unknown ) >&2

mkdir -p "$OUT_DIR"
"$BINDGEN" --target web --out-dir "$OUT_DIR" --out-name wdmg "$WASM"
ls -l "$OUT_DIR" >&2
echo "$OUT_DIR"
