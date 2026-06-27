#!/usr/bin/env python3
"""
Synthetic-data generator for the unhosted distillation recipe.

Takes a directory of documents (txt / md), asks a teacher model to
generate (question, answer) pairs grounded in each document, and
writes a JSONL of {prompt, response} pairs that `train.py` can consume.

Pointed by default at a local unhosted daemon
(`http://127.0.0.1:7777/v1/chat/completions`) because that's the
loop unhosted itself wants to enable: you can distill from your own
running stack. Override with --base-url + --api-key for OpenAI,
Together, Groq, or anything else that speaks the OpenAI-compatible
chat-completions wire format.

For Claude teachers (recommended for highest-quality data), pass the
model name directly and set ANTHROPIC_API_KEY:

  python gen_data.py --docs docs/ --out data/train.jsonl \\
      --model claude-opus-4-8

The script detects "claude-*" model names and switches automatically
to the Anthropic SDK (native messages API, not the compat layer).
`pip install anthropic` is required only for this path.

Key design choices:

  - One LLM call per document, not per Q&A pair. Per-pair would
    multiply cost by N and produce duplicates; one call asks for N
    grounded pairs at a time and the teacher self-deduplicates.
  - Strict JSON parsing of the teacher response. If the teacher
    free-styles, we skip the document and log. Better one missing
    doc than 50 ungrounded synthetic pairs polluting the trainset.
  - Output is incremental — every successful document appends its
    pairs to the JSONL immediately. A crash 80% through doesn't
    lose 80% of an hour of teacher tokens.
  - Resume support via --resume: skips documents whose hash is
    already represented in the output file, so an interrupted run
    picks up where it stopped without re-spending teacher tokens.

Run:

  python gen_data.py --docs path/to/docs --out data/train.jsonl
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


DEFAULT_BASE_URL = "http://127.0.0.1:7777"
DEFAULT_MODEL = "default"          # local daemon route doesn't care
DEFAULT_PAIRS_PER_DOC = 6
DEFAULT_TIMEOUT_S = 120
MAX_RETRIES = 3
RETRY_BACKOFF_S = 4

# The teacher prompt. Kept blunt: most failure modes in synthetic data
# come from teachers being chatty, including the document text in the
# answer, or producing meta-questions ("what does this document
# discuss?") instead of substantive ones. The instructions push the
# teacher away from each of those.
SYSTEM_PROMPT = (
    "You generate training pairs for a small language model. "
    "Respond ONLY with a valid JSON array. Do not include explanation, "
    "preamble, or markdown code fences."
)

USER_PROMPT_TEMPLATE = """\
Generate exactly {n} question-answer pairs grounded in the document below.

Requirements:
- Each question must be answerable from the document alone (no outside knowledge).
- Each question must be substantive — do NOT ask "what is this document about" or "summarise this".
- Each answer must be standalone — the reader will see only the question and answer, not the document.
- Vary the question style: some factual, some inferential, some asking for definitions or comparisons that appear in the text.
- Keep answers concise but complete (1–5 sentences typically).

Document:
\"\"\"
{document}
\"\"\"

