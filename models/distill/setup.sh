#!/bin/sh
# unhosted distill — one-command setup for the "train your own local model" flow.
#
# Handles the fresh-user friction the QUICKSTART leaves manual: a correct
# Python venv, the platform-right PyTorch, the training deps, and a llama.cpp
# checkout (needed for the GGUF export step). After this, follow QUICKSTART.md
# from step 2.
#
# Usage (from models/distill/):
#   ./setup.sh
#
# Env vars:
#   PYTHON        python interpreter to use (default: auto-detect 3.12/3.11/3.10)
#   LLAMA_CPP_DIR where to put / find the llama.cpp checkout (default: ~/llama.cpp)
#   SKIP_LLAMA    set to 1 to skip the llama.cpp step (export won't work without it)

set -e

# ---- pretty output (matches scripts/install.sh) ------------------------------
if [ -t 1 ]; then
  BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'
  GREEN='\033[32m'; CYAN='\033[36m'; YELLOW='\033[33m'; RED='\033[31m'
else
  BOLD=''; DIM=''; RESET=''; GREEN=''; CYAN=''; YELLOW=''; RED=''
fi
ok()   { printf "  ${GREEN}✓${RESET}  %s\n"  "$*"; }
info() { printf "  ${DIM}%s${RESET}\n"        "$*"; }
warn() { printf "  ${YELLOW}!${RESET}  %s\n" "$*"; }
die()  { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }
step() { printf "\n${BOLD}%s${RESET}\n" "$*"; }

cd "$(dirname "$0")"

# ---- 1. pick a Python 3.10–3.12 ----------------------------------------------
step "1/4  Python"
if [ -n "$PYTHON" ]; then
  PY="$PYTHON"
else
  PY=""
  for c in python3.12 python3.11 python3.10; do
    if command -v "$c" >/dev/null 2>&1; then PY="$c"; break; fi
  done
  [ -z "$PY" ] && PY="python3"
fi
command -v "$PY" >/dev/null 2>&1 || die "no usable python found. Install Python 3.10–3.12."
PYVER=$("$PY" -c 'import sys;print("%d.%d"%sys.version_info[:2])')
case "$PYVER" in
  3.10|3.11|3.12) ok "using $PY (Python $PYVER)";;
  *) warn "Python $PYVER may lack wheels for the training stack (3.10–3.12 recommended). Continuing.";;
esac

# ---- 2. venv -----------------------------------------------------------------
step "2/4  virtualenv (.venv)"
if [ -d .venv ]; then
  ok ".venv already exists — reusing"
else
  "$PY" -m venv .venv
  ok "created .venv"
fi
# shellcheck disable=SC1091
. .venv/bin/activate
python -m pip install -q --upgrade pip >/dev/null 2>&1 || true

# ---- 3. PyTorch + training deps ----------------------------------------------
step "3/4  PyTorch + training deps"
if python -c "import torch" >/dev/null 2>&1; then
  ok "torch already installed"
else
  info "installing torch (CPU/MPS wheel — for CUDA, install torch yourself first, then re-run)"
  pip install -q torch || die "torch install failed. See https://pytorch.org for the right wheel for your platform."
  ok "torch installed"
fi
info "installing training deps (transformers, trl, peft, datasets, gguf, …)"
pip install -q -r requirements.txt
ok "training deps installed"

# ---- 4. llama.cpp (for GGUF export) ------------------------------------------
step "4/4  llama.cpp (GGUF export)"
LLAMA_DIR="${LLAMA_CPP_DIR:-$HOME/llama.cpp}"
if [ "$SKIP_LLAMA" = "1" ]; then
  warn "skipping llama.cpp (SKIP_LLAMA=1). The 'distill export' step won't work until you set it up."
elif [ -f "$LLAMA_DIR/convert_hf_to_gguf.py" ] && [ -f "$LLAMA_DIR/build/bin/llama-quantize" ]; then
  ok "llama.cpp already set up at $LLAMA_DIR"
else
  if ! command -v git >/dev/null 2>&1; then
    warn "git not found — can't fetch llama.cpp. Install git, or set up llama.cpp manually."
  elif ! command -v cmake >/dev/null 2>&1; then
    warn "cmake not found — needed to build llama-quantize. Install cmake (brew install cmake), then re-run."
  else
    if [ ! -d "$LLAMA_DIR" ]; then
      info "cloning llama.cpp to $LLAMA_DIR"
      git clone --depth 1 https://github.com/ggerganov/llama.cpp "$LLAMA_DIR"
    fi
    info "building llama-quantize (this takes a few minutes)…"
    cmake -S "$LLAMA_DIR" -B "$LLAMA_DIR/build" >/dev/null
    cmake --build "$LLAMA_DIR/build" --target llama-quantize -j >/dev/null
    ok "llama.cpp ready at $LLAMA_DIR"
  fi
fi

# ---- done --------------------------------------------------------------------
step "Done."
info "Activate the env in new shells with:  . models/distill/.venv/bin/activate"
info "Next: follow QUICKSTART.md from step 2 (get training data)."
printf "\n"
