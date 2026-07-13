#!/usr/bin/env python3
"""
Convert a Hugging Face conversational dataset into the {prompt, response}
JSONL that `train.py` consumes.

The distillation recipe's native path is gen_data.py — generate your own
(prompt, response) pairs from a teacher. But a large, high-quality,
teacher-generated dataset often already exists on the Hub (someone has
already spent the teacher tokens). This script lets you start from one of
those instead of regenerating it.

It targets the common "conversational" schema — each row carries a
`messages` list of {role, content} turns — and flattens it to the flat
prompt/response shape train.py expects:

  - prompt   = the last user turn (optionally prefixed with the system turn)
  - response = the assistant turn, optionally prefixed with a reasoning /
               thinking trace when the dataset ships one separately.

It speaks two input sources:

  1. A local JSONL/Parquet file you already downloaded (--file).
  2. A Hub dataset id (--dataset), pulled via the `datasets` library.
     `pip install datasets` is required only for this path.

Example — the Opus-4.6 reasoning set, keeping the reasoning trace so the
student learns to show its work:

  python from_hf.py \\
      --dataset Roman1111111/claude-opus-4.6-10000x \\
      --out data/train.jsonl \\
      --include-reasoning

Then feed data/train.jsonl straight into train.py (or point pipeline.py at
the parent dir with --resume so it skips gen_data and trains on this).

Design notes:
  - Output is append-friendly and deduped on (prompt, response) so re-runs
    or merging two datasets don't pile up duplicates.
  - Rows missing a user or assistant turn are skipped and counted, not
    fatal — Hub datasets are messy.
  - The reasoning/thinking field goes by several names across datasets
    (`reasoning`, `thinking`, `reasoning_content`, `cot`); we look for any
    of them. Combined "<think>...</think>" content already inside the
    assistant turn is left untouched.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REASONING_KEYS = ("reasoning", "thinking", "reasoning_content", "cot", "rationale")


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="convert a HF conversational dataset to {prompt, response} JSONL",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    src = p.add_mutually_exclusive_group(required=True)
    src.add_argument("--dataset", help="HF Hub dataset id, e.g. Roman1111111/claude-opus-4.6-10000x")
    src.add_argument("--file", type=Path, help="local .jsonl file already downloaded")
    p.add_argument("--out", type=Path, required=True, help="output JSONL path (train.py format)")
    p.add_argument("--split", default="train", help="dataset split to read (default: train)")
    p.add_argument("--config", default=None, help="dataset config/subset name, if any")
    p.add_argument("--include-system", action="store_true",
                   help="prepend the system turn to the prompt")
    p.add_argument("--include-reasoning", action="store_true",
                   help="prepend the reasoning/thinking trace to the response, "
                        "wrapped in <think>…</think> so the student learns to show its work")
    p.add_argument("--limit", type=int, default=0, help="cap rows processed (0 = all)")
    return p.parse_args()


# ─── input loading ──────────────────────────────────────────────────────

def iter_rows(args: argparse.Namespace):
    """Yield dict rows from either a local JSONL file or the Hub."""
    if args.file:
        if not args.file.exists():
            sys.exit(f"error: --file {args.file} does not exist")
        with args.file.open(encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    continue
        return

    try:
        from datasets import load_dataset
    except ImportError:
        sys.exit(
            "error: the 'datasets' package is required for --dataset.\n"
            "  pip install datasets\n"
            "or download the file and use --file instead."
        )
    ds = load_dataset(args.dataset, args.config, split=args.split)
    for row in ds:
        yield row


# ─── row → (prompt, response) ───────────────────────────────────────────

def extract_pair(row: dict, include_system: bool, include_reasoning: bool):
    """Return (prompt, response) or None if the row can't be flattened."""
    messages = row.get("messages")
    if not isinstance(messages, list):
        return None

    system_turn = next((m for m in messages if m.get("role") == "system"), None)
    # last user / last assistant — datasets vary in ordering
    user_turn = next((m for m in reversed(messages) if m.get("role") == "user"), None)
    asst_turn = next((m for m in reversed(messages) if m.get("role") == "assistant"), None)
    if not user_turn or not asst_turn:
        return None

    prompt = (user_turn.get("content") or "").strip()
    response = (asst_turn.get("content") or "").strip()
    if not prompt or not response:
        return None

    if include_system and system_turn:
        sys_text = (system_turn.get("content") or "").strip()
        if sys_text:
            prompt = f"{sys_text}\n\n{prompt}"

    if include_reasoning:
        trace = ""
        for key in REASONING_KEYS:
            val = row.get(key)
            if isinstance(val, str) and val.strip():
                trace = val.strip()
                break
        if trace:
            response = f"<think>\n{trace}\n</think>\n\n{response}"

    return prompt, response


def main() -> None:
    args = parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)

    written = skipped = 0
    seen: set[int] = set()

    with args.out.open("w", encoding="utf-8") as out:
        for i, row in enumerate(iter_rows(args)):
            if args.limit and written >= args.limit:
                break
            pair = extract_pair(row, args.include_system, args.include_reasoning)
            if pair is None:
                skipped += 1
                continue
            prompt, response = pair
            key = hash((prompt, response))
            if key in seen:
                skipped += 1
                continue
            seen.add(key)
            out.write(json.dumps({"prompt": prompt, "response": response}, ensure_ascii=False) + "\n")
            written += 1
            if written % 1000 == 0:
                print(f"[from_hf] {written} pairs written…")

    print(f"[from_hf] done. {written} pairs → {args.out}  ({skipped} rows skipped)")
    if written == 0:
        sys.exit("error: 0 pairs written — check the dataset schema (expected a 'messages' list per row)")


if __name__ == "__main__":
    main()
