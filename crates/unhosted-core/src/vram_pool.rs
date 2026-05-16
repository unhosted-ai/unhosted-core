//! VRAM-pooling capability detection.
//!
//! The v0.1.0 slice of VRAM-pooling (ADR 0009) ships orchestration —
//! `unhosted vram-pool start/stop/status` over an RPC-capable
//! llama.cpp build distributed across two LAN peers. This module
//! is the *detection foundation* that lands first: it probes the
//! local environment to answer a single question — "is this machine
//! ready to participate in VRAM-pooling?" — and reports the result
//! in `/v1/status` and via `unhosted vram-pool detect`.
//!
//! The probe is non-trivial because Homebrew's `llama.cpp` 9090
//! ships **without** `-DGGML_RPC=ON`, so every Mac user installing
//! via the standard formula lacks both the `rpc-server` binary and
//! the `--rpc` flag on `llama-server`. ADR 0009 §Q4 covers the
//! distribution plan to fix that; this module is the surface that
//! tells users (and the daemon) where they currently stand so the
//! orchestrator in v0.1.0 can fail loudly and constructively rather
//! than silently producing a broken cluster.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command as AsyncCommand};
use tokio::sync::Mutex;

/// Snapshot of the local machine's readiness to participate in
/// VRAM-pooling. Computed via [`probe`] at daemon startup and
/// (cheaply) on demand thereafter.
#[derive(Clone, Debug, Serialize, Default)]
pub struct RpcCapability {
    /// Does `rpc-server` exist on PATH? Required to act as a
    /// *layer host* (a peer whose GPU runs some of the model's
    /// transformer layers under the orchestrator's direction).
    pub has_rpc_server_bin: bool,
    /// Does `llama-server --help` list a `--rpc` flag? Required
    /// to act as an *orchestrator* (the peer that loads the model
    /// definition and distributes inference across layer hosts).
    pub llama_server_has_rpc_flag: bool,
    /// Resolved path to `rpc-server`, if found. Surfaced for
    /// diagnostics — `unhosted vram-pool detect` prints this so
    /// users can correlate against their package manager's view.
    pub rpc_server_path: Option<String>,
    /// Resolved path to `llama-server`, if found.
    pub llama_server_path: Option<String>,
}

impl RpcCapability {
    /// True when this machine can participate in VRAM-pooling at
    /// all — as orchestrator (needs `--rpc` on llama-server) or as
    /// layer host (needs `rpc-server` binary). False means a build
    /// without `-DGGML_RPC=ON`, which is the homebrew-core default
    /// as of llama.cpp 9090.
    pub fn ready(&self) -> bool {
        self.has_rpc_server_bin && self.llama_server_has_rpc_flag
    }

    /// Human-readable install hint, surfaced by both the CLI
    /// (`unhosted vram-pool detect`) and a future
    /// `unhosted vram-pool start` failure path. Pins the message
    /// to the actual gap detected — telling a user to `brew install
    /// llama.cpp` when they already have it is the kind of advice
    /// that erodes trust in the tool.
    pub fn install_hint(&self) -> String {
        if self.ready() {
            return "VRAM-pooling capability detected. \
                    This machine can act as both orchestrator and \
                    layer host."
                .to_string();
        }
        if self.llama_server_path.is_none() {
            return "llama-server not found on PATH. \
                    Install llama.cpp via your package manager — \
                    Homebrew: `brew install llama.cpp` (then see \
                    below about RPC support)."
                .to_string();
        }
        if !self.has_rpc_server_bin || !self.llama_server_has_rpc_flag {
            return "llama.cpp is installed but was NOT built with \
                    -DGGML_RPC=ON, which VRAM-pooling requires. \
                    Homebrew's default formula omits this flag. \
                    Install the RPC-enabled build from our tap:\n\
                    \n  brew tap unhosted-ai/unhosted\
                    \n  brew install unhosted-ai/unhosted/llama-cpp-rpc\n\
                    \nThen re-run this command."
                .to_string();
        }
        // Unreachable in practice given the conditions above; kept
        // as a fallback so an unexpected combination doesn't print
        // an empty string.
        "VRAM-pooling capability is incomplete; \
         see design/0009-vram-pooling.md for the current state."
            .to_string()
    }
}

