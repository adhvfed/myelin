#!/usr/bin/env bash
set -euo pipefail

TARGET=wasm32-unknown-unknown
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! rustc --print target-libdir --target "$TARGET" >/dev/null 2>&1 \
   || [ ! -d "$(rustc --print target-libdir --target "$TARGET" 2>/dev/null)" ]; then
  echo "FLOOR (red-until-proven): wasm32-unknown-unknown std component not installed on this host."
  echo "Install the target, then re-run this script to flip the WASM-artifact gate green."
  exit 3
fi

echo "Building myelin-content for $TARGET (the one render path)..."
cargo build -p myelin-content --target "$TARGET" --release

ARTIFACT="$(find "$CRATE_DIR/../../target/$TARGET/release" -name 'libmyelin_content.rlib' -o -name 'myelin_content.wasm' 2>/dev/null | head -1)"
echo "GREEN: WASM render path compiled. artifact: ${ARTIFACT:-target/$TARGET/release/}"
