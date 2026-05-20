# Upstream contributions

Drafts of issues and PRs the project would like to submit to upstream projects we depend on. **These are not yet submitted** — they're paste-ready text the user reviews and files under their own GitHub identity.

Why drafted-but-not-filed: filing an issue or PR makes a public claim under your name on someone else's tracker. The maintainer's contribution flow requires explicit sign-off before that happens.

To submit one, copy the file's body into the target tracker's "new issue" or PR form. The header in each file says where it goes and what title to use.

| File | Status | Target | Summary |
| --- | --- | --- | --- |
| [`llama-cpp-rms-norm-blas.md`](llama-cpp-rms-norm-blas.md) | **FILED** as [llama.cpp#23382](https://github.com/ggml-org/llama.cpp/issues/23382) | `ggml-org/llama.cpp` issues | BLAS + RMS_NORM abort in `rpc-server`, with repro + suggested fix. Live and open. |
| [`homebrew-core-llama-cpp-rpc.md`](homebrew-core-llama-cpp-rpc.md) | **PARKED** — blocked on llama.cpp#23382 | `Homebrew/homebrew-core` PR (against `Formula/g/ggml.rb`) | Will add `-DGGML_RPC=ON` to ggml's formula once the upstream BLAS+RPC bug is resolved. Filing it now would ship a known crash. |