/// Run the probe. Cheap — a handful of PATH/well-known-prefix
/// lookups and at most one `--help` subprocess call. Safe to call
/// at startup and again on demand from a request handler.
///
/// Resolution order, both for `llama-server` and `rpc-server`:
///
/// 1. Homebrew opt-prefix at
///    `/opt/homebrew/opt/llama-cpp-rpc/bin/<name>` (Apple Silicon)
///    or `/usr/local/opt/llama-cpp-rpc/bin/<name>` (Intel macOS /
///    older brew layouts). This is the keg-only RPC-enabled build
///    from the `unhosted-ai/homebrew-unhosted` tap. Checking it
///    explicitly means the user doesn't need to mess with PATH —
///    the tap install just works.
/// 2. `PATH` search for the standard name. Catches users on a
///    custom-built llama.cpp, or whoever's distro / package
///    manager defaults RPC on.
///
/// If both resolve, the opt-prefix path wins. Subprocess
/// `--help` is only invoked when the resolved binary isn't from
/// the opt-prefix (we trust the tap install by construction; the
/// formula's `test` block proves the --rpc flag is present before
/// install is allowed to succeed).
pub fn probe() -> RpcCapability {
    let llama_server_path = resolve_with_tap_priority("llama-server");
    let rpc_server_path = resolve_with_tap_priority("rpc-server");

    let llama_server_has_rpc_flag = match &llama_server_path {
        Some(path) => is_tap_install(path) || help_includes_rpc(path),
        None => false,
    };

    RpcCapability {
        has_rpc_server_bin: rpc_server_path.is_some(),
        llama_server_has_rpc_flag,
        rpc_server_path: rpc_server_path.map(|p| p.to_string_lossy().to_string()),
        llama_server_path: llama_server_path.map(|p| p.to_string_lossy().to_string()),
    }
}

/// Standard Homebrew opt-prefix paths for the keg-only
/// `llama-cpp-rpc` formula on macOS. The tap ships only macOS for
/// now; Linux users have to install RPC-capable llama.cpp via their
/// own package manager or build from source, and that hits the PATH
/// search fallback in `resolve_with_tap_priority`.
const TAP_BIN_PATHS: &[&str] = &[
    "/opt/homebrew/opt/llama-cpp-rpc/bin",
    "/usr/local/opt/llama-cpp-rpc/bin",
];

fn resolve_with_tap_priority(name: &str) -> Option<PathBuf> {
    for prefix in TAP_BIN_PATHS {
        let candidate = PathBuf::from(prefix).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    which(name)
}

fn is_tap_install(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    TAP_BIN_PATHS.iter().any(|prefix| s.starts_with(prefix))
}

/// Find a binary on PATH. Returns `None` when missing, matching
/// the shape of `which` from the `which` crate without taking it
/// on as a dep.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            // PATHEXT handling on Windows would go here if we
            // shipped this on Windows — we do, but for the v0.1.0
            // slice macOS + Linux are the canonical targets.
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
        }
    }
    None
}

/// Run `<llama-server> --help` and grep the output for `--rpc`.
/// The flag's presence is the canonical signal that this build was
/// compiled with `-DGGML_RPC=ON` — checking the binary's help text
/// rather than parsing CMake artifacts means we don't depend on
/// where the user got their build from.
fn help_includes_rpc(bin: &PathBuf) -> bool {
    let out = Command::new(bin).arg("--help").output();
    match out {
        Ok(o) => {
            let combined = String::from_utf8_lossy(&o.stdout).into_owned()
                + &String::from_utf8_lossy(&o.stderr);
            // Matches both `--rpc` and `--rpc <args>` documentation
            // styles. Looking for the bare token avoids false
            // positives on words like "rpc-server".
            combined.contains("--rpc")
        }
        Err(_) => false,
    }
}

