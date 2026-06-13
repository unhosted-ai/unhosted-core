#!/usr/bin/env bash
# Build a reproducible macOS desktop release bundle.
#
# Pipeline (similar to a Makefile release flow):
#   1) Build + bundle unhosted.app
#   2) Optionally Developer-ID sign the app
#   3) Build .dmg from that app
#   4) Optionally sign + notarize + staple
#   5) Emit versioned artifacts + SHA-256 checksums
#
# Usage:
#   bash scripts/release-macos.sh
#
# Optional env:
#   UNHOSTED_VERSION=v0.1.0
#   UNHOSTED_RELEASE_DIR=dist/release-macos
#   UNHOSTED_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   UNHOSTED_NOTARY_PROFILE="AC_PASSWORD"
#
# Notes:
# - If signing identity is not set, artifacts are built unsigned/ad-hoc only.
# - Notarization runs only when both signing identity + notary profile are set.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "release-macos.sh only runs on macOS."
  exit 1
fi

cd "$(git rev-parse --show-toplevel)"

ROOT="$(pwd)"
DIST="$ROOT/dist"
APP="$DIST/unhosted.app"
DMG="$DIST/unhosted.dmg"

TARGET_DEFAULT="$(rustc -vV | sed -n 's/^host: //p' | head -1)"
TARGET="${UNHOSTED_TARGET:-$TARGET_DEFAULT}"
VERSION="${UNHOSTED_VERSION:-$(git describe --tags --always --dirty 2>/dev/null || echo dev)}"
RELEASE_DIR="${UNHOSTED_RELEASE_DIR:-$DIST/release-macos}"
SIGN_IDENTITY="${UNHOSTED_SIGN_IDENTITY:-}"
NOTARY_PROFILE="${UNHOSTED_NOTARY_PROFILE:-}"
ALLOW_DIRTY="${UNHOSTED_ALLOW_DIRTY:-0}"

APP_TAR_NAME="unhosted-macos-app-${TARGET}.tar.gz"
DMG_NAME="unhosted-macos-${VERSION}-${TARGET}.dmg"

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1"
    exit 1
  }
}

need_cmd bash
need_cmd tar
need_cmd shasum
need_cmd hdiutil
need_cmd codesign
need_cmd rustc

if [ -n "$NOTARY_PROFILE" ]; then
  need_cmd xcrun
fi

echo "→ release target: $TARGET"
echo "→ release version: $VERSION"

if [ "$ALLOW_DIRTY" != "1" ] && [ -n "$(git status --porcelain)" ]; then
  echo "refusing to build a release from a dirty worktree"
  echo "commit/stash changes, or override with UNHOSTED_ALLOW_DIRTY=1"
  exit 1
fi

echo "→ building app bundle"
bash scripts/bundle-macos.sh

if [ ! -d "$APP" ]; then
  echo "expected app bundle at $APP but it was not found"
  exit 1
fi

if [ -n "$SIGN_IDENTITY" ]; then
  echo "→ signing app with Developer ID identity"
  codesign --force --deep --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
else
  echo "→ skipping Developer ID signing (UNHOSTED_SIGN_IDENTITY is not set)"
fi

echo "→ building dmg"
bash scripts/build-dmg.sh

if [ ! -f "$DMG" ]; then
  echo "expected dmg at $DMG but it was not found"
  exit 1
fi

if [ -n "$SIGN_IDENTITY" ]; then
  echo "→ signing dmg"
  codesign --force --sign "$SIGN_IDENTITY" "$DMG"
  codesign --verify --verbose=2 "$DMG"
fi

if [ -n "$SIGN_IDENTITY" ] && [ -n "$NOTARY_PROFILE" ]; then
  echo "→ submitting dmg for notarization (this may take a while)"
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait

  echo "→ stapling notarization tickets"
  xcrun stapler staple "$APP"
  xcrun stapler staple "$DMG"

  echo "→ gatekeeper assessment"
  spctl -a -t open --context context:primary-signature -vv "$APP" || true
else
  echo "→ skipping notarization (set both UNHOSTED_SIGN_IDENTITY and UNHOSTED_NOTARY_PROFILE)"
fi

echo "→ staging release artifacts"
rm -rf "$RELEASE_DIR"
mkdir -p "$RELEASE_DIR"

cp -R "$APP" "$RELEASE_DIR/unhosted.app"
cp "$DMG" "$RELEASE_DIR/$DMG_NAME"

tar -czf "$RELEASE_DIR/$APP_TAR_NAME" -C "$RELEASE_DIR" unhosted.app

(
  cd "$RELEASE_DIR"
  shasum -a 256 "$APP_TAR_NAME" > "$APP_TAR_NAME.sha256"
  shasum -a 256 "$DMG_NAME" > "$DMG_NAME.sha256"
  shasum -a 256 -c "$APP_TAR_NAME.sha256"
  shasum -a 256 -c "$DMG_NAME.sha256"
)

echo
echo "done."
echo "  release dir: $RELEASE_DIR"
echo "  artifacts:"
echo "    - $RELEASE_DIR/$APP_TAR_NAME"
echo "    - $RELEASE_DIR/$DMG_NAME"
echo "    - $RELEASE_DIR/$APP_TAR_NAME.sha256"
echo "    - $RELEASE_DIR/$DMG_NAME.sha256"
if [ -n "$SIGN_IDENTITY" ] && [ -n "$NOTARY_PROFILE" ]; then
  echo "  notarization: completed"
elif [ -n "$SIGN_IDENTITY" ]; then
  echo "  notarization: skipped (missing UNHOSTED_NOTARY_PROFILE)"
else
  echo "  signing/notarization: skipped"
fi
