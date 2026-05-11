# 0003 — Two-node LAN cluster (v0.0.2)

**Status:** Accepted (skeleton landed; routing not yet wired)
**Captured:** 2026-05-09
**Targets:** v0.0.2 ships request distribution. v0.0.3+ adds VRAM pooling via model-layer splitting.

v0.0.1 wraps a single `llama-server`. v0.0.2 introduces the first taste of the "pool your hardware" pitch: two nodes on the same LAN, behaving as one cluster from the user's point of view.

## The two paths to multi-node

There are two distinct ways to use a second machine. We must not conflate them.

### A. Request distribution (load-balancing)

Each node runs a full `llama-server` with its own copy of the model. The primary node's daemon receives requests and routes them to either local llama-server or a peer's daemon, based on availability and configured priority.

**Pros:** Works with any llama.cpp build (the Homebrew one is fine). Easy to implement on top of v0.0.1's HTTP API — peers expose the same `/v1/run` endpoint. No new compile-time dependencies for users.

**Cons:** Doesn't actually pool VRAM. Each machine still needs to fit the whole model. The 70B-across-MacBook-plus-4090 promise doesn't materialize from this alone.

**Real value it delivers:** parallel throughput across requests, automatic failover, the architectural substrate (peer registry, routing, discovery) that VRAM-pooling will sit on top of.

### B. Model-layer splitting (VRAM pooling)

Layers of a single model are distributed across multiple machines. llama.cpp supports this via `rpc-server` (the layer-host daemon) plus `llama-server --rpc <host:port>` (the orchestrator). One inference call genuinely spans both GPUs.

**Pros:** This is the actual headline feature. Pooled VRAM means you can run a model bigger than any single machine could handle.

**Cons:** Requires llama.cpp built with `-DGGML_RPC=ON`. The Homebrew formula doesn't enable it as of llama.cpp 9090. Users would have to build from source or use a third-party tap. That's real friction.

## Decisions

### 1. v0.0.2 ships request distribution (path A)

The peer registry, peer protocol, routing logic, and CLI surface all land in v0.0.2. Layer splitting follows in v0.0.3+ on the same infrastructure.

This is honest about what works today and avoids requiring users to build llama.cpp from source for the first multi-node version.

### 2. v0.0.3+ adds layer splitting (path B), documented as opt-in

When a user has built llama.cpp with RPC enabled, the daemon detects the `rpc-server` capability and offers the option. Default path stays request-distribution.

### 3. Peer discovery — manual for v0.0.2, mDNS for v0.0.3

v0.0.2:

```
unhosted peer add thunder 192.168.1.42:7777
unhosted peer list
unhosted peer remove thunder
```

v0.0.3+: zero-config discovery via mDNS / Bonjour. Devices on the same LAN show up automatically. ADR for that lands when v0.0.3 starts.

### 4. Peer transport — same daemon protocol, no new format

The peer protocol *is* the existing `unhosted` HTTP API. A peer node is literally another `unhosted serve` process. Primary calls `POST /v1/run` on the peer just like a CLI client does. No new RPC framework, no new protobufs, no peer-only port — the daemon is symmetric.

### 5. Peer authentication — none in v0.0.2

LAN-only assumption. The daemon will refuse to bind to non-localhost addresses unless explicitly given `--bind 0.0.0.0` with a warning printed. Pre-shared key authentication arrives in v0.1.0 alongside trusted-peer mode over the internet.

This is acceptable because v0.0.2's threat model is: "you trust everything on your LAN." If that's wrong for you, wait for v0.1.0.

### 6. Failure handling — fail to local, fail loud

If a peer is unreachable at request time, the router falls back to local inference and logs a warning. If no local backend is available either, the request fails with a clear error pointing at the peer config and connection error.

### 7. Routing strategy — round-robin with priority, no smart placement yet

Each peer has a `priority` (lower = preferred). Within a priority tier, requests are distributed round-robin. Sophisticated placement (which peer has the most free VRAM, which is least-loaded, model-specific routing) is v0.0.3+.

## Config file shape

`~/.config/unhosted/peers.toml` (XDG-respecting):

```toml
[[peers]]
name = "thunder"
addr = "192.168.1.42:7777"
priority = 1
models = ["llama3.2:1b", "llama3.1:8b"]

[[peers]]
name = "homelab"
addr = "192.168.1.99:7777"
priority = 2
models = []     # empty = serves whatever it's asked, lets caller pick
```

Loaded at daemon startup. Reloaded on `unhosted peer add/remove`. SIGHUP support deferred to v0.0.3+.

## What we will not do (yet)

- **No gossip protocol.** Peer state stays in the local config file; nodes don't push their status to each other.
- **No mid-request peer changes.** If you `peer add` while a request is in-flight, the new peer isn't considered for that request.
- **No replication strategy for cached prompts.** Each peer keeps its own KV cache.
- **No "peer reputation."** v0.0.2 assumes all peers are equally honest. Reputation is a v0.3.0+ public-mode concern.

## Open questions (resolve during v0.0.2 implementation)

- Should the daemon advertise its serving capabilities at a `/v1/capabilities` endpoint so peers can introspect what's running?
- How does the matchmaker handle a peer that's known but currently offline — exclude immediately, or retry briefly?
- Do we need a per-peer rate limit to avoid hammering a small home server?

These get resolved in code review as v0.0.2 lands, not in advance.
