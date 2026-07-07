#!/bin/sh
# DISABLED. index.html is now hand-authored (responsive HTML/CSS terminal
# mockups live directly in it). This script would `cp template.html index.html`
# and CLOBBER the live page with the stale captured design. Short-circuited so
# it can't run by accident. To revive the capture pipeline, first sync
# template.html <- index.html, then delete the guard below.
echo "build.sh is disabled: index.html is hand-authored; running this would overwrite it." >&2
echo "See public/README.md. Refusing to clobber public/index.html." >&2
exit 1

# --- legacy pipeline (unreachable) ----------------------------------------
# Assemble public/index.html from template.html + captured frames.
# Regenerate frames first with: sh public/capture.sh
set -eu
cd "$(dirname "$0")"
[ -d frames ] || { echo "run capture.sh first (no frames/)"; exit 1; }
cp template.html index.html
for f in frames/*.html; do
  name=$(basename "$f" .html)
  # Replace the marker line with the fragment file's contents.
  awk -v marker="<!--FRAME:$name-->" -v file="$f" '
    $0 ~ marker { while ((getline line < file) > 0) print line; next }
    { print }
  ' index.html > index.html.tmp && mv index.html.tmp index.html
done
echo "built public/index.html"
