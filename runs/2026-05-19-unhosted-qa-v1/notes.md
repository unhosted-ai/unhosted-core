# Run: `unhosted-qa-v1` (2026-05-19)

The first real distillation run through the unhosted recipe. Trained a TinyLlama-1.1B-Chat LoRA adapter on synthetic Q&A pairs grounded in unhosted's own docs, producing a tiny "support bot" that knows what ADRs are and how the relay works.

This is documentation, not a deliverable. The trained adapter isn't published; nobody outside this repo should use it. The point is to prove the recipe works end-to-end and to write down what we learned.

## Recipe parameters

| Parameter | Value |
| --- | --- |
| Base model | `TinyLlama/TinyLlama-1.1B-Chat-v1.0` (1.1B) |
| Adapter | LoRA r=16, α=32, dropout=0.05 |
| Trainable params | 12.6M (1.13% of total) |
| Training data | 63 (Q, A) pairs synthesized by `qwen2.5:3b` (Ollama, local) |
| Eval data | 12 held-out pairs from the same synthesis (different shuffle index) |
| Epochs | 3 |
| Batch size × grad accum | 2 × 4 (effective batch 8) |
| Learning rate | 2e-4 cosine, 5% warmup |
| dtype | bf16 on MPS |
| Hardware | Apple M-series (MPS), ~60s wall-clock |

## Document corpus

Twelve markdown files: `README.md`, `design/0001`–`design/0009` ADRs, `design/README.md`, `docs/README.md`. ~70 KB total. The teacher generated 6 pairs per document.

## Results

| Metric | Baseline (TinyLlama base) | Candidate (adapter) | Δ |
| --- | --- | --- | --- |
| ROUGE-L F1 (mean) | 0.082 | **0.129** | +57% |
| Win / loss / tie (ROUGE-L) | — | **9 W / 2 L / 1 T** | over 12 examples |
| Length ratio (cand / ref) | 6.16× | **1.02×** | adapter learned reference brevity |
| Exact match | 0/12 | 0/12 | expected — free-form text |

The headline number is **length ratio**. The baseline blabbers — 6× the length of the reference on every prompt, padding answers with definitions, tangents, and made-up details. The adapter learned the corpus's terse style: 1.02× the reference length.

## Qualitative samples

### Sample 1: "What does the term 'draft' in an ADR's status mean?"

- **Reference:** A draft indicates that the decision is still being considered and has not yet been accepted or superseded by another ADR.
- **Candidate:** Drafts are not yet finalized and may be subject to change.
- **Baseline:** In the context of Alternative Dispute Resolution (ADR), the term "draft" refers to the initial stage of the process where the parties involved in the dispute work together to develop a solution to the problem… *(continues for 200+ tokens)*

The baseline doesn't know "ADR" means "Architecture Decision Record" in this corpus. The candidate does — it's seen the term in nine contexts during training.

### Sample 2: "What protocol does the relay use for communication?"

