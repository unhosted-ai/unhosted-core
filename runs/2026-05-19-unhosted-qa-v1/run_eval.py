"""
One-shot programmatic eval for this run.

Loads base + adapter via transformers (in-process, no API server), runs
each test prompt through both, scores with the same metric functions
eval.py uses. Stays a per-run script — the canonical eval.py path
talks OpenAI-compatible HTTP, which we'd need vllm/tgi to serve the
adapter behind. Not worth installing for a smoke run.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import torch
from peft import PeftModel
from transformers import AutoModelForCausalLM, AutoTokenizer

# Reach the project's eval.py for its metric functions, so we don't
# duplicate the scoring logic between the canonical path and this
# script. Drift-proof: any improvement to eval.py's metrics shows up
# here too.
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "models" / "distill"))
from eval import rouge_l_f1, exact_match, length_ratio, mean  # noqa: E402

BASE_MODEL = "TinyLlama/TinyLlama-1.1B-Chat-v1.0"
ADAPTER = Path(__file__).resolve().parent / "out" / "adapter"
TEST = Path(__file__).resolve().parent / "data" / "test.jsonl"
OUT = Path(__file__).resolve().parent / "eval" / "run.jsonl"


def load_test(path: Path) -> list[dict]:
    return [json.loads(l) for l in path.read_text().splitlines() if l.strip()]


def generate(model, tokenizer, prompt: str, max_new_tokens: int = 256) -> str:
    inputs = tokenizer.apply_chat_template(
        [{"role": "user", "content": prompt}],
        tokenize=True,
        add_generation_prompt=True,
        return_tensors="pt",
    )
    device = next(model.parameters()).device
    inputs = inputs.to(device)
    with torch.no_grad():
        out = model.generate(
            inputs,
            max_new_tokens=max_new_tokens,
            do_sample=False,
            pad_token_id=tokenizer.eos_token_id,
        )
    return tokenizer.decode(out[0][inputs.shape[1]:], skip_special_tokens=True).strip()


def main() -> None:
    dtype = torch.bfloat16 if torch.backends.mps.is_available() else torch.float32
    print(f"[eval] dtype={dtype}")

    tokenizer = AutoTokenizer.from_pretrained(ADAPTER, use_fast=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    print(f"[eval] loading base: {BASE_MODEL}")
    base = AutoModelForCausalLM.from_pretrained(BASE_MODEL, dtype=dtype)
    if torch.backends.mps.is_available():
        base = base.to("mps")
    base.eval()

    print(f"[eval] loading candidate (base + adapter from {ADAPTER})")
    cand_base = AutoModelForCausalLM.from_pretrained(BASE_MODEL, dtype=dtype)
    if torch.backends.mps.is_available():
        cand_base = cand_base.to("mps")
    candidate = PeftModel.from_pretrained(cand_base, str(ADAPTER))
    candidate.eval()

    rows = load_test(TEST)
    print(f"[eval] {len(rows)} test rows")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    cand_rouges = []
    base_rouges = []
    cand_lens = []
    base_lens = []
    cand_exacts = 0
    base_exacts = 0

    with OUT.open("w") as f:
        for i, row in enumerate(rows, 1):
            ref = row["response"]
            cand = generate(candidate, tokenizer, row["prompt"])
            ba = generate(base, tokenizer, row["prompt"])

            cr = rouge_l_f1(cand, ref)
            br = rouge_l_f1(ba, ref)
            cl = length_ratio(cand, ref)
            bl = length_ratio(ba, ref)
            ce = exact_match(cand, ref)
            be = exact_match(ba, ref)

            cand_rouges.append(cr)
            base_rouges.append(br)
            cand_lens.append(cl)
            base_lens.append(bl)
            cand_exacts += int(ce)
            base_exacts += int(be)

            f.write(json.dumps({
                "prompt": row["prompt"],
                "reference": ref,
                "candidate": cand,
                "baseline": ba,
                "cand_rouge_l": cr,
                "base_rouge_l": br,
                "cand_len_ratio": cl,
                "base_len_ratio": bl,
                "cand_exact": ce,
                "base_exact": be,
            }, ensure_ascii=False) + "\n")
            f.flush()

            tag = "✓" if cr > br else (" " if cr == br else "✗")
            print(f"[{i}/{len(rows)}] {tag} cand={cr:.3f} base={br:.3f}")

    print()
    print("─── aggregate ───")
    print(f"candidate ROUGE-L mean: {mean(cand_rouges):.3f}")
    print(f"baseline  ROUGE-L mean: {mean(base_rouges):.3f}")
    print(f"candidate exact match : {cand_exacts}/{len(rows)} ({100*cand_exacts/len(rows):.1f}%)")
    print(f"baseline  exact match : {base_exacts}/{len(rows)} ({100*base_exacts/len(rows):.1f}%)")
    print(f"candidate length ratio: {mean(cand_lens):.2f}× reference")
    print(f"baseline  length ratio: {mean(base_lens):.2f}× reference")
    wins = sum(1 for c, b in zip(cand_rouges, base_rouges) if c > b)
    losses = sum(1 for c, b in zip(cand_rouges, base_rouges) if c < b)
    ties = len(rows) - wins - losses
    print(f"candidate vs baseline : {wins} W / {losses} L / {ties} T (ROUGE-L)")
    print(f"per-row log: {OUT}")


if __name__ == "__main__":
    main()
