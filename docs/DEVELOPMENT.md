# Development guide

How `unhosted-core` is built, what it's built from, and how the pieces fit
together. For *why* it exists, read the [MANIFESTO](../MANIFESTO.md); for the
core/app boundary rationale, read [ARCHITECTURE.md](../ARCHITECTURE.md). This
document is the practical map for someone about to build, run, or hack on it.

## What it is, in one paragraph

Unhosted turns the machines you own (and, opt-in, machines your friends own, and
beyond that a paid public swarm) into a single OpenAI-compatible inference
endpoint on `http://localhost:7777`. You point any OpenAI client at it and it
routes the work to a local runtime, a paired peer over the LAN, or a relay —
your data never leaves the trust radius you chose.

```
       ╭───────────────────────────────╮
       │   public · pay (USDC)         │   strangers' GPUs, opt-in
       │   ╭───────────────────────╮   │
       │   │  trusted · free       │   │   friends, family, team
       │   │   ╭───────────────╮   │   │
       │   │   │ local · free  │   │   │   devices you own
       │   │   ╰───────────────╯   │   │
       │   ╰───────────────────────╯   │
       ╰───────────────────────────────╯
```

## Tech stack

Everything is **Rust** (stable toolchain, edition 2021, pinned in
`rust-toolchain.toml`), one Cargo workspace, async on **Tokio**. No runtime
services, no database server — state is files under `~/.cache/unhosted` and
`~/.config/unhosted`.

