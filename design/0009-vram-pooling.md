# 0009 — VRAM-pooling via llama.cpp RPC (the deep-tech bet)

**Status:** Draft. No code yet. ADR is the contract before implementation.
**Captured:** 2026-05-15
**Target:** First useful slice in v0.1.0; full feature multi-quarter.

## The promise we're cashing

unhosted's tagline is *"AI on hardware you own."* v0.0.1–v0.0.25 implemented every supporting piece — daemon, cluster discovery, secured tunnel, private memory loop — except the headline. A user with an M2 Air (16 GB), a Linux box with a 4090 (24 GB), and a Raspberry Pi 5 (8 GB) currently runs a separate model on each machine, picked by whichever one has enough RAM. The promise is that *one inference call spans all three* and a 48-GB-class model becomes usable. That is what VRAM-pooling delivers, and nothing else on the roadmap does.

## What llama.cpp already gives us

`llama.cpp` has supported distributed inference since mid-2024 via two binaries:

- **`rpc-server`** — runs on a *layer host*. Exposes a TCP endpoint that the orchestrator calls to evaluate one or more transformer layers using that machine's GPU/CPU. One per host.
- **`llama-server --rpc <addr1>,<addr2>...`** — the *orchestrator*. Loads the model definition, splits layers across the configured backends (local GPU + one or more `rpc-server` peers), runs inference, returns tokens.

Both require llama.cpp built with `-DGGML_RPC=ON`. The Homebrew formula does **not** ship with RPC enabled as of llama.cpp 9090. This is the single biggest non-design friction in the whole feature — users need a custom build until upstream changes its defaults.

**Implication:** unhosted does **not** need to implement layer routing, weight sharding, tensor transport, or quantization compatibility. Those are upstream's problem. What unhosted owns is everything *around* the RPC mode — discovery, topology, orchestration, observability, failure handling, and the UX that hides those four concerns from the user.

## What unhosted owns

### 1. Capability discovery (peer-side)

The daemon on each peer probes its local llama.cpp for RPC capability at startup. Two signals:

- The presence of the `rpc-server` binary on `$PATH`.
- The presence of `--rpc` in `llama-server --help`.

Result is published in `GET /v1/status` as `upstream.rpc_capable: bool` (new field, defaulted-false for backwards compat). Discovery layer already enumerates LAN peers (mDNS) — they expose `/v1/status`, so the orchestrator can compose a list of RPC-capable peers without new transport.

### 2. Topology negotiation (orchestrator-side)

When the user opts into VRAM-pooling (env var or CLI flag, see §UX), the daemon assembles a *plan*:

