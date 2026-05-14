# Changelog

All notable changes to Unhosted are recorded here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until v0.1.0 the API and CLI surface may break between releases — we'll note it loudly when it does.

## [Unreleased]

## [0.0.12] — 2026-05-14

### Fixed
- **Missing app icon on macOS Dock / Finder / cmd-tab switcher.**
  `crates/unhosted-desktop/Info.plist` said `CFBundleIconFile =
  "unhosted"` (expecting `Resources/unhosted.icns`), but Tauri's
  bundler writes the icon as `Resources/icon.icns`. macOS couldn't
  find the named file and fell back to the generic application icon.
  Fixed by changing `CFBundleIconFile` to `"icon"` to match what
  Tauri actually produces.
- **`CFBundleVersion` was hardcoded at `0.0.7`** in the manual
  `Info.plist` and bled through into every Tauri-built release since.
  Now matches the workspace version (0.0.12).

## [0.0.11] — 2026-05-14

Re-release of v0.0.10 with the publish step actually finishing, plus
an extra hardening fix.

### Fixed
- **CI publish step.** v0.0.10's tag built all four platform artifacts
  successfully but the final `Create GitHub release` step hit a stale
  draft release that the bot couldn't clean up, leaving the release
  unpublished. Draft cleaned out; fresh tag here triggers a clean run.

### Added
- **Server-side stop-guard.** `/v1/tunnel/stop` now requires the
  header `X-Unhosted-Confirm: yes` and returns `428 Precondition
  Required` without it. Stale browser tabs running pre-confirm-dialog
  JS — which the daemon log proved were the source of every "phone
  URL just stopped working" complaint — can no longer kill the
  tunnel. The active `web/ui.js` attaches the header from
  `stopTunnel()`; anything else 428s.

### Also in this release
- Everything from v0.0.10 (desktop-app blank-window fix in the
  bundled placeholder, JS health-probe + retry).

## [0.0.10] — 2026-05-14

Critical fix: the released v0.0.9 desktop app showed a blank window on
both macOS and Linux. The cross-origin meta-refresh in the bundled
placeholder index.html was being silently dropped by Tauri 2's
WebView, so the redirect from `tauri://localhost/` to
`http://127.0.0.1:7777` never fired. The daemon's UI itself was fine
(reachable in Safari/Chrome at the same URL); only the bundled .app
launcher was broken.

### Fixed
- **`crates/unhosted-desktop/dist/index.html`** — replaced the
  `<meta http-equiv="refresh">` redirect with a JS health-probe loop.
  It pings `/health` every 250ms for the first 5s then every 1.5s,
  navigates the WebView the moment the daemon answers, and surfaces a
  real "still waiting — run `unhosted serve`" hint instead of a blank
  page when the daemon isn't up.

  Also adds a dark-mode style block for the placeholder so the
  pre-connect splash matches the system theme. Listens for an optional
  `<meta name="unhosted-node-url">` override the Rust launcher can
  inject so non-default ports work end-to-end.

### Also in this release
- All v0.0.9 features (CI re-release of v0.0.8, see notes below).

## [0.0.9] — 2026-05-14

Re-release of v0.0.8 with the CI release pipeline fixed. v0.0.8 was
tagged but never published — the Tauri updater-signing step failed
(`incorrect updater private key password`), the four platform builds
exited 1 before staging artifacts, and `publish release` was skipped.
No GitHub release exists for v0.0.8.

### Fixed
- **`createUpdaterArtifacts: false`** in `tauri.conf.json` so `cargo
  tauri build` doesn't attempt to sign the updater bundle. The
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secret is wrong / rotated
  and needs to be re-set in GitHub before re-enabling. In-app
  auto-update is paused until then; manual reinstall via install.sh
  works either way.
- **`cargo fmt --check`** was failing across the workspace
  (`unhosted-cli/src/main.rs`, `unhosted-core/src/auth.rs`, etc).
  Reformatted in-tree to match `rustfmt` defaults.
