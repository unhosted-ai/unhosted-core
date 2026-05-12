#!/usr/bin/env bash
# Build a macOS .dmg containing unhosted.app — the real
# "double-click installer" experience on macOS. Drops the result
# at dist/unhosted.dmg.
#
# What's inside the .dmg:
#   - unhosted.app (the desktop shell, built by scripts/bundle-macos.sh)
#   - a symlink to /Applications so the user drag-drops the .app in
#
# Requires: hdiutil (built into macOS). Does NOT require code-signing
# tools — the .dmg itself is unsigned. Gatekeeper will still prompt on
# first launch of the .app inside (right-click → open). Proper Developer
# ID signing is on the roadmap.
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
STAGING="$DIST/dmg-staging"
VOL_NAME="unhosted"

# Build the .app first if it doesn't exist (this is what gets dragged
# into /Applications from the .dmg).
if [ ! -d "$APP" ]; then
  echo "→ building unhosted.app first"
  bash scripts/bundle-macos.sh
fi

# Stage: a clean directory with the .app and a symlink to /Applications.
# hdiutil reads from here when creating the .dmg.
rm -rf "$STAGING" "$DMG"
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"

# Optional: drop in a tiny README so users know what to do.
cat > "$STAGING/INSTALL.txt" <<'EOF'
to install unhosted:
  1. drag the "unhosted" icon onto the "Applications" shortcut.
  2. open /Applications/unhosted.app (right-click → open the first time
     so gatekeeper lets it through; code signing is on the roadmap).

you'll also need a local model runtime — llama.cpp, ollama, or
lm studio. the daemon auto-detects whichever is running.

see https://github.com/unhosted-ai/unhosted-core for details.
EOF

echo "→ creating $DMG"
hdiutil create \
  -volname "$VOL_NAME" \
  -srcfolder "$STAGING" \
  -ov \
  -format UDZO \
  -fs HFS+ \
  "$DMG" > /dev/null

# Cleanup
rm -rf "$STAGING"

SIZE="$(du -h "$DMG" | awk '{print $1}')"
echo
echo "done."
echo "  $DMG  ($SIZE)"
echo
echo "  test it:  open \"$DMG\""
echo "  install:  mv \"$DMG\" ~/Downloads/    # or upload to a release"
