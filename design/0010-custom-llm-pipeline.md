# 0010 — custom LLM pipeline: distil a specialist from open-source bases

**Status:** Accepted
**Captured:** 2026-05-19
**Target:** v0.1.x (pipeline), v0.2.x (CLI integration)

## Motivation

unhosted's value is running AI on hardware you own. Most users run a
general-purpose 7B model. A narrow specialist fine-tuned for *your* documents
and *your* workflow will outperform a 7B general model at a fraction of the
parameter count — and fit comfortably on hardware that can't run 7B.

The goal is **not** to build a better general model. The goal is to give every
user a one-command path from "I have some documents" to "I have a model that
knows them well." The teacher that generates the training data can be Claude,
a large open model, or the user's own running unhosted daemon. The student is
always a small open-source base the user can run locally.

## Decision

### Base models (student)

| Size | Model | When to use |
|------|-------|-------------|
| 1B   | `TinyLlama/TinyLlama-1.1B-Chat-v1.0` | Iteration, CI smoke tests |
| 3B   | `Qwen/Qwen2.5-3B-Instruct`           | Laptop / Raspberry Pi |
| 7B   | `Qwen/Qwen2.5-7B-Instruct`           | Consumer GPU (≥8 GB VRAM) |
| 8B   | `meta-llama/Llama-3.1-8B-Instruct`   | Consumer GPU, best accuracy |

Users pick based on their hardware. The training script's `--base-model` flag
handles all of them without code changes.

### Teacher models (data generation)

The teacher generates (question, answer) pairs from the user's documents.
Any of these work via a single `--teacher` flag in `pipeline.py` / `gen_data.py`:

| Teacher | How | Quality | Cost |
|---------|-----|---------|------|
| Local unhosted daemon | OpenAI-compat `/v1/chat/completions` | ★★★ | Free |
| `claude-opus-4-8` | Anthropic SDK (native) | ★★★★★ | ~$0.01/doc |
| `claude-sonnet-4-6` | Anthropic SDK (native) | ★★★★ | ~$0.003/doc |
| `gpt-4o` | OpenAI-compat | ★★★★ | ~$0.005/doc |

Claude is the recommended teacher when cost is acceptable: it produces tighter,
more grounded Q&A pairs and rarely hallucinates structure.

### Pipeline stages

```
docs/          →  gen_data.py  →  data/train.jsonl + data/test.jsonl
                                           ↓
                                    train.py (QLoRA SFT)
                                           ↓
                                     out/adapter/
                                           ↓
                                    eval.py → eval/report.jsonl
                                           ↓
                                 push_to_hub.py (optional)
```

`pipeline.py` is the one-command runner that wires all four stages together.

### Training method: QLoRA

Base weights stay frozen + 4-bit quantised (bitsandbytes). Only a LoRA adapter
(default r=16) trains. Memory stays under 8 GB for 7B models; the adapter is
~50 MB on disk vs a full 14 GB checkpoint.

### Integration with the rest of the org

- **agentic-ai**: real user→agent→tool conversations are the highest-quality
  training data. `pipeline.py --from-agent-logs <path>` converts captured
  agentic-ai logs to JSONL without a teacher call.
- **unhosted-plugins**: MCP interactions (Claude ↔ unhosted) are another
  harvest source for training pairs.
- **homebrew-unhosted**: the trained model is served by llama.cpp; the tap
  ships the RPC-enabled build for VRAM-pooling across machines.
- **unhosted-core CLI**: `unhosted distill` (slice 4) wraps `pipeline.py` so
  users never touch Python directly.

## Alternatives considered

| Option | Why not chosen |
|--------|----------------|
| Pretrain from scratch | $20K–$200K GPU cost; completely disproportionate |
| RLHF / PPO | Needs a reward model; adds infrastructure; SFT is sufficient for narrow tasks |
| Full fine-tune (no LoRA) | Stores a full 7B checkpoint per run vs 50 MB adapter |
| GPT-2 / older bases | Weaker starting point; Qwen2.5 / Llama3.1 reach better results faster |

## Implementation sketch

1. `gen_data.py` — add `--teacher claude-*` via native Anthropic SDK ✓ this spec
2. `pipeline.py` — end-to-end orchestrator ✓ this spec
3. `requirements.txt` — add `anthropic>=0.40` optional dep ✓ this spec
4. `unhosted distill` CLI subcommand — thin shell-out to pipeline.py (next)
5. `push_to_hub.py` — auto-tag model card with base + teacher + eval scores (future)

## Open questions

- [ ] Should `unhosted pull` support pulling custom adapters from HF Hub alongside base models? (ADR 0011 candidate)
- [ ] Merged checkpoint vs LoRA adapter for serving — merged is simpler but slower to produce.

## Out of scope

- Pretraining, RLHF, reward models, constitutional AI.
- Multi-GPU / DDP training orchestration.
- Producing a general-purpose chat model.
