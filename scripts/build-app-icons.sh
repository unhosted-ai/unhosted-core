#!/usr/bin/env bash
# Regenerate the app icon set referenced by tauri.conf.json's `bundle.icon`
# from the canonical SVG at branding/logo/app-icon.svg.
#
# Outputs (overwrites in place):
#   crates/unhosted-desktop/icons/32x32.png
#   crates/unhosted-desktop/icons/128x128.png
#   crates/unhosted-desktop/icons/128x128@2x.png  (256×256)
#   crates/unhosted-desktop/icons/icon.png        (512×512)
#   crates/unhosted-desktop/icons/icon.icns
#
# .ico (Windows) is NOT regenerated here — macOS has no built-in .ico
# encoder. Run this script's Windows half in CI (the `image` crate or
# icotool from icoutils), or `brew install icoutils` locally.
#
# Why this exists: the previously committed icons in crates/.../icons/
# were the old square (non-squircle) design. Tauri sets the runtime Dock
# icon from icon.icns at startup, so the .app appeared with the rounded
# plate during launch and snapped back to the square version once the
# window came up. Keeping the icons regenerated from one SVG source
# prevents that drift.
#
# Requires: rsvg-convert (`brew install librsvg`), iconutil (macOS built-in).
# Usage:    bash scripts/build-app-icons.sh

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SRC="branding/logo/app-icon.svg"
DST="crates/unhosted-desktop/icons"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v rsvg-convert >/dev/null || { echo "missing rsvg-convert. install: brew install librsvg"; exit 1; }
command -v iconutil      >/dev/null || { echo "missing iconutil (macOS built-in)"; exit 1; }

echo "→ rasterizing PNG variants from $SRC"
mkdir -p "$DST"
rsvg-convert -w 32   -h 32   "$SRC" -o "$DST/32x32.png"
rsvg-convert -w 128  -h 128  "$SRC" -o "$DST/128x128.png"
rsvg-convert -w 256  -h 256  "$SRC" -o "$DST/128x128@2x.png"
rsvg-convert -w 512  -h 512  "$SRC" -o "$DST/icon.png"

echo "→ building icon.icns iconset"
ICONSET="$TMP/icon.iconset"
mkdir -p "$ICONSET"
for spec in \
    "16    icon_16x16.png" \
    "32    icon_16x16@2x.png" \
    "32    icon_32x32.png" \
    "64    icon_32x32@2x.png" \
    "128   icon_128x128.png" \
    "256   icon_128x128@2x.png" \
    "256   icon_256x256.png" \
    "512   icon_256x256@2x.png" \
    "512   icon_512x512.png" \
    "1024  icon_512x512@2x.png"
do
  size="${spec%% *}"
  name="${spec##* }"
  rsvg-convert -w "$size" -h "$size" "$SRC" -o "$ICONSET/$name"
done
iconutil -c icns "$ICONSET" -o "$DST/icon.icns"

echo
echo "Done. Regenerated:"
ls -1 "$DST"/*.png "$DST"/*.icns
echo
echo "  next: rebuild the desktop crate so Tauri picks them up."
echo "    cargo build --release -p unhosted-desktop"
echo "  and re-deploy /Applications/unhosted.app via scripts/bundle-macos.sh"
