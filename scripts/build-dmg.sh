#!/usr/bin/env bash
# Build a macOS .dmg containing unhosted.app — the real
# "double-click installer" experience on macOS. Drops the result
# at dist/unhosted.dmg.
#
# What's inside the .dmg:
#   - unhosted.app (the desktop shell, built by scripts/bundle-macos.sh)
#   - a symlink to /Applications so the user drag-drops the .app in
#   - a styled Finder window: branded background + positioned icons,
#     same drag-to-install layout the Tauri-bundled release DMG uses
#     (bundle.macOS.dmg in crates/unhosted-desktop/tauri.conf.json)
#
# Requires: hdiutil (built into macOS). Does NOT require code-signing
# tools — the .dmg itself is unsigned. Gatekeeper will still prompt on
# first launch of the .app inside (right-click → open). Proper Developer
# ID signing runs in scripts/release-macos.sh when identities are set.
#
# Usage: bash scripts/build-dmg.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "build-dmg.sh only runs on macOS."
  exit 1
fi

ROOT="$(pwd)"
DIST="$ROOT/dist"
APP="$DIST/unhosted.app"
DMG="$DIST/unhosted.dmg"
DMG_RW="$DIST/unhosted-rw.dmg"
STAGING="$DIST/dmg-staging"
VOL_NAME="unhosted"
BACKGROUND="$ROOT/crates/unhosted-desktop/dmg/background.png"

# Build the .app first if it doesn't exist (this is what gets dragged
# into /Applications from the .dmg).
if [ ! -d "$APP" ]; then
  echo "→ building unhosted.app first"
  bash scripts/bundle-macos.sh
fi

# Stage: a clean directory with the .app, a symlink to /Applications,
# and the hidden .background dir Finder reads the window art from.
rm -rf "$STAGING" "$DMG" "$DMG_RW"
mkdir -p "$STAGING/.background"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
if [ -f "$BACKGROUND" ]; then
  cp "$BACKGROUND" "$STAGING/.background/background.png"
fi

echo "→ creating writable image"
hdiutil create \
  -volname "$VOL_NAME" \
  -srcfolder "$STAGING" \
  -ov \
  -format UDRW \
  -fs HFS+ \
  "$DMG_RW" > /dev/null

# Mount the writable image and let Finder lay out the window: icon
# view, no chrome, branded background, app on the left, Applications
# on the right. Mirrors bundle.macOS.dmg in tauri.conf.json so local
# and released DMGs look identical. Best-effort: a headless session
# without Finder scripting still produces a valid (unstyled) DMG.
echo "→ styling installer window"
MOUNT_DIR="/Volumes/$VOL_NAME"
hdiutil attach "$DMG_RW" -readwrite -noverify -noautoopen > /dev/null
if osascript <<EOF
tell application "Finder"
  tell disk "$VOL_NAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, 860, 540}
    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 110
    set background picture of viewOptions to file ".background:background.png"
    set position of item "unhosted.app" of container window to {170, 200}
    set position of item "Applications" of container window to {490, 200}
    close
    open
    update without registering applications
    delay 1
    close
  end tell
end tell
EOF
then
  echo "  styled"
else
  echo "  Finder scripting unavailable — shipping unstyled layout"
fi
sync
hdiutil detach "$MOUNT_DIR" > /dev/null

echo "→ compressing to $DMG"
hdiutil convert "$DMG_RW" -format UDZO -imagekey zlib-level=9 -o "$DMG" > /dev/null

# Cleanup
rm -rf "$STAGING" "$DMG_RW"

SIZE="$(du -h "$DMG" | awk '{print $1}')"
echo
echo "done."
echo "  $DMG  ($SIZE)"
echo
echo "  test it:  open \"$DMG\""
echo "  install:  mv \"$DMG\" ~/Downloads/    # or upload to a release"
