#!/usr/bin/env bash
# Builds the browser version of the desktop app (crates/slicer-ui) into a
# directory of static files, by default target/web. Serve it with
#   SPAGHETTI_WEB_DIR=target/web spaghetti-slicer-server
# and open http://localhost:8080/app/ (any static file server works too).
#
# Needs the wasm target and the wasm-bindgen command line tool in the same
# version as the wasm-bindgen crate in Cargo.lock:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version from Cargo.lock> --locked
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/web}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"

cargo build --release --locked --manifest-path "$root/Cargo.toml" \
  -p slicer-ui --bin spaghetti-slicer-ui --target wasm32-unknown-unknown
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name slicer \
  "$target_dir/wasm32-unknown-unknown/release/spaghetti-slicer-ui.wasm"
cp "$root/crates/slicer-ui/web/index.html" "$root/crates/slicer-ui/web/worker.js" "$out/"
echo "web app in $out"