// ─── plan generation ─────────────────────────────────────────────────────
//
// Pure functions over (local capability, candidate peers, requested
// model) → a `Plan` that the spawn supervisor later turns into actual
// subprocess invocations. Keeping this separated from spawning lets us
// unit-test the decisions (who's the orchestrator, who's a layer host,
// what `--rpc` argument we'll pass) without any process management.
//
// The plan supports two topologies today:
// - Self-loopback: this machine is both orchestrator and the only
//   layer host. Useful for testing the supervisor on a single box
//   without VRAM-pooling anything (the model still fits in one
//   GPU's worth of VRAM; the round-trip just proves the wiring).
// - LAN cluster: this machine is the orchestrator and one or more
//   paired peers are layer hosts. The actually-useful case.
//
// VRAM-aware layer assignment isn't here yet (ADR 0009 §Q2). The
// plan currently lists peers but doesn't try to slice the model
// proportional to each peer's free VRAM — llama-server's own
// auto-split runs when we hand it `--rpc <list>`.

/// The default port `rpc-server` listens on. Configurable per layer
/// host so two peers on the same box (unusual but possible) don't
/// collide. Matches upstream's default so users running `rpc-server`
/// by hand outside unhosted see consistent behavior.
pub const DEFAULT_RPC_PORT: u16 = 50052;

/// A single layer host in the plan. Each one will have an
/// `rpc-server` process listening at `addr` and contribute layers
/// to the orchestrator's `llama-server` via the `--rpc` argument.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayerHost {
    /// Peer name as known to the registry (`unhosted peer list`),
    /// or `"local"` when this entry refers to the orchestrator
    /// machine acting as its own layer host (self-loopback).
    pub name: String,
    /// `host:port` where the layer host's `rpc-server` listens.
    /// For paired peers this is the peer's LAN address with
    /// `DEFAULT_RPC_PORT`; for `"local"` it's `127.0.0.1:50052`.
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    /// The peer that will run `llama-server --rpc=…` and own the
    /// OpenAI-compatible HTTP endpoint. Today this is always the
    /// machine the user typed `unhosted vram-pool start` on
    /// (i.e., `"local"`). A future iteration lets the user pick
    /// a different orchestrator.
    pub orchestrator: String,
    /// Layer hosts in the order they'll appear in the `--rpc`
    /// argument. Order matters only for predictability (the same
    /// inputs always yield the same arg string); llama-server
    /// itself picks a layer distribution per backend.
    pub layer_hosts: Vec<LayerHost>,
    /// Model identifier the orchestrator should load. Short name
    /// like `llama3.1:70b` or an absolute path to a .gguf file —
    /// the resolution lives in `cli`'s `Pull` path and isn't
    /// duplicated here.
    pub model: String,
}

impl Plan {
    /// Render the value for `llama-server --rpc=<this>`. Comma-
    /// separated `host:port`, no spaces — matches llama.cpp's
    /// expected format.
    pub fn rpc_arg(&self) -> String {
        self.layer_hosts
            .iter()
            .map(|h| h.addr.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Local machine isn't RPC-capable AND no peer was offered as
    /// a layer host. There's no one to do the work.
    NotReady,
    /// At least one of the requested peers couldn't be matched to
    /// a known peer entry. We refuse rather than silently dropping
    /// peers; a typo in `--peers` shouldn't quietly result in a
    /// thinner cluster than the user asked for.
    UnknownPeer(String),
    /// Caller didn't supply a model and we don't have a default to
    /// fall back on. The spawn supervisor needs *something* to
    /// hand `llama-server -m`.
    ModelMissing,
    /// Caller asked for at least one peer as a layer host, but
    /// none of those peers (after lookup) was RPC-capable. We
    /// could silently fall back to self-loopback here, but that
    /// would hide a configuration error the user almost certainly
    /// wants to fix.
    NoRpcCapablePeers,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady => write!(
                f,
                "no RPC-capable backend on this machine and no peers offered as layer hosts"
            ),
            Self::UnknownPeer(name) => write!(
                f,
                "requested peer `{name}` is not in this node's peer registry (run `unhosted peer list` to see what is)"
            ),
            Self::ModelMissing => write!(
                f,
                "no model specified — pass --model or set UNHOSTED_VRAM_POOL_DEFAULT_MODEL"
            ),
            Self::NoRpcCapablePeers => write!(
                f,
                "none of the requested peers reported RPC-capable llama.cpp builds. install the unhosted-ai/homebrew-unhosted tap on each peer and try again."
            ),
        }
    }
}

impl std::error::Error for PlanError {}

