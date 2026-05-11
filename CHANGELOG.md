# Changelog

All notable changes to Unhosted are recorded here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until v0.1.0 the API and CLI surface may break between releases — we'll note it loudly when it does.

## [Unreleased]

### Added
- v0.0.2 architecture captured in [design/0003](design/0003-two-node-lan-cluster.md): two-node LAN cluster ships request distribution first; VRAM-pooling via llama.cpp's RPC backend is v0.0.3+ (requires custom llama.cpp build).
- `unhosted peer add | list | remove` subcommands. Peers persist to `~/.config/unhosted/peers.toml` (XDG-respecting). Skeleton only — routing is wired in v0.0.2 proper.
- `unhosted_core::peer` module with `Peer` and `PeerRegistry` types, including unit tests.
- Public-mode payment architecture in [design/0001](design/0001-public-mode-architecture.md): Base first / Solana later, optimistic + redundancy verification, escrow + signed-receipts flow.
- Application frontend plan in [design/0002](design/0002-application-frontends.md): CLI today, web UI v0.1.0+, Tauri desktop app v0.2.0+.
- GitHub Pages site at `/docs` with a Kernel-style landing page, deployed automatically via Actions.
- Branding kit under `/branding` and `/docs/branding`: trust-gradient mark, stacked secondary mark, lockups, social cards, favicons, raster siblings (PNG + JPG) for every SVG.
- Reusable raster build pipeline at `scripts/build-rasters.sh` (rsvg-convert + sips).
- `learn` page at `/docs.html` with trust-radius diagram, runnable quickstart, status pills, and FAQ.

## [0.0.1] — 2026-05-09

First runnable version. Single-machine inference only.

### Added
- Cargo workspace with two crates: `unhosted-core` (library) and `unhosted-cli` (binary `unhosted`).
- `unhosted serve`: starts a local node listening on `127.0.0.1:7777`, proxies inference requests to a llama.cpp `llama-server` running upstream (default `http://127.0.0.1:8080`, configurable).
- `unhosted run "<prompt>"`: sends a request to a node, parses the upstream SSE stream, pipes plain-text tokens to stdout as they arrive.
- `/health` endpoint for liveness checks.
- Project foundations: AGPL-3.0 LICENSE, manifesto, brand guide, contributing guide, code of conduct, security policy, issue templates, .gitignore, rust-toolchain pinning.

### Known limitations
- Requires `llama-server` running separately. We don't manage it for you yet.
- Single host only — no LAN cluster, no peer pairing, no public swarm.
- No authentication. The node binds to localhost; don't expose it to a network without a reverse proxy.
- No persistent state, no model registry, no resumption.
- Not benchmarked.
