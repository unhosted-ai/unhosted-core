#!/usr/bin/env python3
"""
Publish a trained LoRA adapter directory to the Hugging Face Hub.

Reads the adapter from --adapter, the model-card template from this
directory, fills in the placeholders the caller provided via flags,
and creates / updates the target repo. Idempotent: re-pushing the
same adapter to the same repo is a no-op apart from the README
update.

Usage:

  HF_TOKEN=hf_... python push_to_hub.py \\
    --adapter out/adapter \\
    --repo your-username/tinyllama-notes-v1 \\
    --base-model TinyLlama/TinyLlama-1.1B-Chat-v1.0 \\
    --task "answer questions about a small note collection" \\
    --n-pairs 4800 \\
    --epochs 3 \\
    --teacher gpt-4o-mini

What we DON'T do here:

  - Merge the adapter into the base. Adapters stay LoRA-shaped on
    the hub so consumers can swap them in/out at load time. If you
    want a merged model, run `merge_and_unload` separately and push
    the result to a different repo.
  - Re-run eval. The eval numbers in the model card come from --eval-*
    flags; the publisher's job is to surface what was already
    measured, not to re-measure.
  - Set the repo private. Default is public. Pass --private to flip.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
TEMPLATE_PATH = SCRIPT_DIR / "model-card.template.md"


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="push a trained LoRA adapter to HF Hub")
    p.add_argument("--adapter", type=Path, required=True,
                   help="path to the trained adapter directory")
    p.add_argument("--repo", required=True,
                   help="target HF repo id (e.g. 'your-username/tinyllama-notes-v1')")
    p.add_argument("--token", default=os.environ.get("HF_TOKEN", ""),
                   help="HF write token. Default: $HF_TOKEN.")
    p.add_argument("--private", action="store_true",
                   help="create the repo as private (default: public)")
    p.add_argument("--commit-message", default="Upload trained LoRA adapter")

    # Model-card placeholders. The script will fail loudly if any
    # required ones are missing — better than publishing a card full
    # of "{lora_r}" strings.
    p.add_argument("--base-model", required=True)
    p.add_argument("--task", required=True,
                   help="one-line summary of what this adapter is good at")
    p.add_argument("--n-pairs", type=int, required=True)
    p.add_argument("--data-provenance", default="synthetic, gen_data.py")
    p.add_argument("--training-hardware", default="see notes below")
    p.add_argument("--training-time", default="—")
    p.add_argument("--epochs", type=int, default=3)
    p.add_argument("--lr", type=float, default=2e-4)
    p.add_argument("--effective-batch", type=int, default=16)
    p.add_argument("--warmup", type=float, default=5.0,
                   help="warmup ratio as a percentage (5.0 == 5 percent)")
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--lora-alpha", type=int, default=32)
    p.add_argument("--compute-dtype", default="bfloat16")
    p.add_argument("--teacher", required=True,
                   help="teacher model id used for synthetic data")
    p.add_argument("--teacher-provenance", default="single chat-completion call per source document")
    p.add_argument("--data-artifact", default="data/train.jsonl (in this repo)")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--eval-metric-1", default="ROUGE-L F1")
    p.add_argument("--eval-metric-2", default="exact match")
    p.add_argument("--base-score-1", default="—")
    p.add_argument("--adapter-score-1", default="—")
    p.add_argument("--base-score-2", default="—")
    p.add_argument("--adapter-score-2", default="—")
    p.add_argument("--eval-set-description", default="held-out synthetic Q&A pairs (different seed)")
    p.add_argument("--n-eval", type=int, default=0)
    p.add_argument("--intended-use", default="answer questions grounded in the document set the adapter was trained on")
    p.add_argument("--limitations", default="performance outside the training-document distribution is no better than the base model")
    p.add_argument("--antipattern-1", default="open-ended chat unrelated to the training documents")
    p.add_argument("--antipattern-2", default="any task requiring tool use or function calling")

    p.add_argument("--dry-run", action="store_true",
                   help="render the model card and print to stdout; don't push")
    return p.parse_args()


def render_card(args: argparse.Namespace) -> str:
    if not TEMPLATE_PATH.exists():
        sys.exit(f"error: {TEMPLATE_PATH} missing")
    template = TEMPLATE_PATH.read_text(encoding="utf-8")
    # Compute derived placeholders that the user shouldn't have to pass.
    base_params_guess = {
        "TinyLlama/TinyLlama-1.1B-Chat-v1.0": "1.1B",
        "Qwen/Qwen2.5-0.5B-Instruct": "0.5B",
        "Qwen/Qwen2.5-1.5B-Instruct": "1.5B",
        "Qwen/Qwen2.5-7B-Instruct": "7B",
        "meta-llama/Llama-3.2-1B-Instruct": "1B",
        "meta-llama/Llama-3.2-3B-Instruct": "3B",
        "meta-llama/Llama-3.1-8B-Instruct": "8B",
    }
    base_params = base_params_guess.get(args.base_model, "?")
    trainable_params_estimate = {16: "~5–15M", 32: "~10–30M", 64: "~20–60M"}.get(args.lora_r, "varies")
    adapter_name = args.repo.split("/")[-1]

    mapping = {
        "adapter_name": adapter_name,
        "base_model": args.base_model,
        "base_params": base_params,
        "lora_r": args.lora_r,
        "lora_alpha": args.lora_alpha,
        "trainable_params": trainable_params_estimate,
        "n_pairs": args.n_pairs,
        "data_provenance": args.data_provenance,
        "training_hardware": args.training_hardware,
        "training_time": args.training_time,
        "task_summary": args.task,
        "intended_use": args.intended_use,
        "limitations": args.limitations,
        "antipattern_1": args.antipattern_1,
        "antipattern_2": args.antipattern_2,
        "teacher_model": args.teacher,
        "teacher_provenance": args.teacher_provenance,
        "epochs": args.epochs,
        "lr": args.lr,
        "effective_batch": args.effective_batch,
        "warmup": args.warmup,
        "compute_dtype": args.compute_dtype,
        "data_artifact": args.data_artifact,
        "seed": args.seed,
        "eval_metric_1": args.eval_metric_1,
        "eval_metric_2": args.eval_metric_2,
        "base_score_1": args.base_score_1,
        "adapter_score_1": args.adapter_score_1,
        "base_score_2": args.base_score_2,
        "adapter_score_2": args.adapter_score_2,
        "eval_set_description": args.eval_set_description,
        "n_eval": args.n_eval,
        "hf_repo": args.repo,
    }
    rendered = template
    for k, v in mapping.items():
        rendered = rendered.replace("{" + k + "}", str(v))
    # Sanity check: any remaining {placeholder}?
    if "{" in rendered and "}" in rendered:
        # Allow JSON examples in the template (they have {} too), so
        # look only for our known placeholder shape: alphanumeric +
        # underscores between braces.
        import re
        leftover = re.findall(r"\{[a-z_]+\}", rendered)
        if leftover:
            sys.exit(
                f"error: unfilled placeholders in model card: {sorted(set(leftover))}.\n"
                "Add a CLI flag for each (or extend push_to_hub.py's mapping)."
            )
    return rendered


def main() -> None:
    args = parse_args()
    if not args.adapter.exists() or not args.adapter.is_dir():
        sys.exit(f"error: --adapter {args.adapter} is not a directory")
    if not args.token and not args.dry_run:
        sys.exit("error: HF_TOKEN env var (or --token) is required for upload. "
                 "Use --dry-run to render the model card only.")

    card = render_card(args)

    if args.dry_run:
        print(card)
        return

    # Defer the huggingface-hub import so --dry-run / --help works
    # without the dep installed.
    try:
        from huggingface_hub import HfApi
    except ImportError:
        sys.exit("error: huggingface-hub is not installed. `pip install huggingface-hub`")

    api = HfApi(token=args.token)
    api.create_repo(repo_id=args.repo, private=args.private, exist_ok=True)

    # Write the rendered card into the adapter dir before uploading
    # so it travels with the rest of the artifacts.
    readme_path = args.adapter / "README.md"
    readme_path.write_text(card, encoding="utf-8")
    print(f"[push] wrote {readme_path}")

    print(f"[push] uploading {args.adapter} → {args.repo}")
    api.upload_folder(
        folder_path=str(args.adapter),
        repo_id=args.repo,
        commit_message=args.commit_message,
        ignore_patterns=["*.tmp", "checkpoint-*"],
    )
    print(f"[push] published https://huggingface.co/{args.repo}")


if __name__ == "__main__":
    main()