- **`clippy --all-targets --all-features -- -D warnings`** was failing
  on a `doc-lazy-continuation` lint in
  `crates/unhosted-core/src/tunnel.rs`. Added blank lines around the
  bulleted list in `spawn_eager_watchdog`'s rustdoc so the lint
  passes.

All of v0.0.8's feature work ships under this release — see notes
under v0.0.8 below.

## [0.0.8] — 2026-05-14

Reliability + phone-onboarding pass on top of v0.0.7. The "open to
internet" path used to fail in a dozen subtle ways and the phone PWA
assumed you already knew the URL. Both are fixed.

### Added
- **Live QR code in the sidebar** ("send to my phone") encoding the
  active tunnel URL with bearer token baked in. Scan with the phone's
  camera → URL opens → token auto-bootstraps → chat starts. Zero typing.
- **"For developers" panel + modal** showing the daemon's base URL,
  bearer token, and copy-pasteable `curl` / `python` / `javascript`
  snippets pre-filled with the user's actual endpoint and token.
- **`--eager-tunnel` flag (and `UNHOSTED_EAGER_TUNNEL=1` env)** for
  `unhosted serve`. Spawns cloudflared at boot so the public URL is
  already live by the time the UI opens.
- **Auto-restart supervisor** for cloudflared (revives unexpected exits
  with a 3s backoff, up to 3 attempts in a row).
- **Eager-tunnel watchdog** polling every 30s to revive the tunnel from
  Idle/Failed states — unless the user explicitly clicked stop.
- **Toast notification system** for tunnel state changes, URL rotation,
  copy success/failure, and other transient feedback.
- **In-app confirm modal** replacing `window.confirm()` (which silently
  returns false in WKWebView). Used by delete-chat, clear-chats, and
  the new turn-off-tunnel guard.
- **Bundle scripts for Linux + Windows** (`bundle-linux.sh`,
  `bundle-windows.ps1`) and a local cross-compile recipe via `zig` +
  `cargo-zigbuild` in `RELEASING.md`.

### Changed
- **Default chat model substitution.** `/v1/chat/completions` rewrites
  placeholder model names ("local" / "default" / "auto" / missing) to
  the upstream's actual model id. Lets the docs snippet work on Ollama
  and LM Studio, which strictly resolve names.
- **Tunnel toggle now confirms** before turning off a live tunnel.
  Starting from idle stays a single click.
- **Internet preflight before cloudflared spawn** (1.5s HEAD to
  `cloudflare.com`); fails fast with "no internet" instead of hanging.
- **Shared `reqwest::Client`** in `NodeState.http` for HTTP keep-alive
  across chat-completion turns.
- **`Discovery` switched from `Mutex<HashMap>` to `RwLock<HashMap>`** —
  reads dominate, writes are rare mDNS events.
- **App icons regenerated from a single SVG source** so the macOS Dock
  icon matches Tauri's runtime icon — fixes the "icon morphs between
  rounded plate and square blob during launch" bug.
- **Tunnel UI polling hardened** against WKWebView's setInterval
  throttling: two cadences (1.5s fast / 8s slow) plus window focus,
  click-to-refresh on the tunnel header, and an automatic refetch
  800ms after every state-change toast.
- **Mobile / PWA polish.** Safe-area-aware composer, `100dvh` for
  keyboard-aware viewport, 44pt minimum tap targets, hover-only styles
  suppressed on touch. Manifest's primary icon is the rounded plate
  so iOS "Add to Home Screen" matches the Dock icon.

### Fixed
- **Delete-chat and clear-chats silently aborting** in the desktop app.
- **Send/stop button swap** showing both buttons at once (`display:
  inline-flex` was beating `[hidden]`).
- **Stale dock icon after upgrade** — duplicate Launch Services
  registrations of the same bundle id from `dist/` and `/Applications/`.
- **`/v1/chat/completions` returning 502 over the tunnel** with the
  documented `model: "local"` placeholder against Ollama. Fixed by
  the model-name rewrite above.

### Reliability
- **Diagnostic logging on every `POST /v1/tunnel/stop`** (remote addr,
  user-agent, referer, cf-connecting-ip) so unexpected stops can be
  traced.