/// Minimal info the planner needs about each candidate peer. The
/// CLI / route handler will build these from the peer registry +
/// the peer's `/v1/status` probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerCandidate {
    pub name: String,
    pub addr: SocketAddr, // the peer's daemon HTTP address; we derive rpc-server addr from this
    pub rpc_capable: bool,
}

/// Build a `Plan` from a request, or return a `PlanError` explaining
/// why it can't be built.
///
/// Inputs:
/// - `local_capable`: whether this machine has both `rpc-server` and
///   an RPC-flagged `llama-server`. Determines whether self-loopback
///   is on the table when the user gave no peers.
/// - `peers`: every peer in the registry + their probed capability.
///   The planner will only enlist peers the caller explicitly named.
/// - `requested_peers`: peer names from `--peers a,b` (or the
///   future "all paired peers" auto-discovery — that builds this
///   list at the call site, the planner doesn't reach into the
///   registry on its own).
/// - `model`: from `--model` or a config default. None ⇒ error.
pub fn plan(
    local_capable: bool,
    peers: &[PeerCandidate],
    requested_peers: &[String],
    model: Option<String>,
) -> Result<Plan, PlanError> {
    let model = model.ok_or(PlanError::ModelMissing)?;

    // Self-loopback: no peers requested. Local must be capable.
    if requested_peers.is_empty() {
        if !local_capable {
            return Err(PlanError::NotReady);
        }
        return Ok(Plan {
            orchestrator: "local".to_string(),
            layer_hosts: vec![LayerHost {
                name: "local".to_string(),
                addr: format!("127.0.0.1:{DEFAULT_RPC_PORT}")
                    .parse()
                    .expect("static loopback addr is valid"),
            }],
            model,
        });
    }

    // Cluster mode. Resolve each requested peer name against the
    // registry; refuse on any miss rather than silently dropping.
    let mut layer_hosts = Vec::with_capacity(requested_peers.len());
    for req in requested_peers {
        let Some(peer) = peers.iter().find(|p| &p.name == req) else {
            return Err(PlanError::UnknownPeer(req.clone()));
        };
        if !peer.rpc_capable {
            // Skip silently; if NONE of the requested peers are
            // capable we'll error below.
            continue;
        }
        layer_hosts.push(LayerHost {
            name: peer.name.clone(),
            addr: SocketAddr::new(peer.addr.ip(), DEFAULT_RPC_PORT),
        });
    }
    if layer_hosts.is_empty() {
        return Err(PlanError::NoRpcCapablePeers);
    }
    Ok(Plan {
        orchestrator: "local".to_string(),
        layer_hosts,
        model,
    })
}

// ─── spawn supervisor ────────────────────────────────────────────────────
//
// `PoolManager` owns the `rpc-server` + `llama-server --rpc=…` child
// processes for an active VRAM-pool. Modeled on `tunnel::TunnelManager`
// — same pattern of "spawn child, hold handle, kill on stop, transition
// state on unexpected death" — but with two children instead of one and
// a stricter failure stance (a dying child cancels the pool rather than
// triggering an auto-restart; an in-flight inference call doesn't
// gracefully recover from a backend swap).
//
// Scope for the first slice (ADR 0009 phase 2b):
// - Self-loopback only (the plan's single layer host is the local
//   machine). Multi-peer requires peer-side `rpc-server` spawning,
//   which is its own coordination protocol.
// - llama-server is bound to a fixed local port (`DEFAULT_ORCHESTRATOR_PORT`,
//   8080 — matches the existing `UNHOSTED_LLAMA_SERVER_URL` default so
//   the daemon's upstream probe finds it without reconfiguration).
// - No persistent enable flag (cf. tunnel-autostart.txt). The user
//   has to call `vram-pool start` again after a daemon restart.
//   Persistence design awaits the multi-peer slice.

/// HTTP port `llama-server` binds for its OpenAI-compatible endpoint.
/// Matches the default the daemon's upstream probe expects, so a
/// running pool surfaces as "ready" via `/v1/status` without any
/// extra configuration.
pub const DEFAULT_ORCHESTRATOR_PORT: u16 = 8080;

