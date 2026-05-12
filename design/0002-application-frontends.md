# 0002 — Application frontends: CLI, web UI, desktop app

**Status:** Accepted — fully realized.
**Captured:** 2026-05-09
**Updated:** 2026-05-12 (v0.0.7: Tauri migration shipped earlier than originally targeted).
**Targets:** CLI shipped in v0.0.1. Web UI shipped in v0.0.3 (embedded in the daemon). Desktop app shipped first as raw `tao`+`wry` in v0.0.4 and migrated to Tauri 2 in v0.0.7 — earlier than the v0.2.0 target because the auto-updater and signed-installer story was needed to stop manually re-installing every release.

This ADR answers the question "will there be an application to run this?" Yes — and there will be more than one. The CLI is the first surface, not the only one.

## The shape

Unhosted is a daemon plus a thin set of frontends:

```
                       ┌─ unhosted CLI            (v0.0.1, shipped)
                       │
   unhosted-core ──────┼─ built-in web UI          (v0.1.0+)
   (the daemon)        │
                       ├─ desktop app (tray-based) (v0.2.0+)
                       │
                       └─ mobile companion         (post-v1.0, maybe never)
```

Every frontend talks to the same daemon over HTTP. The daemon is the product. Each frontend is a different way to drive it.

## Decisions

### 1. Multi-frontend from day one (architecturally)

The daemon is the source of truth. CLI, web UI, desktop app, mobile — all clients of the same HTTP API. Adding a new frontend should never require changes to `unhosted-core`.

Implication: the HTTP API is a stable contract from v0.1.0 onward. Breaking changes require versioning (`/v1/run` → `/v2/run`).

### 2. Built-in web UI in v0.1.0+

`unhosted serve` will eventually serve a web UI at `http://127.0.0.1:7777/` (or whatever the local node addr is). The same binary that runs the daemon hosts the static UI assets. No separate install for the UI.

Why not external website pulling from a local API: cross-origin, mixed-content, and cert hassles in a browser pointed at localhost. Bundling is simpler.

What it provides: model picker, chat surface, node dashboard (which devices are online, GPU utilization), trust-radius visualizer, public-mode balance.

Stack: vanilla HTML/CSS/JS or a small SPA framework. We'll decide closer to v0.1.0 — over-deciding now is the trap.

### 3. Desktop app via Tauri in v0.2.0+

When a desktop GUI ships, it's **Tauri**, not Electron.

Why Tauri:
- Rust-based — matches the existing stack; we don't have to context-switch to JS for the wrapper layer.
- ~5MB binary, system webview — vs Electron's ~100MB Chromium bundle. The manifesto says "AI on hardware you own"; we don't make people install a second Chromium to use it.
- Tray app, auto-updates, native menus — all first-class.
- Cross-platform (Mac, Windows, Linux) from one codebase.

Why not Electron: bundle size. Why not native (SwiftUI / WPF / GTK): platform fragmentation. Why not just a Progressive Web App: no tray, no auto-start at login, no easy local-process control.

The Tauri shell wraps the same web UI from decision (2). One UI codebase, two deploy surfaces (browser-served and desktop-bundled).

### 4. CLI is forever

The CLI never gets deprecated. Power users, scripts, CI jobs, agents — all need a no-GUI path. `unhosted` the binary remains the canonical interface for non-interactive use.

## What we won't do (yet)

- **No mobile app pre-v1.0.** Phones can't really run inference for the models that matter; mobile would be a control surface for a remote home node. Worth doing eventually, not now.
- **No "Unhosted Cloud" hosted dashboard.** Manifesto rule. There is no managed tier.
- **No browser extension.** Out of scope.
- **No SaaS sign-in.** Auth is local-first; trusted-mode pairing is direct (WireGuard-style); public-mode is wallet-based.

## Open questions (deliberately not deciding yet)

- Web UI framework — vanilla, htmx, Svelte, Solid, React. Decide at v0.1.0 start based on team and weight.
- How the web UI handles streaming — SSE (current daemon), WebSocket, or something else.
- Single-binary embedded UI (assets compiled into the Rust binary via `rust-embed`) vs. shipped alongside as static files.
- Desktop app's relationship to the daemon — embed the daemon in the same process, or always spawn it as a subprocess.

These become ADRs when v0.1.0 / v0.2.0 design starts.
