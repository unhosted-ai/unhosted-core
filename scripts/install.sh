#!/bin/sh
# unhosted install script
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.sh | sh
#
# Env vars:
#   UNHOSTED_INSTALL_DIR  override install directory (default: /usr/local/bin)
#   UNHOSTED_VERSION      pin a specific version, e.g. v0.0.2 (default: latest)

set -e

REPO="unhosted-ai/unhosted-core"
INSTALL_DIR="${UNHOSTED_INSTALL_DIR:-/usr/local/bin}"
VERSION="${UNHOSTED_VERSION:-latest}"

# ---- detect platform ---------------------------------------------------------

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64)   TARGET="aarch64-apple-darwin" ;;
  darwin-x86_64)                 TARGET="x86_64-apple-darwin" ;;
  linux-x86_64)                  TARGET="x86_64-unknown-linux-gnu" ;;
  linux-aarch64|linux-arm64)     TARGET="aarch64-unknown-linux-gnu" ;;
  *)
    echo "unhosted: unsupported platform '$OS-$ARCH'."
    echo "see https://github.com/$REPO/releases — or build from source."
    exit 1
    ;;
esac

echo "unhosted installer"
echo "  platform: $OS / $ARCH  →  $TARGET"
echo "  install:  $INSTALL_DIR/unhosted"

# ---- find release ------------------------------------------------------------

if [ "$VERSION" = "latest" ]; then
  API="https://api.github.com/repos/$REPO/releases/latest"
else
  API="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

ASSET_URL="$(
  curl -fsSL "$API" \
    | grep -o "\"browser_download_url\": *\"[^\"]*unhosted-$TARGET\\.tar\\.gz\"" \
    | head -1 \
    | sed 's/.*"\(https:[^"]*\)".*/\1/'
)"

if [ -z "$ASSET_URL" ]; then
  echo "unhosted: no release found for $TARGET ($VERSION)."
  echo "see https://github.com/$REPO/releases"
  exit 1
fi

# ---- download + extract ------------------------------------------------------

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "  downloading $ASSET_URL ..."
curl -fsSL "$ASSET_URL" -o "$TMP/unhosted.tar.gz"
tar -xzf "$TMP/unhosted.tar.gz" -C "$TMP"

BIN="$(find "$TMP" -type f -name unhosted | head -1)"
if [ -z "$BIN" ]; then
  echo "unhosted: archive did not contain a binary named 'unhosted'."
  exit 1
fi

# ---- install -----------------------------------------------------------------

mkdir -p "$INSTALL_DIR" 2>/dev/null || true
if [ -w "$INSTALL_DIR" ]; then
  mv "$BIN" "$INSTALL_DIR/unhosted"
else
  echo "  needs sudo to write to $INSTALL_DIR"
  sudo mv "$BIN" "$INSTALL_DIR/unhosted"
fi
chmod +x "$INSTALL_DIR/unhosted"

# ---- verify ------------------------------------------------------------------

echo
if command -v unhosted >/dev/null 2>&1; then
  echo "installed:"
  unhosted --version
else
  echo "installed to $INSTALL_DIR/unhosted, but it isn't on your PATH."
  echo "add this line to your shell rc:"
  echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
fi

echo
echo "next:"
echo "  1. install llama.cpp:  brew install llama.cpp   (mac)"
echo "  2. download a model:   see docs/learn for a 1B starter model"
echo "  3. start the backend:  llama-server -m model.gguf --port 8080"
echo "  4. run the daemon:     unhosted serve"
echo "  5. open the app:       http://127.0.0.1:7777"