/// State of the active pool (or lack thereof). Exposed verbatim via
/// `GET /v1/vram-pool` so the UI can render it without translation.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PoolState {
    /// No pool running.
    Idle,
    /// Children being spawned. `stage` advances as each child reports
    /// ready; the UI shows it as a progress indicator.
    Starting { stage: PoolStartingStage, plan: Plan },
    /// Both children up; `llama-server` answering on `endpoint`. The
    /// daemon's chat-completions proxy can now route through this
    /// endpoint instead of (or in addition to) the user's pre-existing
    /// upstream.
    Running { plan: Plan, endpoint: String },
    /// A child exited unexpectedly or never came up. `error` carries
    /// the surfaced reason; `plan` is the most-recent plan attempted
    /// (useful for the UI to display "tried to start X, failed because
    /// Y, click here to retry").
    Failed { error: String, plan: Option<Plan> },
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolStartingStage {
    SpawningLocalRpc,
    WaitingForLocalRpc,
    SpawningOrchestrator,
    WaitingForOrchestrator,
}

/// Owner of the children + state machine for an active pool. Cloning
/// is cheap (it's an `Arc` under the hood) so handler tasks can hold
/// shared references.
#[derive(Clone)]
pub struct PoolManager {
    inner: Arc<Mutex<PoolInner>>,
    cap: RpcCapability,
}

struct PoolInner {
    state: PoolState,
    /// Local `rpc-server` child if we spawned one (self-loopback or
    /// the local-as-layer-host case). `None` for "this machine is
    /// orchestrator only, all layer hosts are peers".
    rpc_child: Option<Child>,
    /// Local `llama-server` child. `None` when state is Idle or
    /// Failed-before-spawn.
    llama_child: Option<Child>,
    /// Set by `stop()`. Prevents a future watchdog (when we add one)
    /// from reviving a pool the user explicitly turned off.
    user_stopped: bool,
}

/// How long to wait for `rpc-server` to bind its port before
/// proceeding to spawn `llama-server`. The handshake `llama-server`
/// does on connect is fast, but if it fires before `rpc-server` is
/// listening it'll error out and we'd need to teardown + retry. A
/// short sleep is the simplest reliable answer here; future work
/// can swap in a TCP probe loop.
const RPC_SERVER_BIND_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// How often the model-load poller hits `llama-server`'s /v1/models
/// endpoint to detect when the model is actually ready to serve
/// requests. Tighter than the supervisor's 2 s loop because the
/// transition from "process is up" → "answering requests" is what
/// the UI is most impatient to see.
const MODEL_LOAD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(800);

/// Hard cap on how long we wait for the model to finish loading
/// before transitioning to Failed. A multi-GB model on a cold mmap
/// can take 30+ s on a Mac with SSD, more on Linux without
/// page-cache pre-warm. 90 s is generous for the models a
/// single-machine self-loopback can hold; multi-peer slices won't
/// load larger models any faster (RPC adds latency, not throughput
/// to disk).
const MODEL_LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

