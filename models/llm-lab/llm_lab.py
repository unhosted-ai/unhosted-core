"""
LLM Internals Lab
=================

Load a small, fully-open language model (GPT-2 by default) and *see* how it
works under the hood for a prompt you choose:

  1. TOKENIZATION      - how your text becomes integer tokens
  2. NEXT-TOKEN         - the probability distribution over what comes next
  3. LOGIT LENS         - the model's best guess at *each layer* (watch it decide)
  4. ATTENTION          - which earlier tokens each position attends to
  5. GENERATION         - autoregressive sampling, one token at a time

Everything runs locally on CPU or Apple-Silicon (MPS). Nothing is sent anywhere.

Usage
-----
    uv run llm_lab.py --prompt "The capital of France is"
    uv run llm_lab.py --prompt "Once upon a time" --model gpt2 --topk 10
    uv run llm_lab.py --prompt "2 + 2 =" --plot   # save attention/logit-lens PNGs
"""

from __future__ import annotations

import argparse

import torch
import torch.nn.functional as F
from transformers import AutoModelForCausalLM, AutoTokenizer


def pick_device() -> torch.device:
    """Prefer Apple MPS, then CUDA, then CPU."""
    if torch.backends.mps.is_available():
        return torch.device("mps")
    if torch.cuda.is_available():
        return torch.device("cuda")
    return torch.device("cpu")


def banner(title: str) -> None:
    print(f"\n{'=' * 64}\n{title}\n{'=' * 64}")


def load(model_name: str, device: torch.device):
    print(f"Loading '{model_name}' on {device} (first run downloads weights)...")
    tok = AutoTokenizer.from_pretrained(model_name)
    # eager attention is required to get attention weights out of Llama-style models
    model = AutoModelForCausalLM.from_pretrained(
        model_name,
        output_hidden_states=True,
        output_attentions=True,
        attn_implementation="eager",
        torch_dtype=torch.float32,
    )
    model.to(device).eval()
    n_params = sum(p.numel() for p in model.parameters())
    print(f"Loaded. {n_params / 1e6:.1f}M parameters, "
          f"{model.config.num_hidden_layers} layers, "
          f"{model.config.num_attention_heads} heads/layer.")
    return tok, model


def final_norm_and_head(model):
    """Return (final_norm, unembedding) for either GPT-2-style or Llama-style models.

    GPT-2:  model.transformer.ln_f + model.lm_head
    Llama:  model.model.norm       + model.lm_head
    """
    head = model.get_output_embeddings()  # lm_head for both families
    inner = getattr(model, "transformer", None) or getattr(model, "model", None)
    norm = getattr(inner, "ln_f", None) or getattr(inner, "norm", None)
    return norm, head


def show_tokenization(tok, prompt: str) -> None:
    banner("1. TOKENIZATION  —  your text -> integer tokens")
    ids = tok(prompt)["input_ids"]
    print(f"Prompt: {prompt!r}")
    print(f"{len(ids)} tokens:\n")
    for i, tid in enumerate(ids):
        piece = tok.decode([tid])
        print(f"  [{i:>2}] id={tid:<6} -> {piece!r}")
    print("\nNote: a 'token' is often a word-piece, not a whole word. "
          "Leading spaces are part of the token.")


def build_input_text(tok, prompt: str, chat: bool) -> str:
    """Wrap the prompt in the model's chat template when --chat is set and the
    tokenizer defines one (e.g. TinyLlama-Chat). Otherwise pass it through raw."""
    if chat and getattr(tok, "chat_template", None):
        return tok.apply_chat_template(
            [{"role": "user", "content": prompt}],
            tokenize=False,
            add_generation_prompt=True,
        )
    return prompt


@torch.no_grad()
def forward(model, tok, text: str, device):
    ids = tok(text, return_tensors="pt").to(device)
    out = model(**ids)
    return ids["input_ids"][0], out


def show_next_token(tok, ids, out, topk: int) -> None:
    banner(f"2. NEXT-TOKEN PREDICTION  —  top {topk} candidates")
    logits = out.logits[0, -1]            # logits for the position after the last token
    probs = F.softmax(logits, dim=-1)
    top = torch.topk(probs, topk)
    ctx = tok.decode(ids)
    print(f"Context: {ctx!r}\n")
    print(f"{'token':<16}{'prob':>10}   bar")
    for prob, tid in zip(top.values, top.indices):
        piece = tok.decode([tid])
        bar = "#" * int(prob.item() * 50)
        print(f"{piece!r:<16}{prob.item():>9.2%}   {bar}")
    print("\nThe model never 'knows' the answer — it outputs a probability "
          "distribution over the entire vocabulary every step.")


@torch.no_grad()
def show_logit_lens(tok, model, ids, out) -> None:
    """Project every layer's hidden state through the output head to see the
    model's intermediate 'guess' for the next token at each layer."""
    banner("3. LOGIT LENS  —  the model's best guess at EACH layer")
    # hidden_states: tuple of (num_layers + 1) tensors, each [1, seq, hidden]
    hidden = out.hidden_states
    # The final norm + unembedding (works for GPT-2-style AND Llama-style models)
    ln_f, unembed = final_norm_and_head(model)
    print(f"(position predicted: after token {tok.decode([ids[-1]])!r})\n")
    print(f"{'layer':<8}{'top guess':<16}{'prob':>8}")
    for layer_idx, h in enumerate(hidden):
        vec = h[0, -1]
        if ln_f is not None:
            vec = ln_f(vec)
        logits = unembed(vec)
        probs = F.softmax(logits, dim=-1)
        p, tid = torch.max(probs, dim=-1)
        tag = "embed" if layer_idx == 0 else f"L{layer_idx}"
        print(f"{tag:<8}{tok.decode([tid])!r:<16}{p.item():>7.1%}")
    print("\nWatch the guess sharpen as information flows up the layers — "
          "early layers are vague, later layers commit.")