| Concern | What we use | Why |
|---|---|---|
| Async runtime | **tokio** | Everything is I/O-bound: HTTP, peer sockets, child processes. |
| HTTP server / API | **axum** + **tower-http** | The `:7777` OpenAI-compatible endpoint and the web UI host. |
| HTTP client | **reqwest** (rustls) | Talks to the upstream runtime, HuggingFace, relays. |
| Peer transport | **quinn** (QUIC) + **rustls** | Encrypted, multiplexed streams between peers; the swarm + VRAM-pool wire. |
| LAN discovery | **mdns-sd** | Zero-config `_unhosted._tcp.local.` peer discovery. |
| Identity / crypto | **ed25519-dalek**, **rcgen**, **rustls-pki-types** | An Ed25519 keypair *is* the node identity; self-signed certs pin peers. |
| CLI | **clap** (derive) | `unhosted serve`, `peer`, `vram-pool`, `seed-status`, … |
| Desktop shell | **tauri** (+ **tao**/**wry**) | Native window wrapping the daemon's web UI; auto-updater + signed installers. |
| Web UI | vanilla JS/HTML/CSS, **rust-embed**ded | Shipped inside the binary — no separate frontend build, no node at runtime. |
| Embeddings | **fastembed** + **tokenizers** | Local vector memory for the agent, no external embedding API. |
| Serialization | **serde** / **serde_json** / **toml** | JSON on the wire, TOML for config. |
| Errors / logs | **anyhow**, **tracing** | `Result`-based errors up to the daemon; structured logs via `RUST_LOG`. |

Payments live in a **separate repo** (`unhosted-payments`) and are pulled in as
`unhosted-payments-core` / `-lightning` via a pinned git rev, feature-gated here.

## The workspace: 6 crates

```
crates/
├── unhosted-core-base   shared kernel  ── primitives everything imports
├── unhosted-core        the engine     ── the :7777 inference endpoint + daemon
├── unhosted-agent       agent runtime  ── a CLIENT of :7777 (tools, memory, critique)
├── unhosted-cli         the binary     ── `unhosted …`; boots the daemon
├── unhosted-desktop     desktop shell  ── tauri window pointed at :7777
└── unhosted-relay       rendezvous     ── byte-forwarder for peers behind NAT
```

The dividing line (full rationale in [ARCHITECTURE.md](../ARCHITECTURE.md)):
**the core is the distributed inference endpoint, and nothing else.** Anything
that merely *consumes* `:7777` — the agent, policy/safety, payments — is an app
layer that can move to its own crate without breaking the endpoint contract.

- **`unhosted-core-base`** — the small kernel both the engine and the agent
  import: paths, audit log, metrics, `web_fetch`. No business logic.
- **`unhosted-core`** — the engine. Owns five irreducible responsibilities:
  the endpoint (`router`, `web`), cluster formation (`discovery`, `peer`,
  `swarm`, `transport`, `relay_client`, `tunnel`, `vram_pool`), inference
  orchestration (`model_manager`, `upstream`), identity/trust (`auth`,
  `identity`, `audit`), and signed self-update (`update_check`).
- **`unhosted-agent`** — the tool-using agent. Deliberately a *client* of the
  endpoint, because it's the most security-sensitive, fastest-moving part and
  shouldn't force a core release to iterate.
- **`unhosted-cli`** — the `unhosted` binary; parses args and starts the daemon.
- **`unhosted-desktop`** — a thin Tauri shell that opens a native window on the
  local daemon's web UI. No tests (nothing but window wiring).
- **`unhosted-relay`** — a standalone rendezvous + byte-forwarding service for
  trusted-mode peers that can't reach each other directly.

## How a request flows

```
OpenAI client ──HTTP──▶ :7777  (axum router in unhosted-core)
                          │
                          ├─▶ auth / policy check           (identity, dlp, public_mode)
                          │
                          ├─▶ where can this run?           (upstream + vram_pool + peers)
                          │      • local llama-server        ── model_manager supervises it
                          │      • a paired LAN peer         ── discovery → quinn transport
                          │      • a relay-forwarded peer    ── relay_client
                          │      • (public mode) a paid peer ── escrow via unhosted-payments
                          │
                          └─▶ stream tokens back to the client
```

The **agent** sits *in front* of this: when you use agent mode, `unhosted-agent`
plans, calls tools (fs, web fetch, memory), runs a critique gate, and issues its
own chat completions against the same `:7777` endpoint like any other client.

## Model distribution (ADR-0014, swarm)

Models are **content-addressed**: a GGUF is identified by `sha256:<hex>` of its
bytes, not by where it came from. Bytes are **trusted by hash, never by source**
— a poisoned chunk fails verification regardless of which peer sent it. A node
that already has a model becomes a source for peers on the same LAN, so the
whole cluster doesn't re-pull gigabytes from the HTTPS origin. See
[design/0014-swarm-model-distribution.md](../design/0014-swarm-model-distribution.md).

## Build & run

Prereqs: a stable Rust toolchain (rustup installs the pinned channel
automatically) and, on Linux, GTK/WebKit for the desktop crate:

```bash
# Linux desktop deps (skip if you only build core/cli)
sudo apt-get install -y libgtk-3-dev libsoup-3.0-dev libwebkit2gtk-4.1-dev
```

```bash
cargo build --workspace            # debug build of everything
cargo run -p unhosted-cli -- serve # start the daemon on :7777
cargo run -p unhosted-cli -- --help
```

Inference needs a local runtime (`llama-server`, LM Studio, or Ollama) that the
daemon supervises or proxies to; `scripts/start-llama.sh` is the quick path.

## Build profiles

Defined in the root `Cargo.toml`. **The release profile is what ships; the
dev/test tuning only affects local + CI iteration and never changes the
release binary.**

| Profile | Use | Key settings |
|---|---|---|
| `release` | shipped binaries | `lto = "fat"`, `codegen-units = 1`, `strip`, `panic = "abort"` — small, fast, slow to link |
| `release-debug` | profiling | `inherits release` but keeps line-table debuginfo |
| `dev` | day-to-day | `debug = "line-tables-only"`; **dependencies** compiled at `opt-level = 1` so crypto/QUIC-heavy tests aren't crawling |
| `test` | `cargo test` | inherits dev, drops debuginfo — a passing test needs none |

The `[profile.dev.package."*"] opt-level = 1` line is the important one: your own
crates stay at `opt-level 0` (fast to recompile every edit), but the 600+
dependencies — which rarely change and are cached — run optimized. This is a
large win for the sha256/QUIC-heavy swarm tests.

### Optional: a faster linker

Linking the ~26 MB binary is a meaningful slice of every incremental rebuild.
A parallel linker (`mold` on Linux, `lld` on macOS) cuts that. It barely helps
*release* builds — those are dominated by `lto = "fat"`, not the link step — so
it's purely a dev-loop convenience.

It's **opt-in** so a fresh clone never breaks on a missing linker:
`.cargo/config.toml` ships with every block commented out (a true no-op).
Install a linker and uncomment the block for your platform:

```bash
# Linux
sudo apt-get install mold
# macOS (lld ships with LLVM; stock Apple ld is already fast on recent macOS)
brew install llvm
```

## The gate (run before every push)

All four must pass locally; the pre-commit hook enforces `fmt`, and CI
(`.github/workflows/rust.yml`) enforces the rest:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

## Releasing

See [RELEASING.md](../RELEASING.md). Short version: roll `CHANGELOG.md`'s
`[Unreleased]` into a dated section, bump `[workspace.package].version` in the
root `Cargo.toml` (all crates inherit it — there is no per-crate version and
`tauri.conf.json` has no version field), commit, tag `vX.Y.Z`, and push the tag.
GitHub Actions builds installers for macOS (Intel + Apple Silicon) and Linux
(x86_64 + aarch64) and publishes the release.