impl PoolManager {
    pub fn new(cap: RpcCapability) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolInner {
                state: PoolState::Idle,
                rpc_child: None,
                llama_child: None,
                user_stopped: false,
            })),
            cap,
        }
    }

    pub async fn status(&self) -> PoolState {
        self.inner.lock().await.state.clone()
    }

    /// Spawn `rpc-server` and `llama-server --rpc=…` according to
    /// `plan`. Self-loopback only for now — the plan's `layer_hosts`
    /// must contain exactly one entry named `"local"`.
    pub async fn start(self: Arc<Self>, plan: Plan) -> anyhow::Result<PoolState> {
        // Refuse to start when one's already running. Caller should
        // `stop()` first if they want a different plan.
        {
            let mut inner = self.inner.lock().await;
            if matches!(
                inner.state,
                PoolState::Starting { .. } | PoolState::Running { .. }
            ) {
                return Ok(inner.state.clone());
            }
            inner.user_stopped = false;
            inner.state = PoolState::Starting {
                stage: PoolStartingStage::SpawningLocalRpc,
                plan: plan.clone(),
            };
        }

        // First-slice constraint: self-loopback only. A multi-peer
        // plan needs peer-side rpc-server orchestration that we
        // don't have yet.
        let local_only = plan
            .layer_hosts
            .iter()
            .all(|h| h.name == "local");
        if !local_only {
            let err = "multi-peer VRAM-pooling is not yet implemented (phase 2b ships self-loopback only — single machine acting as both orchestrator and layer host)";
            let mut inner = self.inner.lock().await;
            inner.state = PoolState::Failed {
                error: err.into(),
                plan: Some(plan),
            };
            anyhow::bail!(err);
        }

        let rpc_bin = self
            .cap
            .rpc_server_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("rpc-server binary not found — install the unhosted-ai/homebrew-unhosted tap"))?;
        let llama_bin = self
            .cap
            .llama_server_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("llama-server binary not found"))?;

        let rpc_port = plan
            .layer_hosts
            .first()
            .map(|h| h.addr.port())
            .unwrap_or(DEFAULT_RPC_PORT);

        // Spawn rpc-server first; llama-server connects to it on
        // boot, so the order matters.
        tracing::info!(bin = %rpc_bin, port = rpc_port, "vram-pool: spawning rpc-server");
        let rpc_child = AsyncCommand::new(rpc_bin)
            .arg("-p")
            .arg(rpc_port.to_string())
            .arg("-H")
            .arg("127.0.0.1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn rpc-server: {e}"))?;

        {
            let mut inner = self.inner.lock().await;
            inner.rpc_child = Some(rpc_child);
            inner.state = PoolState::Starting {
                stage: PoolStartingStage::WaitingForLocalRpc,
                plan: plan.clone(),
            };
        }

        tokio::time::sleep(RPC_SERVER_BIND_GRACE).await;

        // Now llama-server with --rpc pointing at the rpc-server we
        // just started. The model argument is plan.model verbatim;
        // resolving short names like "llama3.1:70b" to a .gguf path
        // is the user's responsibility for v0.0.30 (CLI's `pull`
        // command produces the file; the user passes the path).
        {
            let mut inner = self.inner.lock().await;
            inner.state = PoolState::Starting {
                stage: PoolStartingStage::SpawningOrchestrator,
                plan: plan.clone(),
            };
        }

        let endpoint = format!("http://127.0.0.1:{DEFAULT_ORCHESTRATOR_PORT}");
        tracing::info!(
            bin = %llama_bin,
            port = DEFAULT_ORCHESTRATOR_PORT,
            rpc = %plan.rpc_arg(),
            model = %plan.model,
            "vram-pool: spawning llama-server"
        );
        let llama_child = AsyncCommand::new(llama_bin)
            .arg("-m")
            .arg(&plan.model)
            .arg("--rpc")
            .arg(plan.rpc_arg())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(DEFAULT_ORCHESTRATOR_PORT.to_string())
            .arg("--gpu-layers")
            .arg("99")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn llama-server: {e}"))?;

        {
            let mut inner = self.inner.lock().await;
            inner.llama_child = Some(llama_child);
            inner.state = PoolState::Starting {
                stage: PoolStartingStage::WaitingForOrchestrator,
                plan: plan.clone(),
            };
        }

        // Keep state at Starting{WaitingForOrchestrator}. Don't lie
        // about Running until /v1/models actually answers — a chat
        // UI that gates on `state == "running"` would otherwise let
        // the user fire a prompt against a half-loaded llama-server
        // and get a 503. The dedicated poller below flips state.
        // start() returns the Starting state immediately so the
        // HTTP handler doesn't block for the multi-GB-mmap window.
        spawn_model_load_poller(self.inner.clone(), endpoint.clone());

        // Background supervisor: watch both children, transition to
        // Failed if either dies unexpectedly (during loading OR after
        // running).
        spawn_pool_supervisor(self.inner.clone());

        Ok(PoolState::Starting {
            stage: PoolStartingStage::WaitingForOrchestrator,
            plan,
        })
    }

    pub async fn stop(&self) -> anyhow::Result<PoolState> {
        let mut inner = self.inner.lock().await;
        tracing::info!("vram-pool: stop requested");
        if let Some(mut c) = inner.llama_child.take() {
            let _ = c.start_kill();
        }
        if let Some(mut c) = inner.rpc_child.take() {
            let _ = c.start_kill();
        }
        inner.state = PoolState::Idle;
        inner.user_stopped = true;
        Ok(PoolState::Idle)
    }
}

