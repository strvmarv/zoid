#!/bin/sh
# Render each scene's frame sequence into public/frames/<scene>/.
# Run from repo root: sh public/capture-preview.sh
#
# The site is deliberately version-free: the status bar's version comes from
# CARGO_PKG_VERSION at compile time, so every capture would otherwise pin the
# page to whatever version it was built at and go stale on the next release.
# strip_version blanks it, keeping the exact column count so the wordmark stays
# centered — byte-for-byte what title_line() renders when a terminal is too
# narrow for the version (see zoid-tui/src/render.rs).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN="cargo run -q -p zoid-tui --features web-capture --example web_capture --"
SCENES="context-economy tools-models extensibility"

# Replace the version span with spaces of identical width (v0.4.0 -> 6, but
# v0.10.0 -> 7: substituting a fixed width would shift the bar).
strip_version() {
  perl -pi -e 's|<span style="color:#6e7681">(v\d+\.\d+\.\d+)</span>|" " x length($1)|ge' "$1"
}

for scene in $SCENES; do
  OUT="$ROOT/public/frames/$scene"
  mkdir -p "$OUT"
  rm -f "$OUT"/*.html
  N=$($RUN --count "$scene")
  i=0
  while [ "$i" -lt "$N" ]; do
    f=$(printf "%02d" "$i")
    $RUN --frame "$i" "$scene" 160 40 > "$OUT/$f.html"
    strip_version "$OUT/$f.html"
    i=$((i + 1))
  done
  echo "captured $N frames → $OUT (version stripped)"
done
