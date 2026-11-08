#!/usr/bin/env bash
# Build the WebAssembly client into dist/ and the native server.
#   ./build.sh            release build
#   ./build.sh --dev      faster debug build of the client
set -euo pipefail
cd "$(dirname "$0")"
PROFILE=release
FLAG=--release
if [ "${1:-}" = "--dev" ]; then PROFILE=debug; FLAG=; fi
cargo build -p rift-client --target wasm32-unknown-unknown $FLAG
rm -rf dist && mkdir -p dist/pkg
wasm-bindgen --target web --no-typescript --out-dir dist/pkg "target/wasm32-unknown-unknown/$PROFILE/rift_client.wasm"
cp -r web/* dist/
# Stamp the service worker so each build replaces the cached app shell.
BUILD_ID=$(cat dist/pkg/rift_client_bg.wasm dist/pkg/rift_client.js dist/*.css dist/index.html | sha256sum | cut -c1-12)
sed -i "s/__BUILD__/$BUILD_ID/" dist/sw.js
cargo build -p rift-server $FLAG
echo "Built dist/ ($(du -sh dist/pkg/rift_client_bg.wasm | cut -f1) wasm) and target/$PROFILE/rift-server"
