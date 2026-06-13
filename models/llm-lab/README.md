# LLM Internals Lab

A tiny, **fully local** lab for *seeing* how a language model actually works.
It loads a small open model — **TinyLlama-1.1B-Chat** by default (~2.2 GB) — and
prints out the five things that make an LLM tick for any prompt you give it.

TinyLlama uses the same modern architecture as production chat models (RoPE,
RMSNorm, SwiGLU) and is instruction-tuned, so it behaves like a real assistant.
The lab is architecture-agnostic — pass `--model gpt2` (or `distilgpt2`) for a
smaller, faster, CPU-friendly model.

> Why not "copy the model you're using"? Hosted models (Claude, GPT, Gemini)
> don't expose their weights, so they can't be downloaded or copied. But you
> don't need them to *understand* LLMs — a small open model uses the exact same
> machinery (tokens → embeddings → attention → layers → next-token), and unlike
> a giant frozen API model, you can crack it open and watch every step.

## Run it

```bash
# from this folder — default is TinyLlama-1.1B-Chat
uv run llm_lab.py --prompt "What is the capital of France?"

# smaller/faster GPT-2, more candidates, a different attention head
uv run llm_lab.py --prompt "Once upon a time" --model gpt2 --topk 12 --head 5

# feed the prompt verbatim (skip the chat template)
uv run llm_lab.py --prompt "The capital of France is" --raw

# save PNG attention heatmaps
uv run llm_lab.py --prompt "2 + 2 =" --plot
```

Runs on Apple Silicon (MPS), CUDA, or plain CPU — auto-detected.

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--prompt` | a sample question | the text to analyze |
| `--model` | `TinyLlama/TinyLlama-1.1B-Chat-v1.0` | any HF causal LM |
| `--raw` | off | skip the chat template, feed prompt verbatim |
| `--topk` | 10 | how many next-token candidates to show |
| `--layer` / `--head` | 0 / 0 | which attention layer/head to display |
| `--gen` | 40 | tokens to generate |
| `--plot` | off | save attention heatmap PNGs |

## What each section shows

| Section | What you're looking at |
|---|---|
| **1. Tokenization** | How your text is chopped into integer *tokens* (usually word-pieces, not whole words). This is the model's actual input. |
| **2. Next-token prediction** | The probability distribution over the *entire vocabulary* for what comes next. The model never "knows" an answer — it ranks every possible next token. |
| **3. Logit lens** | The model's best guess at **each layer**. Early layers are vague; later layers commit. This is the closest thing to watching the model "reason." |
| **4. Attention** | For one layer/head, how much each token "looks at" every earlier token. GPT-2 is *causal*, so a token can only attend backwards. |
| **5. Generation** | Autoregressive decoding: predict one token, append it, repeat. This is literally how text comes out. |

## The mental model (in one paragraph)

Your text becomes **tokens** → each token becomes a **vector (embedding)** →
that stack of vectors flows through ~12 **transformer layers**, where
**attention** lets each token mix in information from earlier tokens and a
**feed-forward** network transforms it → the final vector is projected back onto
the vocabulary to get a **probability for every possible next token** → pick one,
append it, and do it all again. That loop, scaled up massively, is a chat model.

## Try these prompts to build intuition

- `"What is the capital of France?"` — watch the answer ("The"… "Paris") snap into
  place in the upper layers (logit lens).
- `"Write a haiku about the sea."` — see a chat model follow an instruction.
- `"def add(a, b):" --raw` — feed raw text and watch it complete code.
- Same prompt with `--model gpt2` vs TinyLlama — compare a base completion model
  against an instruction-tuned chat model.

## Next steps

- **Richer attention views:** `BertViz` gives interactive head-by-head diagrams.
- **Deeper interpretability:** `TransformerLens` / `nnsight` let you patch and ablate
  individual neurons and heads.
- **From scratch:** Karpathy's `nanoGPT` builds and trains a GPT in ~300 lines —
  the best way to *truly* get it. (Ties into this repo's `models/distill/` recipe.)
