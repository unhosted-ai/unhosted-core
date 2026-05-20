# Run: `vrampool-loopback` (2026-05-20)

First measured tokens-per-second comparison between unhosted's VRAM-pool split-inference path and a single-machine baseline. **Loopback simulation only** — both layer hosts on the same Mac. This proves the plumbing works and gives the *floor* of pool overhead; a real LAN run will be slower because of network latency.

This is the experiment whose absence I called out in the [previous "is this useful?" check-in](../../README.md): "tokens/sec on a real cluster is the cheapest, highest-information experiment you could run."

## Setup

- **Hardware:** Apple M1 Max, 32 GB unified memory, macOS 26.3.1
- **Model:** `Meta-Llama-3.1-8B-Instruct-Q4_K_M` (4.6 GB GGUF)
- **Build:** llama-cpp-rpc from our Homebrew tap (`-DGGML_RPC=ON -DGGML_BLAS=OFF`), `llama-server` + `rpc-server`
- **Two daemons** on the same Mac with separate `XDG_CONFIG_HOME`s:
  - Daemon A (orchestrator + layer host) on `127.0.0.1:7787`, RPC `:50052`
  - Daemon B (layer host only) on `127.0.0.1:7789`, RPC `:50053`
- **Orchestration:** `POST /v1/vram-pool/start` with both peers in the layer-hosts list. llama-server invoked with `--rpc=127.0.0.1:50052,127.0.0.1:50053`.
- **Pool came up in 16.7 seconds** (model load + RPC handshake).

## Results

Each prompt was the same exact text against both endpoints. Temperature 0 for determinism. After a discarded warmup request to amortize the first-token JIT/setup costs.

| Prompt | Pool (split, loopback) | Single llama-server | Ratio |
| --- | ---: | ---: | ---: |
| "Explain quantum tunneling in three sentences." (114 out) | 3.91 tok/s | 45.07 tok/s | pool 11.5× slower |
| "Write a short haiku about rain on a tin roof." (17 out) | 3.41 tok/s | 38.26 tok/s | 11.2× slower |
| "What's the difference between TCP and UDP?" (200 out) | 3.26 tok/s | 44.77 tok/s | 13.7× slower |

**Mean: pool 3.5 tok/s vs single 42.7 tok/s. Pool is ~12× slower.**

## What this means

For models that fit on one machine, VRAM-pool is **not useful** — the 12× penalty is huge. If you can run 8B locally at 45 tok/s, splitting it across two daemons makes no sense.

**The VRAM-pool thesis doesn't depend on 8B.** It depends on 70B (and larger) — models that *do not fit at all* on a single 32 GB Mac. For 70B-Q4 (~40 GB), the alternative to pool on a 32 GB machine isn't "single-machine at 45 tok/s" — it's **swap thrashing at <1 tok/s, or the model not loading at all**. Against that, even 3 tok/s is a meaningful win.

So this run doesn't validate the thesis, but it doesn't kill it either. The next run that actually validates or kills VRAM-pool as a feature is:

> **Loopback 70B-Q4 with two daemons each given half the layers.**
> If that runs at 1+ tok/s — pool wins where single-machine couldn't load the model at all.
> If it runs at <0.3 tok/s — pool is theoretically interesting but practically too slow even for "can't otherwise run" use.

That's the next benchmark to commission.

## Loopback-overhead floor

This run measures the *floor* of pool overhead because loopback has effectively zero network latency. On a real LAN the RPC traffic between layer hosts crosses real ethernet or wifi, which adds:

- ~0.1 ms ethernet RTT (gigabit, switched LAN) → modest hit
- ~5 ms wifi RTT → very significant hit
- Hops × bandwidth-per-token-per-layer → can swamp the compute

The single-Mac baseline against the *same* model on the *same* daemon will always beat the LAN-split baseline. The right comparison for "pool is useful" is **pool-vs-fail**, not pool-vs-single.

## What broke / surfaced

