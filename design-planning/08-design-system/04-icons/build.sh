#!/usr/bin/env bash
#
# build.sh — render + export every icon in strok/ to svg/ and preview/.
#
#   svg/<name>.svg      themeable SVG, stroke="currentColor" preserved (shipped file)
#   preview/<name>.png  raster preview at 64px, inked for visual review
#
# Uses `strok batch` (the icon-set export path): the .strok sources author colour as
# the literal `currentColor` (icon profile), so the exported SVG inherits the host
# stylesheet's colour and PNG previews substitute a concrete --color. No sentinel,
# no sed, no per-file loop.
#
# Idempotent: regenerates the same outputs from the .strok sources (the source of
# truth). No icon names are hardcoded.
#
# Requires: strok (vector CLI) with `batch` support on PATH.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STROK_DIR="$ROOT/strok"
SVG_DIR="$ROOT/svg"
PREVIEW_DIR="$ROOT/preview"
INK="#1a1a1a"   # preview ink (SVGs stay currentColor; this only colours the PNGs)

if ! command -v strok >/dev/null 2>&1; then
  echo "error: 'strok' not found on PATH" >&2; exit 1
fi
if ! strok batch --help >/dev/null 2>&1; then
  echo "error: this strok build lacks 'batch' — rebuild/reinstall strok (cargo install --path strok-cli --force)" >&2; exit 1
fi

mkdir -p "$SVG_DIR" "$PREVIEW_DIR"

# Themeable SVGs (currentColor preserved).
strok batch "$STROK_DIR" --svg --out "$SVG_DIR" >/dev/null

# 64px inked PNG previews (single size -> <name>.png).
strok batch "$STROK_DIR" --png --color "$INK" --sizes 64 --out "$PREVIEW_DIR" >/dev/null

# Verify every shipped SVG inherits currentColor and carries no baked hex.
missing="$(grep -L 'currentColor' "$SVG_DIR"/*.svg || true)"
if [ -n "$missing" ]; then
  echo "error: these SVGs do not inherit currentColor:" >&2; echo "$missing" >&2; exit 1
fi
if grep -lE '#[0-9a-fA-F]{3,6}' "$SVG_DIR"/*.svg >/dev/null 2>&1; then
  echo "error: a shipped SVG contains a baked hex colour" >&2
  grep -lE '#[0-9a-fA-F]{3,6}' "$SVG_DIR"/*.svg >&2; exit 1
fi

echo "built $(ls "$SVG_DIR"/*.svg | wc -l | tr -d ' ') icon(s) -> svg/ (currentColor) + preview/ (64px @ $INK)"
