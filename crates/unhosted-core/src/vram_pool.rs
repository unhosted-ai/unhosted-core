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

use serde::{Deserialize, Serialize};

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
