# Upstream contributions

Drafts of issues and PRs the project would like to submit to upstream projects we depend on. **These are not yet submitted** — they're paste-ready text the user reviews and files under their own GitHub identity.

Why drafted-but-not-filed: filing an issue or PR makes a public claim under your name on someone else's tracker. The maintainer's contribution flow requires explicit sign-off before that happens.

To submit one, copy the file's body into the target tracker's "new issue" or PR form. The header in each file says where it goes and what title to use.

| File | Target | What it does |
| --- | --- | --- |
| [`llama-cpp-rms-norm-blas.md`](llama-cpp-rms-norm-blas.md) | `ggml-org/llama.cpp` issues | Reports the BLAS + RMS_NORM abort in `rpc-server` we hit during VRAM-pool work, with repro + suggested fix. |
| [`homebrew-core-llama-cpp-rpc.md`](homebrew-core-llama-cpp-rpc.md) | `Homebrew/homebrew-core` PR | Adds `-DGGML_RPC=ON` to the official `llama.cpp` formula so users don't need our custom tap to run VRAM-pool. |
