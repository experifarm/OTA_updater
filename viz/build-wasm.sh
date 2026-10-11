#!/usr/bin/env bash
# Build the OTA simulation (viz/wasm) to WebAssembly and generate JS bindings into viz/web/src/pkg.
# Uses the locally installed wasm-bindgen CLI (must match the crate's pinned version) and, if present,
# wasm-opt. No downloads.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p ota_viz --release --target wasm32-unknown-unknown
OUT=viz/web/src/pkg
mkdir -p "$OUT"
# --omit-default-module-path: the app instantiates from embedded bytes, so the glue must not reference
# the .wasm file by URL (a bundler would otherwise inline it a second time).
wasm-bindgen --target web --omit-default-module-path --out-dir "$OUT" \
  target/wasm32-unknown-unknown/release/ota_viz.wasm
if command -v wasm-opt >/dev/null 2>&1; then
  # Features enabled by default by rustc's wasm32-unknown-unknown target.
  wasm-opt -O3 \
    --enable-bulk-memory --enable-mutable-globals --enable-nontrapping-float-to-int \
    --enable-sign-ext --enable-reference-types --enable-multivalue \
    "$OUT/ota_viz_bg.wasm" -o "$OUT/ota_viz_bg.wasm"
fi
ls -l "$OUT"/ota_viz_bg.wasm
