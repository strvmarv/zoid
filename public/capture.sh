#!/bin/sh
# Capture each marketing scene into public/frames/<scene>.html.
# Run from repo root: sh public/capture.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/public/frames"
mkdir -p "$OUT"
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"

# scene            w    h   → file
$RUN chat    140 24 > "$OUT/hero.html"
$RUN economy 140 24 > "$OUT/economy.html"
$RUN palette 140 24 > "$OUT/palette.html"
$RUN summary  96 20 > "$OUT/summary.html"
$RUN detail   96 20 > "$OUT/detail.html"
echo "captured: $(ls "$OUT")"
