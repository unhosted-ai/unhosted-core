# DRAFT Homebrew formula for an RPC-enabled llama.cpp build.
#
# NOT yet published. Lives here while we decide between:
#  a) submit `-DGGML_RPC=ON` upstream to homebrew-core (smaller change,
#     uncertain acceptance timeline)
#  b) carry our own tap at `unhosted-ai/homebrew-unhosted` until upstream
#     lands the flag (full control, ongoing maintenance cost)
#
# If we go with (b): create the repo `unhosted-ai/homebrew-unhosted`,
# move this file to `Formula/llama.cpp-rpc.rb`, drop the `-rpc` suffix
# in the class name to match Homebrew's expected `Llama.cpp` -> filename
# convention if we want to shadow the upstream. (We probably don't want
# to shadow — naming the binary distinctly avoids PATH order surprises
# for users who also have the upstream installed.)
#
# Version + url + sha256 are templated against llama.cpp 9090 to match
# what the user's existing Homebrew install has. Bump in lockstep with
# upstream until they ship RPC by default and we can deprecate this.

class LlamaCppRpc < Formula
  desc "Inference of LLMs in pure C/C++ — RPC-enabled build for unhosted VRAM-pooling"
  homepage "https://github.com/ggerganov/llama.cpp"
  url "https://github.com/ggerganov/llama.cpp/archive/refs/tags/b9090.tar.gz"
  # sha256 "..."   # filled in when we cut the formula
  license "MIT"
  head "https://github.com/ggerganov/llama.cpp.git", branch: "master"

  depends_on "cmake" => :build
  depends_on "openssl@3"
  depends_on "curl"

  # Distinct binary names so this formula can coexist with the upstream
  # `llama.cpp` install without PATH conflicts. The unhosted daemon
  # looks for both flavors at startup.
  def install
    args = std_cmake_args + %W[
      -DBUILD_SHARED_LIBS=ON
      -DCMAKE_INSTALL_RPATH=#{rpath}
      -DLLAMA_ALL_WARNINGS=OFF
      -DLLAMA_BUILD_TESTS=OFF
      -DLLAMA_OPENSSL=ON
      -DLLAMA_USE_SYSTEM_GGML=OFF
      -DGGML_RPC=ON
    ]
    # On Apple Silicon we want Metal. Upstream defaults to ON on macOS;
    # asserting it explicitly here so a non-default env doesn't silently
    # produce a CPU-only build.
    args << "-DGGML_METAL=ON" if OS.mac?

    system "cmake", "-S", ".", "-B", "build", *args
    system "cmake", "--build", "build", "--config", "Release"
    system "cmake", "--install", "build", "--config", "Release", "--prefix", prefix

    # Rename the orchestrator and the layer-host binaries so a user who
    # also has the upstream formula installed doesn't get PATH-ordering
    # surprises. Unhosted's `vram-pool start` looks for these names
    # specifically.
    (bin/"llama-server-rpc").write_env_script bin/"llama-server", {}
    (bin/"rpc-server-llama").write_env_script bin/"rpc-server", {} if (bin/"rpc-server").exist?
  end

  test do
    # Sanity: the binary has the `--rpc` flag we built this formula for.
    output = shell_output("#{bin}/llama-server --help 2>&1")
    assert_match "--rpc", output, "llama-server build is missing --rpc support"

    # rpc-server is what makes a peer a layer host. Bail if missing.
    assert_predicate bin/"rpc-server", :exist?,
      "rpc-server binary not built — check -DGGML_RPC=ON took effect"
  end
end