Output a JSON array of objects with "q" and "a" string fields:
[{{"q": "...", "a": "..."}}, ...]
"""


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="generate (prompt, response) pairs grounded in a document set")
    p.add_argument("--docs", type=Path, required=True,
                   help="directory of .txt / .md documents to ground questions in")
    p.add_argument("--out", type=Path, required=True,
                   help="output JSONL path")
    p.add_argument("--base-url", default=os.environ.get("OPENAI_BASE_URL", DEFAULT_BASE_URL),
                   help=f"OpenAI-compatible base URL. Default: $OPENAI_BASE_URL or {DEFAULT_BASE_URL}")
    p.add_argument("--api-key", default=os.environ.get("OPENAI_API_KEY", ""),
                   help="Bearer token. Default: $OPENAI_API_KEY. Empty is fine for loopback unhosted.")
    p.add_argument("--model", default=os.environ.get("DISTILL_TEACHER_MODEL", DEFAULT_MODEL),
                   help=f"Teacher model name. Default: $DISTILL_TEACHER_MODEL or {DEFAULT_MODEL}")
    p.add_argument("--pairs-per-doc", type=int, default=DEFAULT_PAIRS_PER_DOC,
                   help="How many Q/A pairs to generate per document")
    p.add_argument("--max-doc-chars", type=int, default=8000,
                   help="Truncate documents above this many characters before prompting")
    p.add_argument("--temperature", type=float, default=0.7)
    p.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT_S)
    p.add_argument("--resume", action="store_true",
                   help="Skip documents whose hash is already in --out")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--dry-run", action="store_true",
                   help="List the documents that would be processed and exit")
    return p.parse_args()


# ─── document loading ───────────────────────────────────────────────────

DOC_EXTENSIONS = {".txt", ".md", ".markdown"}


def collect_documents(root: Path, max_chars: int) -> list[tuple[str, str]]:
    """Return [(doc_hash, doc_text)]. doc_hash is a short content hash
    used for the --resume mechanism. We hash the *truncated* text so a
    longer-edited doc doesn't accidentally re-trigger as "new"."""
    if not root.exists():
        sys.exit(f"error: --docs path {root} does not exist")
    if not root.is_dir():
        sys.exit(f"error: --docs path {root} is not a directory")
    out: list[tuple[str, str]] = []
    for p in sorted(root.rglob("*")):
        if not p.is_file() or p.suffix.lower() not in DOC_EXTENSIONS:
            continue
        try:
            text = p.read_text(encoding="utf-8", errors="replace").strip()
        except OSError as e:
            print(f"warn: skipping {p}: {e}")
            continue
        if not text:
            continue
        if len(text) > max_chars:
            text = text[:max_chars]
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]
        out.append((digest, text))
    return out


# ─── existing-output dedup ──────────────────────────────────────────────

def load_seen_hashes(path: Path) -> set[str]:
    """Read the doc_hashes already present in the output JSONL so a
    --resume run skips them. Each line carries `doc_hash` in the
    metadata; gen_data.py writes it on every row."""
    seen: set[str] = set()
    if not path.exists():
        return seen
    with path.open() as f:
        for line in f:
            try:
                obj = json.loads(line)
                if "doc_hash" in obj:
                    seen.add(obj["doc_hash"])
            except json.JSONDecodeError:
                continue
    return seen


# ─── teacher call ───────────────────────────────────────────────────────

def call_teacher(
    base_url: str,
    api_key: str,
    model: str,
    document: str,
    n_pairs: int,
    temperature: float,
    timeout: int,
) -> list[dict]:
    """Route to the right backend based on the model name and return [{q, a}]."""
    if model.startswith("claude-"):
        return _call_teacher_claude(model, document, n_pairs, temperature)
    return _call_teacher_openai(base_url, api_key, model, document, n_pairs, temperature, timeout)


def _call_teacher_claude(
    model: str,
    document: str,
    n_pairs: int,
    temperature: float,
) -> list[dict]:
    """Use the native Anthropic SDK (not the compat layer) for Claude models.
    Requires: pip install anthropic   and   ANTHROPIC_API_KEY in environment."""
    try:
        import anthropic
    except ImportError:
        sys.exit(
            "error: the 'anthropic' package is required for Claude teacher models.\n"
            "  pip install anthropic"
        )

    client = anthropic.Anthropic()  # reads ANTHROPIC_API_KEY automatically

    last_err: Exception | None = None
    for attempt in range(1, MAX_RETRIES + 1):
        try:
            msg = client.messages.create(
                model=model,
                max_tokens=4096,
                system=SYSTEM_PROMPT,
                messages=[
                    {"role": "user", "content": USER_PROMPT_TEMPLATE.format(n=n_pairs, document=document)},
                ],
                temperature=temperature,
            )
            content = msg.content[0].text
            return parse_teacher_response(content, n_pairs)
        except Exception as e:
            last_err = e
            if attempt < MAX_RETRIES:
                wait = RETRY_BACKOFF_S * attempt
                print(f"warn: Claude call failed ({e}); retry {attempt}/{MAX_RETRIES} in {wait}s")
                time.sleep(wait)
                continue
            raise RuntimeError(f"Claude call failed after {MAX_RETRIES} attempts: {e}") from last_err
    raise RuntimeError("unreachable")


