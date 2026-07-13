#!/usr/bin/env python3
"""
Turn a trained LoRA adapter into a runnable GGUF — the step between
"training finished" and "I can load this in LM Studio / llama.cpp".

The distillation recipe leaves you with a LoRA adapter (a small diff over
a base model). That's not directly loadable by GGUF runtimes. This script
runs the three steps that close the gap:

  1. merge      — fold the adapter into the base model's weights, producing
                  a standalone model.
  2. convert    — convert that merged model to GGUF (F16) via llama.cpp's
                  convert_hf_to_gguf.py.
  3. quantize   — shrink the F16 GGUF to a smaller, faster quant (default
                  Q4_K_M) with llama.cpp's llama-quantize.

It can also drop the finished GGUF straight into LM Studio's models folder
so it shows up in the picker (--install-lmstudio).

llama.cpp is required for steps 2 and 3. Point at it with --llama-cpp (a
source checkout containing convert_hf_to_gguf.py and a built
llama-quantize), or set $LLAMA_CPP_DIR. On macOS, `brew install llama.cpp`
provides llama-quantize on PATH; the converter script ships only in the
source tree, so a checkout is the reliable path.

Example (matches how Helmsman was built):

  python export_gguf.py \\
      --adapter runs/my-run/adapter \\
      --base-model Qwen/Qwen3-4B-Instruct-2507 \\
      --name helmsman-4b \\
      --quant Q4_K_M \\
      --install-lmstudio
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="merge a LoRA adapter into its base and export a runnable GGUF",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--adapter", type=Path, required=True,
                   help="Path to the trained LoRA adapter directory.")
    p.add_argument("--base-model", required=True,
                   help="HF id (or local path) of the base the adapter was trained on.")
    p.add_argument("--name", required=True,
                   help="Output model name (used for filenames, e.g. 'helmsman-4b').")
    p.add_argument("--out-dir", type=Path, default=None,
                   help="Where to write merged model + GGUFs. Default: <adapter>/../export.")
    p.add_argument("--quant", default="Q4_K_M",
                   help="Quantization type for llama-quantize (default: Q4_K_M). "
                        "Use F16 to skip quantization and keep full precision.")
    p.add_argument("--llama-cpp", type=Path, default=os.environ.get("LLAMA_CPP_DIR"),
                   help="Path to a llama.cpp checkout (has convert_hf_to_gguf.py and "
                        "a built llama-quantize). Default: $LLAMA_CPP_DIR.")
    p.add_argument("--install-lmstudio", action="store_true",
                   help="Copy the final GGUF into LM Studio's models folder "
                        "(~/.lmstudio/models/<publisher>/<name>/).")
    p.add_argument("--publisher", default="local",
                   help="Publisher folder under LM Studio models (default: 'local').")
    p.add_argument("--keep-merged", action="store_true",
                   help="Keep the merged full-precision model (for HF upload / further "
                        "training). Default: removed after GGUF conversion to save space.")
    return p.parse_args()


def run(cmd: list[str], **kwargs) -> None:
    print(f"\n[export] $ {' '.join(str(c) for c in cmd)}")
    result = subprocess.run(cmd, **kwargs)
    if result.returncode != 0:
        sys.exit(f"[export] step failed (exit {result.returncode})")


def merge_adapter(base_model: str, adapter: Path, merged_dir: Path) -> None:
    """Fold the LoRA adapter into the base weights -> a standalone model."""
    print(f"[export] merging adapter {adapter} into {base_model}")
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from peft import PeftModel

    tok = AutoTokenizer.from_pretrained(adapter, use_fast=True)
    base = AutoModelForCausalLM.from_pretrained(base_model, torch_dtype=torch.float16)
    model = PeftModel.from_pretrained(base, adapter)
    model = model.merge_and_unload()
    merged_dir.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(merged_dir, safe_serialization=True)
    tok.save_pretrained(merged_dir)
    print(f"[export] merged model -> {merged_dir}")


# Common places a user is likely to have cloned llama.cpp. Checked when
# --llama-cpp / $LLAMA_CPP_DIR isn't given, so the tool "just works" for the
# common case instead of failing on a fresh setup.
def _autodetect_llama_cpp() -> Path | None:
    candidates = [
        Path.home() / "llama.cpp",
        Path.home() / "src" / "llama.cpp",
        Path.home() / "code" / "llama.cpp",
        Path("/tmp/llama.cpp"),
        Path("llama.cpp"),  # cwd
    ]
    for c in candidates:
        if (c / "convert_hf_to_gguf.py").is_file():
            return c
    return None


def find_converter(llama_cpp: Path | None) -> Path:
    # Explicit path wins; otherwise try to auto-detect a checkout.
    search = [llama_cpp] if llama_cpp else []
    auto = _autodetect_llama_cpp()
    if auto:
        search.append(auto)
    for base in search:
        conv = Path(base) / "convert_hf_to_gguf.py"
        if conv.is_file():
            if not llama_cpp:
                print(f"[export] auto-detected llama.cpp at {base}")
            return conv
    sys.exit(
        "error: convert_hf_to_gguf.py not found. Pass --llama-cpp <checkout> "
        "(or set $LLAMA_CPP_DIR) pointing at a llama.cpp source tree.\n"
        "  git clone https://github.com/ggerganov/llama.cpp\n"
        "(auto-checked ~/llama.cpp, ~/src/llama.cpp, ~/code/llama.cpp, "
        "/tmp/llama.cpp, ./llama.cpp)"
    )


def find_quantizer(llama_cpp: Path | None) -> str:
    """Prefer a built llama-quantize in the checkout; fall back to PATH."""
    bases = [llama_cpp] if llama_cpp else []
    auto = _autodetect_llama_cpp()
    if auto:
        bases.append(auto)
    for base in bases:
        for cand in (Path(base) / "build/bin/llama-quantize",
                     Path(base) / "llama-quantize"):
            if cand.is_file():
                return str(cand)
    # PATH (e.g. `brew install llama.cpp` puts llama-quantize on PATH).
    found = shutil.which("llama-quantize")
    if found:
        return found
    sys.exit(
        "error: llama-quantize not found. Build it in your llama.cpp checkout "
        "(cmake -B build && cmake --build build --target llama-quantize) or "
        "install llama.cpp so it's on PATH."
    )


def main() -> None:
    args = parse_args()

    if not args.adapter.is_dir():
        sys.exit(f"error: --adapter {args.adapter} is not a directory")

    out_dir = args.out_dir or (args.adapter.parent / "export")
    out_dir.mkdir(parents=True, exist_ok=True)
    merged_dir = out_dir / f"{args.name}-merged"
    f16_gguf = out_dir / f"{args.name}-f16.gguf"
    final_gguf = out_dir / f"{args.name}-{args.quant}.gguf"

    # 1. Merge.
    merge_adapter(args.base_model, args.adapter, merged_dir)

    # 2. Convert merged -> F16 GGUF.
    converter = find_converter(args.llama_cpp)
    run([sys.executable, str(converter), str(merged_dir),
         "--outfile", str(f16_gguf), "--outtype", "f16"])

    # 3. Quantize (unless the user asked for F16, in which case F16 is final).
    if args.quant.upper() == "F16":
        shutil.copyfile(f16_gguf, final_gguf)
    else:
        quantizer = find_quantizer(args.llama_cpp)
        run([quantizer, str(f16_gguf), str(final_gguf), args.quant])

    print(f"\n[export] GGUF ready: {final_gguf}")

    # Optional: install into LM Studio.
    if args.install_lmstudio:
        dest_dir = Path.home() / ".lmstudio" / "models" / args.publisher / args.name
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / final_gguf.name
        shutil.copyfile(final_gguf, dest)
        print(f"[export] installed into LM Studio: {dest}")
        print("[export] open LM Studio — the model now appears in the picker.")

    # Clean up the bulky merged model unless asked to keep it.
    if not args.keep_merged:
        shutil.rmtree(merged_dir, ignore_errors=True)
        print(f"[export] removed merged model {merged_dir} (pass --keep-merged to retain)")

    print("\n[export] done. Run it:")
    print(f"  llama-cli -m {final_gguf} -p \"your prompt\"   # or load it in LM Studio")


if __name__ == "__main__":
    main()
