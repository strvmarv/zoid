#!/bin/sh
# Inline captured context-economy frames into preview.template.html.
# Run from repo root: sh public/assemble-preview.sh  (after capture-preview.sh)
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR="$ROOT/public/frames/context-economy"
TPL="$ROOT/public/preview.template.html"
OUT="$ROOT/public/preview.html"
[ -d "$DIR" ] || { echo "run capture-preview.sh first (no frames/)"; exit 1; }

# Wrap each captured <pre> fragment in a .tui-frame div → a temp block.
TMP=$(mktemp)
for f in "$DIR"/*.html; do
  printf '<div class="tui-frame">' >> "$TMP"
  cat "$f" >> "$TMP"
  printf '</div>\n' >> "$TMP"
done

# Replace the single marker line with the frames block.
awk -v marker="<!--SEQ:context-economy-->" -v ff="$TMP" '
  $0 ~ marker { while ((getline line < ff) > 0) print line; next }
  { print }
' "$TPL" > "$OUT"
rm -f "$TMP"
echo "assembled → $OUT"