def _call_teacher_openai(
    base_url: str,
    api_key: str,
    model: str,
    document: str,
    n_pairs: int,
    temperature: float,
    timeout: int,
) -> list[dict]:
    """POST to /v1/chat/completions (OpenAI-compatible) and return [{q, a}]."""
    body = {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": USER_PROMPT_TEMPLATE.format(n=n_pairs, document=document)},
        ],
        "temperature": temperature,
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
            content = payload["choices"][0]["message"]["content"]
            return parse_teacher_response(content, n_pairs)
        except (urllib.error.URLError, KeyError, json.JSONDecodeError) as e:
            last_err = e
            if attempt < MAX_RETRIES:
                wait = RETRY_BACKOFF_S * attempt
                print(f"warn: teacher call failed ({e}); retry {attempt}/{MAX_RETRIES} in {wait}s")
                time.sleep(wait)
                continue
            raise RuntimeError(f"teacher call failed after {MAX_RETRIES} attempts: {e}") from last_err
    raise RuntimeError("unreachable")


def parse_teacher_response(content: str, expected_n: int) -> list[dict]:
    """Extract the JSON array from a teacher response. Defensive
    because teachers like to wrap things in ```json … ``` fences even
    when told not to."""
    s = content.strip()
    # Strip leading/trailing markdown fences if present.
    if s.startswith("```"):
        s = s.split("\n", 1)[1] if "\n" in s else s[3:]
        if s.endswith("```"):
            s = s.rsplit("```", 1)[0]
        s = s.strip()
    # Try direct parse first.
    try:
        arr = json.loads(s)
    except json.JSONDecodeError:
        # Fall back to extracting the first balanced [..] block.
        start = s.find("[")
        end = s.rfind("]")
        if start == -1 or end == -1 or end <= start:
            raise ValueError(f"no JSON array in teacher response: {s[:200]!r}")
        arr = json.loads(s[start:end + 1])
    if not isinstance(arr, list):
        raise ValueError(f"teacher returned non-array: {type(arr).__name__}")
    pairs: list[dict] = []
    for item in arr:
        if not isinstance(item, dict):
            continue
        q = item.get("q") or item.get("question")
        a = item.get("a") or item.get("answer")
        if isinstance(q, str) and isinstance(a, str) and q.strip() and a.strip():
            pairs.append({"q": q.strip(), "a": a.strip()})
    if not pairs:
        raise ValueError("teacher returned 0 valid (q, a) pairs")
    if len(pairs) < expected_n:
        # Under-shoot is OK — keep going.
        print(f"warn: teacher returned {len(pairs)}/{expected_n} pairs")
    return pairs


# ─── main loop ──────────────────────────────────────────────────────────

def main() -> None:
    args = parse_args()
    random.seed(args.seed)

    docs = collect_documents(args.docs, args.max_doc_chars)
    if not docs:
        sys.exit(f"error: no .txt/.md documents found under {args.docs}")

    seen = load_seen_hashes(args.out) if args.resume else set()
    to_process = [(h, t) for (h, t) in docs if h not in seen]
    if args.resume:
        print(f"[gen_data] resuming: {len(seen)} docs already in {args.out}, {len(to_process)} remaining")

    if args.dry_run:
        print(f"[gen_data] dry-run: would process {len(to_process)} documents:")
        for h, t in to_process:
            preview = t[:60].replace("\n", " ")
            print(f"  {h}  {preview}…")
        return

    args.out.parent.mkdir(parents=True, exist_ok=True)

    total_pairs = 0
    failures = 0
    with args.out.open("a", encoding="utf-8") as f:
        for i, (doc_hash, doc_text) in enumerate(to_process, 1):
            try:
                pairs = call_teacher(
                    base_url=args.base_url,
                    api_key=args.api_key,
                    model=args.model,
                    document=doc_text,
                    n_pairs=args.pairs_per_doc,
                    temperature=args.temperature,
                    timeout=args.timeout,
                )
            except Exception as e:
                failures += 1
                print(f"[{i}/{len(to_process)}] {doc_hash} FAILED: {e}")
                continue
            for p in pairs:
                row = {"prompt": p["q"], "response": p["a"], "doc_hash": doc_hash}
                f.write(json.dumps(row, ensure_ascii=False) + "\n")
                total_pairs += 1
            f.flush()
            print(f"[{i}/{len(to_process)}] {doc_hash} +{len(pairs)} pairs (running total: {total_pairs})")

    print(f"[gen_data] done. {total_pairs} pairs written to {args.out}; {failures} doc failures.")


if __name__ == "__main__":
    main()
