#!/bin/sh
# unhosted install script
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.sh | sh
#
# Env vars:
#   UNHOSTED_INSTALL_DIR  override install directory (default: /usr/local/bin)
#   UNHOSTED_VERSION      pin a specific version, e.g. v0.0.2 (default: latest)
#   UNHOSTED_NO_DESKTOP   set to 1 to skip the desktop shell (CLI only)

set -e

REPO="unhosted-ai/unhosted-core"
INSTALL_DIR="${UNHOSTED_INSTALL_DIR:-/usr/local/bin}"
VERSION="${UNHOSTED_VERSION:-latest}"

# ---- detect platform ---------------------------------------------------------

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64)   TARGET="aarch64-apple-darwin";   PLATFORM="macos" ;;
  darwin-x86_64)                 TARGET="x86_64-apple-darwin";    PLATFORM="macos" ;;
  linux-x86_64)                  TARGET="x86_64-unknown-linux-gnu"; PLATFORM="linux" ;;
  linux-aarch64|linux-arm64)     TARGET="aarch64-unknown-linux-gnu"; PLATFORM="linux" ;;
  *windows*|*mingw*|*msys*|*cygwin*)
    echo "unhosted: this script is for unix shells. on Windows, use PowerShell:"
    echo "  irm https://raw.githubusercontent.com/$REPO/main/scripts/install.ps1 | iex"
    exit 1
    ;;
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

CLI_BIN="$(find "$TMP" -type f -name unhosted -not -name 'unhosted-*' | head -1)"
DESKTOP_BIN="$(find "$TMP" -type f -name 'unhosted-desktop' | head -1)"

if [ -z "$CLI_BIN" ]; then
  echo "unhosted: archive did not contain a binary named 'unhosted'."
  exit 1
fi

# ---- install binaries --------------------------------------------------------

mkdir -p "$INSTALL_DIR" 2>/dev/null || true

install_bin() {
  src="$1"
  dst="$2"
  if [ -w "$INSTALL_DIR" ]; then
    mv "$src" "$dst"
  else
    echo "  needs sudo to write to $INSTALL_DIR"
    sudo mv "$src" "$dst"
  fi
  chmod +x "$dst"
}

install_bin "$CLI_BIN" "$INSTALL_DIR/unhosted"

if [ "${UNHOSTED_NO_DESKTOP:-0}" != "1" ] && [ -n "$DESKTOP_BIN" ]; then
  install_bin "$DESKTOP_BIN" "$INSTALL_DIR/unhosted-desktop"
  echo "  installed: $INSTALL_DIR/unhosted-desktop"
fi

# ---- Linux desktop integration ----------------------------------------------
# Wire up a .desktop file + icon so the GUI shell shows up in the launcher.
# Skipped on macOS (the .app bundle handles this separately).

if [ "$PLATFORM" = "linux" ] && [ -x "$INSTALL_DIR/unhosted-desktop" ]; then
  APPS_DIR="$HOME/.local/share/applications"
  ICONS_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
  mkdir -p "$APPS_DIR" "$ICONS_DIR"

  ICON_SRC="$(find "$TMP" -type f -name 'unhosted.svg' | head -1)"
  if [ -n "$ICON_SRC" ]; then
    cp "$ICON_SRC" "$ICONS_DIR/unhosted.svg"
  fi

  DESKTOP_SRC="$(find "$TMP" -type f -name 'unhosted.desktop' | head -1)"
  if [ -n "$DESKTOP_SRC" ]; then
    # Rewrite Exec= to the absolute install path so it works regardless of PATH.
    sed "s|^Exec=.*|Exec=$INSTALL_DIR/unhosted-desktop|; s|^TryExec=.*|TryExec=$INSTALL_DIR/unhosted-desktop|" \
      "$DESKTOP_SRC" > "$APPS_DIR/unhosted.desktop"
  fi

  # Refresh the desktop database if we can (silent if the tool isn't installed).
  command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true

  echo "  installed: $APPS_DIR/unhosted.desktop"

  # Print a runtime-deps hint. tao+wry need these at runtime; missing pkgs
  # are the most common "the GUI won't open" failure mode on Linux.
  echo
  echo "  desktop runtime deps (Linux):"
  echo "    Debian/Ubuntu:  sudo apt install libgtk-3-0 libsoup-3.0-0 libwebkit2gtk-4.1-0"
  echo "    Fedora:         sudo dnf install gtk3 libsoup3 webkit2gtk4.1"
  echo "    Arch:           sudo pacman -S gtk3 libsoup3 webkit2gtk-4.1"
fi

# ---- macOS .app install (optional, separate asset) --------------------------
# The release ships a `unhosted-macos-app-*.tar.gz` containing unhosted.app.
# Drop it in /Applications by hand, or fetch it here when present.
if [ "$PLATFORM" = "macos" ]; then
  APP_URL="$(
    curl -fsSL "$API" \
      | grep -o "\"browser_download_url\": *\"[^\"]*unhosted-macos-app-$TARGET\\.tar\\.gz\"" \
      | head -1 \
      | sed 's/.*"\(https:[^"]*\)".*/\1/'
  )"
  if [ -n "$APP_URL" ]; then
    echo "  fetching macOS .app bundle ..."
    curl -fsSL "$APP_URL" -o "$TMP/unhosted-app.tar.gz"
    tar -xzf "$TMP/unhosted-app.tar.gz" -C "$TMP"
    if [ -d "$TMP/unhosted.app" ]; then
      DEST="/Applications/unhosted.app"
      if [ -w "/Applications" ]; then
        rm -rf "$DEST" && mv "$TMP/unhosted.app" "$DEST"
      else
        echo "  needs sudo to write to /Applications"
        sudo rm -rf "$DEST" && sudo mv "$TMP/unhosted.app" "$DEST"
      fi
      echo "  installed: $DEST"
    fi
  fi
fi

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
if [ -x "$INSTALL_DIR/unhosted-desktop" ]; then
  echo "  5. open the app:       unhosted-desktop      (or use the launcher)"
else
  echo "  5. open the app:       http://127.0.0.1:7777"
fi