### Performance / cache
- **UI assets now use `Cache-Control: no-store, max-age=0,
  must-revalidate`** for HTML/JS/CSS/JSON. WKWebView's interpretation of
  the previous `no-cache` was serving stale-while-revalidate, which kept
  shipping yesterday's JS to the user. Binary assets keep `no-cache`.

## [0.0.7] — 2026-05-12

### Added
- **Cross-device chat sync.** Chat history is now stored daemon-side at `~/.config/unhosted/chats.json` and served via `GET/POST/PUT/DELETE /v1/chats[/:id]`. Every device paired to the daemon — desktop browser, phone PWA over LAN, public-tunnel URL — sees the same conversation list, instead of the per-origin localStorage stores that previously diverged. Web UI does a one-time migration of pre-existing localStorage chats on first load. Endpoints are local-user-only (loopback or valid bearer); paired peers can use your GPU but not read your history.
- **"Open to internet" button** in the sidebar. One click spawns `cloudflared` and surfaces a `*.trycloudflare.com` URL with the bearer token embedded as `?api_token=…`, so opening it on a phone over cellular Just Works. New `/v1/tunnel[/start|stop]` endpoints; subprocess gets `kill_on_drop(true)` so daemon shutdown takes the tunnel down with it. Requires `cloudflared` on PATH (`brew install cloudflared`).
- **Stop button** during streaming, alongside the existing send button. Aborts the in-flight `/v1/run` fetch via `AbortController`; partial text stays in the transcript with a `[stopped]` marker. Verified that upstream cancellation propagates to ollama via socket-close — no wasted GPU cycles.
- **Clear-all-chats button** on hover next to the "conversations" header. Issues `DELETE /v1/chats`.

### Changed
- **Desktop shell migrated from raw tao+wry to Tauri 2.** Same underlying WebView (WKWebView / WebView2 / WebKitGTK) — the wrap buys us the official Tauri bundler (signed `.dmg` / `.msi` / `.AppImage` / `.deb` produced by `cargo tauri build` on each platform's CI runner), the updater plugin (Phase 1 wired against the GitHub release feed; pending the `TAURI_SIGNING_PRIVATE_KEY` secret + signed releases to actually serve updates), and a clean place to hang the Phase 2 polish (system tray, deep-link handler for `unhosted://pair?…`, native notifications). The desktop binary still bundles **zero** HTML/JS of its own; the window loads `http://127.0.0.1:7777` and renders whatever the daemon serves, so a daemon upgrade is also a UI upgrade — no separate desktop release per UI change.
- **Release workflow uses Tauri's bundler.** `bundle-macos.sh` + `build-dmg.sh` are retired; `.github/workflows/release.yml` now installs `tauri-cli` on each matrix runner and runs `cargo tauri build` to produce platform-native installers in one pass. New `.github/scripts/build-updater-manifest.py` assembles `latest.json` from the per-asset `.sig` files Tauri's signer emits.

### Security
- **Tunnel-source detection in the auth classifier.** Requests carrying `cf-connecting-ip`, or with a non-loopback IP anywhere in `x-forwarded-for`, are now classified as non-loopback even when the TCP source is `127.0.0.1`. Without this, cloudflared forwarding to `localhost` would have inherited the loopback bypass — anyone with the public URL would have driven the daemon unauthenticated. Bearer is now required for tunneled traffic; local browser keeps its no-bearer convenience.

## [0.0.6] — 2026-05-12

### Fixed
- **Windows: `unhosted serve` now starts.** The daemon previously aborted at startup on every Windows machine with `Error: HOME env var not set` — peer registry, identity, and the local API token all read `HOME` directly, which doesn't exist on Windows (it's `USERPROFILE` there). Surfaced by the new release smoke test on `windows-latest`. New `paths::home_dir()` tries `HOME → USERPROFILE → HOMEDRIVE+HOMEPATH` in order. `--version` and `doctor` already worked; the bug only hit `serve` because only it touches the peer registry.

