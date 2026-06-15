# Design notes

Architectural decision records (ADRs) — short, dated documents that capture *what we decided* and *why*, so future contributors don't have to reverse-engineer it from the code.

Each file is numbered (`NNNN-slug.md`) and never renumbered. Decisions can be superseded by later ADRs but never silently rewritten.

## Workflow

### Spec-first (default)
Write the spec before any code lands. Status stays `Draft` until reviewed, then moves to `Accepted`.

```
bash scripts/new-spec.sh <slug>          # creates design/NNNN-slug.md
# fill it in, get sign-off
# then implement
```

### Hybrid
Spec and code land together in the same PR. Use this when the design is obvious but you still want a record.

```
bash scripts/new-spec.sh <slug> Hybrid   # status is Hybrid from the start
```

A Claude hook will remind you to run one of the above when you create a new source module with no matching spec. It warns but never blocks.

### Superseding a decision
Set the old spec's status to `Superseded by NNNN` and link to the new one. Never edit the rationale in the old doc.

## Spec format

Each ADR has:

- **Status**: `Draft` / `Hybrid` / `Accepted` / `Superseded by NNNN`
- **Captured**: date the spec was first written
- **Target**: which version this is planned for
- **Motivation**: the problem and who it affects
- **Decision**: what exactly we're building (interface, data structures, protocol)
- **Alternatives considered**: what we ruled out and why
- **Implementation sketch**: high-level steps to confirm it's buildable
- **Open questions**: things not yet decided
- **Out of scope**: explicit non-decisions to prevent scope creep

See `TEMPLATE.md` for the blank form.

## Index

- [`0001-public-mode-architecture.md`](0001-public-mode-architecture.md) — payment chain, verifiable inference stance, flow shape
- [`0002-application-frontends.md`](0002-application-frontends.md) — desktop/mobile/web surface choices
- [`0003-two-node-lan-cluster.md`](0003-two-node-lan-cluster.md) — LAN-mode request routing + peer registry
- [`0004-trusted-mode.md`](0004-trusted-mode.md) — Ed25519 pairing, signed requests, peer registry
- [`0005-relay-and-connection-topology.md`](0005-relay-and-connection-topology.md) — direct / hole-punched / relay attempts
- [`0006-public-mode-onboarding-and-wallet.md`](0006-public-mode-onboarding-and-wallet.md) — public-mode account model + wallet binding + first-90s flow
- [`0007-security-hardening.md`](0007-security-hardening.md) — local bearer auth, replay defense, relay caps + rate limits
- [`0008-quic-peer-transport.md`](0008-quic-peer-transport.md) — encrypted peer-to-peer via QUIC + Ed25519-bound certs (no separate Noise layer)
- [`0009-vram-pooling.md`](0009-vram-pooling.md) — distributed inference across LAN peers via llama.cpp RPC
- [`0010-custom-llm-pipeline.md`](0010-custom-llm-pipeline.md) — distil a specialist model from open-source bases using Claude or a local daemon as teacher
- [`0012-agent-runtime.md`](0012-agent-runtime.md) — agent loop, tool-calling runtime, and the tool roadmap
- [`0013-agent-tool-read-file.md`](0013-agent-tool-read-file.md) — first agent tool: confined file reads
- [`0014-swarm-model-distribution.md`](0014-swarm-model-distribution.md) — content-addressed peer-to-peer GGUF distribution (the torrent-shaped slice); weights only, not inference
