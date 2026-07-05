#!/bin/sh
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