### Added
- **macOS `.dmg` installer.** v0.0.6 onwards ships `unhosted-aarch64-apple-darwin.dmg` as a release asset — actual double-click install, no tarball-and-drag dance. `scripts/build-dmg.sh` wraps the `.app` via `hdiutil`.
- **Release smoke-test CI** (`.github/workflows/smoke-release.yml`). Triggers on every release publish. Downloads the artifact for macOS, Linux x86_64/arm64, and Windows; runs `--version` + `doctor`, then starts the daemon and asserts `/health` returns 200 and `/v1/run` returns the structured 503 when no runtime is up. Catches platform-specific regressions before they reach users.

### Page
- **Modes section rebuilt.** The three "mode" cards used near-identical concentric-circle icons. Replaced with a real relational trust-radius diagram (one figure, three labelled rings, clickable to scroll), three visually distinct mode icons (single device / paired devices / you-among-mesh), and a `<details>` "what does that actually mean?" expansion under each card with hardware / network / privacy / cost / flow as a key-value list.
- **Status pills on mode cards** (`shipped · v0.0.1`, `building · v0.0.4`, `designed · v0.3.0+`).
- **Margins fixed.** Single `--prose-max: 720px` for all sub-paragraphs (was: 620 / 720 / 760 / 820 mixed), `--pad` clamps `24px → 64px` via viewport width, `--max` reduced 1320 → 1240px.
- **Footer credit.** `built by sinhaankur.com · open-source pet project, kept light on purpose`.

## [0.0.5] — 2026-05-12

### Fixed
- **Chat no longer 502s when llama-server isn't on `:8080`.** The daemon now picks a live upstream per-request: it tries the configured URL first, falls back to Ollama (`:11434`) and LM Studio (`:1234`), and discovers a chat-capable model from the selected backend's `/v1/models` before forwarding. Ollama and LM Studio reject `/v1/chat/completions` without a `model` field; we now populate it.
- **Empty 502 → structured 503.** When *no* runtime is reachable, the daemon returns HTTP 503 with a JSON `{error: {type: "upstream_offline", configured, checked, hint}}` body instead of a bare 502 with an empty body.
- **Connection sidebar tells the truth.** When the configured upstream is offline but an alternate backend is running, the sidebar now says "ollama reachable · auto-routing to 127.0.0.1:11434" instead of "upstream offline — start `llama-server`".
- **Chat error banner is actionable.** The "no model runtime is responding" banner lists which ports were probed and links to install docs, replacing the cryptic `[error: node returned 502 bad gateway]`.

### Added
- **`unhosted doctor`** — CLI command that probes llama-server / Ollama / LM Studio on their default ports and prints OS-specific install hints when none are reachable.
- **Startup probe banner.** `unhosted serve` reports which runtimes are alive on boot, and prints the `UNHOSTED_LLAMA_SERVER_URL=…` line to set when the configured upstream is down but an alternate is up.
- **`/v1/status` reports per-backend reachability** in a new `upstream.backends[]` array, used by the connection sidebar to suggest a switch.
- **QUIC peer transport with Ed25519-bound certs** (diagnostic in this release; `UNHOSTED_QUIC_RUN=1` opts in to QUIC-routed `/v1/run`). Each daemon's TLS cert is bound to its identity key — MITM-resistant by construction.
- **Hole-punch coordination via the relay** so two paired peers on different home networks can establish a direct UDP path. Falls back to ciphertext-relay when symmetric NATs prevent direct connection.
- **Pair-accept-via-relay** — cross-NAT pairing now works end-to-end without manual port forwarding.
- **Short pair codes** — 4-letter codes replace 100-char URIs for the common case.
- **Phase A security hardening:** bearer auth for non-loopback callers, signed-request replay defense, relay capacity caps, mDNS pubkey announcements, signed `X-Unhosted-Auth` headers between trusted peers.
- **Linux + Windows desktop shell** — `unhosted-desktop` ships in every release; installer drops a `.desktop` launcher (Linux) or Start Menu shortcut (Windows).
- **Trust badge in the peer list** so paired-with-pubkey peers are visibly distinct from unauthenticated LAN peers.
- **Auto-restore paired peers** — when a peer's IP drifts (router reboot), mDNS-discovered pubkey matches restore the registry entry without re-pairing.
- **CORS support** so browser-based clients on other origins can call the daemon.
- **Landing page rework.** New `how it works` deep-dive section (five numbered blocks with diagrams), `what works today` status table, `use it as a backend` (OpenAI-compat curl + client list), FAQ section, dedicated per-OS desktop-app install block. Reordered for legibility, primary CTAs above the fold.

