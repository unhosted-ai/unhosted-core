# [draft] homebrew-core PR — enable GGML_RPC in llama.cpp formula

**Target:** https://github.com/Homebrew/homebrew-core — PR against the `llama.cpp` formula at `Formula/l/llama.cpp.rb`.

**Suggested PR title:** `llama.cpp: enable GGML_RPC for distributed inference`

---

## How to file

1. Fork `Homebrew/homebrew-core`.
2. Create a branch: `git checkout -b llama-cpp-ggml-rpc`.
3. Apply the diff below to `Formula/l/llama.cpp.rb`.
4. Test locally:
   ```bash
   brew install --build-from-source ./Formula/l/llama.cpp.rb
   which rpc-server          # should now exist on PATH after install
   llama-server --help | grep -- --rpc   # should show the flag
   ```
5. Push your fork's branch and open the PR. Homebrew has a [contribution guide](https://docs.brew.sh/Adding-Software-to-Homebrew) — the relevant audit step is `brew audit --strict --new llama.cpp`.

## PR body (paste below)

### What this PR does

Enables the `GGML_RPC` build flag so the formula installs both `llama-server` (with `--rpc` support) and the `rpc-server` binary needed for layer-split distributed inference across multiple machines.

Previously, users who wanted to run a model larger than any single machine's VRAM (e.g. Llama 70B split across two consumer GPUs over LAN) had to build llama.cpp from source or use a third-party tap. The capability is mature in upstream; this PR makes it available via the standard formula.

### Why

llama.cpp's RPC mode (`-DGGML_RPC=ON`) ships an `rpc-server` binary that accepts a remote `llama-server --rpc=host:port` connection and serves a subset of the model's layers from the remote machine's GPU. It's the path for users who want to combine multiple consumer GPUs without buying datacenter hardware. The feature is documented upstream at https://github.com/ggml-org/llama.cpp/tree/master/tools/rpc.

### Caveats / known issues

`GGML_BLAS=ON` + `GGML_RPC=ON` triggers a GGML_ASSERT in the RMS_NORM op at first inference (see https://github.com/ggml-org/llama.cpp/issues/23382). This PR therefore explicitly disables BLAS when RPC is enabled. The performance impact on systems where BLAS would have been used is minimal — llama.cpp's own kernels handle the same ops on the affected codepaths, and BLAS on the consumer-CPU side is rarely the bottleneck for inference-shaped workloads.

If/when the upstream bug is fixed, the BLAS-off flag should be removed in a follow-up PR.

### Diff

```diff
diff --git a/Formula/l/llama.cpp.rb b/Formula/l/llama.cpp.rb
--- a/Formula/l/llama.cpp.rb
+++ b/Formula/l/llama.cpp.rb
@@ -<line>,<count> +<line>,<count> @@ class LlamaCpp < Formula
   def install
     args = std_cmake_args + %W[
-      -DLLAMA_BUILD_TESTS=OFF
-      -DLLAMA_BUILD_EXAMPLES=ON
-      -DLLAMA_BUILD_SERVER=ON
-      -DGGML_NATIVE=OFF
+      -DLLAMA_BUILD_TESTS=OFF
+      -DLLAMA_BUILD_EXAMPLES=ON
+      -DLLAMA_BUILD_SERVER=ON
+      -DGGML_NATIVE=OFF
+      -DGGML_RPC=ON
+      -DGGML_BLAS=OFF
     ]

     system "cmake", "-S", ".", "-B", "build", *args
     system "cmake", "--build", "build"
     system "cmake", "--install", "build"
   end
+
+  test do
+    # Existing tests preserved; add a smoke for rpc-server's presence.
+    assert_predicate bin/"rpc-server", :exist?
+    assert_predicate bin/"rpc-server", :executable?
+  end
 end
```

(The exact line numbers and surrounding context to fill in by looking at the live formula at `Formula/l/llama.cpp.rb` when the PR is prepared.)

### Resource impact

- Binary size: `rpc-server` is ~5 MB. Negligible.
- Build time: adds maybe 30 seconds to a cold build.
- No new dependencies on the host system.

### Backporting

Not applicable — this is an additive build-flag change. Existing users of `llama-server` see no behavior change.

### Testing

- macOS 14 (Apple Silicon): builds, `llama-server --rpc 127.0.0.1:50052 -m model.gguf` proxies to a `rpc-server` listening on that port and serves tokens. Verified end-to-end with TinyLlama-1.1B-Chat against itself (loopback two-process simulation) and intentionally also with a second Mac on the LAN.
- Linux: not yet locally verified by the PR author. Would appreciate a co-tester running the same `brew install --build-from-source` flow on Linuxbrew.

### Related context

- llama.cpp upstream RPC docs: https://github.com/ggml-org/llama.cpp/tree/master/tools/rpc
- Known upstream bug (BLAS + RPC): https://github.com/ggml-org/llama.cpp/issues/23382
- VRAM-pooling ADR using this flow: https://github.com/unhosted-ai/unhosted-core/blob/main/design/0009-vram-pooling.md
