#!/usr/bin/env bash
# Build the Linux release tarball for unhosted.
#
# What it produces:
#   dist/unhosted-<target>.tar.gz   containing:
#     unhosted            (CLI binary)
#     unhosted-desktop    (GUI binary)
#     unhosted.desktop    (Linux .desktop entry; install.sh wires it up)
#     unhosted.svg        (icon used by the .desktop file)
#     README              (one-paragraph runtime-deps + run-it pointer)
#
# install.sh on a user's machine downloads this asset, extracts it, and
# wires up the .desktop + icon under ~/.local/share — same layout we
# already document there.
#
# Defaults to the host triple. Override with UNHOSTED_TARGET, e.g.:
#   UNHOSTED_TARGET=aarch64-unknown-linux-gnu bash scripts/bundle-linux.sh
#
# Cross-compile note: needs the matching rustup target installed
# (`rustup target add aarch64-unknown-linux-gnu`) plus a matching linker
# (gcc-aarch64-linux-gnu on Debian/Ubuntu). We don't auto-install those.
#
# Usage: bash scripts/bundle-linux.sh
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Detect target if not provided. `rustc -vV | host:` is the canonical way
# to read the host triple — works on every box without parsing uname.
TARGET="${UNHOSTED_TARGET:-$(rustc -vV | awk '/^host:/ { print $2 }')}"
if [[ ! "$TARGET" == *linux* ]]; then
  echo "bundle-linux.sh: target '$TARGET' is not a Linux triple."
  echo "set UNHOSTED_TARGET=x86_64-unknown-linux-gnu (or similar) to cross-bundle."
  exit 1
fi

ROOT="$(pwd)"
DIST="$ROOT/dist"
STAGE="$DIST/stage-linux-$TARGET"
TARBALL="$DIST/unhosted-$TARGET.tar.gz"

mkdir -p "$DIST"
rm -rf "$STAGE"
mkdir -p "$STAGE"

# ----- build binaries ---------------------------------------------------------

echo "→ cargo build --release --target $TARGET (cli + desktop)"
cargo build --release --target "$TARGET" -p unhosted-cli -p unhosted-desktop

BIN_DIR="$ROOT/target/$TARGET/release"
[[ -x "$BIN_DIR/unhosted" ]]         || { echo "missing $BIN_DIR/unhosted";         exit 1; }
[[ -x "$BIN_DIR/unhosted-desktop" ]] || { echo "missing $BIN_DIR/unhosted-desktop"; exit 1; }

cp "$BIN_DIR/unhosted"         "$STAGE/unhosted"
cp "$BIN_DIR/unhosted-desktop" "$STAGE/unhosted-desktop"

# Strip — `strip = true` in [profile.release] already does this, but be
# explicit so the bundle is reliably lean across rustc versions.
command -v strip >/dev/null 2>&1 && strip "$STAGE/unhosted" "$STAGE/unhosted-desktop" || true

# ----- icon + .desktop --------------------------------------------------------

ICON_SRC="$ROOT/crates/unhosted-desktop/icons"
cp "$ROOT/branding/logo/app-icon.svg" "$STAGE/unhosted.svg"
cp "$ICON_SRC/32x32.png"       "$STAGE/icon-32.png"
cp "$ICON_SRC/128x128.png"     "$STAGE/icon-128.png"
cp "$ICON_SRC/128x128@2x.png"  "$STAGE/icon-256.png"
cp "$ICON_SRC/icon.png"        "$STAGE/icon-512.png"
cp "$ROOT/scripts/unhosted.desktop"   "$STAGE/unhosted.desktop"

# ----- README ----------------------------------------------------------------

cat > "$STAGE/README" <<'EOF'
unhosted — local AI mesh
========================

Binaries:
  unhosted          CLI daemon + helpers      (`unhosted --help`)
  unhosted-desktop  Native window for the UI  (loads http://127.0.0.1:7777)

Linux runtime deps for the desktop shell (Tauri WebView):
  Debian/Ubuntu:  sudo apt install libgtk-3-0 libsoup-3.0-0 libwebkit2gtk-4.1-0
  Fedora:        sudo dnf install gtk3 libsoup3 webkit2gtk4.1
  Arch:          sudo pacman -S gtk3 libsoup3 webkit2gtk-4.1

Quick start:
  ./unhosted serve       # in one terminal — starts the daemon on :7777
  ./unhosted-desktop     # in another — opens the native window

Full docs: https://github.com/unhosted-ai/unhosted-core
EOF

# ----- tar.gz -----------------------------------------------------------------

echo "→ packing $TARBALL"
# Files are extracted directly into the user's tmp dir by install.sh,
# which `find`s the binaries by name. No nested top-level directory.
tar -czf "$TARBALL" -C "$STAGE" \
  unhosted unhosted-desktop \
  unhosted.desktop unhosted.svg \
  icon-32.png icon-128.png icon-256.png icon-512.png \
  README

echo
echo "Done."
echo "  $TARBALL"
echo "  $(du -h "$TARBALL" | cut -f1)"
echo
echo "  smoke-test on this host:"
echo "    tar -xzf $TARBALL -C /tmp && /tmp/unhosted --version"