### Changed
- Intel Mac (`x86_64-apple-darwin`) dropped from the release matrix — macos-13 runners queue hours behind the others. Apple Silicon binary runs fine under Rosetta 2; Intel users on bare hardware can build from source.
- Aggressive release profile: -26% desktop binary, -23% relay binary.

### Project
- ADR 0001: clarified what "designed multi-chain" actually means.
- ADR 0006: public-mode onboarding + wallet binding.

## [0.0.3] — 2026-05-11

### Added
- **`unhosted pull <model>`** — downloads a known GGUF into `~/.cache/unhosted/models/` with a live progress bar. Short names registered today: `llama3.2:1b`, `llama3.2:3b`, `llama3.1:8b`, `qwen2.5:0.5b`, `qwen2.5-coder:7b`. Direct `https://…/.gguf` URLs also accepted.
- **`unhosted models`** — lists registered models, sizes, and which are already cached locally.
- **System prompt** anchors the assistant's voice across all requests: plain, direct, no "as an AI" disclaimers, no marketing words. Same rules as `BRAND.md`.
- **`/v1/chat/completions`** is now the upstream endpoint (was `/completion`) — applies the model's instruction-tuning chat template so prompts are interpreted correctly instead of as raw text continuation.
- **mDNS discovery + pairing.** Each daemon auto-announces as `_unhosted._tcp.local.` and browses for peers. The UI shows discovered-on-LAN peers with a one-click pair button. Pairing hot-reloads the router with no daemon restart.
- **Embedded web UI** at `http://127.0.0.1:7777/` with sidebar layout, real localStorage-backed conversation history, theme toggle (auto / dark / light), and a live "served by" tag on every assistant message.
- **macOS `.app` bundle** at `dist/unhosted.app` via `scripts/bundle-macos.sh` — branded trust-gradient icon, proper Info.plist, ad-hoc codesigned.
- **Desktop shell** (`unhosted-desktop`) via tao + wry — native window pointed at the daemon, no Chromium bundle.
- **Request distribution wired end-to-end (v0.0.2).** The daemon now loads peers from the registry at startup and round-robins each request across `Local + Peer(s)` in priority order. Peer requests forward over HTTP to the peer's own `/v1/run`; loop prevention via `X-Unhosted-Forwarded` header; on peer failure the request falls back to local. Each response carries `X-Unhosted-Served-By: local | peer:<name>` so callers can see where work happened. Verified end-to-end with two daemons on one machine: 4 sequential requests alternated cleanly `local → peer:peerB → local → peer:peerB`.
- **Embedded web UI** served by `unhosted serve` at `http://127.0.0.1:7777/`. Chat interface with streaming responses, status indicator polling `/health`, suggestion chips, dark-mode aware. Vanilla HTML/CSS/JS embedded into the binary via `rust-embed`. Foundation for the Tauri desktop shell in v0.2.0 — the desktop app wraps this same UI.
- v0.0.2 architecture captured in [design/0003](design/0003-two-node-lan-cluster.md): two-node LAN cluster ships request distribution first; VRAM-pooling via llama.cpp's RPC backend is v0.0.3+ (requires custom llama.cpp build).
- `unhosted peer add | list | remove` subcommands. Peers persist to `~/.config/unhosted/peers.toml` (XDG-respecting). Routing picks up new peers on next `unhosted serve` restart; hot-reload deferred to v0.0.3.
- `unhosted_core::peer` and `unhosted_core::router` modules with `Peer`, `PeerRegistry`, `Target`, and `Router` types. Four unit tests cover dedup-by-name, priority-sort, local-only routing, and round-robin rotation.
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
