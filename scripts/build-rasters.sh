#!/usr/bin/env bash
# Build PNG + JPG raster versions of every SVG in docs/branding/.
# Requires: rsvg-convert (brew install librsvg), sips (built into macOS).
# Run from repo root: bash scripts/build-rasters.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)/docs/branding"

echo "→ social cards (PNG + JPG, native viewBox size)"
for img in og-image github-social x-banner; do
  rsvg-convert "${img}.svg" -o "${img}.png"
  sips -s format jpeg -s formatOptions 92 "${img}.png" --out "${img}.jpg" >/dev/null
  echo "    ${img}.{svg,png,jpg}"
done

echo "→ logos and marks (PNG @ 512 height)"
for img in logo logo-light logo-dark stacked-mark; do
  rsvg-convert -h 512 "${img}.svg" -o "${img}.png"
  echo "    ${img}.{svg,png}"
done

echo "→ wordmark + lockups (PNG at usable widths)"
rsvg-convert -w 1024 wordmark.svg        -o wordmark.png
rsvg-convert -w 1200 lockup.svg          -o lockup.png
rsvg-convert -w 800  lockup-stacked.svg  -o lockup-stacked.png
echo "    wordmark.png lockup.png lockup-stacked.png"

echo "→ favicons (PNG at 32 / 64 / 128 / 256 / 512)"
for size in 32 64 128 256 512; do
  rsvg-convert -w "${size}" -h "${size}" favicon.svg -o "favicon-${size}.png"
done
rsvg-convert -w 180 -h 180 apple-touch-icon.svg -o apple-touch-icon.png
echo "    favicon-32/64/128/256/512.png + apple-touch-icon.png"

echo "→ JPG with solid backgrounds for logos (for surfaces that don't do transparency)"
# logo on cream
rsvg-convert -h 512 logo-light.svg -b "#F5F5F0" -o /tmp/_logo-on-cream.png
sips -s format jpeg -s formatOptions 92 /tmp/_logo-on-cream.png --out logo-on-cream.jpg >/dev/null
# logo on near-black
rsvg-convert -h 512 logo-dark.svg -b "#0A0A0A" -o /tmp/_logo-on-dark.png
sips -s format jpeg -s formatOptions 92 /tmp/_logo-on-dark.png --out logo-on-dark.jpg >/dev/null
rm -f /tmp/_logo-on-cream.png /tmp/_logo-on-dark.png
echo "    logo-on-cream.jpg logo-on-dark.jpg"

echo
echo "Done."
ls -1 *.png *.jpg | sort
