# Design notes

Architectural decision records (ADRs) — short, dated documents that capture *what we decided* and *why*, so future contributors don't have to reverse-engineer it from the code.

Each file is numbered (`NNNN-slug.md`) and never renumbered. Decisions can be superseded by later ADRs but never silently rewritten.

## Format

Each ADR has:

- **Status**: Draft / Accepted / Superseded by NNNN
- **Targets**: which version this lands in
- The decisions themselves
- The reasoning (especially the *not chosen* options and why)
- Open questions that aren't being decided yet

## Index

- [`0001-public-mode-architecture.md`](0001-public-mode-architecture.md) — payment chain, verifiable inference stance, flow shape
- [`0002-application-frontends.md`](0002-application-frontends.md) — desktop/mobile/web surface choices
- [`0003-two-node-lan-cluster.md`](0003-two-node-lan-cluster.md) — LAN-mode request routing + peer registry
- [`0004-trusted-mode.md`](0004-trusted-mode.md) — Ed25519 pairing, signed requests, peer registry
- [`0005-relay-and-connection-topology.md`](0005-relay-and-connection-topology.md) — direct / hole-punched / relay attempts
- [`0006-public-mode-onboarding-and-wallet.md`](0006-public-mode-onboarding-and-wallet.md) — public-mode account model + wallet binding + first-90s flow
