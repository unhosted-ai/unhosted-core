# [draft] llama.cpp issue — rpc-server aborts when BLAS handles RMS_NORM

**Target:** https://github.com/ggml-org/llama.cpp/issues/new — pick "Bug" template.

**Suggested title:** `rpc-server: GGML_ASSERT in ggml_rms_norm when BLAS backend is enabled (GGML_RPC=ON + GGML_BLAS=ON)`

---

## Bug body (paste below)

### Name and Version

```
$ ./build/bin/llama-server --version
version: <output from your machine here — `llama-server --version`>
built with: <compiler version>
```

Reproduced on `llama.cpp` HEAD as of <YYYY-MM-DD> (commit `<short-sha>`).

### Operating systems

macOS 14+ (Apple Silicon, Metal). Linux untested, but the failure path is in graph-routing code that is platform-agnostic, so we expect it to reproduce wherever `GGML_BLAS=ON` is built alongside `GGML_RPC=ON`.

### GGML backends

- Metal (host machine)
- BLAS (Accelerate framework on macOS)
- RPC (this is the new path that exposes the bug)

### Which llama.cpp modules does this affect?

`rpc-server` (`tools/rpc/rpc-server.cpp`), via the graph routing in `ggml/src/ggml-backend.cpp`. The host-side `llama-server --rpc` is the trigger but the abort is in `rpc-server` on the remote.

### Problem description and steps to reproduce

Building llama.cpp with both `-DGGML_RPC=ON` and `-DGGML_BLAS=ON` produces an `rpc-server` that aborts at the first inference request from a `llama-server --rpc=…` orchestrator. The crash is `GGML_ASSERT` inside the RMS_NORM op when the graph router tries to dispatch the op via BLAS.

#### Reproduce

```bash
# 1. Build a stock llama.cpp with BLAS + RPC enabled (the combination
#    that's commonly produced by package managers).
git clone https://github.com/ggml-org/llama.cpp
cd llama.cpp
cmake -B build -DGGML_RPC=ON -DGGML_BLAS=ON
cmake --build build --target llama-server rpc-server -j

# 2. Pull any small GGUF model (TinyLlama-1.1B is sufficient).
huggingface-cli download TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf --local-dir .

# 3. Start rpc-server on one terminal:
./build/bin/rpc-server --host 127.0.0.1 --port 50052

# 4. Start llama-server pointing at it on another terminal:
./build/bin/llama-server -m tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf \
    --rpc 127.0.0.1:50052 --port 8080

# 5. Send any chat completion:
curl -sS -X POST http://127.0.0.1:8080/v1/chat/completions \
    -H "content-type: application/json" \
    -d '{"messages":[{"role":"user","content":"hi"}], "stream": false}'

# Observed: rpc-server aborts with GGML_ASSERT failure in
# ggml_rms_norm. llama-server returns 502 / connection error.
```

Output on rpc-server (paste your real trace here when filing):

```
GGML_ASSERT(...): failed
[exit, no further output]
```

### First Bad Commit

Not bisected. Reproduces on HEAD; the BLAS + RMS_NORM interaction has been in the codebase long enough that bisection is unlikely to point at a clear regression — the bug is in the *combination* with RPC, which is comparatively newer.

### Compile command

```
cmake -B build -DGGML_RPC=ON -DGGML_BLAS=ON
```

### Relevant log output

(paste your real stack trace when filing — `ulimit -c unlimited && lldb -c <core>` if needed)

### Workaround

Building rpc-server with `-DGGML_BLAS=OFF` resolves the crash:

```
cmake -B build -DGGML_RPC=ON -DGGML_BLAS=OFF
```

The unhosted-ai project ships a Homebrew tap formula (`unhosted-ai/unhosted/llama-cpp-rpc`) with `-DGGML_BLAS=OFF` for exactly this reason. We can drop the tap and use upstream once this is fixed.

### Suggested fix

The bug appears to be in the dispatch logic that chooses a backend for the RMS_NORM op when BLAS is registered. Either:

- BLAS should not be considered as a candidate backend for RMS_NORM (it isn't a BLAS-shaped op), or
- The fallback path should kick in when the chosen backend can't actually execute the op.

Happy to send a PR with a minimal repro test once the path is confirmed.

### Related

- VRAM-pool design in unhosted-core ([ADR-0009](https://github.com/unhosted-ai/unhosted-core/blob/main/design/0009-vram-pooling.md)) uses `rpc-server` to split model layers across LAN peers. This bug was the blocker that forced us to ship a custom Homebrew tap.
- The tap formula (one-line fix): https://github.com/unhosted-ai/homebrew-unhosted/blob/main/Formula/llama-cpp-rpc.rb
