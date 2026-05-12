#!/bin/sh
# unhosted: start the llama.cpp upstream the daemon talks to.
#
# The unhosted node listens on 127.0.0.1:7777 and proxies chat turns to a
# local llama-server. With no upstream running, every turn 502s and the
# sidebar shows "upstream offline — start `llama-server`". This script
# is that "start `llama-server`" step.
#
# Usage:
#   scripts/start-llama.sh                              # auto-pick model + defaults
#   scripts/start-llama.sh -m /path/to/model.gguf
#   UNHOSTED_LLAMA_PORT=8080 scripts/start-llama.sh
#
# Env vars:
#   UNHOSTED_LLAMA_PORT     port to bind (default: 8080)
#   UNHOSTED_LLAMA_HOST     host to bind (default: 127.0.0.1 — local only)
#   UNHOSTED_LLAMA_MODEL    model .gguf path (overrides auto-detect)
#   UNHOSTED_LLAMA_BIN      llama-server binary (overrides PATH lookup)

set -e

PORT="${UNHOSTED_LLAMA_PORT:-8080}"
HOST="${UNHOSTED_LLAMA_HOST:-127.0.0.1}"
MODEL="${UNHOSTED_LLAMA_MODEL:-}"

# ---- parse args --------------------------------------------------------------

while [ $# -gt 0 ]; do
  case "$1" in
    -m|--model) MODEL="$2"; shift 2 ;;
    --port)     PORT="$2";  shift 2 ;;
    --host)     HOST="$2";  shift 2 ;;
    -h|--help)
      sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "start-llama: unknown arg '$1' (try --help)"
      exit 1
      ;;
  esac
done

# ---- locate llama-server -----------------------------------------------------

BIN="${UNHOSTED_LLAMA_BIN:-}"
if [ -z "$BIN" ]; then
  if command -v llama-server >/dev/null 2>&1; then
    BIN="$(command -v llama-server)"
  elif [ -x /opt/homebrew/bin/llama-server ]; then
    BIN="/opt/homebrew/bin/llama-server"
  elif [ -x /usr/local/bin/llama-server ]; then
    BIN="/usr/local/bin/llama-server"
  else
    echo "start-llama: llama-server not found on PATH."
    echo "  install:"
    echo "    macOS:        brew install llama.cpp"
    echo "    Debian/Ubuntu: see https://github.com/ggml-org/llama.cpp"
    exit 1
  fi
fi

# ---- locate a model ----------------------------------------------------------

if [ -z "$MODEL" ]; then
  # The unhosted installer / docs drop a starter model under ~/.cache/unhosted.
  # Anything ending in .gguf is fair game; we pick the first one we find so a
  # user who downloaded a larger model still gets a usable default.
  CANDIDATE_DIR="$HOME/.cache/unhosted/models"
  if [ -d "$CANDIDATE_DIR" ]; then
    MODEL="$(find "$CANDIDATE_DIR" -maxdepth 2 -type f -name '*.gguf' | head -1)"
  fi
fi

if [ -z "$MODEL" ] || [ ! -f "$MODEL" ]; then
  echo "start-llama: no model.gguf found."
  echo "  pass one explicitly:    scripts/start-llama.sh -m /path/to/model.gguf"
  echo "  or drop a .gguf under:  $HOME/.cache/unhosted/models/"
  echo "  starter model:          see docs/learn (Llama-3.2-1B-Instruct-Q4_K_M)"
  exit 1
fi

# ---- check the port is free --------------------------------------------------
# Better to fail fast with a helpful message than to let llama-server's own
# "bind: address already in use" race against the daemon's health check.

if command -v lsof >/dev/null 2>&1; then
  if lsof -iTCP:"$PORT" -sTCP:LISTEN -P -n >/dev/null 2>&1; then
    echo "start-llama: port $PORT already in use."
    echo "  if that's already llama-server, you're done — the unhosted UI should be live."
    echo "  otherwise pick another port: UNHOSTED_LLAMA_PORT=8081 scripts/start-llama.sh"
    exit 1
  fi
fi

echo "start-llama:"
echo "  binary: $BIN"
echo "  model:  $MODEL"
echo "  bind:   $HOST:$PORT"
echo

exec "$BIN" -m "$MODEL" --host "$HOST" --port "$PORT"
