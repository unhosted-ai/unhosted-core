#!/bin/bash
# Take screenshots of the running Unhosted UI via WKWebView's headless
# snapshot API. No Screen Recording permission needed — WebKit renders
# through its own compositor, not the OS screen pipeline.
#
# Build dependency: macOS with the Xcode command-line tools (provides
# `xcrun swiftc`). Standard on any Mac that's done a `git` install.
#
# Usage:
#   ./scripts/screenshots.sh
#   ./scripts/screenshots.sh --addr 127.0.0.1:7798
#   ./scripts/screenshots.sh --keep-running
#
# After this runs, four PNGs land under assets/screenshots/. Commit
# them; the main README's image embeds will pick them up.

set -euo pipefail

ADDR="127.0.0.1:7798"
KEEP_RUNNING=0
while [ $# -gt 0 ]; do
    case "$1" in
        --addr) ADDR="$2"; shift 2 ;;
        --keep-running) KEEP_RUNNING=1; shift ;;
        -h|--help)
            sed -n '2,/^set -e/p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $1"; exit 1 ;;
    esac
done

if [ "$(uname -s)" != "Darwin" ]; then
    echo "error: this script renders via macOS WebKit (WKWebView)."
    echo "       On Linux/Windows, take screenshots manually and follow the"
    echo "       naming convention in assets/screenshots/README.md."
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHOT_DIR="$REPO_ROOT/assets/screenshots"
SHOTTER_SRC="$REPO_ROOT/scripts/shotter.swift"
SHOTTER_BIN="$REPO_ROOT/target/shotter"
mkdir -p "$SHOT_DIR" "$REPO_ROOT/target"

# ─── build the Swift shotter if missing or out-of-date ────────────────
if [ ! -x "$SHOTTER_BIN" ] || [ "$SHOTTER_SRC" -nt "$SHOTTER_BIN" ]; then
    echo "[screenshots] building shotter (~3s)"
    if ! xcrun --find swiftc >/dev/null 2>&1; then
        echo "error: swiftc not found. Install the Xcode command-line tools:"
        echo "       xcode-select --install"
        exit 1
    fi
    xcrun swiftc -O -o "$SHOTTER_BIN" "$SHOTTER_SRC"
fi

# ─── start a fresh daemon if the addr is free ─────────────────────────
DAEMON_PID=""
if ! curl -sf "http://$ADDR/health" >/dev/null 2>&1; then
    BIN="$REPO_ROOT/target/debug/unhosted"
    if [ ! -x "$BIN" ]; then
        BIN="$REPO_ROOT/target/release/unhosted"
    fi
    if [ ! -x "$BIN" ]; then
        echo "no unhosted binary found. build with: cargo build -p unhosted-cli"
        exit 1
    fi
    echo "[screenshots] starting fresh daemon on $ADDR"
    rm -rf /tmp/unhosted-shot-cfg
    XDG_CONFIG_HOME=/tmp/unhosted-shot-cfg "$BIN" serve --addr "$ADDR" \
        > /tmp/screenshots-daemon.log 2>&1 &
    DAEMON_PID=$!
    sleep 3
    if ! curl -sf "http://$ADDR/health" >/dev/null 2>&1; then
        echo "daemon didn't come up. tail of log:"
        tail /tmp/screenshots-daemon.log
        kill "$DAEMON_PID" 2>/dev/null || true
        exit 1
    fi
fi

cleanup() {
    if [ -n "$DAEMON_PID" ] && [ "$KEEP_RUNNING" -eq 0 ]; then
        echo "[screenshots] stopping daemon"
        kill "$DAEMON_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ─── pre-populate interesting state ───────────────────────────────────
echo "[screenshots] pre-populating state"
# Set a meaningful public-mode policy (sanctions defaults auto-merge in)
curl -sf -X PUT "http://$ADDR/v1/public-mode/policy" \
    -H "content-type: application/json" \
    -d '{"accepted_rails":["lightning","usdc_base"],"min_kyc":"email","blocked_countries":[]}' >/dev/null

# ─── shots ────────────────────────────────────────────────────────────
# Args: shotter <url> <out> [width] [height] [predelay_ms] [js]
# Width/height set the WKWebView's logical size; the snapshot comes
# back at the screen's backing scale (2× on Retina = ultra-sharp).

# 01: overview — default landing view at chat aspect
"$SHOTTER_BIN" "http://$ADDR/" "$SHOT_DIR/01-overview.png" 1280 820 2500

# 02: chat composer with a real-looking prompt typed in
"$SHOTTER_BIN" "http://$ADDR/" "$SHOT_DIR/02-chat.png" 1280 820 2500 \
    "(function(){const p=document.querySelector('#prompt'); if(p){p.focus(); p.value='How does the vram pool split layers across paired peers?';}})();"

# 03: public-mode panel expanded + scrolled into view; other sidebar
#     sections collapsed so the panel is the visible focus.
"$SHOTTER_BIN" "http://$ADDR/" "$SHOT_DIR/03-public-mode.png" 1280 1100 2500 \
    "(function(){['memory-section','vram-pool-section','developer-section'].forEach(id=>{const e=document.getElementById(id); if(e)e.open=false}); const p=document.getElementById('public-mode-section'); if(p){p.open=true; p.scrollIntoView({block:'start'});}})();"

# 04: VRAM-pool panel expanded + scrolled in
"$SHOTTER_BIN" "http://$ADDR/" "$SHOT_DIR/04-vram-pool.png" 1280 1100 2500 \
    "(function(){['memory-section','public-mode-section','developer-section'].forEach(id=>{const e=document.getElementById(id); if(e)e.open=false}); const v=document.getElementById('vram-pool-section'); if(v){v.open=true; v.scrollIntoView({block:'start'});}})();"

echo
echo "[screenshots] done. PNGs under: $SHOT_DIR"
ls -la "$SHOT_DIR"/*.png 2>/dev/null
echo
echo "next:  git add assets/screenshots/*.png && git commit -m 'refresh screenshots'"