- Determine which machine is the orchestrator. Default: the one the user ran `unhosted vram-pool start` on. Optionally `--orchestrator <peer>`.
- For every other RPC-capable peer in the plan, mark them as a *layer host*. Their daemon will spawn (or supervise) an `rpc-server` process on a deterministic port (default 50052 to stay clear of llama.cpp's own conventions but configurable).
- The orchestrator's daemon spawns its `llama-server` with `--rpc <peer1:port>,<peer2:port>` arguments wired from the plan.

The plan is a serialized struct sent peer-to-peer over the existing signed-request HTTP protocol. No new transport.

### 3. Layer-assignment hints

`llama-server` auto-splits layers across backends but doesn't know each peer's *actual* free VRAM, just the totals. unhosted can do better:

- The peer daemon probes free VRAM at plan time:
  - macOS Metal: parse `system_profiler SPDisplaysDataType` for VRAM, query `vm_stat` for pressure
  - Linux NVIDIA: `nvidia-smi --query-gpu=memory.free --format=csv,noheader,nounits`
  - Linux AMD: `rocm-smi --showmeminfo vram`
  - Linux CPU-only: report 0 (skip, don't run rpc-server on a CPU node — its layer-eval is dramatically slower and bottlenecks the cluster)
- Free VRAM is included in the plan response. The orchestrator chooses layer counts via `--gpu-layers` per backend proportional to free VRAM.
- If `llama-server` doesn't expose per-RPC-backend layer counts (current upstream limitation), we fall back to letting it auto-balance and only use the probe to *exclude* peers that can't carry their share.

### 4. Boot orchestration (single-command UX)

Without unhosted, getting layer-split inference working today looks like:

```
# on peer1
llama.cpp/build/bin/rpc-server -p 50052 -H 0.0.0.0
# on peer2
llama.cpp/build/bin/rpc-server -p 50052 -H 0.0.0.0
# on orchestrator
llama.cpp/build/bin/llama-server -m model.gguf --rpc peer1:50052,peer2:50052 --gpu-layers 99
```

With unhosted, it should be:

```
unhosted vram-pool start --model llama3.1:70b
```

The orchestrator daemon:

1. Resolves `llama3.1:70b` against its model registry, ensures the file exists locally OR triggers a coordinated pull.
2. Sends a plan-RFC to each candidate peer over signed HTTP. Each peer replies with capability + free VRAM + accept/reject.
3. SSH-equivalent: peer daemon spawns `rpc-server` locally via the same supervisor pattern `tunnel.rs` uses for cloudflared. Records the child PID for cleanup.
4. Orchestrator spawns its own `llama-server --rpc=...` once peers report listening.
5. Health-probes the resulting `llama-server` on its OpenAI-compatible port, returns control to the user when ready.

`unhosted vram-pool stop` reverses all of the above. `unhosted vram-pool status` shows the current cluster: which peer is the orchestrator, which are layer hosts, current per-peer VRAM utilization, current tokens/sec.

### 5. Failure handling

Three failure modes worth distinguishing:

- **Peer doesn't come up at plan time**: drop it from the plan, retry with the remaining peers. If the remaining VRAM total is insufficient for the requested model, fail with a useful error including which model size *would* fit.
- **Peer drops mid-inference**: `llama-server` will error the in-flight request. The supervisor restarts `llama-server` against the remaining peers (with the dropped peer's layers re-allocated locally if possible, or downgraded to a smaller model). The user-facing chat sees an error toast + automatic retry on a smaller model.
- **Orchestrator crashes**: peer `rpc-server` processes detect the disconnect via the existing daemon supervisor and exit cleanly via `kill_on_drop` semantics already in `tunnel.rs`.

### 6. Observability

The chat UI gains a "cluster" panel when VRAM-pooling is on, showing per-peer layer count, per-peer VRAM utilization, and a tokens/sec readout. This is the "I see my hardware working as one cluster" moment that justifies the whole feature.

## Slicing strategy (multi-release)

The whole thing is a quarter-plus if done end-to-end. Honest slicing:

| Slice | Ships | Scope |
| --- | --- | --- |
| **0.0.x** | now | This ADR. No code change. |
| **v0.1.0** | first useful version | `unhosted vram-pool start/stop/status`. Two peers only. Manual model path argument. No layer-assignment hints (let llama.cpp auto-split). No automatic peer re-routing on failure. Single architecture target (aarch64-darwin orchestrator, x86_64-linux layer host as the canonical test pair). |
| **v0.2.0** | LAN scale | N peers. mDNS-driven plan suggestions ("found 3 RPC-capable machines on your LAN, run on all of them?"). Free-VRAM probing on every backend. Automatic layer count per peer. |
| **v0.3.0** | failure tolerance | Peer-drop recovery without losing the conversation. Live re-plan when free VRAM changes. Cluster-aware request routing (small models stay on one peer, big models pool). |
| **post-0.3.0** | quality | Speculative decoding across peers (small draft on orchestrator, big verifier on the layer-host cluster). Cluster-aware model recommendations ("you have 56 GB total — try llama3.1:70b-q4"). |

The point of slicing this hard is that **the v0.1.0 slice is shippable in roughly two months** even though the whole feature is months further. Two peers, single command, no failure handling — but the headline finally works.

## Non-goals

- **We do not reimplement layer transport.** llama.cpp's RPC is the substrate. If it gets faster, we get faster for free; if it adds a new feature (per-RPC-backend layer counts, mixed precision per peer), we adopt it.
- **We do not bundle llama.cpp.** Same as today — users install via Homebrew / their package manager. The friction of needing an RPC-enabled build is the responsibility of upstream + the user's package manager, not unhosted. We document workarounds and pressure upstream Homebrew to enable the flag.
- **We do not support cross-WAN VRAM pooling** in v0.1.0. The latency between layer evaluations is on the per-token critical path. WAN means seconds-per-token, which is unusable. LAN-only by design. WAN-pooling enters scope only with research-grade pipeline parallelism that hides latency (post-0.3.0 at earliest).
- **We do not support pooling between heterogeneous quantizations.** All peers must serve the same model file. Quant-mixing is upstream's call when they support it.
- **We do not solve the security question of "is the layer host running the model it says it is".** That's ADR 0001 / verifiable inference territory. v0.1.0 assumes trusted LAN, same as the rest of the v0.0.x cluster.

## Open questions

1. **Does `llama-server` cleanly hot-add peers, or does adding a new RPC backend require a restart?** Upstream behavior we should test before promising live re-plan in v0.3.0. If restart-required, v0.3.0 still works but the conversation has to absorb a 2–5 s gap when topology changes.

   **Status:** Blocked. Test requires two RPC-capable machines and we only have one in the current dev environment. Deferring until a real test cluster exists. v0.1.0 doesn't depend on this answer (no hot-add in v0.1.0); v0.3.0 design assumes restart-required and adds a "topology change ↦ short pause" UX affordance. If hot-add turns out to be free, that affordance becomes optional polish.

2. **What's the right answer when the orchestrator's local backend is *also* RPC-capable but offers worse-than-LAN bandwidth (e.g., the orchestrator is on Wi-Fi 5)?** Probably: skip the orchestrator's GPU as a layer host, use it only as the request endpoint. Needs measurement before deciding.

3. **Should the plan be ephemeral (recomputed every `vram-pool start`) or persisted (so a reboot of one peer doesn't kill the pool)?** Lean ephemeral for v0.1.0 — restarts are rare enough that recompute is cheap and persistence has a flock of edge cases (peer added a new GPU? quant changed?). **Resolved: ephemeral for v0.1.0.**

4. **Distribution: who builds RPC-enabled llama.cpp for the average user?**

   **Investigated 2026-05-15:**
   - Homebrew `llama.cpp` 9090 (current release) does **not** ship with RPC support. `which rpc-server` returns nothing, `llama-server --help` doesn't list `--rpc`.
   - The upstream formula's CMake args:
     ```
     -DBUILD_SHARED_LIBS=ON
     -DCMAKE_INSTALL_RPATH=#{rpath}
     -DLLAMA_ALL_WARNINGS=OFF
     -DLLAMA_BUILD_TESTS=OFF
     -DLLAMA_OPENSSL=ON
     -DLLAMA_USE_SYSTEM_GGML=ON
     ```
     Adding `-DGGML_RPC=ON` is a one-line change. The build itself is a few minutes longer.
   - Likely-friction with upstream: `rpc-server` binds a network port by default (`-H 0.0.0.0`). Homebrew maintainers may push back on "default-on security-sensitive binaries". Counter: same posture as redis, postgres, etc. — formula installs, user runs.

   **Decision for v0.1.0:** parallel path.

   - **a) Submit upstream PR to homebrew-core** adding `-DGGML_RPC=ON`. Low cost to write, high payoff if accepted (every user's `brew install llama.cpp` just works). Acceptance is uncertain; deferral or rejection is possible.

   - **b) Stand up `homebrew-unhosted` tap** as the always-available fallback. One formula file in a new repo:
     ```
     brew tap unhosted-ai/unhosted
     brew install unhosted-ai/unhosted/llama.cpp-rpc
     ```
     The formula is essentially the upstream one with `-DGGML_RPC=ON` added. We carry it until upstream lands the change, then archive the tap.

   - **c) `unhosted vram-pool start` performs detection up-front:**
     - `which rpc-server` → present?
     - `llama-server --help` includes `--rpc` → yes?
     If either fails, print the exact `brew tap` + `brew install` from (b), or for non-Homebrew users a link to the build-from-source recipe. No silent fall-through to a broken state. This is the v0.1.0-blocking work: detection is small, the formula in (b) is small, the upstream PR in (a) is small.

   **Linux + Windows note:** `llama.cpp` builds with RPC enabled cleanly on both. Linux distro packages (Ubuntu, Arch, Fedora) tend to also default-disable RPC; same pattern of "user must `apt install / pacman -S` a community build or compile from source." We track the same problem with the same approach: detection in unhosted + a packaging-help page in docs.

## Why this bet

The other three roadmap items had elegant alternatives. *Web browsing*: the Delta browser exists and integrates with unhosted as a backend with zero glue (ADR 0002 covers the application-frontend split). *Browser extension*: Delta IS the browser. *Default system prompt*: already shipped in v0.0.24.

VRAM-pooling has no alternative. No other local-AI project is shipping single-command multi-machine inference for end users. Ollama doesn't. LM Studio doesn't. llama.cpp ships the substrate but ships zero UX around it. The market gap is real, the technical depth is real, and the value is exactly what unhosted's first line of marketing has promised the whole time.

This is the bet.
