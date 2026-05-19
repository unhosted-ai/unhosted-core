#!/usr/bin/env python3
"""
QLoRA SFT loop for the unhosted distillation recipe.

Defaults target TinyLlama-1.1B-Chat-v1.0 because it's the smallest
sane Llama-architecture base — runs to convergence in ~20 min on a
single 4090, ~30 min on M2 MPS, and fits well under 8 GB VRAM with
4-bit quantization. Production specialists would start bigger
(Qwen2.5-7B, Llama-3.1-8B); the script's flags (--base-model, --lora-r,
--epochs) cover the upgrade path without code changes.

The script intentionally has two modes:

  python train.py --data data/train.jsonl --out out/adapter
    -> SFT loop. Loads base + tokenizer, adds a LoRA adapter,
       runs SFTTrainer over the JSONL, saves the adapter to --out.

  python train.py --inference --adapter out/adapter --prompt "..."
    -> Load base + adapter, generate, print. Smoke test only;
       eval.py (slice 3) will be the real harness.

Data format: JSONL, one object per line, with `prompt` and `response`
string fields. Anything else in the object is ignored. Example:
  {"prompt": "what is the capital of France?", "response": "Paris."}
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# Most of the heavy imports happen inside main() so --help is fast and
# import errors are surfaced with context. The user might be running
# on a fresh venv where bitsandbytes isn't installed (e.g. Apple
# Silicon) — we tolerate that and fall back.

DEFAULT_BASE = "TinyLlama/TinyLlama-1.1B-Chat-v1.0"
DEFAULT_CHAT_TEMPLATE = (
    # TinyLlama-Chat-v1.0 uses ChatML-flavoured tokens. Keep the
    # template explicit rather than relying on the tokenizer's default
    # so the same script works against any Llama-family base.
    "{% for m in messages %}"
    "<|im_start|>{{ m['role'] }}\n{{ m['content'] }}<|im_end|>\n"
    "{% endfor %}"
    "{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}"
)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="unhosted distill SFT trainer")
    p.add_argument("--base-model", default=DEFAULT_BASE,
                   help=f"HF model id of the base. Default: {DEFAULT_BASE}")
    p.add_argument("--data", type=Path,
                   help="JSONL with {prompt, response} per line.")
    p.add_argument("--out", type=Path, default=Path("out/adapter"),
                   help="Where to save the trained LoRA adapter.")
    p.add_argument("--epochs", type=int, default=3)
    p.add_argument("--batch-size", type=int, default=2,
                   help="Per-device train batch. Bump grad-accum if memory-bound.")
    p.add_argument("--grad-accum", type=int, default=8,
                   help="Gradient accumulation steps. Effective batch = batch-size * grad-accum.")
    p.add_argument("--learning-rate", type=float, default=2e-4)
    p.add_argument("--max-seq-len", type=int, default=2048)
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--lora-alpha", type=int, default=32)
    p.add_argument("--lora-dropout", type=float, default=0.05)
    p.add_argument("--no-4bit", action="store_true",
                   help="Skip bitsandbytes 4-bit quantization. Required on macOS/MPS.")
    p.add_argument("--seed", type=int, default=42)

    # Inference mode (smoke test, not a real eval — that's slice 3).
    p.add_argument("--inference", action="store_true",
                   help="Skip training; load adapter and generate from --prompt.")
    p.add_argument("--adapter", type=Path,
                   help="Path to a saved LoRA adapter (with --inference).")
    p.add_argument("--prompt", type=str,
                   help="Inference prompt (with --inference).")
    p.add_argument("--max-new-tokens", type=int, default=256)

    return p.parse_args()


def load_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        sys.exit(f"error: {path} does not exist. Supply a JSONL of {{prompt, response}} pairs.")
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
                sys.exit(f"error: {path}:{i}: missing 'prompt' or 'response' field")
            rows.append({"prompt": obj["prompt"], "response": obj["response"]})
    if not rows:
        sys.exit(f"error: {path} is empty")
    return rows


def format_for_sft(rows: list[dict], tokenizer) -> list[dict]:
    """Convert {prompt, response} into the chat template the tokenizer
    expects. SFTTrainer needs a single "text" field per row containing
    the fully-rendered conversation (the supervision signal is the
    whole sequence; we don't bother masking the prompt out)."""
    out: list[dict] = []
    for r in rows:
        messages = [
            {"role": "user", "content": r["prompt"]},
            {"role": "assistant", "content": r["response"]},
        ]
        text = tokenizer.apply_chat_template(
            messages, tokenize=False, add_generation_prompt=False
        )
        out.append({"text": text})
    return out


def detect_dtype() -> str:
    """Pick a sensible torch dtype for the platform. We don't import
    torch here yet — return a string the trainer can map."""
    try:
        import torch  # noqa: WPS433
    except ImportError:
        sys.exit("error: torch is not installed. See requirements.txt header.")
    if torch.cuda.is_available():
        # bf16 if Ampere+ (cc>=8), fp16 otherwise.
        cc = torch.cuda.get_device_capability(0)[0]
        return "bfloat16" if cc >= 8 else "float16"
    if hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
        # Use bf16 on MPS. fp16's range is too narrow for SFT — we hit
        # gradient overflow → NaN within the first few steps of a real
        # training run, despite "successfully" completing the loop with
        # mean_token_accuracy=0 (the canary). bf16 has fp32's exponent
        # range so gradients survive; supported on M-series Macs since
        # macOS 14. If you're on older macOS, pass --force-fp32.
        return "bfloat16"
    return "float32"


def build_model_and_tokenizer(args, dtype: str):
    """Load the base model and tokenizer. Returns (model, tokenizer)."""
    from transformers import AutoModelForCausalLM, AutoTokenizer
    import torch

    tokenizer = AutoTokenizer.from_pretrained(args.base_model, use_fast=True)
    if tokenizer.pad_token is None:
        # Llama tokenizers don't ship a pad token; reuse eos. This is
        # fine for SFT but DON'T do it for inference loops that need to
        # distinguish padding from "real" eos.
        tokenizer.pad_token = tokenizer.eos_token
    if tokenizer.chat_template is None:
        tokenizer.chat_template = DEFAULT_CHAT_TEMPLATE

    quant_cfg = None
    if not args.no_4bit:
        try:
            from transformers import BitsAndBytesConfig
            quant_cfg = BitsAndBytesConfig(
                load_in_4bit=True,
                bnb_4bit_quant_type="nf4",
                bnb_4bit_compute_dtype=getattr(torch, dtype),
                bnb_4bit_use_double_quant=True,
            )
        except Exception as e:
            print(f"note: bitsandbytes unavailable ({e}); falling back to {dtype} weights.")
            quant_cfg = None

    model = AutoModelForCausalLM.from_pretrained(
        args.base_model,
        quantization_config=quant_cfg,
        torch_dtype=getattr(torch, dtype),
        device_map="auto" if torch.cuda.is_available() else None,
    )
    # Gradient checkpointing trades compute for memory — almost always
    # the right call when training adapters on a small GPU.
    model.config.use_cache = False
    model.gradient_checkpointing_enable()
    return model, tokenizer


def train(args) -> None:
    from datasets import Dataset
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from trl import SFTConfig, SFTTrainer

    dtype = detect_dtype()
    print(f"[distill] dtype={dtype} base={args.base_model}")
    model, tokenizer = build_model_and_tokenizer(args, dtype)

    if not args.no_4bit:
        model = prepare_model_for_kbit_training(model)

    lora_cfg = LoraConfig(
        r=args.lora_r,
        lora_alpha=args.lora_alpha,
        lora_dropout=args.lora_dropout,
        bias="none",
        task_type="CAUSAL_LM",
        # Default targets cover Llama-family attention + MLP projections.
        # Adjust via the script if you swap to a non-Llama base.
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                        "gate_proj", "up_proj", "down_proj"],
    )
    model = get_peft_model(model, lora_cfg)
    model.print_trainable_parameters()

    rows = load_jsonl(args.data)
    print(f"[distill] loaded {len(rows)} training pairs from {args.data}")
    formatted = format_for_sft(rows, tokenizer)
    ds = Dataset.from_list(formatted)

    args.out.mkdir(parents=True, exist_ok=True)
    sft_cfg = SFTConfig(
        output_dir=str(args.out),
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        gradient_accumulation_steps=args.grad_accum,
        learning_rate=args.learning_rate,
        lr_scheduler_type="cosine",
        warmup_ratio=0.05,
        max_length=args.max_seq_len,
        logging_steps=10,
        save_strategy="epoch",
        save_total_limit=2,
        bf16=(dtype == "bfloat16"),
        fp16=(dtype == "float16"),
        seed=args.seed,
        report_to="none",
        packing=False,
        dataset_text_field="text",
    )
    trainer = SFTTrainer(
        model=model,
        args=sft_cfg,
        train_dataset=ds,
        processing_class=tokenizer,
    )
    trainer.train()
    trainer.save_model(str(args.out))
    tokenizer.save_pretrained(str(args.out))
    print(f"[distill] adapter saved to {args.out}")


