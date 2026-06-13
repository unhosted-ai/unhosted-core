#!/usr/bin/env bash
# Build the macOS `.app` bundle for unhosted-desktop.
#
# What it does:
#   1. Builds the release binary if it doesn't exist
#   2. Rasterizes branding/logo/app-icon.svg → multi-size iconset → .icns
#   3. Lays out unhosted.app/ with Info.plist, MacOS/<binary>, Resources/<icns>
#
# Requires: rsvg-convert (brew install librsvg), iconutil (macOS built-in).
# Output: dist/unhosted.app
#
# Usage: bash scripts/bundle-macos.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

ROOT="$(pwd)"
DIST="$ROOT/dist"
APP="$DIST/unhosted.app"
APP_BIN_DIR="$APP/Contents/MacOS"
APP_RES_DIR="$APP/Contents/Resources"
ICON_SRC="$ROOT/branding/logo/app-icon.svg"
ICONSET="$DIST/unhosted.iconset"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "bundle-macos.sh only runs on macOS. on Linux/Windows, use the raw binary."
  exit 1
fi

command -v rsvg-convert >/dev/null || { echo "missing rsvg-convert. install with: brew install librsvg"; exit 1; }
command -v iconutil      >/dev/null || { echo "missing iconutil (should be built into macOS)"; exit 1; }

# ----- build binary if needed --------------------------------------------------

BIN="${UNHOSTED_BIN:-$ROOT/target/release/unhosted-desktop}"
if [ ! -x "$BIN" ]; then
  echo "→ cargo build --release -p unhosted-desktop"
  cargo build --release -p unhosted-desktop
  BIN="$ROOT/target/release/unhosted-desktop"
fi

# ----- generate .icns ----------------------------------------------------------

echo "→ generating iconset from $ICON_SRC"
rm -rf "$ICONSET"
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
  rsvg-convert -w "$size" -h "$size" "$ICON_SRC" -o "$ICONSET/$name"
done

iconutil -c icns "$ICONSET" -o "$DIST/unhosted.icns"

# ----- lay out the .app --------------------------------------------------------

echo "→ assembling $APP"
rm -rf "$APP"
mkdir -p "$APP_BIN_DIR" "$APP_RES_DIR"

cp "$ROOT/crates/unhosted-desktop/Info.plist" "$APP/Contents/Info.plist"
cp "$BIN" "$APP_BIN_DIR/unhosted-desktop"
cp "$DIST/unhosted.icns" "$APP_RES_DIR/unhosted.icns"

# Bundle the daemon CLI next to the shell binary. The shell's
# try_spawn_daemon() checks its own directory first, so a DMG-only
# install works on a machine that never ran install.sh.
DAEMON_BIN="${UNHOSTED_DAEMON_BIN:-$ROOT/target/release/unhosted}"
if [ ! -x "$DAEMON_BIN" ]; then
  echo "→ cargo build --release -p unhosted-cli"
  cargo build --release -p unhosted-cli
  DAEMON_BIN="$ROOT/target/release/unhosted"
fi
cp "$DAEMON_BIN" "$APP_BIN_DIR/unhosted"

# Ad-hoc sign so macOS will at least launch it locally (no Gatekeeper warning
# beyond "downloaded from internet" for distribution; that needs a real cert).
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true

# Tell Finder to refresh icon caches for this binary.
touch "$APP"

# LaunchServices will register this dist build the first time anyone opens it,
# competing with /Applications/unhosted.app (same CFBundleIdentifier). When
# two paths claim the same bundle id, Dock alternates icons unpredictably —
# users see the "logo switches" symptom. Unregister the build artifact here
# so only the installed copy advertises itself.
LSREG=/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister
[ -x "$LSREG" ] && "$LSREG" -u "$APP" >/dev/null 2>&1 || true

echo
echo "Done."
echo "  $APP"
echo
echo "  open $APP       # to launch"
echo "  cp -R $APP /Applications/  # to install"
