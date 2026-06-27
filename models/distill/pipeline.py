#!/usr/bin/env python3
"""
End-to-end distillation pipeline for unhosted.

Wires gen_data → train → eval into a single command.
Each stage writes to --out-dir so you can restart from any
checkpoint without re-running earlier stages.

Usage:

  # Local daemon as teacher (free):
  python pipeline.py --docs path/to/docs --out-dir runs/my-run

  # Claude as teacher (best quality, needs ANTHROPIC_API_KEY):
  python pipeline.py --docs path/to/docs --out-dir runs/my-run \\
      --teacher claude-sonnet-4-6

  # Bigger student model on a GPU machine:
  python pipeline.py --docs path/to/docs --out-dir runs/my-run \\
      --teacher claude-opus-4-8 \\
      --base-model Qwen/Qwen2.5-7B-Instruct

  # Resume an interrupted run (skips completed stages):
  python pipeline.py --docs path/to/docs --out-dir runs/my-run --resume

Stage outputs:
  <out-dir>/data/train.jsonl   training pairs
  <out-dir>/data/test.jsonl    held-out eval pairs (10% split)
  <out-dir>/adapter/           trained LoRA adapter
  <out-dir>/eval/report.jsonl  per-row eval results
  <out-dir>/eval/summary.txt   aggregate metrics
"""

from __future__ import annotations

import argparse
import json
import os
import random
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).parent


def run(cmd: list[str], **kwargs) -> None:
    print(f"\n[pipeline] $ {' '.join(cmd)}")
    result = subprocess.run(cmd, **kwargs)
    if result.returncode != 0:
        sys.exit(f"[pipeline] stage failed (exit {result.returncode})")


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="end-to-end distillation pipeline",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--docs", type=Path, required=True,
                   help="Directory of .txt/.md documents to learn from")
    p.add_argument("--out-dir", type=Path, required=True,
                   help="Root directory for all stage outputs")
    p.add_argument("--teacher",
                   default=os.environ.get("DISTILL_TEACHER_MODEL", "default"),
                   help="Teacher model. 'default' = local unhosted daemon. "
                        "'claude-opus-4-8', 'claude-sonnet-4-6' = Anthropic SDK. "
                        "Any other string = OpenAI-compat model name.")
    p.add_argument("--base-model",
                   default="TinyLlama/TinyLlama-1.1B-Chat-v1.0",
                   help="HF model ID of the student base (default: TinyLlama 1.1B)")
    p.add_argument("--base-url",
                   default=os.environ.get("OPENAI_BASE_URL", "http://127.0.0.1:7777"),
                   help="Base URL for OpenAI-compat teacher endpoint")
    p.add_argument("--api-key",
                   default=os.environ.get("OPENAI_API_KEY", ""),
                   help="API key for OpenAI-compat teacher (empty = local daemon)")
    p.add_argument("--pairs-per-doc", type=int, default=6,
                   help="Q/A pairs to generate per document")
    p.add_argument("--test-split", type=float, default=0.1,
                   help="Fraction of pairs to hold out for eval (default: 0.10)")
    p.add_argument("--epochs", type=int, default=3)
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--no-4bit", action="store_true",
                   help="Disable bitsandbytes 4-bit quantization (required on macOS/MPS)")
    p.add_argument("--resume", action="store_true",
                   help="Skip stages whose outputs already exist")
    p.add_argument("--skip-eval", action="store_true",
                   help="Skip the eval stage (useful if no second endpoint is available)")
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def stage_header(name: str) -> None:
    bar = "─" * 60
    print(f"\n{bar}")
    print(f"  {name}")
    print(bar)


def split_jsonl(src: Path, train_out: Path, test_out: Path,
                test_frac: float, seed: int) -> None:
    """Split a JSONL file into train/test subsets in place."""
    rows = src.read_text().splitlines()
    rows = [r for r in rows if r.strip()]
    random.seed(seed)
    random.shuffle(rows)
    n_test = max(1, int(len(rows) * test_frac))
    test_rows, train_rows = rows[:n_test], rows[n_test:]
    train_out.parent.mkdir(parents=True, exist_ok=True)
    train_out.write_text("\n".join(train_rows) + "\n")
    test_out.write_text("\n".join(test_rows) + "\n")
    print(f"[pipeline] split: {len(train_rows)} train / {len(test_rows)} test pairs")


