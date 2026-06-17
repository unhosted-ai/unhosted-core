# Architecture: the core boundary

`unhosted-core`'s one job (per the MANIFESTO): **turn the machines you own into one private,
OpenAI-compatible inference cluster.** This document marks the seam between *the core* and the
layers that merely *consume* it, so the codebase can be split into small, embeddable crates
over time without a big-bang rewrite.

> Full rationale + the staged extraction plan live in the `unhosted-os` repo as
> **ADR-0002 (`docs/ADR-0002-core-boundary.md`)**. This file is the in-repo summary and the
> contract the module seam in `lib.rs` encodes.

## The core = the distributed inference endpoint

Five irreducible responsibilities. If a module doesn't serve one of these, it is **not core**.

| # | Responsibility | Modules |
|---|---|---|
| 1 | **The endpoint** — OpenAI-compatible HTTP API on `:7777` | `router`, `web`, `lib` (daemon wiring) |
| 2 | **Cluster formation** — discover, pair, pool VRAM, split layers, transport | `discovery`, `peer`, `relay_client`, `swarm`, `transport`, `tunnel`, `vram_pool` |
| 3 | **Inference orchestration** — manage the runtime, route work to the pool | `model_manager`, `upstream` |
| 4 | **Identity & trust** — who may use the cluster | `auth`, `audit`, `identity` |
| 5 | **Self-maintenance** — signed self-update | `update_check` |

Plus shared plumbing the core uses: `metrics`, `paths`, `web_fetch`.

## App layer — consumers of the endpoint (scheduled to move out)

These talk to the core's `:7777` surface; they are not the endpoint. Each is tagged with its
destination crate.

| Layer | Modules | Destination |
|---|---|---|
| **Agent runtime** | `agent`, `agent_fs`, `critique`, `memory`, `chats` | `unhosted-agent` crate (a client of `:7777`) |
| **Policy / safety** | `dlp`, `public_mode`, `connectors` | `unhosted-policy` crate (API middleware) |
| **Payments** | `lightning_cfg` | `unhosted-payments` (already its own repo); feature-gated here meanwhile |

The seam is currently expressed as **grouped, banner-commented module declarations** in
`crates/unhosted-core/src/lib.rs`. No code has moved — this is slice 0 (draw the seam).

## Why split (and why staged)

- The OS, desktop app, and mobile OS each want to embed *the inference fabric* — not payments
  and DLP. A small core is embeddable and auditable.
- The agent is the most security-sensitive, fastest-moving part (see the unhosted-os
  `docs/security.md` tool-capability rules). As its own crate it iterates without forcing a
  core release.
- `lib.rs` is large because it is both the endpoint *and* the daemon that wires every layer
  together. Extraction shrinks it toward just the fabric.

Done as **one shippable slice at a time**, always preserving the public `:7777` contract and
the auth/audit guarantees:

0. **Draw the seam** (this change) — grouped modules + this doc. No behavior change.
1. Extract `unhosted-agent` (`agent`/`agent_fs`/`critique`/`memory`/`chats`).
2. Extract `unhosted-policy` (`dlp`/`public_mode`/`connectors`).
3. Evict `lightning_cfg` into `unhosted-payments`.
4. Shrink `lib.rs` to the inference-fabric core; re-baseline the public API.

## Boundary style

Agent ↔ core is intended to be **HTTP-first** (the agent could run on a different machine than
the endpoint — matching the distributed thesis), falling back to an in-process trait only if
latency demands it.
