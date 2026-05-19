#!/usr/bin/env python3
"""
Eval harness for the unhosted distillation recipe.

Compares a candidate model (your fine-tuned adapter, served behind any
OpenAI-compatible endpoint) against a baseline (typically the base
model before fine-tuning) on a held-out JSONL test set, then prints an
aggregate report plus a per-row breakdown.

Both models are reached over the OpenAI chat-completions wire — so
neither this script nor gen_data.py imports torch / transformers /
peft. Serve the adapter with `vllm serve`, `text-generation-inference`,
or even `unhosted serve` pointed at a merged adapter, then point
--candidate-url at it.

Metrics:

  - ROUGE-L F1: longest-common-subsequence-based similarity to the
    reference answer. Robust, no deps, language-agnostic.
  - Exact match: case-insensitive whitespace-collapsed equality.
    Rarely meaningful for free-text, but the right metric when the
    task is short-form Q&A or extraction.
  - Length ratio: candidate-length / reference-length. Catches
    chronic over-elaboration ("As an AI language model, …" prefixes).

Per-row output is JSONL so a follow-up tool can rank failures, build
confusion sets, etc. The aggregate report goes to stdout.

Run:

  python eval.py \\
      --test data/test.jsonl \\
      --candidate-url http://127.0.0.1:8001 \\
      --baseline-url http://127.0.0.1:8002 \\
      --out eval/run-2026-05-19.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


DEFAULT_TIMEOUT_S = 60
MAX_RETRIES = 2
RETRY_BACKOFF_S = 3


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="evaluate a distilled model against a baseline on a held-out JSONL")
    p.add_argument("--test", type=Path, required=True,
                   help="JSONL hold-out set, same {prompt, response, ...} shape as train data")
    p.add_argument("--candidate-url", required=True,
                   help="OpenAI-compatible base URL for the candidate (e.g. fine-tuned adapter)")
    p.add_argument("--candidate-model", default="default",
                   help="Model name to send in the API call for the candidate")
    p.add_argument("--candidate-key", default=os.environ.get("CANDIDATE_API_KEY", ""))
    p.add_argument("--baseline-url",
                   help="OpenAI-compatible base URL for the baseline. Omit to skip the side-by-side.")
    p.add_argument("--baseline-model", default="default")
    p.add_argument("--baseline-key", default=os.environ.get("BASELINE_API_KEY", ""))
    p.add_argument("--out", type=Path,
                   help="Per-row eval log (JSONL). Optional; aggregates always print to stdout.")
    p.add_argument("--max-tokens", type=int, default=512)
    p.add_argument("--temperature", type=float, default=0.0,
                   help="Default 0 — deterministic eval. Bump only if intentionally evaluating sampling.")
    p.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT_S)
    p.add_argument("--limit", type=int, default=0,
                   help="Stop after N test rows. 0 = all. Useful for quick sanity runs.")
    return p.parse_args()


# ─── test set loading ───────────────────────────────────────────────────

def load_test(path: Path, limit: int) -> list[dict]:
    if not path.exists():
        sys.exit(f"error: {path} does not exist")
    rows: list[dict] = []
    with path.open() as f:
        for i, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError as e:
                sys.exit(f"error: {path}:{i}: invalid JSON: {e}")
            if "prompt" not in obj or "response" not in obj:
                sys.exit(f"error: {path}:{i}: missing 'prompt' or 'response'")
            rows.append({"prompt": obj["prompt"], "response": obj["response"]})
            if limit and len(rows) >= limit:
                break
    if not rows:
        sys.exit(f"error: {path} is empty")
    return rows


# ─── OpenAI-compatible call ─────────────────────────────────────────────

def chat_completion(
    base_url: str,
    api_key: str,
    model: str,
    prompt: str,
    temperature: float,
    max_tokens: int,
    timeout: int,
) -> str:
    body = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": False,
    }
    headers = {"content-type": "application/json"}
    if api_key:
        headers["authorization"] = f"Bearer {api_key}"
    url = f"{base_url.rstrip('/')}/v1/chat/completions"

    last_err: Exception | None = None
    for attempt in range(1, MAX_RETRIES + 1):
        try:
            req = urllib.request.Request(
                url, data=json.dumps(body).encode("utf-8"), headers=headers, method="POST"
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                payload = json.loads(resp.read().decode("utf-8"))
            return payload["choices"][0]["message"]["content"]
        except (urllib.error.URLError, KeyError, json.JSONDecodeError) as e:
            last_err = e
            if attempt < MAX_RETRIES:
                time.sleep(RETRY_BACKOFF_S * attempt)
                continue
            raise RuntimeError(f"completion failed: {e}") from last_err
    raise RuntimeError("unreachable")


# ─── metrics ────────────────────────────────────────────────────────────

WS_RE = re.compile(r"\s+")


def normalize(s: str) -> str:
    """Collapse whitespace, lowercase, strip. Used for both exact-match
    and the tokenization that feeds ROUGE-L."""
    return WS_RE.sub(" ", s.strip().lower())


def tokenize(s: str) -> list[str]:
    # Split on non-alphanumeric, drop empties. Good enough for ROUGE-L
    # over English-ish text; ROUGE-L doesn't really need linguistic
    # tokenization, just consistency between hypothesis and reference.
    return [t for t in re.split(r"[^a-z0-9]+", normalize(s)) if t]


def lcs_length(a: list[str], b: list[str]) -> int:
    if not a or not b:
        return 0
    # Use the rolling 1-D DP variant; O(min(|a|,|b|)) memory.
    if len(b) < len(a):
        a, b = b, a
    prev = [0] * (len(a) + 1)
    curr = [0] * (len(a) + 1)
    for bj in b:
        for i, ai in enumerate(a, 1):
            curr[i] = prev[i - 1] + 1 if ai == bj else max(curr[i - 1], prev[i])
        prev, curr = curr, prev
        for k in range(len(curr)):
            curr[k] = 0
    return prev[len(a)]


def rouge_l_f1(hypothesis: str, reference: str) -> float:
    h = tokenize(hypothesis)
    r = tokenize(reference)
    if not h or not r:
        return 0.0
    lcs = lcs_length(h, r)
    if lcs == 0:
        return 0.0
    precision = lcs / len(h)
    recall = lcs / len(r)
    return (2 * precision * recall) / (precision + recall)


def exact_match(hypothesis: str, reference: str) -> bool:
    return normalize(hypothesis) == normalize(reference)


def length_ratio(hypothesis: str, reference: str) -> float:
    h = len(tokenize(hypothesis))
    r = len(tokenize(reference)) or 1
    return h / r


# ─── main loop ──────────────────────────────────────────────────────────

def main() -> None:
    args = parse_args()
    rows = load_test(args.test, args.limit)
    print(f"[eval] loaded {len(rows)} test rows from {args.test}")
    print(f"[eval] candidate: {args.candidate_url} (model={args.candidate_model})")
    if args.baseline_url:
        print(f"[eval] baseline:  {args.baseline_url} (model={args.baseline_model})")
    else:
        print("[eval] no baseline — reporting candidate-vs-reference only")

    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        out_f = args.out.open("w", encoding="utf-8")
    else:
        out_f = None

    cand_rouges: list[float] = []
    cand_exacts: list[bool] = []
    cand_len_ratios: list[float] = []
    base_rouges: list[float] = []
    base_exacts: list[bool] = []
    base_len_ratios: list[float] = []
    failures = 0

    try:
        for i, row in enumerate(rows, 1):
            ref = row["response"]
            try:
                cand = chat_completion(
                    args.candidate_url, args.candidate_key, args.candidate_model,
                    row["prompt"], args.temperature, args.max_tokens, args.timeout,
                )
            except Exception as e:
                failures += 1
                print(f"[{i}/{len(rows)}] candidate failed: {e}")
                continue
            cand_r = rouge_l_f1(cand, ref)
            cand_e = exact_match(cand, ref)
            cand_l = length_ratio(cand, ref)
            cand_rouges.append(cand_r)
            cand_exacts.append(cand_e)
            cand_len_ratios.append(cand_l)
            base = None
            base_r = base_l = 0.0
            base_e = False
            if args.baseline_url:
                try:
                    base = chat_completion(
                        args.baseline_url, args.baseline_key, args.baseline_model,
                        row["prompt"], args.temperature, args.max_tokens, args.timeout,
                    )
                except Exception as e:
                    failures += 1
                    print(f"[{i}/{len(rows)}] baseline failed: {e}")
                else:
                    base_r = rouge_l_f1(base, ref)
                    base_e = exact_match(base, ref)
                    base_l = length_ratio(base, ref)
                    base_rouges.append(base_r)
                    base_exacts.append(base_e)
                    base_len_ratios.append(base_l)

            if out_f:
                out_f.write(json.dumps({
                    "prompt": row["prompt"],
                    "reference": ref,
                    "candidate": cand,
                    "baseline": base,
                    "cand_rouge_l": cand_r,
                    "cand_exact": cand_e,
                    "cand_len_ratio": cand_l,
                    "base_rouge_l": base_r if base else None,
                    "base_exact": base_e if base else None,
                    "base_len_ratio": base_l if base else None,
                }, ensure_ascii=False) + "\n")
                out_f.flush()

            tag = "✓" if cand_r >= base_r else " "
            print(f"[{i}/{len(rows)}] {tag} cand_rouge={cand_r:.3f} base_rouge={base_r:.3f}")
    finally:
        if out_f:
            out_f.close()

    print()
    print("─── aggregate ───")
    if cand_rouges:
        print(f"candidate ROUGE-L mean: {mean(cand_rouges):.3f}")
        print(f"candidate exact match : {sum(cand_exacts)}/{len(cand_exacts)} ({100*sum(cand_exacts)/len(cand_exacts):.1f}%)")
        print(f"candidate length ratio: {mean(cand_len_ratios):.2f}× reference")
    if base_rouges:
        print(f"baseline  ROUGE-L mean: {mean(base_rouges):.3f}")
        print(f"baseline  exact match : {sum(base_exacts)}/{len(base_exacts)} ({100*sum(base_exacts)/len(base_exacts):.1f}%)")
        print(f"baseline  length ratio: {mean(base_len_ratios):.2f}× reference")
        wins = sum(1 for c, b in zip(cand_rouges, base_rouges) if c > b)
        losses = sum(1 for c, b in zip(cand_rouges, base_rouges) if c < b)
        ties = len(cand_rouges) - wins - losses
        print(f"candidate vs baseline : {wins} W / {losses} L / {ties} T (by ROUGE-L)")
    if failures:
        print(f"[eval] {failures} request failures")
    if args.out:
        print(f"[eval] per-row log written to {args.out}")


def mean(xs: list[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


if __name__ == "__main__":
    main()
