#!/usr/bin/env bash
# KN-D2 / contract 13.1 WASM render-path gate.
#
# Builds `myelin-content` for the wasm32-unknown-unknown target from the SAME source the
# server compiles natively — proving the one-render-path mandate (no second renderer).
# The produced .wasm is the dated green artifact for the "compiles to both native and
# WASM from one source" leg of KN-D2.
#
# This crate is dependency-clean and std-only (serde + pure data newtypes), so it builds
# for wasm32-unknown-unknown with no extra glue. The editor crate (KN-P08) wraps the
# `myelin_content::wasm` free functions behind its own wasm-bindgen boundary.
#
# Requires the wasm32-unknown-unknown std component:
#   rustup target add wasm32-unknown-unknown      # rustup toolchains
#   pacman -S ... / the distro's rust-wasm pkg    # system toolchains
#
# FLOOR (named in lib.rs): on a host WITHOUT that std component installed this gate is
# red-until-proven; it flips green on CI / any host with the target. The round-trip
# correctness gate (cargo test -p myelin-content) is proven green natively against the
# identical single source regardless.
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
