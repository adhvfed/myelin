#!/usr/bin/env bash
#
# build.sh — render + export every icon in strok/ to preview/ and svg/.
#
# For every strok/<name>.strok:
#   1. render a PNG preview         -> preview/<name>.png
#   2. export a raw SVG (sentinel)  -> svg/<name>.raw.svg
#   3. post-process the $ink sentinel hex (#ff00ff) -> currentColor
#                                    -> svg/<name>.svg   (the shipped file)
#
# Idempotent: re-running regenerates the same outputs from the .strok sources,
# which remain the source of truth. No icon names are hardcoded — it loops over
# whatever .strok files exist, so the refine passes can add/edit freely.
#
# Requires: strok (vector CLI) on PATH, and sed.

set -euo pipefail

# Resolve directories relative to this script so it runs from anywhere.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STROK_DIR="$ROOT/strok"
SVG_DIR="$ROOT/svg"
PREVIEW_DIR="$ROOT/preview"

# The reserved sentinel hex bound to the $ink palette token in every .strok.
# It is used NOWHERE else, so the swap to currentColor can never collide.
SENTINEL="#ff00ff"

mkdir -p "$SVG_DIR" "$PREVIEW_DIR"

if ! command -v strok >/dev/null 2>&1; then
  echo "error: 'strok' not found on PATH" >&2
  exit 1
fi

shopt -s nullglob
files=("$STROK_DIR"/*.strok)
shopt -u nullglob

if [ ${#files[@]} -eq 0 ]; then
  echo "warning: no .strok files found in $STROK_DIR" >&2
  exit 0
fi

count=0
for src in "${files[@]}"; do
  name="$(basename "$src" .strok)"
  # Skip the optional shared-defaults include (not a renderable icon).
  case "$name" in
    _*) continue ;;
  esac

  png="$PREVIEW_DIR/$name.png"
  raw="$SVG_DIR/$name.raw.svg"
  out="$SVG_DIR/$name.svg"

  # 1. PNG preview (base palette: the sentinel renders as magenta, which is
  #    fine for a preview — it just proves the geometry).
  strok -f "$src" render --out "$png" >/dev/null

  # 2. Raw SVG export (sentinel hex still present).
  strok -f "$src" export svg --out "$raw" >/dev/null

  # 3. Sentinel -> currentColor. fill="none" survives untouched.
  sed "s/${SENTINEL}/currentColor/g" "$raw" > "$out"

  # Drop the intermediate raw file; the .strok + svg are the kept artifacts.
  rm -f "$raw"

  # Safety check: no leftover sentinel hex in the shipped file.
  if grep -qi "$SENTINEL" "$out"; then
    echo "error: $out still contains the sentinel hex $SENTINEL" >&2
    exit 1
  fi

  count=$((count + 1))
  echo "built $name"
done

echo "----"
echo "built $count icon(s) -> svg/ + preview/"
