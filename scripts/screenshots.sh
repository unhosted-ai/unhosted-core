#!/bin/bash
# Take screenshots of the running unhosted UI and place them under
# `assets/screenshots/`. Uses macOS-bundled tools only — no downloads.
#
# Why this exists as a script you run, not CI:
#   - macOS requires Screen Recording permission (System Settings →
#     Privacy & Security → Screen Recording). The Terminal app running
#     this script needs that permission granted once. CI runners don't
#     have it; nor do agents.
#   - Safari has to be allowed to open / focus a window during capture.
#     This is annoying in headless contexts and fine on your machine.
#
# Usage:
#   ./scripts/screenshots.sh
#   ./scripts/screenshots.sh --addr 127.0.0.1:7798  # if 7777 is busy
#
# After this runs, six PNGs will exist under `assets/screenshots/`.
# Commit them; the main README's image embeds will start showing them.
#
# If you'd rather take screenshots manually, the filenames the README
# expects are listed in `assets/screenshots/README.md`.

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
    echo "error: this script uses macOS-bundled tools (screencapture, osascript)."
    echo "       on Linux/Windows, take screenshots manually and follow the"
    echo "       naming convention in assets/screenshots/README.md."
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHOT_DIR="$REPO_ROOT/assets/screenshots"
mkdir -p "$SHOT_DIR"

# ─── start a fresh daemon if 7798 is free ────────────────────────────
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
    echo "[screenshots] starting fresh daemon on $ADDR …"
    rm -rf /tmp/unhosted-shot-cfg
    XDG_CONFIG_HOME=/tmp/unhosted-shot-cfg "$BIN" serve --addr "$ADDR" \
        > /tmp/screenshots-daemon.log 2>&1 &
    DAEMON_PID=$!
    sleep 3
    if ! curl -sf "http://$ADDR/health" >/dev/null 2>&1; then
        echo "daemon didn't come up. tail of log:"
        tail /tmp/screenshots-daemon.log
        kill $DAEMON_PID 2>/dev/null || true
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
# A meaningful public-mode policy (sanctions defaults auto-merge)
curl -sf -X PUT "http://$ADDR/v1/public-mode/policy" \
    -H "content-type: application/json" \
    -d '{"accepted_rails":["lightning","usdc_base"],"min_kyc":"email","blocked_countries":[]}' >/dev/null

# Seed a sample chat so the chat view isn't empty
SAMPLE_CHAT=$(cat <<'JSON'
{
  "id": "chat_demo",
  "title": "What is unhosted?",
  "messages": [
    {"role": "user", "content": "What is unhosted in one sentence?"},
    {"role": "assistant", "content": "Unhosted pools the computers you already own — and optionally your friends' machines, and a public swarm of strangers' GPUs — into one inference cluster you control."},
    {"role": "user", "content": "How does the trust radius work?"},
    {"role": "assistant", "content": "Three concentric rings: local (your devices), trusted (paired peers — friends, family, team), and public (strangers' GPUs, opt-in, paid in stablecoin). You decide which rings your daemon uses."}
  ],
  "model": "qwen2.5:3b",
  "createdAt": 1716096000000,
  "updatedAt": 1716096000000
}
JSON
)
curl -sf -X PUT "http://$ADDR/v1/chats/chat_demo" \
    -H "content-type: application/json" \
    -d "$SAMPLE_CHAT" >/dev/null

# ─── capture helpers ──────────────────────────────────────────────────
# Drive Safari, wait, get its window rect, screencapture by rect.
# Safari briefly takes focus during the capture. Tolerate it.
shot() {
    local url="$1"
    local out="$2"
    local pre_js="${3:-}"

    osascript >/dev/null <<APPLESCRIPT
tell application "Safari"
    activate
    if (count of documents) is 0 then
        make new document
    end if
    set URL of document 1 to "$url"
    tell window 1
        set bounds to {60, 60, 1340, 880}
    end tell
end tell
APPLESCRIPT

    sleep 2

    if [ -n "$pre_js" ]; then
        osascript >/dev/null <<APPLESCRIPT
tell application "Safari"
    tell document 1 to do JavaScript "$pre_js"
end tell
APPLESCRIPT
        sleep 1
    fi

    # Bounds of front Safari window in screen coords.
    BOUNDS=$(osascript <<'APPLESCRIPT'
tell application "System Events"
    tell process "Safari"
        set p to position of front window
        set s to size of front window
        return (item 1 of p as text) & "," & (item 2 of p as text) & "," & (item 1 of s as text) & "," & (item 2 of s as text)
    end tell
end tell
APPLESCRIPT
)
    echo "[shot] $out  rect=$BOUNDS"
    screencapture -R "$BOUNDS" -x "$out"
}

# ─── shots ────────────────────────────────────────────────────────────
shot "http://$ADDR/" "$SHOT_DIR/01-overview.png"
shot "http://$ADDR/#chat_demo" "$SHOT_DIR/02-chat.png"
shot "http://$ADDR/" "$SHOT_DIR/03-public-mode.png" \
    "document.getElementById('public-mode-section').open = true; document.getElementById('memory-section').open = false; document.getElementById('vram-pool-section').open = false; document.getElementById('developer-section').open = false; window.scrollTo(0,0);"
shot "http://$ADDR/" "$SHOT_DIR/04-vram-pool.png" \
    "document.getElementById('vram-pool-section').open = true; document.getElementById('public-mode-section').open = false; document.getElementById('memory-section').open = false; document.getElementById('developer-section').open = false; window.scrollTo(0,0);"

echo
echo "[screenshots] done. PNGs under: $SHOT_DIR"
ls -la "$SHOT_DIR"
echo
echo "next:  git add assets/screenshots/ && git commit -m 'add app screenshots'"
