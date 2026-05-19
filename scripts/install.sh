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

# ---- color output (only when stdout is a real terminal) ----------------------

if [ -t 1 ]; then
  BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'
  GREEN='\033[32m'; CYAN='\033[36m'; YELLOW='\033[33m'; RED='\033[31m'
else
  BOLD=''; DIM=''; RESET=''; GREEN=''; CYAN=''; YELLOW=''; RED=''
fi

say()  { printf "${CYAN}${BOLD}%s${RESET}\n"   "$*"; }
ok()   { printf "  ${GREEN}✓${RESET}  %s\n"   "$*"; }
info() { printf "  ${DIM}%s${RESET}\n"         "$*"; }
warn() { printf "  ${YELLOW}!${RESET}  %s\n"  "$*"; }
die()  { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }
step() { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---- config ------------------------------------------------------------------

REPO="unhosted-ai/unhosted-core"
INSTALL_DIR="${UNHOSTED_INSTALL_DIR:-/usr/local/bin}"
VERSION="${UNHOSTED_VERSION:-latest}"

# ---- detect platform ---------------------------------------------------------

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64)   TARGET="aarch64-apple-darwin";     PLATFORM="macos" ;;
  darwin-x86_64)                 TARGET="x86_64-apple-darwin";      PLATFORM="macos" ;;
  linux-x86_64)                  TARGET="x86_64-unknown-linux-gnu"; PLATFORM="linux" ;;
  linux-aarch64|linux-arm64)     TARGET="aarch64-unknown-linux-gnu"; PLATFORM="linux" ;;
  *windows*|*mingw*|*msys*|*cygwin*)
    die "this script is for unix shells. on Windows, use PowerShell:
  irm https://raw.githubusercontent.com/$REPO/main/scripts/install.ps1 | iex"
    ;;
  *)
    die "unsupported platform '$OS-$ARCH'.
  see https://github.com/$REPO/releases — or build from source."
    ;;
esac

printf "\n${BOLD}  unhosted${RESET}  —  local AI mesh\n\n"
info "platform  $OS / $ARCH  →  $TARGET"
info "install   $INSTALL_DIR/unhosted"
printf "\n"

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

[ -n "$ASSET_URL" ] || die "no release found for $TARGET ($VERSION).
  see https://github.com/$REPO/releases"

# ---- download + extract ------------------------------------------------------

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

step "Downloading"
info "$ASSET_URL"
curl -fsSL --progress-bar "$ASSET_URL" -o "$TMP/unhosted.tar.gz"
tar -xzf "$TMP/unhosted.tar.gz" -C "$TMP"

CLI_BIN="$(find "$TMP" -type f -name unhosted -not -name 'unhosted-*' | head -1)"
DESKTOP_BIN="$(find "$TMP" -type f -name 'unhosted-desktop' | head -1)"

[ -n "$CLI_BIN" ] || die "archive did not contain a binary named 'unhosted'."

# ---- install binaries --------------------------------------------------------

step "Installing"
mkdir -p "$INSTALL_DIR" 2>/dev/null || true

install_bin() {
  src="$1"; dst="$2"
  if [ -w "$INSTALL_DIR" ]; then
    mv "$src" "$dst"
  else
    warn "needs sudo to write to $INSTALL_DIR"
    sudo mv "$src" "$dst"
  fi
  chmod +x "$dst"
}

install_bin "$CLI_BIN" "$INSTALL_DIR/unhosted"
ok "$INSTALL_DIR/unhosted"

DESKTOP_PATH=""
if [ "${UNHOSTED_NO_DESKTOP:-0}" != "1" ] && [ -n "$DESKTOP_BIN" ]; then
  install_bin "$DESKTOP_BIN" "$INSTALL_DIR/unhosted-desktop"
  ok "$INSTALL_DIR/unhosted-desktop"
  DESKTOP_PATH="$INSTALL_DIR/unhosted-desktop"
fi

# ---- Linux desktop integration ----------------------------------------------

