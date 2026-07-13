#!/bin/sh
# Inline captured frames into public/index.html between per-scene markers.
# Idempotent: re-running with the same frames yields a byte-identical file.
# Run from repo root: sh public/assemble-site.sh  (after capture-preview.sh)
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HTML="$ROOT/public/index.html"
SCENES="context-economy tools-models"

for scene in $SCENES; do
  DIR="$ROOT/public/frames/$scene"
  [ -d "$DIR" ] || { echo "missing frames for $scene (run capture-preview.sh)"; exit 1; }

  # Build the frames block (each fragment wrapped in a .tui-frame div).
  BLOCK=$(mktemp)
  for f in "$DIR"/*.html; do
    printf '        <div class="tui-frame">' >> "$BLOCK"
    cat "$f" >> "$BLOCK"
    printf '</div>\n' >> "$BLOCK"
  done

  # Replace everything between the BEGIN/END markers (exclusive) with the block.
  TMP=$(mktemp)
  awk -v b="FRAMES:$scene:BEGIN" -v e="FRAMES:$scene:END" -v ff="$BLOCK" '
    $0 ~ b { print; while ((getline line < ff) > 0) print line; skip=1; next }
    $0 ~ e { skip=0 }
    skip==1 { next }
    { print }
  ' "$HTML" > "$TMP"
  mv "$TMP" "$HTML"
  rm -f "$BLOCK"
  echo "inlined $scene → index.html"
done