def main() -> None:
    args = parse_args()
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)

    data_dir   = out / "data"
    raw_jsonl  = data_dir / "raw.jsonl"
    train_jsonl = data_dir / "train.jsonl"
    test_jsonl  = data_dir / "test.jsonl"
    adapter_dir = out / "adapter"
    eval_dir    = out / "eval"

    python = sys.executable

    # ── Stage 1: generate data ────────────────────────────────────────────
    stage_header("Stage 1 / 3  — generate training data")

    if args.resume and raw_jsonl.exists():
        print(f"[pipeline] skipping gen_data (found {raw_jsonl})")
    else:
        cmd = [
            python, str(HERE / "gen_data.py"),
            "--docs", str(args.docs),
            "--out", str(raw_jsonl),
            "--model", args.teacher,
            "--pairs-per-doc", str(args.pairs_per_doc),
            "--seed", str(args.seed),
        ]
        if not args.teacher.startswith("claude-"):
            cmd += ["--base-url", args.base_url]
            if args.api_key:
                cmd += ["--api-key", args.api_key]
        if args.resume:
            cmd += ["--resume"]
        run(cmd)

    # Split into train / test
    if args.resume and train_jsonl.exists() and test_jsonl.exists():
        print(f"[pipeline] skipping split (found {train_jsonl} and {test_jsonl})")
    else:
        split_jsonl(raw_jsonl, train_jsonl, test_jsonl, args.test_split, args.seed)

    # ── Stage 2: train ────────────────────────────────────────────────────
    stage_header("Stage 2 / 3  — QLoRA fine-tuning")

    if args.resume and (adapter_dir / "adapter_config.json").exists():
        print(f"[pipeline] skipping train (found {adapter_dir})")
    else:
        cmd = [
            python, str(HERE / "train.py"),
            "--data", str(train_jsonl),
            "--out", str(adapter_dir),
            "--base-model", args.base_model,
            "--epochs", str(args.epochs),
            "--lora-r", str(args.lora_r),
            "--seed", str(args.seed),
        ]
        if args.no_4bit:
            cmd += ["--no-4bit"]
        run(cmd)

    # ── Stage 3: eval ─────────────────────────────────────────────────────
    stage_header("Stage 3 / 3  — evaluation")

    if args.skip_eval:
        print("[pipeline] skipping eval (--skip-eval)")
    elif args.resume and (eval_dir / "summary.txt").exists():
        print(f"[pipeline] skipping eval (found {eval_dir / 'summary.txt'})")
    else:
        eval_dir.mkdir(parents=True, exist_ok=True)
        cmd = [
            python, str(HERE / "eval.py"),
            "--test", str(test_jsonl),
            "--candidate-url", "http://127.0.0.1:8001",
            "--out", str(eval_dir / "report.jsonl"),
        ]
        print("[pipeline] note: eval compares two served endpoints.")
        print("           Start your fine-tuned model on :8001 before this stage.")
        print("           Run with --skip-eval to skip if you haven't merged the adapter yet.")
        run(cmd)

    # ── Done ──────────────────────────────────────────────────────────────
    stage_header("Done")
    print(f"  adapter  →  {adapter_dir}")
    print(f"  data     →  {train_jsonl}  ({test_jsonl.name} held out)")
    if not args.skip_eval:
        print(f"  eval     →  {eval_dir / 'report.jsonl'}")
    print()
    print("  next steps:")
    print("  1. merge adapter into base:  python train.py --merge --adapter", adapter_dir)
    print("  2. quantize for llama.cpp:   see docs/distill-to-gguf.md (upcoming)")
    print("  3. publish to HF Hub:        python push_to_hub.py --adapter", adapter_dir)
    print()


if __name__ == "__main__":
    main()