- **Reference:** The relay uses WebSocket-based JSON messages. Binary CBOR is a later optimization.
- **Candidate:** The relay uses the `relay` protocol, which is a custom protocol for relaying messages between peers.
- **Baseline:** The relay uses the TCP/IP protocol for communication… *(invents a TCP/UDP hybrid that doesn't exist in the docs)*

Candidate is still hand-wavy here — 63 training rows isn't enough to teach the exact WebSocket+CBOR detail — but it stays in the right conceptual neighborhood. The baseline hallucinates.

## Failure modes worth noting

- **Two regressions out of 12.** Both are cases where the baseline happened to repeat phrasing from the question in a way that scored well on ROUGE-L, while the candidate paraphrased. ROUGE-L's word-overlap bias is a known weakness; the qualitative read is closer to "candidate is better but lost on the metric."
- **0% exact match for both.** Expected — the test set is free-form Q&A. Exact match is only useful for closed-form extraction tasks.
- **Teacher quality is the ceiling.** `qwen2.5:3b` is a 3B model serving as teacher; the synthetic pairs occasionally have small inaccuracies (e.g., the "relay protocol" mention in the test set itself is vague). With a stronger teacher (gpt-4o-mini, Claude Haiku) the gains would be larger.

## What broke during the run

1. **TRL 0.29 dropped `max_seq_length`** from `SFTConfig` in favor of `max_length`. Fixed in [train.py](../../models/distill/train.py).
2. **fp16 on MPS produced NaN gradients.** Train loop "completed" with mean_token_accuracy=0 and loss=0.0 (the canary). Switched to bf16 in `detect_dtype()` — MPS bf16 is reliable since macOS 14. Fixed in [train.py](../../models/distill/train.py).
3. **Python 3.14 has no compatible torch wheels** as of this run. Used Homebrew's `python@3.12`.
4. **Teacher emitted invalid JSON 3 times** out of 12 documents (Ollama-local qwen2.5:3b under load). `gen_data.py`'s retry+bracket-extraction fallback caught all three.

## Reproducing this run

```bash
# 1. Teacher: any OpenAI-compatible endpoint. We used local Ollama:
ollama pull qwen2.5:3b

# 2. Build the corpus (`runs/2026-05-19-unhosted-qa-v1/docs/` already
#    holds copies, but you can rebuild from the live repo):
RUN_DIR=runs/2026-05-19-unhosted-qa-v1
mkdir -p "$RUN_DIR/docs"
cp README.md "$RUN_DIR/docs/00-readme.md"
cp design/README.md "$RUN_DIR/docs/01-design-index.md"
cp design/00*.md "$RUN_DIR/docs/"
cp docs/README.md "$RUN_DIR/docs/99-docs-readme.md"

# 3. Generate data.
unhosted distill data -- \
  --docs "$RUN_DIR/docs" \
  --out  "$RUN_DIR/data/all.jsonl" \
  --base-url http://127.0.0.1:11434 \
  --model qwen2.5:3b \
  --pairs-per-doc 6 \
  --seed 42

# 4. Split 85/15. (Script lives at /tmp/split.py in this run; one-off.)
python3 /tmp/split.py "$RUN_DIR/data/all.jsonl" "$RUN_DIR/data/train.jsonl" "$RUN_DIR/data/test.jsonl" 0.85

# 5. venv + training stack.
python3.12 -m venv "$RUN_DIR/.venv"
source "$RUN_DIR/.venv/bin/activate"
pip install torch>=2.4 transformers>=4.45 trl>=0.11 peft>=0.13 datasets>=3.0 accelerate>=1.0

# 6. Train.
export UNHOSTED_PYTHON="$PWD/$RUN_DIR/.venv/bin/python"
unhosted distill train -- \
  --data "$RUN_DIR/data/train.jsonl" \
  --out  "$RUN_DIR/out/adapter" \
  --base-model TinyLlama/TinyLlama-1.1B-Chat-v1.0 \
  --epochs 3 --batch-size 2 --grad-accum 4 --no-4bit

# 7. Eval (in-process, since we don't have vllm/tgi on MPS):
"$RUN_DIR/.venv/bin/python" "$RUN_DIR/run_eval.py"
```

## What would make this useful

The adapter is a smoke test, not a product. To turn it into something you'd actually want:

- **Bigger / better teacher.** `gpt-4o-mini` or Claude Haiku as the teacher would produce far cleaner synthetic pairs. Cost ~$1–3 for 500 pairs over this corpus.
- **More data.** 500–2000 pairs lets the adapter generalize beyond the seed documents.
- **Base model upgrade.** Qwen2.5-1.5B or 3B as the base, once you have the data to justify it.
- **A real eval.** Hand-write 30 questions, score with `--judge-url gpt-4o`. ROUGE-L misses the qualitative wins (and losses) that human/judge eval catches.