1. **Pair-flow bug.** `unhosted pair offer` + `unhosted pair accept` completed successfully, but ended up with the wrong pubkeys on disk:
   - Daemon A's `peers.toml` had B's pubkey set to a value that didn't match B's actual `identity.toml`.
   - Daemon B's `peers.toml` had no pubkey for A at all.

   With wrong/missing pubkeys, every signed peer request fails auth — including `/v1/vram-pool/layer-host/start`, which is exactly the call the orchestrator needs to make to ask a peer to host layers.

   I patched around it by writing both `peers.toml` files manually with the real values from each daemon's `identity.toml`. The pool then came up. **This needs a separate issue + fix.** Pair-flow can't be the way to onboard peers if it produces unverifiable signed requests.

2. **Pair-offer URI's `addr` uses LAN IP, not the addr the daemon is bound to.** I had to manually rewrite `addr=10.88.111.150:7787` to `addr=127.0.0.1:7787` for the loopback test. For a real LAN test this would be fine; for loopback or for daemons bound to specific interfaces, the URI generator should reflect the bind address. Minor follow-up.

3. **Pool startup time: ~16.7 s.** Acceptable for a cluster you bring up once and run many requests against. Excessive for a per-request startup model — but we don't do that.

4. **Bandwidth between layer hosts is the real story for a LAN run.** Loopback gave us 3.5 tok/s; gigabit LAN will give us less; wifi will give us much less. The next run should report the LAN type + latency too.

## Reproduce this run

```bash
# Build the daemon (or have it installed).
cargo build -p unhosted-cli

# Pull the model.
unhosted pull llama3.1:8b

# Start two daemons (separate XDG_CONFIG_HOMEs).
mkdir -p /tmp/uh-A /tmp/uh-B
XDG_CONFIG_HOME=/tmp/uh-A unhosted serve --addr 127.0.0.1:7787 --upstream http://127.0.0.1:11434 &
XDG_CONFIG_HOME=/tmp/uh-B unhosted serve --addr 127.0.0.1:7789 --upstream http://127.0.0.1:11434 &
sleep 3

# Pair them. (Workaround: rewrite addr to 127.0.0.1 in the offer URI.)
OFFER=$(unhosted pair offer --node http://127.0.0.1:7787 | grep -o 'unhosted://[^"]*')
LOOPBACK=$(echo "$OFFER" | sed -E 's|addr=[0-9.]+:|addr=127.0.0.1:|')
unhosted pair accept "$LOOPBACK" --node http://127.0.0.1:7789

# IMPORTANT — manual peer-pubkey fix, until the pair-flow bug is resolved:
# write the real pubkeys from each identity.toml into the corresponding
# peers.toml entry's `pubkey = "..."` field, then restart both daemons.

# Start the pool.
curl -sS -X POST http://127.0.0.1:7787/v1/vram-pool/start -H content-type:application/json -d '{
  "plan": {
    "orchestrator": "local",
    "layer_hosts": [
      {"name":"local","addr":"127.0.0.1:50052"},
      {"name":"<peer-name-as-seen-by-A>","addr":"127.0.0.1:50053"}
    ],
    "model": "/path/to/Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf"
  }
}'

# Poll /v1/vram-pool until state == "running"; the endpoint is at the
# pool's llama-server (default http://127.0.0.1:8080).

# Bench (script committed at /tmp/bench.py for this run — same pattern
# as eval.py from models/distill/).
python3 bench.py http://127.0.0.1:8080 "Explain quantum tunneling in three sentences." 200 pool
python3 bench.py http://127.0.0.1:8081 "Explain quantum tunneling in three sentences." 200 baseline
```

## Verdict

- Plumbing **works**. Two-daemon VRAM-pool is real, not a thought experiment.
- Performance for 8B-Q4: **12× penalty on loopback**, meaningfully more on real LAN.
- This run does **not** validate the thesis (didn't test 70B). It does set the *floor* of pool overhead.
- Two bugs surfaced (pair-flow pubkey storage, offer URI addr selection) that block real-world pair-then-pool flow without manual intervention.

Next run that matters: **70B-Q4 loopback** with `unhosted pull llama3.1:70b` (or similar 70B GGUF) and the same two-daemon split. If that produces 1+ tok/s, pool is useful in its intended use case. If <0.3 tok/s, the headline thesis needs revision.
