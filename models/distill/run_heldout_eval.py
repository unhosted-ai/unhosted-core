#!/usr/bin/env python3
"""
Run the held-out eval set against a GGUF model via llama-server.

Boots llama-server for the given model, waits for health, sends every
prompt at temperature 0, and writes one JSONL row per prompt with the
model output plus mechanical metrics (finish reason, token count,
latency, ROUGE-L vs the gold reference, length ratio).

Rubric judging (correctness, decisiveness, guardrails) happens on the
output file afterward — this script only collects clean, reproducible
transcripts.

Usage:
  python run_heldout_eval.py \
      --gguf runs/helmsman-4b-Q4_K_M.gguf \
      --test data/eval_heldout_v1.jsonl \
      --out eval/helmsman-4b-v0.1.jsonl
"""

from __future__ import annotations

import argparse
import json
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from eval import rouge_l_f1, length_ratio

SERVER_BIN = "llama-server"
HEALTH_TIMEOUT_S = 120
REQUEST_TIMEOUT_S = 180


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="run held-out eval prompts against a GGUF via llama-server")
    p.add_argument("--gguf", type=Path, required=True)
    p.add_argument("--test", type=Path, required=True,
                   help="JSONL with {category, prompt, response} rows")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--port", type=int, default=8089)
    p.add_argument("--max-tokens", type=int, default=512)
    p.add_argument("--ctx", type=int, default=4096)
    p.add_argument("--limit", type=int, default=0)
    return p.parse_args()


def wait_healthy(port: int, proc: subprocess.Popen) -> None:
    deadline = time.time() + HEALTH_TIMEOUT_S
    url = f"http://127.0.0.1:{port}/health"
    while time.time() < deadline:
        if proc.poll() is not None:
            sys.exit(f"error: llama-server exited early with code {proc.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, ConnectionError, OSError):
            pass
        time.sleep(1)
    sys.exit(f"error: llama-server not healthy after {HEALTH_TIMEOUT_S}s")


def chat(port: int, prompt: str, max_tokens: int) -> dict:
    body = {
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "stream": False,
    }
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT_S) as resp:
        return json.loads(resp.read().decode("utf-8"))


def main() -> None:
    args = parse_args()
    if not args.gguf.exists():
        sys.exit(f"error: {args.gguf} does not exist")

    rows = []
    with args.test.open() as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    if args.limit:
        rows = rows[: args.limit]
    print(f"[eval] {len(rows)} prompts | model: {args.gguf.name}")

    server = subprocess.Popen(
        [SERVER_BIN, "-m", str(args.gguf), "--port", str(args.port),
         "-c", str(args.ctx), "-ngl", "99", "--jinja"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        wait_healthy(args.port, server)
        print(f"[eval] server healthy on :{args.port}")

        args.out.parent.mkdir(parents=True, exist_ok=True)
        n_length_cut = 0
        with args.out.open("w", encoding="utf-8") as out_f:
            for i, row in enumerate(rows, 1):
                t0 = time.time()
                payload = chat(args.port, row["prompt"], args.max_tokens)
                dt = time.time() - t0
                choice = payload["choices"][0]
                output = choice["message"]["content"]
                finish = choice.get("finish_reason")
                usage = payload.get("usage", {})
                if finish == "length":
                    n_length_cut += 1
                out_f.write(json.dumps({
                    "category": row.get("category", "?"),
                    "prompt": row["prompt"],
                    "reference": row["response"],
                    "output": output,
                    "finish_reason": finish,
                    "completion_tokens": usage.get("completion_tokens"),
                    "latency_s": round(dt, 2),
                    "rouge_l": round(rouge_l_f1(output, row["response"]), 4),
                    "len_ratio": round(length_ratio(output, row["response"]), 3),
                }, ensure_ascii=False) + "\n")
                out_f.flush()
                cut = " CUT" if finish == "length" else ""
                print(f"[{i}/{len(rows)}] {row.get('category','?'):24s} "
                      f"{usage.get('completion_tokens', '?'):>4} tok {dt:5.1f}s{cut}")

        print(f"\n[eval] done → {args.out}")
        print(f"[eval] truncated answers (finish_reason=length): {n_length_cut}/{len(rows)}")
    finally:
        server.send_signal(signal.SIGTERM)
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()


if __name__ == "__main__":
    main()
