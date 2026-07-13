# Quickstart: from zero to your own local model

This takes you from nothing to a small, private, specialized model running in
LM Studio — trained on your data, with no subscription and no cloud inference.

It's the whole point of Unhosted's distill flow: turn a big model's behavior
into a small one you own and run yourself.

**What you'll end up with:** a `~2.3 GB` GGUF file, loaded in LM Studio,
answering in the style you trained it on.

**Time:** ~1 hour, most of it unattended training.

---

## What you need

- **Python 3.10–3.12.** (3.13+ may not have wheels for the training stack yet.)
- **A GPU helps but isn't required.** Apple Silicon (M-series) works via MPS.
  CPU-only works but is slow — fine for a first try.
- **~15 GB free disk** (base model + intermediate files).
- **A llama.cpp checkout** for the GGUF export step:
  ```bash
  git clone https://github.com/ggerganov/llama.cpp ~/llama.cpp
  cd ~/llama.cpp && cmake -B build && cmake --build build --target llama-quantize
  ```
  (The export step auto-detects `~/llama.cpp` and a few other common spots.)
- **LM Studio** installed (https://lmstudio.ai) to run the result.

---

## 1. Set up the Python environment

**One command** — creates the venv, installs PyTorch + training deps, and sets
up llama.cpp for the export step:

```bash
cd models/distill
./setup.sh
```

(NVIDIA users: install your CUDA `torch` first, then run `./setup.sh`. Set
`SKIP_LLAMA=1` to skip the llama.cpp build and set it up yourself later.)

<details>
<summary>…or do it manually</summary>

```bash
python3.12 -m venv .venv && source .venv/bin/activate
pip install torch                      # macOS (MPS) / CPU
# pip install torch --index-url https://download.pytorch.org/whl/cu121   # NVIDIA
pip install -r requirements.txt
```
</details>

## 2. Get your training data

You need a JSONL file of `{"prompt": ..., "response": ...}` pairs — the
examples your model learns from. Two ways:

**a) Reuse a dataset from Hugging Face** (fastest):
```bash
python from_hf.py --file your_downloaded_dataset.jsonl --out data/train.jsonl
# or pull straight from a Hub id (needs `pip install datasets`):
python from_hf.py --dataset <user/dataset> --out data/train.jsonl
```

**b) Write your own** — one JSON object per line:
```json
{"prompt": "How do I prioritize three urgent tasks?", "response": "Sequence by impact and deadline..."}
```

Even 100–200 good examples produce a noticeably specialized model.

## 3. Train

```bash
python train.py \
  --data data/train.jsonl \
  --out runs/mymodel/adapter \
  --base-model Qwen/Qwen3-4B-Instruct-2507 \
  --no-4bit          # required on Apple Silicon; drop it on NVIDIA
```

This produces a **LoRA adapter** — a small diff over the base model.
On an M1/M2 this is ~45 min for a few hundred examples.

## 4. Export to a runnable GGUF (and install into LM Studio)

```bash
python export_gguf.py \
  --adapter runs/mymodel/adapter \
  --base-model Qwen/Qwen3-4B-Instruct-2507 \
  --name mymodel \
  --install-lmstudio
```

This merges the adapter into the base, converts to GGUF, quantizes to
`Q4_K_M` (~2.3 GB), and drops it into LM Studio's models folder.

## 5. Use it

Open LM Studio → your model appears in the picker as **`mymodel`** → load it
→ chat. It now runs entirely on your machine, offline, free.

---

## Doing it in one command

Steps 2–3 are also wrapped by the daemon CLI:

```bash
unhosted distill run    -- --base-model Qwen/Qwen3-4B-Instruct-2507 --data data/train.jsonl --out-dir runs/mymodel --no-4bit
unhosted distill export -- --adapter runs/mymodel/adapter --base-model Qwen/Qwen3-4B-Instruct-2507 --name mymodel --install-lmstudio
```

---

## When something breaks

- **`convert_hf_to_gguf.py not found`** → you need a llama.cpp checkout; pass
  `--llama-cpp /path/to/llama.cpp` or clone it to `~/llama.cpp`.
- **`llama-quantize not found`** → build it in your llama.cpp checkout
  (`cmake --build build --target llama-quantize`), or `brew install llama.cpp`.
- **Training is slow / OOMs** → use a smaller base (`Qwen/Qwen3-4B-Instruct-2507`
  is already small; try fewer `--epochs` or a shorter `--max-seq-len`).
- **`bitsandbytes` errors on Mac** → make sure `--no-4bit` is set.
- **Model loads but answers oddly** → a tiny dataset overfits; add more varied
  examples, or lower `--epochs`.

This flow was validated end-to-end on Apple Silicon (M1 Max, 32 GB): a 4B base
distilled on ~200 examples produced a 2.3 GB GGUF that loads and generates
correctly in LM Studio.
