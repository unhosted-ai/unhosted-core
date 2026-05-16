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

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

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
}
