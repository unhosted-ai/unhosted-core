# Changelog

All notable changes to Unhosted are recorded here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until v0.1.0 the API and CLI surface may break between releases — we'll note it loudly when it does.

## [Unreleased]

## [0.0.37] — 2026-05-16

### Added
- **VRAM-pool sidebar UI now exposes multi-peer.** Until now the
  "start pool" button in the sidebar fired a hardcoded
  self-loopback plan (one layer host: `local @ 127.0.0.1:50052`).
  v0.0.35 made the multi-peer wire path functional, but the UI
  was stuck on the single-machine plan. v0.0.37 adds a
  layer-host picker below the model input:

  ```
  cluster (vram-pool)
  ready — pick a model and click start
  [ path to .gguf                       ]
  layer hosts
    □ thunder         [TRUSTED]
    □ homelab         [TRUSTED]
  [ ▶ start pool ]
  ```

  The picker is rebuilt from `/v1/status.peers` on every poll, so
  newly-paired peers show up without a reload. Checkbox state is
  persisted to `localStorage` (key
  `unhosted-vram-pool-selected-peers`) so a reload doesn't drop
  the selection. Unpaired peers are pruned from the saved set
  automatically. Hidden when no peers are paired (self-loopback
  is the only sensible option) and hidden during
  `Starting/Running/Hosting/Failed` states (mid-flight topology
  changes aren't supported; stop + restart with a new selection
  is the path).

  When a peer is selected, the click builds a plan with
  `LayerHost { name, addr: <peer-host>:50052 }` for each chosen
  peer. With no peers selected, the plan stays as the
  self-loopback default — matching the planner's behavior so
  pre-v0.0.37 users see no regression.

## [0.0.36] — 2026-05-16