/// Poll `llama-server`'s /v1/models until it answers (model is
/// loaded and the server is serving) or the timeout fires. On
/// success, flip state to Running. On timeout, kill the children
/// and flip to Failed with a message the UI can actually surface.
///
/// Separate from the child-death supervisor below because the
/// concerns are different: this task only matters during the
/// Starting → Running transition window, the supervisor watches
/// children for their entire lifetime including the long Running
/// tail.
fn spawn_model_load_poller(inner: Arc<Mutex<PoolInner>>, endpoint: String) {
    let url = format!("{endpoint}/v1/models");
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(800))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "vram-pool: poller client build failed");
                return;
            }
        };
        let start = std::time::Instant::now();
        loop {
            // Bail if the user pulled the plug or the supervisor
            // already marked us Failed.
            {
                let g = inner.lock().await;
                if matches!(g.state, PoolState::Idle | PoolState::Failed { .. } | PoolState::Running { .. })
                {
                    return;
                }
                if g.user_stopped {
                    return;
                }
            }

            if start.elapsed() > MODEL_LOAD_TIMEOUT {
                let plan = {
                    let g = inner.lock().await;
                    if let PoolState::Starting { plan, .. } = &g.state {
                        Some(plan.clone())
                    } else {
                        None
                    }
                };
                tracing::warn!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "vram-pool: model didn't finish loading within timeout"
                );
                let mut g = inner.lock().await;
                if let Some(mut c) = g.llama_child.take() {
                    let _ = c.start_kill();
                }
                if let Some(mut c) = g.rpc_child.take() {
                    let _ = c.start_kill();
                }
                g.state = PoolState::Failed {
                    error: format!(
                        "model didn't finish loading within {}s — check the .gguf path and free VRAM",
                        MODEL_LOAD_TIMEOUT.as_secs()
                    ),
                    plan,
                };
                return;
            }

            match client.get(&url).send().await {
                Ok(r) if r.status().is_success() => {
                    let plan = {
                        let g = inner.lock().await;
                        if let PoolState::Starting { plan, .. } = &g.state {
                            plan.clone()
                        } else {
                            return; // raced; supervisor already moved us
                        }
                    };
                    tracing::info!(
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        url = %url,
                        "vram-pool: orchestrator answering /v1/models — transitioning to Running"
                    );
                    let mut g = inner.lock().await;
                    g.state = PoolState::Running {
                        plan,
                        endpoint: endpoint.clone(),
                    };
                    return;
                }
                _ => {
                    // Not ready yet (connection refused, 503, etc).
                    // Sleep and retry. The supervisor below catches
                    // child-death separately, so we don't have to
                    // check try_wait here.
                }
            }
            tokio::time::sleep(MODEL_LOAD_POLL_INTERVAL).await;
        }
    });
}

