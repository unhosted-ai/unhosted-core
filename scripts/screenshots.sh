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

# Seed a sample chat so the chat view isn't empty. Schema lives in
# crates/unhosted-core/src/chats.rs: { id, title, createdAt, updatedAt,
# messages: [{role, text, ts, stats?}] }.
SAMPLE_CHAT=$(cat <<'JSON'
{
  "id": "chat_demo",
  "title": "What is unhosted?",
  "createdAt": 1716096000,
  "updatedAt": 1716096300,
  "messages": [
    {"role": "user", "text": "What is unhosted in one sentence?", "ts": 1716096000},
    {"role": "assistant", "text": "Unhosted pools the computers you already own — and optionally your friends' machines, and a public swarm of strangers' GPUs — into one inference cluster you control.", "ts": 1716096060},
    {"role": "user", "text": "How does the trust radius work?", "ts": 1716096180},
    {"role": "assistant", "text": "Three concentric rings: local (your own devices), trusted (paired peers — friends, family, team), and public (strangers' GPUs, opt-in, paid in stablecoin). You decide which rings your daemon uses.", "ts": 1716096300}
  ]
}
JSON
)
# Tolerant: a schema drift here shouldn't kill the script — we'll still
# get useful sidebar shots even if the seeded chat doesn't render.
if ! curl -sf -X PUT "http://$ADDR/v1/chats/chat_demo" \
    -H "content-type: application/json" \
    -d "$SAMPLE_CHAT" >/dev/null; then
    echo "[screenshots] warn: chat seed failed (schema may have drifted) — continuing"
fi

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

    # Bounds of Safari's window in screen coords. Use Safari's own
    # AppleScript dictionary, not System Events — the latter can
    # surface helper windows (download progress, popovers) as the
    # "front window" and return their bounds instead. Safari's
    # `bounds of window 1` is the real browsing window we just sized.
    BOUNDS=$(osascript <<'APPLESCRIPT'
tell application "Safari"
    set b to bounds of window 1
    set x1 to item 1 of b
    set y1 to item 2 of b
    set x2 to item 3 of b
    set y2 to item 4 of b
    return (x1 as text) & "," & (y1 as text) & "," & ((x2 - x1) as text) & "," & ((y2 - y1) as text)
end tell
APPLESCRIPT
)
    echo "[shot] $out  rect=$BOUNDS"
    if ! err=$(screencapture -R "$BOUNDS" -x "$out" 2>&1); then
        : # screencapture returned non-zero, $err has the message
    fi
    if [ ! -s "$out" ]; then
        cat >&2 <<EOF

[screenshots] screencapture did not produce a file (or produced 0 bytes).
              This is almost always Screen Recording permission on macOS.

              Grant your Terminal app permission:
                System Settings → Privacy & Security → Screen Recording
                → toggle the terminal you're running this from on.
              Then fully quit and reopen the terminal and rerun this script.

              Underlying message: ${err:-no output}
EOF
        exit 1
    fi
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