### Fixed
- **mDNS auto-restore was rewriting peer addrs to broken
  IPv6 link-locals.** `status_handler` had an auto-restore step
  that updated a paired peer's stored address whenever mDNS
  reported a different one for the matching pubkey. The
  intended use case (router reboot ⇒ peer's IP rotated) worked
  fine, but on macOS mDNS frequently broadcasts an IPv6
  link-local (`fe80::*`) for peers on the same interface. Those
  addresses need a zone identifier (`%en0`) to connect, which
  `SocketAddr` doesn't carry — so every peer-to-peer call after
  the first `/v1/status` poll started failing with
  "Connection refused". The bug was harmless before v0.0.34
  because no in-daemon code paths actually made peer HTTP
  calls; v0.0.34's vram-pool orchestrator started doing so and
  the bug surfaced.

  Fixed with a `discovered_is_better` heuristic: only swap the
  stored address when the new one is at least as reachable as
  the old. Loopback > LAN-private (RFC1918 / IPv6 unique-local)
  > public > link-local. A paired peer at `192.168.1.42:7777`
  no longer gets stomped by `fe80::*` broadcasts; mDNS for the
  rotated-LAN-IP case still works because both addresses are
  rank "LAN-private" so the swap is accepted.

  5 unit tests cover the heuristic, including the exact
  two-daemon-same-Mac scenario that surfaced the bug. Total
  daemon tests now 45 passing.

## [0.0.35] — 2026-05-16

### Milestone
- **VRAM-pool multi-peer end-to-end VERIFIED.** The full path from
  `unhosted vram-pool start --peers <name>` on the orchestrator
  through to `pong` returning from a chat completion routed across
  two daemons works. ADR 0009's deep-tech bet is functionally
  proven.

  Verified by running two unhosted daemons on this Mac (different
  config dirs via `XDG_CONFIG_HOME`, bound to `127.0.0.1:7777`
  and `:7778`), pairing them via manually-written `peers.toml`,
  and observing the orchestration dance live in the logs:

      A: vram-pool: asking peer to host    peer=daemonB
                       ↓ X-Unhosted-Auth signed request
      B: vram-pool: spawning rpc-server as layer host
      B: vram-pool: hosting layers         port=50052 orchestrator=<A_pubkey>
      A: vram-pool: spawning llama-server  rpc=127.0.0.1:50052
      A: vram-pool: orchestrator answering /v1/models — transitioning to Running
      $ curl /v1/chat/completions { … "say pong" … } → "pong"
      $ unhosted vram-pool stop          (A propagates stop to B)
      A: idle  B: idle  (zero leaked rpc-server / llama-server processes)

  The single-Mac simulation uses one GPU under both roles so there's
  no actual VRAM pooling, but the protocol — signed peer request,
  layer-host child supervision, orchestrator transition, chat
  routing, coordinated shutdown — is exactly the same code path
  that runs on two physical machines.

### Fixed
- **Pure-orchestrator (no local layer host) collapsed instantly on
  start.** The spawn supervisor treated `rpc_child.is_none()` as
  "child is dead" and transitioned to `Failed` within 2 s of
  `Starting`. Multi-peer plans where the local box is orchestrator-
  only (every layer host is remote) hit this on the first poll.
  Fixed by changing the supervisor's `None` arm to "nothing to
  watch" rather than "dead", and having the layer-host supervisor
  exit cleanly when its tracked child is absent.

## [0.0.34] — 2026-05-16

### Added
- **VRAM-pool: orchestrator side of multi-peer (phase 2c, half 2).**
  Completes ADR 0009 phase 2c. v0.0.33 shipped the layer-host
  endpoint; v0.0.34 wires the orchestrator to call it. When the
  start request includes peer layer hosts:

  1. For each non-local layer host in the plan, the orchestrator
     looks the peer up in its registry, signs a JSON body
     `{ port, orchestrator: <our_pubkey> }` with its own identity,
     and POSTs to `<peer>/v1/vram-pool/layer-host/start` with an
     `X-Unhosted-Auth` header.
  2. The peer's daemon validates the signature against its own
     peer registry (this orchestrator must be paired with the
     peer), then spawns `rpc-server` and returns when the bind
     probe succeeds.
  3. The orchestrator only calls `PoolManager.start()` (which
     spawns local `rpc-server` + `llama-server --rpc=…`) AFTER
     every remote peer confirms `Hosting`.
  4. On any peer rejection mid-loop, the orchestrator sends
     `layer-host/stop` to peers that did succeed before bailing —
     so a partial failure doesn't leak `rpc-server` processes on
     other boxes.
  5. `vram_pool_stop_handler` snapshots remote layer hosts from
     the current plan, stops locally, then sends `layer-host/stop`
     to each. Best-effort: an unreachable peer doesn't block
     local stop.

  `PoolManager` itself stays local-machine-only. Peer coordination
  lives in the route handler so PoolManager doesn't need NodeState
  access (identity, registry, http client). Clean separation —
  PoolManager is "what this box does", the handler is "how the
  cluster coordinates".

### Verified
- Self-loopback still works end-to-end after the refactor:
  `vram-pool start` → `running` in ~8 s, chat routes through
  pool, `stop` cleans up, no leaked processes. The multi-peer
  path is not testable on this machine without a second box
  (a stretch goal would simulate by running two daemons on the
  same Mac at different ports + pairing them — not in this
  release).

## [0.0.33] — 2026-05-16

### Added
- **VRAM-pool: layer-host role (ADR 0009 phase 2c, half 1 of 2).**
  Until now, `PoolManager` knew only the orchestrator role — it
  could spawn `rpc-server` + `llama-server --rpc=…` locally, but
  there was no way to ask a *remote* peer to spawn an `rpc-server`
  on the orchestrator's behalf. This release ships the other side
  of that protocol: any daemon can now host layers for a paired
  peer's pool.

  - New `PoolState::Hosting { orchestrator, port }` variant.
  - New `PoolManager::start_as_layer_host(port, orchestrator)`.
    Spawns `rpc-server -p <port> -H 0.0.0.0`, TCP-probes for the
    bind, transitions state to `Hosting`. A `spawn_layer_host_supervisor`
    task watches the child and transitions to `Failed` if it dies
    unexpectedly so the remote orchestrator's status probe can
    pick that up and re-plan.
  - Two new HTTP endpoints, **paired-peer auth required** (not
    loopback or bearer like the orchestrator-side endpoints — the
    point is that a remote daemon is calling in):
    - `POST /v1/vram-pool/layer-host/start` body `{ port, orchestrator }`
    - `POST /v1/vram-pool/layer-host/stop`

  Verified the auth gate on this Mac: loopback-without-signature
  call to `/v1/vram-pool/layer-host/start` returns `403 layer-host
  operations require a paired-peer signed request`. The signed-
  request path (orchestrator daemon → layer-host daemon) lands in
  the next release.

### Pending for v0.0.34
- Orchestrator side: when `PoolManager::start` sees remote layer
  hosts in the plan, signs and sends `/v1/vram-pool/layer-host/start`
  to each peer before spawning local `llama-server --rpc=…`. On
  stop, sends the matching layer-host/stop. PoolManager-internal
  vs. route-handler-internal split for the peer calls is a design
  question to resolve before that ships — currently leaning toward
  putting the peer dance in the route handler so PoolManager
  stays subprocess-only.

## [0.0.32] — 2026-05-16

### Added
- **Chat proxy auto-routes through the VRAM-pool when Running.**
  New `resolve_upstream` runs before every chat-completion proxy.
  When `state.vram_pool.status() == Running`, it returns the
  pool's `endpoint` + `plan.model` instead of probing the
  configured-upstream chain. Net effect: starting the pool from
  the sidebar (or CLI) makes EVERY chat through this daemon hit
  the pool, including chats over the tunnel and via external
  agents — no separate routing config.
- **`/v1/status.upstream` reflects the pool when Running.** The
  status handler short-circuits to the pool's endpoint + model
  instead of running its own `probe_upstream` against the user's
  configured upstream URL. Matches the routing decision exactly,
  so the UI's "node ready" indicator can't disagree with where
  chats actually go.

### Fixed
- **VRAM-pool: rpc-server bind race.** v0.0.30/0.0.31 used a
  static 1500 ms sleep between spawning `rpc-server` and
  `llama-server`. Insufficient on macOS Metal — `rpc-server`'s
  Metal init takes 2–4 s before the TCP port binds. `llama-server`
  fired during the gap, failed to dial its `--rpc` backend, and
  exited — supervisor then read both children as dead and
  collapsed the pool. Replaced the sleep with a TCP probe loop
  capped at 10 s. Logs the actual wait time so we can tighten
  later if useful.
- **VRAM-pool: stderr deadlock.** Children were spawned with
  `stderr(Stdio::piped())` but the pipe was never drained. After
  ~64 KB of model-load logs from `llama-server` the pipe filled,
  the child blocked on write, the supervisor saw the now-frozen
  child as "dead", and the pool collapsed ~10 s in. Switched to
  `Stdio::inherit()` so child stderr surfaces in the daemon's
  own logs — both gives operators visibility and avoids the
  deadlock. Piping with a proper drainer task is a future
  improvement.

### Tap (separate repo)
- `unhosted-ai/homebrew-unhosted/Formula/llama-cpp-rpc.rb` now
  passes `-DGGML_BLAS=OFF`. Workaround for an upstream llama.cpp
  b9090 bug: when `rpc-server` receives a graph containing
  `RMS_NORM` and BLAS is the assigned backend, it aborts with
  "unsupported op RMS_NORM" and `llama-server` reports the remote
  RPC crash. Disabling BLAS routes RMS_NORM through CPU/Metal
  which handle it correctly. Re-enable when upstream catches up.
  `brew reinstall unhosted-ai/unhosted/llama-cpp-rpc` to pick
  up the fix.

### Verified end-to-end on this Mac (Apple M1 Max)

      $ unhosted vram-pool start --model ~/.cache/.../Llama-3.2-1B-Instruct-Q4_K_M.gguf
      [...]
      [t+5s] state=running

      $ curl http://127.0.0.1:7777/v1/status | jq .upstream
      {
        "url": "http://127.0.0.1:8080",
        "reachable": true,
        "model": "…/Llama-3.2-1B-Instruct-Q4_K_M.gguf"
      }

      $ curl -X POST http://127.0.0.1:7777/v1/chat/completions \
          -d '{"model":"local","messages":[{"role":"user","content":"hi"}], ...}'
      "I'll control the paddle. You hit"   # served by the pool

Pool start → /v1/status flip → chat round-trip, all in ~5 s on
a 1B model. Multi-GB models scale with mmap time (~30 s for a
7B Q4); the model-load poller's 90 s cap covers them.

## [0.0.31] — 2026-05-16

### Added
- **VRAM-pool: real model-load detection (phase 2d).** v0.0.30
  spawned children, slept 800 ms, declared `Running`. A chat against
  `:8080` during the model's multi-GB mmap window would 503 even
  though the daemon reported `Running`. Now: spawn → state stays
  at `Starting{stage: waiting_for_orchestrator}` → background
  poller hits `/v1/models` every 800 ms → flip to `Running` the
  moment the orchestrator answers. Hard cap of 90 s; on timeout we
  kill the children and surface a `Failed` state with a useful
  message ("model didn't finish loading within 90s — check the
  .gguf path and free VRAM").

  Consumers gating on `state === "running"` now see the truth, not
  an optimistic guess. The `unhosted vram-pool start` HTTP handler
  still returns immediately with the `Starting` state — clients
  poll `/v1/vram-pool` for the transition.

- **VRAM-pool: sidebar controls in the web UI.** The capability
  panel from v0.0.27 grows a model-path input + start/stop buttons
  + an endpoint readout. Layout:

  ```
  cluster (vram-pool)
  ready — pick a model and click start
  [ path to .gguf                       ]
  [ ▶ start pool ]
  ```

  After start:
  ```
  starting — waiting for orchestrator…
  [ ./model.gguf (disabled) ]
  [ ■ stop pool ]
  ```

  After Running:
  ```
  running ./model.gguf across 1 layer host
  [ ./model.gguf (disabled) ]
  [ ■ stop pool ]
  http://127.0.0.1:8080
  ```

  Pool state polling cadence: every 1.5 s while transitioning,
  on the normal /v1/status tick otherwise. Stop pops the in-app
  confirm dialog so an accidental click doesn't kill an
  in-flight pool.

  Multi-peer not in this UI slice — start button currently fires
  a self-loopback plan only, matching what the supervisor knows
  how to run (phase 2c remains gated on multi-peer orchestration).

## [0.0.30] — 2026-05-16

### Added
- **VRAM-pool spawn supervisor (ADR 0009 phase 2b).** The piece
  that turns the v0.0.29 plan into actual running subprocesses.
  Self-loopback only for this slice — multi-peer needs peer-side
  `rpc-server` orchestration that we don't have yet.

  - New `vram_pool::PoolManager` (modeled on `TunnelManager`)
    owns the `rpc-server` + `llama-server --rpc=…` child processes.
    State machine: `Idle → Starting{spawning_rpc, waiting_rpc,
    spawning_orch, waiting_orch} → Running{plan, endpoint}`. A
    background supervisor task watches both children and
    transitions to `Failed{error, plan}` if either exits
    unexpectedly. Stricter than `tunnel::TunnelManager`'s
    auto-restart posture: a dying child cancels the pool rather
    than reviving — in-flight inferences don't gracefully recover
    from a backend swap.

  - Three new HTTP endpoints, local-user-only:
    - `GET    /v1/vram-pool`       → current `PoolState` JSON
    - `POST   /v1/vram-pool/start` → body `{ plan: Plan }`
    - `POST   /v1/vram-pool/stop`  → returns `Idle`

  - `unhosted vram-pool start --model <path>` now builds the plan,
    POSTs to the daemon, and reports the daemon's response.
    `unhosted vram-pool stop` POSTs the stop. `unhosted vram-pool
    status` GETs.

  Verified end-to-end on this Mac (tap install from v0.0.28):

      $ unhosted vram-pool start --model ~/.cache/unhosted/models/Llama-3.2-1B-Instruct-Q4_K_M.gguf
      VRAM-pool plan:
        orchestrator       : local
        layer hosts        :
          - local        @ 127.0.0.1:50052
      posting to local daemon at http://127.0.0.1:7777/v1/vram-pool/start …
      daemon accepted. current state:
      { "state": "running", "endpoint": "http://127.0.0.1:8080", ... }

  Daemon log captured the spawn sequence:

      vram-pool: spawning rpc-server   bin=/opt/homebrew/opt/llama-cpp-rpc/bin/rpc-server port=50052
      vram-pool: spawning llama-server bin=…/llama-server port=8080 rpc=127.0.0.1:50052 model=…
      vram-pool: stop requested  (after `unhosted vram-pool stop`)

### Known limitations / scope notes
- Self-loopback only. Pooling across multiple peers requires the
  orchestrator daemon to ask each peer's daemon to spawn its own
  `rpc-server` — that coordination protocol lands in the next
  slice. Right now `unhosted vram-pool start --peers a,b` will
  build the plan but the supervisor rejects with "multi-peer not
  yet implemented".
- Optimistic `Running` transition. We sleep
  `LLAMA_SERVER_BIND_GRACE` (800 ms) after spawning `llama-server`
  and call it `Running` — the actual model `mmap` can take 5–30 s
  for a multi-GB model. The pool reports `Running` before
  `llama-server` is actually answering on `:8080`. The watchdog
  catches a child that crashes during that window. A future
  improvement polls `llama-server`'s `/v1/models` to confirm the
  model loaded before declaring `Running`.
- No persistence. A daemon restart while a pool is active drops
  the pool. ADR 0009 §Q3 is "ephemeral for v0.1.0"; that holds.

## [0.0.29] — 2026-05-16

### Added
- **VRAM-pool plan generator (ADR 0009 phase 2a).** `vram_pool::plan`
  is a pure function over (local capability, candidate peers, requested
  peers, model) → `Plan { orchestrator, layer_hosts, model }`. The
  spawn supervisor (phase 2b) consumes the plan to actually spawn
  `rpc-server` + `llama-server --rpc=…` processes; getting the
  decision logic isolated first keeps spawn work clean and lets the
  CLI surface a useful "what would run" preview today.

  Two topologies in scope:
  - **Self-loopback** — no peers requested, local machine runs both
    `llama-server` and a local `rpc-server` on `127.0.0.1:50052`.
    Useful for testing the supervisor on a single box without
    actually pooling any VRAM (the model still fits on one GPU; the
    `--rpc` round-trip just exercises the wiring).
  - **LAN cluster** — orchestrator is local, layer hosts are named
    peers from `--peers a,b`. Refuses on unknown peer names rather
    than silently dropping; refuses if none of the requested peers
    are RPC-capable.

  Error type `PlanError` covers four failure modes with messages
  that name the gap: `NotReady`, `UnknownPeer(name)`,
  `ModelMissing`, `NoRpcCapablePeers`.

  `unhosted vram-pool start --model llama3.1:70b` now builds the
  plan and prints the exact `llama-server` + `rpc-server` commands
  it would invoke. End-to-end on this Mac with the tap installed:

      VRAM-pool plan (preview — actual spawn lands in the next slice):
        orchestrator       : local
        model              : llama3.1:70b
        layer hosts        :
          - local        @ 127.0.0.1:50052
        llama-server cmd   :
          /opt/homebrew/opt/llama-cpp-rpc/bin/llama-server \
            -m llama3.1:70b --rpc 127.0.0.1:50052 --gpu-layers 99
        rpc-server cmd     :
          /opt/homebrew/opt/llama-cpp-rpc/bin/rpc-server -p 50052

  7 new unit tests cover the planner's branches: self-loopback when
  capable, error when local-incapable + no peers, error without
  model, cluster plan with mixed-capable peers, silent skip of
  non-capable peers, error when all requested peers incapable,
  error on unknown peer name.

## [0.0.28] — 2026-05-16

### Added
- **VRAM-pool probe now finds the `unhosted-ai/homebrew-unhosted`
  tap install.** The new tap (published today at
  `https://github.com/unhosted-ai/homebrew-unhosted`) ships an
  RPC-enabled `llama.cpp` build at the keg-only opt-prefix
  (`/opt/homebrew/opt/llama-cpp-rpc/bin/`). Unlinked because its
  `lib/` and `include/` directories collide with the upstream
  `ggml` and `llama.cpp` formulas every Mac user already has —
  letting brew force-link would break the standard `llama-server`.

  The probe in `vram_pool::probe` now checks the well-known
  opt-prefix paths first (`/opt/homebrew/opt/llama-cpp-rpc/bin/`
  for Apple Silicon, `/usr/local/opt/llama-cpp-rpc/bin/` for
  Intel macOS), then falls back to `PATH` for users on custom
  llama.cpp builds. When the resolved binary is from the tap,
  the `--rpc` capability is trusted without spawning a `--help`
  subprocess (the formula's `test` block proves the flag is
  present before install is allowed to succeed).

  The install-hint message now points users at the tap by name
  and prints the exact `brew tap` + `brew install` commands.

### Verified end-to-end on this Mac

  $ brew tap unhosted-ai/unhosted
  $ brew install unhosted-ai/unhosted/llama-cpp-rpc
  $ unhosted vram-pool detect
  VRAM-pooling capability on this machine:
    llama-server        : /opt/homebrew/opt/llama-cpp-rpc/bin/llama-server
    llama-server --rpc  : yes
    rpc-server          : /opt/homebrew/opt/llama-cpp-rpc/bin/rpc-server
    ready for pool      : YES

That's the distribution loop closed for macOS Homebrew users —
two `brew` commands, no PATH changes, no source-builds. Linux /
Windows users still need their own RPC-capable build until ADR
0009 §Q4's parallel work lands distribution stories for those
platforms.

## [0.0.27] — 2026-05-16

### Added
- **VRAM-pool sidebar panel (UI view #3).** v0.0.26 shipped two
  ways to view cluster capability — `unhosted vram-pool detect`
  on the CLI and the `vram_pool` field on `GET /v1/status`. This
  release adds the third: a "cluster (vram-pool)" section in the
  sidebar between "send to my phone" and "for developers". One
  short status line (`ready — this machine can join…` /
  `built without -DGGML_RPC=ON — click details` / `no llama-server
  found`), a `details` button that opens a modal with the resolved
  binary paths plus the targeted install hint, and live updates
  on every `/v1/status` poll so a `brew install` flip shows up
  without a daemon restart.

  The orchestration commands (`unhosted vram-pool start/stop/status`)
  still ship in v0.1.0; this is purely the visibility layer.

## [0.0.26] — 2026-05-15

### Added
- **VRAM-pooling — detection foundation (ADR 0009).** First slice
  of the deep-tech bet that's been on the roadmap since ADR 0003.
  Orchestration ships in v0.1.0; v0.0.26 lands the detection layer
  underneath:

  - New module `unhosted-core/src/vram_pool.rs` with
    `RpcCapability` + `probe()`. Cheap: two PATH lookups and one
    `llama-server --help` subprocess. Reports whether this machine
    has `rpc-server` on PATH and `--rpc` in `llama-server --help`
    — the canonical signals that the local llama.cpp build was
    compiled with `-DGGML_RPC=ON`.

  - `GET /v1/status` exposes a new optional `vram_pool` field with
    the probe result, so the UI (and any external observer) can
    surface "this machine is ready for VRAM-pooling" without
    re-running the probe themselves.

  - New CLI subcommand tree:
    ```
    unhosted vram-pool detect             # works today
    unhosted vram-pool start --model …    # stub, prints capability + plan
    unhosted vram-pool stop               # stub
    unhosted vram-pool status             # reports local capability
    ```
    `detect` does the real work. `start` / `stop` / `status` are
    deliberate stubs that print "not yet implemented in this slice"
    plus the local capability — the command surface lands now so
    v0.1.0 fills in the orchestration without reshaping the CLI.

  Verified locally on this Mac (Homebrew `llama.cpp` 9090, NOT
  built with RPC), `unhosted vram-pool detect` correctly reports:

  ```
  llama-server        : /opt/homebrew/bin/llama-server
  llama-server --rpc  : no — build lacks -DGGML_RPC=ON
  rpc-server          : (not found on PATH)
  ready for pool      : no
  ```

  with a targeted install hint pointing at the
  `unhosted-ai/homebrew-unhosted` tap (also unpublished, draft at
  `design/0009-vram-pooling.tap-formula-draft.rb`) as the
  recommended fix.

## [0.0.25] — 2026-05-15

### Added
- **LLM web browsing — phase 1: the endpoint.** New
  `POST /v1/tools/web_fetch` lets the UI, an external agent, or
  (eventually) the LLM via a tool-use loop pull a web page through
  the daemon and get back plain-text content the model can reason
  about. Local-user-only auth, same posture as `/v1/memory` and
  `/v1/tunnel` — only the daemon owner can drive outbound fetches
  through their machine.

  New module `unhosted-core/src/web_fetch.rs`. The request is a
  thin `{ url, max_bytes? }`, the response carries `final_url`,
  `status`, `content_type`, `bytes`, `truncated`, and a stripped
  `content` field suitable for splicing into a chat-completion
  request.

  Phase 2 (separate release) will close the tool-use loop:
  intercept model output for `<fetch>...</fetch>` markers, resolve
  them, feed the result back into the next turn, and pipe the
  reads through the memory summarizer so what the model
  researches becomes persistent context.

### Security
- **SSRF guards.** The fetcher refuses any host that resolves to
  loopback, RFC-1918 (10/8, 172.16/12, 192.168/16), link-local
  (169.254/16, fe80::/10), unique-local (fc00::/7), CGNAT
  (100.64/10), multicast, broadcast, or the unspecified address
  (0.0.0.0, ::). Resolution is done up-front via `tokio::net::
  lookup_host` so a public hostname resolving to a private IP is
  rejected — not just literal private IPs typed into the URL.
- **HTTPS-only.** `http://` is refused at the URL parser, before
  DNS or any network IO. The only safe HTTP target inside the
  same machine is the daemon itself, which callers can reach
  directly.
- **Bandwidth cap.** Default `max_bytes = 200_000`. Callers can
  request smaller but not larger. The body is streamed; the
  connection drops the moment the cap is hit, so an attacker
  can't make the daemon spool a multi-GB response.
- **Network timeout.** 15 s connect+read deadline. A slow-loris
  source can't tie up the chat loop indefinitely.
- **Recognizable User-Agent.** `unhosted/<version>
  (+https://github.com/unhosted-ai/unhosted-core)` — target
  sites can robots.txt us if they want; we're not hiding behind
  a vanilla curl UA.
- **No request-header passthrough.** The outbound fetch is a
  fresh client. Cookies, auth headers, and any other headers on
  the incoming `/v1/tools/web_fetch` request never reach the
  target.

### Tests
- 6 unit tests cover the SSRF guard against every IPv4/v6
  private range, the HTML stripper's tag/script/style removal,
  whitespace collapsing, entity decoding, and the text/binary
  content-type discrimination. 5 e2e smoke tests verified
  locally before push: real HTTPS fetch through `example.com`,
  HTTP rejected, RFC-1918 rejected, invalid URL rejected,
  byte-cap honored with `truncated: true`.

## [0.0.24] — 2026-05-15

### Added
- **Default system prompt for `/v1/chat/completions`.** `unhosted run`
  (the CLI one-shot) has always injected `DEFAULT_SYSTEM_PROMPT` to
  anchor the assistant's voice; `/v1/chat/completions` historically did
  not, so external callers (curl one-liners, agents, OpenAI-API
  libraries) inherited whatever the upstream model's default behavior
  was — usually the marketing-toned "I'm an AI assistant here to help!"
  opener that the project's voice explicitly rejects.

  New `ensure_default_system_prompt` runs in `proxy_chat_local` between
  `rewrite_placeholder_model` and `inject_memory_context`. If the
  caller already supplied a system message (anywhere in the messages
  array), we leave it alone. Otherwise we prepend one with the project
  default. Memory context, when enabled, still prepends to whichever
  system message ended up present.

  Verified end-to-end:

  - Request without a system message: model adapts to the prompt
    ("I am running a variety of open-source and user-supplied software
    designed for various AI tasks on GPU hardware provided by users.")
  - Request WITH `system: "You are a pirate, respond in pirate speak"`:
    daemon respects it, model replies "Avast ye! How fares the weather?"

  Debug trace `chat: injected default system prompt (caller sent
  none)` available under `RUST_LOG=unhosted_core=debug` for diagnosing
  cases where a downstream agent's own system prompt is being silently
  appended through.

## [0.0.23] — 2026-05-15

### Fixed
- **Peer-list row overflow.** When a paired peer's address was an
  IPv6 link-local (e.g. `[fe80::18bb:e56:8e3b:d558]:7777` — the
  common case for mDNS-discovered LAN peers), the address pushed
  the row wider than the sidebar and clipped the `unpair` button
  off the right edge. Root cause: the left column was a flex
  container without `min-width: 0`, so flex children couldn't
  shrink below their intrinsic content width.
  Fix: new `.peer-info` class with `min-width: 0` + `flex: 1 1
  auto`, ellipsis on both `.pname-text` and `.paddr`,
  `flex-shrink: 0` on `.unpair` so the action stays clickable.
  Full peer name + address still available on hover via `title`
  attributes. Same overflow guard applied to the
  `.discovered-list .dname .addr` line, which had the same shape
  bug (and the same affected addresses).

## [0.0.22] — 2026-05-15

### Added
- **Private memory — phase 3: semantic retrieval via bundled embedder.**
  Replaces the v0.0.20 keyword-overlap retriever with cosine similarity
  over real 384-dim embeddings from `BAAI/bge-small-en-v1.5`. The model
  is fetched once from Hugging Face on the first memory write (~33 MB,
  cached at `~/.cache/fastembed/`) and runs CPU-only thereafter; warm
  embeds take ~20 ms for the short summary strings we feed it.

  `MemoryEntry` gains an `embedding: Vec<f32>` field (empty by default,
  skipped on serialize when empty so the on-disk JSON stays small).
  Both `memory::add` and `memory::upsert_for_chat` now embed at write
  time, so every new entry — manual or auto-summarized — gets a vector
  the moment it lands. Old entries (pre-phase-3) read fine: serde
  defaults the field to an empty vec, and retrieval falls through to
  the keyword path for those.

  New top-level `memory::retrieve()` is what `proxy_chat_local` calls.
  Logic: embed the query, score every entry that has an embedding by
  cosine, drop matches below 0.30 as noise, return the top-3 by score.
  If the embedder hasn't initialized (no network on first run, no
  cache permission, etc.) the call falls all the way back to keyword
  overlap so retrieval still works degraded — never silently breaks.

  Verified end-to-end against Ollama + qwen2.5:3b:

    POST /v1/memory  (summary: "Rust developer building local AI daemon")
    POST /v1/memory  (summary: "knows Python well, likes async patterns")
    POST /v1/chat/completions  ("what languages do I work with?")
    → "You primarily work with Python, leveraging async patterns,
       and know Rust through developing a local AI daemon."

    The query never mentions Rust or Python by name — pure semantic
    match.

### Dependencies
- `fastembed = "5"` with `default-features = false` and only
  `hf-hub-native-tls` + `ort-download-binaries-native-tls` enabled.
  Drops the `image-models` bag and its `image` crate transitive deps.
  ONNX Runtime ships as a downloaded shared lib alongside our binary
  — users don't have to install onnxruntime themselves.

### Trade-offs (known, accepted)
- First-write cost on a fresh install: ~3 s to download + load the
  embedder. Subsequent writes are <50 ms. The init failure path is
  not cached, so a transient first-run network blip doesn't lock the
  feature out forever.
- Binary size grows ~30–40 MB across the four shipped platforms.
  Trade-off documented at the dep declaration in
  `crates/unhosted-core/Cargo.toml`.

## [0.0.21] — 2026-05-15

### Added
- **Private memory — phase 2: auto-summarize.** v0.0.20 shipped the
  storage layer with a manual "+ add note" textarea. v0.0.21 removes
  the manual step: every chat that's been updated with at least two
  messages now triggers a debounced background summarizer that calls
  the local LLM 30 s after the last edit, asks for a 1–2 sentence
  third-person summary "about the user, not the topic", and writes
  the result to the memory store keyed by `chat_id`. The same chat
  re-summarized later replaces its old entry (new
  `memory::upsert_for_chat`) rather than stacking duplicates, so even
  a chat that's been touched twenty times occupies one slot.

  Debounce is per-chat: rapid back-to-back saves (e.g., the per-token
  saves a streaming chat does) collapse into one upstream-LLM round
  trip 30 s after the burst stops, instead of N. Net cost on a
  typical session: ~1 short summarization call per active chat per
  burst, not per turn.

  `NodeState` gains a `summarize_inflight` map keyed by `chat_id`
  to hold the active `tokio::JoinHandle` so each new upsert can
  cancel and re-spawn the timer.

  Privacy posture is unchanged: still off by default. The
  summarizer no-ops on every entry path (chats_upsert handler check,
  inside the task before calling upstream) when `memory::is_enabled()`
  returns false. Nothing about chat history goes to anything other
  than the user's own configured local LLM.

  Verified locally end-to-end:
    `PUT /v1/chats/...` (4 msgs)
    →  log: "memory: scheduling summarizer ... msgs=4"
    →  (30 s)
    →  log: "memory: chat summary updated chat_id=..."
    →  `GET /v1/memory` shows the LLM-written summary

### Pending
- Replace the keyword retriever with a bundled embedder
  (`fastembed-rs`, ~25 MB ONNX). The summarization quality already
  makes a meaningful difference; embedding-based retrieval is the
  last piece before this loop reaches its target accuracy.

## [0.0.20] — 2026-05-15

### Added
- **Private memory — phase 1 (storage + UI toggle + keyword retrieval).**
  Opt-in RAG over the user's own past chats. New sidebar section "private
  memory" with the same toggle pattern as "open to internet": click once,
  state persists to `~/.config/unhosted/memory-enabled.txt`. When on, the
  daemon stores user-supplied summaries in
  `~/.config/unhosted/memories.json` (atomic write, capped at 50 FIFO),
  and the chat-completions proxy prepends the top-3 keyword-overlap
  matches to the system prompt before forwarding upstream. Nothing leaves
  the user's machine — retrieval is in-process, embeddings stay local.

  Five new endpoints, all local-user-only:
    - `GET    /v1/memory` — list + enabled flag
    - `POST   /v1/memory` — add `{ summary, chat_id? }`
    - `DELETE /v1/memory/{id}` — remove one
    - `POST   /v1/memory/clear` — wipe all
    - `POST   /v1/memory/enable` — set `{ enabled }`

  UI: sidebar toggle, "manage" button opens a modal listing every entry
  with delete + a "+ add note" textarea so the v0.0.20 loop is testable
  end-to-end without auto-summarize. Auto-summarize at chat end + a
  bundled embedder (`fastembed-rs`) land in v0.0.21 — same surface, the
  keyword retriever swaps out for cosine similarity.

  Privacy posture: default off. A missing or unreadable enable file
  reads as "off", so we can never inject memory context into an upstream
  call without an affirmative user click. Same posture as
  `tunnel-autostart.txt` in v0.0.19.

## [0.0.19] — 2026-05-15

### Added
- **Tunnel-enabled state persists across daemon restarts.** Until this
  release, every time the user closed the .app (or the system rebooted),
  cloudflared came down with the daemon and the user had to re-click
  "open to internet" before their phone / agent could reach the daemon
  again. Now the tunnel manager writes
  `~/.config/unhosted/tunnel-autostart.txt` on every `start`/`stop`
  call, and the daemon boot path OR's that file into its `eager_tunnel`
  decision alongside the existing `UNHOSTED_EAGER_TUNNEL` env var and
  the `--eager-tunnel` CLI flag.

  Effect: click `enable` once, and the tunnel comes back up
  automatically on every subsequent .app launch — no more re-clicking
  before the phone can reach the daemon. Click `stop` and it stays
  off until you re-enable.

  Reason this matters for agentic AI: an agent calling
  `/v1/chat/completions` over the public URL needs the daemon to be
  reachable without a human in the loop. Persistence closes that
  gap. The URL itself still rotates per cloudflared Quick Tunnel
  restart — that's a Cloudflare-side limit; the agent has to either
  re-resolve via a discovery channel or accept short-lived URLs.

  Defaults stay conservative: a missing or unreadable file reads as
  "off", so we can never publish the daemon without an affirmative
  user click. The env var still wins for operators running `unhosted
  serve` from systemd.

## [0.0.18] — 2026-05-15

### Fixed
- **Tunnel goes live but UI never shows URL / QR / copy button.**
  When the tunnel transitioned to running, the toast fired
  ("tunnel live — your phone can chat with this mac now"), then
  `renderTunnel`'s running branch immediately tripped on
  `const token = getToken() || "";` — `getToken` was never defined
  anywhere in `ui.js`. The function had always been called
  `getApiToken`. The `ReferenceError` aborted the rest of the
  render, so:

  - `els.tunnelUrl.textContent` never got the URL
  - `els.tunnelLink.hidden = false` never ran → copy button stayed
    hidden
  - `renderPhoneQr(linkHref)` never ran → QR stayed on the
    "enable open to internet first" hint

  User-visible outcome: enabling the tunnel showed the green toast,
  then nothing else changed — making the whole panel look broken,
  which is why this got tagged as the foundational gap blocking
  agentic AI use. The bug had been latent since v0.0.7 (when the
  toast was introduced); only the recent v0.0.17 optimistic-UI
  click handler made it consistently reachable end-to-end and
  surfaced it.

  Fixed by renaming the three call sites (`getToken()` → the
  actual function name `getApiToken()`). Same bug class was sitting
  in the dev-snippet panel and the phone-link builder too — those
  are also fixed.

## [0.0.17] — 2026-05-15

### Fixed
- **"Click 'enable' on tunnel toggle, nothing happens."** The toggle's
  click handler used to await `fetchTunnel()` before deciding what
  action to take. That had two failure modes both surfacing as a dead
  toggle:

  1. WKWebView throttles fetches when the window is backgrounded, so
     the initial `fetchTunnel()` could return `null` on transient
     blips. `cur === null` made the handler think no tunnel was
     running, it tried to start one, and if `startTunnel()` was
     similarly throttled the panel never visibly moved.
  2. If the rendered UI was stale "off" but the daemon was actually
     "running" (e.g., tunnel started from the phone PWA), clicking
     the toggle that *read* "enable" popped a "turn off tunnel?"
     confirm dialog instead — confusing enough that users would
     dismiss it and report "the button does nothing".

  The handler now keys off `lastTunnelState` (the same value driving
  the rendered UI) so the action matches what the user is looking
  at. And the panel optimistically flips to `starting…` (with the
  progress bar at the spawning stage) the same frame the click
  registers, before any network call lands — so even when fetch is
  silently slow the user sees the click took effect. A toast
  ("starting tunnel…" / "stopping tunnel…") fires on click as
  belt-and-suspenders feedback.

## [0.0.16] — 2026-05-15

### Fixed
- **macOS + linux-aarch64 missing from updater manifest.** v0.0.15
  re-enabled the Tauri updater and published `.sig` files, but the
  resulting `latest.json` listed only `linux-x86_64` and
  `windows-x86_64` — meaning darwin-aarch64 (every Apple Silicon
  Mac) and linux-aarch64 had no auto-update path. Two bugs:

  1. `.github/workflows/release.yml`'s macOS staging step re-tarred
     the .app under a new name, but Tauri's signature was already
     committed to the bytes of the original `unhosted.app.tar.gz`,
     so the renamed asset and its `.sig` no longer matched. The
     manifest builder dropped the entry because it couldn't find an
     asset whose bytes the signature would verify against. Fixed
     by copying Tauri's `unhosted.app.tar.gz` straight through to
     `unhosted-macos-app-<target>.tar.gz` and renaming the `.sig`
     to match — no re-tar, byte-identical to what Tauri signed.

  2. `.github/scripts/build-updater-manifest.py`'s platform
     classifier listed `.AppImage` and `.deb` as needles for
     `linux-x86_64` only, so an `_aarch64.AppImage` matched
     x86_64 first and a real linux-aarch64 entry was never
     created. Re-ordered the keys (specific arches first) and
     replaced generic suffix needles with arch-specific ones
     (`aarch64.AppImage`, `amd64.AppImage`, etc.) so the two
     architectures can't poach each other's slot.

  Result: `latest.json` for v0.0.16 should list all four
  platforms we actually ship for. Verified locally by replaying
  the classifier against the v0.0.15 asset list before pushing.

## [0.0.15] — 2026-05-14

### Changed
- **Tauri auto-updater re-enabled.** The updater was switched off back
  in v0.0.7 when the `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secret in CI
  drifted out of sync with the locally-held private key, breaking every
  signing step in the release pipeline. Rather than keep guessing which
  half of the keypair was wrong, the keypair has been fully regenerated:
  fresh public key committed to `tauri.conf.json`, fresh private key
  and password rotated into the matching GitHub secrets in one shot.
  `createUpdaterArtifacts` flipped back to `true`, so each platform's
  bundle now ships with a `.sig` and the release job publishes a signed
  `latest.json`.

  **One-time manual install:** v0.0.14 was built with the updater
  disabled, so it won't pick this up automatically — users on v0.0.14
  need to run `install.sh` once more to get v0.0.15. From v0.0.15
  onward, the .app will prompt to install signed updates itself.

## [0.0.14] — 2026-05-14

### Fixed
- **Blank window on first launch (real-real fix this time, Rust-side).**
  Both v0.0.10 (embedded JS probe page) and v0.0.13 (`url` field removed
  so the probe page would actually load) tried to fix this inside the
  WebView. Both failed: WKWebView throttled the `setTimeout` chain and
  the probe page wasn't reliably running. We now do the wait Rust-side
  in `unhosted-desktop/src/main.rs` *before* Tauri ever opens the
  WebView — `wait_for_daemon` polls `/health` every 200ms for up to 60s,
  and only then calls `Tauri::run`. The WebView always opens against a
  live daemon, so the first paint is always the real UI.

### Added
- **Auto-spawn the daemon from the .app.** If the daemon isn't already
  listening when the user opens the .app, the desktop shell now
  searches a short whitelist of standard install locations
  (`/usr/local/bin`, `/opt/homebrew/bin`, `$HOME/.local/bin`,
  `$HOME/.cargo/bin`, `/usr/bin`) for an `unhosted` binary and spawns
  `unhosted serve` as a detached background process. Users who
  installed via `install.sh` no longer need to keep a terminal running
  to use the .app. The spawned daemon is intentionally not tied to the
  .app's lifetime so the phone PWA / API / cron jobs keep working when
  the user closes the desktop window.

## [0.0.13] — 2026-05-14

### Fixed
- **Blank window on first launch (real fix this time).** v0.0.10's
  commit message claimed to fix the blank-window bug by adding a JS
  health-probe page to `crates/unhosted-desktop/dist/index.html`, but
  only bumped the version in `tauri.conf.json` — it never removed the
  `url: "http://127.0.0.1:7777"` window field. With `url` set, Tauri
  navigates the WebView directly at the daemon URL and the bundled
  probe page is dead code. When the daemon was down at launch (e.g.
  user installs + launches before running `unhosted serve`), WKWebView
  showed a blank error page that never retried. Removed the `url`
  field so Tauri now loads the bundled `dist/index.html` first, which
  polls `/health` and `location.replace`s to the daemon once it's up.

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
