#!/bin/sh
# Render each scene's frame sequence into public/frames/<scene>/.
# Run from repo root: sh public/capture-preview.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"
SCENES="context-economy tools-models extensibility"
for scene in $SCENES; do
  OUT="$ROOT/public/frames/$scene"
  mkdir -p "$OUT"
  rm -f "$OUT"/*.html
  N=$($RUN --count "$scene")
  i=0
  while [ "$i" -lt "$N" ]; do
    f=$(printf "%02d" "$i")
    $RUN --frame "$i" "$scene" 160 40 > "$OUT/$f.html"
    i=$((i + 1))
  done
  echo "captured $N frames → $OUT"
done
