# [parked] homebrew-core PR — enable GGML_RPC in ggml formula

**Status: PARKED, blocked on [ggml-org/llama.cpp#23382](https://github.com/ggml-org/llama.cpp/issues/23382) being resolved upstream.** Do not file in current form.

## Why parked

When this draft was first written (May 2026), it targeted `Formula/l/llama.cpp.rb` with a `+ -DGGML_RPC=ON + -DGGML_BLAS=OFF` diff. Both assumptions are now wrong:

1. **`ggml` is a separate Homebrew formula now.** The `llama.cpp` formula at `Formula/l/llama.cpp.rb` carries a literal maintainer comment: `# NOTE: reject all PRs that try to bundle ggml`. Build flags like `GGML_RPC` and `GGML_BLAS` belong on `Formula/g/ggml.rb`, not the llama.cpp formula.
2. **`ggml` already ships with `-DGGML_BLAS=ON`** alongside `-DGGML_BACKEND_DL=ON` (backends are dlopen'd plugins).
3. **Adding `-DGGML_RPC=ON` to ggml's formula today would ship a known crash** to every `brew install ggml && llama-server --rpc=...` user — they'd load both BLAS and RPC plugins and hit the bug filed at [#23382](https://github.com/ggml-org/llama.cpp/issues/23382).

The original draft's "disable BLAS to work around" approach worked for our own tap (`unhosted-ai/homebrew-unhosted`) because we're the sole consumer and our users self-select for the RPC use case. It would NOT work for homebrew-core — disabling BLAS in the official ggml formula would regress CPU performance for every existing user, the vast majority of whom don't use RPC at all.

## When to unpark

When ggml-org/llama.cpp#23382 is one of:

- **Fixed (closed as resolved)** in a tagged llama.cpp release. The fix lives in ggml's graph dispatcher, so the same release of ggml that picks up the upstream tag will carry the fix. After Homebrew's ggml formula bumps to that tag (their version bumper runs ~every 10 llama.cpp tags), file the PR adding `-DGGML_RPC=ON`. With the bug gone, `GGML_BACKEND_DL` keeps everything isolated and there's no regression risk.

- **Confirmed-not-a-bug by maintainers** with guidance like "the user must build with one of `GGML_BLAS=OFF` or `GGML_RPC=OFF`." In that case, adding GGML_RPC to homebrew-core is a non-starter and this draft can be deleted.

- **Worked around in upstream cmake** (e.g., `GGML_RPC=ON` automatically disables BLAS dispatch for unsupported ops). Same as the "fixed" path.

## Live formula state (snapshot, 2026-05-20)

For when future-you re-reads this:

```ruby
# Formula/l/llama.cpp.rb (current upstream)
class LlamaCpp < Formula
  # ... depends_on "ggml" — NO ggml flags here ...
  def install
    args = %W[
      -DBUILD_SHARED_LIBS=ON
      -DCMAKE_INSTALL_RPATH=#{rpath}
      -DLLAMA_ALL_WARNINGS=OFF
      -DLLAMA_BUILD_TESTS=OFF
      -DLLAMA_OPENSSL=ON
      -DLLAMA_USE_SYSTEM_GGML=ON
    ]
    # ...
  end
end
```

```ruby
# Formula/g/ggml.rb (current upstream)
class Ggml < Formula
  def install
    args = %W[
      -DBUILD_SHARED_LIBS=ON
      -DCMAKE_INSTALL_RPATH=#{rpath}
      -DGGML_ALL_WARNINGS=OFF
      -DGGML_BACKEND_DIR=#{libexec}
      -DGGML_BACKEND_DL=ON      # ← backends are dlopen'd plugins
      -DGGML_BLAS=ON            # ← already on
      -DGGML_BUILD_EXAMPLES=OFF
      -DGGML_BUILD_TESTS=OFF
      -DGGML_CCACHE=OFF
      -DGGML_LTO=ON
      -DGGML_NATIVE=OFF
    ]
    args += %w[-DGGML_BLAS_VENDOR=OpenBLAS -DGGML_VULKAN=ON] if OS.linux?
    # ... no GGML_RPC ← this is what the eventual PR adds ...
  end
end
```

## Open secondary question

Whether `rpc-server` (the binary in `tools/rpc/` of the llama.cpp source) is actually built by the current Homebrew install of llama.cpp. The current install line is just `cmake --install build` with no `LLAMA_BUILD_*` flags, so it ships whatever the upstream CMakeLists builds by default. If `rpc-server` isn't in the bottled binary set, a second PR (against `Formula/l/llama.cpp.rb`) may be needed to enable it. Worth verifying before filing the ggml PR.

## What the eventual PR will look like

When unblocked, the homebrew-core PR is small:

```diff
--- a/Formula/g/ggml.rb
+++ b/Formula/g/ggml.rb
@@ -<line>,<count> +<line>,<count> @@ class Ggml < Formula
       -DGGML_LTO=ON
       -DGGML_NATIVE=OFF
+      -DGGML_RPC=ON
     ]
```

PR body — short and factual, no LLM polish needed:

```
Enables the RPC backend in ggml so that downstream consumers (llama.cpp's
--rpc flag, etc.) can use distributed-inference setups via `brew install`
instead of building from source.

Per GGML_BACKEND_DL=ON (already enabled), the RPC backend lands as a
separately-loadable plugin (libggml-rpc.<dylib|so>). Users who don't
use RPC see no behavior change.

Upstream bug ggml-org/llama.cpp#23382 (RPC + BLAS abort in RMS_NORM)
was resolved in <commit>, included in the current bottled ggml release.
```

That's the whole thing. The current ~1500 word draft was overdoing it.

## What we ship in the meantime

Our own tap (`unhosted-ai/homebrew-unhosted`) ships `llama-cpp-rpc` with `-DGGML_RPC=ON -DGGML_BLAS=OFF`. That formula stays for as long as the upstream bug isn't fixed; it's the only known way to get a working RPC build on macOS via Homebrew today. Users following our docs install both `brew install llama.cpp` (for normal use) and `brew install unhosted-ai/unhosted/llama-cpp-rpc` (for VRAM-pooling).
