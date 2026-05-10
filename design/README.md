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