/// Background watcher. Polls both child handles for exit; if either
/// dies while state is Running, transition to Failed. The user_stopped
/// flag protects against logging a "child died" warning for the kill
/// we just sent in `stop()`.
fn spawn_pool_supervisor(inner: Arc<Mutex<PoolInner>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let mut guard = inner.lock().await;
            if guard.user_stopped || matches!(guard.state, PoolState::Idle | PoolState::Failed { .. })
            {
                return;
            }
            // Check llama-server first; it's the user-facing endpoint
            // and is more likely to be the one that fails (model
            // mmap, port collision, etc).
            let llama_dead = match guard.llama_child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            let rpc_dead = match guard.rpc_child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if llama_dead || rpc_dead {
                let plan = if let PoolState::Running { plan, .. } = &guard.state {
                    Some(plan.clone())
                } else {
                    None
                };
                let error = if llama_dead && rpc_dead {
                    "llama-server and rpc-server both exited"
                } else if llama_dead {
                    "llama-server exited unexpectedly (check the model path and free VRAM)"
                } else {
                    "rpc-server exited unexpectedly"
                };
                tracing::warn!(error, "vram-pool: supervisor detected child death");
                guard.state = PoolState::Failed {
                    error: error.into(),
                    plan,
                };
                // Kill the survivor for clean shutdown.
                if let Some(mut c) = guard.llama_child.take() {
                    let _ = c.start_kill();
                }
                if let Some(mut c) = guard.rpc_child.take() {
                    let _ = c.start_kill();
                }
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hint_when_no_llama_server() {
        let c = RpcCapability::default();
        let hint = c.install_hint();
        assert!(hint.contains("llama-server not found"));
    }

    #[test]
    fn install_hint_when_built_without_rpc() {
        let c = RpcCapability {
            has_rpc_server_bin: false,
            llama_server_has_rpc_flag: false,
            llama_server_path: Some("/opt/homebrew/bin/llama-server".to_string()),
            rpc_server_path: None,
        };
        let hint = c.install_hint();
        assert!(hint.contains("-DGGML_RPC=ON"));
    }

    #[test]
    fn install_hint_when_ready() {
        let c = RpcCapability {
            has_rpc_server_bin: true,
            llama_server_has_rpc_flag: true,
            llama_server_path: Some("/usr/local/bin/llama-server".to_string()),
            rpc_server_path: Some("/usr/local/bin/rpc-server".to_string()),
        };
        assert!(c.ready());
        let hint = c.install_hint();
        assert!(hint.contains("can act as"));
    }

    #[test]
    fn probe_returns_something_on_this_machine() {
        // Doesn't assert outcomes (depends on the test runner's
        // env), just that the call doesn't panic on a fresh
        // process.
        let _ = probe();
    }

    fn peer(name: &str, ip: &str, rpc_capable: bool) -> PeerCandidate {
        PeerCandidate {
            name: name.to_string(),
            addr: format!("{ip}:7777").parse().unwrap(),
            rpc_capable,
        }
    }

    #[test]
    fn plan_self_loopback_when_no_peers_and_local_capable() {
        let p = plan(true, &[], &[], Some("llama3.1:70b".into())).unwrap();
        assert_eq!(p.orchestrator, "local");
        assert_eq!(p.layer_hosts.len(), 1);
        assert_eq!(p.layer_hosts[0].name, "local");
        assert_eq!(p.rpc_arg(), "127.0.0.1:50052");
        assert_eq!(p.model, "llama3.1:70b");
    }

    #[test]
    fn plan_errors_when_local_not_capable_and_no_peers() {
        let err = plan(false, &[], &[], Some("m".into())).unwrap_err();
        assert_eq!(err, PlanError::NotReady);
    }

    #[test]
    fn plan_errors_without_model() {
        let err = plan(true, &[], &[], None).unwrap_err();
        assert_eq!(err, PlanError::ModelMissing);
    }

    #[test]
    fn plan_cluster_with_capable_peers() {
        let peers = vec![
            peer("thunder", "192.168.1.42", true),
            peer("homelab", "192.168.1.99", true),
            peer("pi5", "192.168.1.50", false), // not capable
        ];
        let p = plan(
            true,
            &peers,
            &["thunder".to_string(), "homelab".to_string()],
            Some("m".into()),
        )
        .unwrap();
        assert_eq!(p.layer_hosts.len(), 2);
        assert_eq!(p.layer_hosts[0].name, "thunder");
        assert_eq!(p.layer_hosts[0].addr.port(), DEFAULT_RPC_PORT);
        assert_eq!(p.rpc_arg(), "192.168.1.42:50052,192.168.1.99:50052");
    }

    #[test]
    fn plan_skips_non_capable_peers_silently() {
        // Mixed list: one capable, one not. Plan keeps the capable one.
        let peers = vec![
            peer("thunder", "192.168.1.42", true),
            peer("pi5", "192.168.1.50", false),
        ];
        let p = plan(
            true,
            &peers,
            &["thunder".to_string(), "pi5".to_string()],
            Some("m".into()),
        )
        .unwrap();
        assert_eq!(p.layer_hosts.len(), 1);
        assert_eq!(p.layer_hosts[0].name, "thunder");
    }

    #[test]
    fn plan_errors_when_all_requested_peers_incapable() {
        let peers = vec![peer("pi5", "192.168.1.50", false)];
        let err = plan(
            true,
            &peers,
            &["pi5".to_string()],
            Some("m".into()),
        )
        .unwrap_err();
        assert_eq!(err, PlanError::NoRpcCapablePeers);
    }

    #[test]
    fn plan_errors_on_unknown_peer_name() {
        let peers = vec![peer("thunder", "192.168.1.42", true)];
        let err = plan(
            true,
            &peers,
            &["thunder".to_string(), "ghost".to_string()],
            Some("m".into()),
        )
        .unwrap_err();
        assert_eq!(err, PlanError::UnknownPeer("ghost".to_string()));
    }
}