if [ "$PLATFORM" = "linux" ] && [ -x "$INSTALL_DIR/unhosted-desktop" ]; then
  APPS_DIR="$HOME/.local/share/applications"
  mkdir -p "$APPS_DIR"

  # SVG icon (scalable)
  ICON_SVG_SRC="$(find "$TMP" -type f -name 'unhosted.svg' | head -1)"
  if [ -n "$ICON_SVG_SRC" ]; then
    SCALABLE_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
    mkdir -p "$SCALABLE_DIR"
    cp "$ICON_SVG_SRC" "$SCALABLE_DIR/unhosted.svg"
  fi

  # PNG rasters at each size (bundled as icon-32.png, icon-128.png, etc.)
  for pair in "32:icon-32.png" "128:icon-128.png" "256:icon-256.png" "512:icon-512.png"; do
    SIZE="${pair%%:*}"; FILE="${pair##*:}"
    PNG_SRC="$(find "$TMP" -type f -name "$FILE" | head -1)"
    if [ -n "$PNG_SRC" ]; then
      ICON_DIR="$HOME/.local/share/icons/hicolor/${SIZE}x${SIZE}/apps"
      mkdir -p "$ICON_DIR"
      cp "$PNG_SRC" "$ICON_DIR/unhosted.png"
    fi
  done

  DESKTOP_SRC="$(find "$TMP" -type f -name 'unhosted.desktop' | head -1)"
  if [ -n "$DESKTOP_SRC" ]; then
    sed "s|^Exec=.*|Exec=$INSTALL_DIR/unhosted-desktop|; s|^TryExec=.*|TryExec=$INSTALL_DIR/unhosted-desktop|" \
      "$DESKTOP_SRC" > "$APPS_DIR/unhosted.desktop"
    ok "$APPS_DIR/unhosted.desktop"
  fi

  command -v gtk-update-icon-cache >/dev/null 2>&1 \
    && gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
  command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true

  SERVICE_SRC="$(find "$TMP" -type f -name 'unhosted.service' | head -1)"
  if [ -n "$SERVICE_SRC" ]; then
    SYSTEMD_DIR="$HOME/.config/systemd/user"
    mkdir -p "$SYSTEMD_DIR"
    sed "s|^ExecStart=.*|ExecStart=$INSTALL_DIR/unhosted serve|" \
      "$SERVICE_SRC" > "$SYSTEMD_DIR/unhosted.service"
    ok "$SYSTEMD_DIR/unhosted.service"
  fi
fi

# ---- macOS .app install (optional, separate asset) --------------------------

if [ "$PLATFORM" = "macos" ]; then
  APP_URL="$(
    curl -fsSL "$API" \
      | grep -o "\"browser_download_url\": *\"[^\"]*unhosted-macos-app-$TARGET\\.tar\\.gz\"" \
      | head -1 \
      | sed 's/.*"\(https:[^"]*\)".*/\1/'
  )"
  if [ -n "$APP_URL" ]; then
    info "fetching macOS .app bundle ..."
    curl -fsSL "$APP_URL" -o "$TMP/unhosted-app.tar.gz"
    tar -xzf "$TMP/unhosted-app.tar.gz" -C "$TMP"
    if [ -d "$TMP/unhosted.app" ]; then
      DEST="/Applications/unhosted.app"
      if [ -w "/Applications" ]; then
        rm -rf "$DEST" && mv "$TMP/unhosted.app" "$DEST"
      else
        warn "needs sudo to write to /Applications"
        sudo rm -rf "$DEST" && sudo mv "$TMP/unhosted.app" "$DEST"
      fi
      ok "$DEST"
    fi
  fi
fi

# ---- verify ------------------------------------------------------------------

step "Installed"
if command -v unhosted >/dev/null 2>&1; then
  printf "  "; unhosted --version
else
  warn "installed to $INSTALL_DIR/unhosted, but it isn't on your PATH."
  info "add this line to your shell rc:"
  info "  export PATH=\"\$PATH:$INSTALL_DIR\""
fi

# ---- next steps --------------------------------------------------------------

step "Next steps"

if [ "$PLATFORM" = "linux" ]; then
  info "1. install llama.cpp   https://github.com/ggerganov/llama.cpp/releases"
elif [ "$PLATFORM" = "macos" ]; then
  info "1. install llama.cpp   brew install llama.cpp"
fi
info "2. pull a model        unhosted pull llama3.2:1b"
info "3. run the daemon      unhosted serve"

if [ -x "$INSTALL_DIR/unhosted-desktop" ]; then
  info "4. open the app        unhosted-desktop   (or launch from your app menu)"
else
  info "4. open the app        http://127.0.0.1:7777"
fi

if [ "$PLATFORM" = "linux" ] && [ -n "$SERVICE_SRC" ]; then
  printf "\n"
  info "run as a background service (survives closing the terminal):"
  info "  systemctl --user daemon-reload"
  info "  systemctl --user enable --now unhosted"
  info "  journalctl --user -u unhosted -f"
fi

if [ "$PLATFORM" = "linux" ]; then
  printf "\n"
  info "desktop runtime deps:"
  info "  Debian/Ubuntu:  sudo apt install libgtk-3-0 libsoup-3.0-0 libwebkit2gtk-4.1-0"
  info "  Fedora:         sudo dnf install gtk3 libsoup3 webkit2gtk4.1"
  info "  Arch:           sudo pacman -S gtk3 libsoup3 webkit2gtk-4.1"
fi

printf "\n"