def inference(args) -> None:
    if args.adapter is None or args.prompt is None:
        sys.exit("error: --inference requires --adapter and --prompt")

    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from peft import PeftModel

    dtype = detect_dtype()
    tokenizer = AutoTokenizer.from_pretrained(args.adapter, use_fast=True)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    if tokenizer.chat_template is None:
        tokenizer.chat_template = DEFAULT_CHAT_TEMPLATE

    base = AutoModelForCausalLM.from_pretrained(
        args.base_model,
        torch_dtype=getattr(torch, dtype),
        device_map="auto" if torch.cuda.is_available() else None,
    )
    model = PeftModel.from_pretrained(base, args.adapter)
    model.eval()

    inputs = tokenizer.apply_chat_template(
        [{"role": "user", "content": args.prompt}],
        tokenize=True,
        add_generation_prompt=True,
        return_tensors="pt",
    )
    if torch.cuda.is_available():
        inputs = inputs.to("cuda")
    with torch.no_grad():
        out = model.generate(
            inputs,
            max_new_tokens=args.max_new_tokens,
            do_sample=False,
            pad_token_id=tokenizer.eos_token_id,
        )
    text = tokenizer.decode(out[0][inputs.shape[1]:], skip_special_tokens=True)
    print(text.strip())


def main() -> None:
    args = parse_args()
    # Silence the worst of HF's startup noise unless the user really wants it.
    os.environ.setdefault("TRANSFORMERS_VERBOSITY", "warning")
    os.environ.setdefault("DATASETS_VERBOSITY", "warning")
    if args.inference:
        inference(args)
    else:
        if args.data is None:
            sys.exit("error: --data is required for training (or pass --inference)")
        train(args)


if __name__ == "__main__":
    main()