def show_attention(tok, ids, out, layer: int, head: int, max_tokens: int = 16) -> None:
    banner(f"4. ATTENTION  —  layer {layer}, head {head}  (who attends to whom)")
    # attentions: tuple of num_layers tensors, each [1, heads, seq, seq]
    attn = out.attentions[layer][0, head]      # [seq, seq]
    n = len(ids)
    # Chat templates make long sequences; show only the last window so the grid
    # stays readable. The causal structure is identical anywhere in the sequence.
    start = max(0, n - max_tokens)
    if start:
        print(f"(showing last {max_tokens} of {n} tokens for readability)\n")
    sub = attn[start:, start:]
    toks = [tok.decode([t]) for t in ids[start:]]
    width = max(len(repr(t)) for t in toks) + 1
    print("Each row = a query token; values = how much it attends to each key "
          "token (rows sum to 1).\n")
    header = " " * (width + 2) + "".join(f"{i:>6}" for i in range(len(toks)))
    print(header)
    for qi, row in enumerate(sub):
        cells = "".join(f"{v.item():>6.2f}" for v in row)
        print(f"{repr(toks[qi]):<{width}} [{qi:>2}]{cells}")
    print("\nThese models are causal: a token can only attend to itself and "
          "tokens before it (upper-right is 0).")


@torch.no_grad()
def generate(tok, model, text: str, device, n: int) -> None:
    banner(f"5. GENERATION  —  sampling {n} tokens, one at a time")
    ids = tok(text, return_tensors="pt").to(device)["input_ids"]
    eos = tok.eos_token_id
    print(text, end="", flush=True)
    new_ids: list[int] = []
    prev = ""
    for _ in range(n):
        logits = model(ids).logits[0, -1]
        nxt = torch.argmax(logits)            # greedy for reproducibility
        if eos is not None and nxt.item() == eos:
            break
        new_ids.append(int(nxt))
        # Decode the whole generated run each step so word-piece spacing is
        # reconstructed correctly, then print only the newly added text.
        full = tok.decode(new_ids)
        print(full[len(prev):], end="", flush=True)
        prev = full
        ids = torch.cat([ids, nxt.view(1, 1)], dim=1)
    print("\n\n(greedy decoding shown; real chat models also sample with "
          "temperature/top-p for variety.)")


def maybe_plot(tok, model, ids, out, prompt: str) -> None:
    try:
        import matplotlib.pyplot as plt
    except ImportError:
        print("matplotlib not available; skipping --plot")
        return
    toks = [tok.decode([t]) for t in ids]

    # Attention grid for layer 0 across a few heads
    n_heads = min(4, out.attentions[0].shape[1])
    fig, axes = plt.subplots(1, n_heads, figsize=(4 * n_heads, 4))
    if n_heads == 1:
        axes = [axes]
    for h in range(n_heads):
        a = out.attentions[0][0, h].cpu().numpy()
        axes[h].imshow(a, cmap="viridis", vmin=0, vmax=1)
        axes[h].set_title(f"layer 0 head {h}")
        axes[h].set_xticks(range(len(toks)))
        axes[h].set_yticks(range(len(toks)))
        axes[h].set_xticklabels(toks, rotation=90, fontsize=7)
        axes[h].set_yticklabels(toks, fontsize=7)
    fig.suptitle(f"Attention — {prompt!r}")
    fig.tight_layout()
    fig.savefig("attention.png", dpi=120)
    print("Saved attention.png")


def main() -> None:
    ap = argparse.ArgumentParser(description="See how an LLM works, locally.")
    ap.add_argument("--prompt", default="What is the capital of France?")
    ap.add_argument("--model", default="TinyLlama/TinyLlama-1.1B-Chat-v1.0",
                    help="any HF causal LM, e.g. gpt2, distilgpt2, "
                         "TinyLlama/TinyLlama-1.1B-Chat-v1.0")
    ap.add_argument("--topk", type=int, default=10)
    ap.add_argument("--layer", type=int, default=0, help="attention layer to show")
    ap.add_argument("--head", type=int, default=0, help="attention head to show")
    ap.add_argument("--gen", type=int, default=40, help="tokens to generate")
    ap.add_argument("--raw", action="store_true",
                    help="skip the chat template; feed the prompt verbatim")
    ap.add_argument("--plot", action="store_true", help="save PNG visualizations")
    args = ap.parse_args()

    device = pick_device()
    tok, model = load(args.model, device)
    text = build_input_text(tok, args.prompt, chat=not args.raw)

    show_tokenization(tok, text)
    ids, out = forward(model, tok, text, device)
    show_next_token(tok, ids, out, args.topk)
    show_logit_lens(tok, model, ids, out)
    show_attention(tok, ids, out, args.layer, args.head)
    generate(tok, model, text, device, args.gen)
    if args.plot:
        maybe_plot(tok, model, ids, out, args.prompt)

    print("\nDone. Try a different --prompt, --model, --layer, or --head.")


if __name__ == "__main__":
    main()
