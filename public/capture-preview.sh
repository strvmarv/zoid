#!/bin/sh
# Render the context-economy frame sequence into public/frames/context-economy/.
# Run from repo root: sh public/capture-preview.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/public/frames/context-economy"
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"
mkdir -p "$OUT"
rm -f "$OUT"/*.html
N=$($RUN --count context-economy)
i=0
while [ "$i" -lt "$N" ]; do
  f=$(printf "%02d" "$i")
  $RUN --frame "$i" context-economy 160 40 > "$OUT/$f.html"
  i=$((i + 1))
done
echo "captured $N frames → $OUT"
