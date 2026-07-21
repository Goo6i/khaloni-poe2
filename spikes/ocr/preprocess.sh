#!/usr/bin/env bash
# PoE2 renders light text on dark panels; tesseract wants dark-on-light.
# Produces <name>.pre.png next to each input.
set -euo pipefail
for f in "$@"; do
  out="${f%.png}.pre.png"
  magick "$f" -colorspace Gray -resize 300% -negate -normalize "$out"
  echo "wrote $out"
done
