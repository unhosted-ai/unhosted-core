//! Cloudflare Tunnel control.
//!
//! Spawns `cloudflared tunnel --url http://127.0.0.1:<port>` as a child
//! process, parses the `*.trycloudflare.com` URL out of its stderr, and
//! exposes start/stop/status to the web UI so the user can publish
//! their daemon to the internet with one click.
//!
//! Safety:
//! - The auth classifier ([`crate::auth::classify`]) treats requests
//!   carrying `cf-connecting-ip` as non-loopback. Combined with the
//!   bearer-token requirement for non-loopback callers, tunneled
//!   traffic must present the token. The token is embedded in the
//!   tunnel URL we hand the user (as `?api_token=…`) so the PWA picks
//!   it up the first time and never carries it on the wire after.
//! - Subprocess is killed on `Drop`, so daemon shutdown takes the
//!   tunnel down with it.

use std::process::Stdio;
use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TunnelState {
    Idle,
    Starting { stage: StartingStage },
    Running { url: String },
    Failed { error: String },
}

/// Sub-stage of [`TunnelState::Starting`]. Drives the progress bar in the
/// UI — we parse cloudflared's stderr for known milestone lines and bump
/// the stage so the user sees real progress, not a hung spinner.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum StartingStage {
    /// Process spawned, no useful output yet.
    Spawning,
    /// Cloudflared has reached out to Cloudflare ("Requesting new quick
    /// Tunnel on trycloudflare.com").
    Requesting,
    /// Cloudflared got back a tunnel and is negotiating the QUIC
    /// connection ("Initial protocol quic", "Starting metrics server",
    /// "Generated Connector ID").
    Connecting,
}

pub struct TunnelManager {
    inner: Arc<Mutex<Inner>>,
    /// Local port we publish (the daemon's own HTTP port).
    local_port: u16,
}

struct Inner {
    state: TunnelState,
    child: Option<Child>,
    /// How many consecutive auto-restart attempts the supervisor has tried.
    /// Reset to 0 when a tunnel reaches `Running` or when `stop()` is called.
    /// Capped at [`MAX_AUTO_RESTARTS`] so a permanently broken cloudflared
    /// doesn't churn forever.
    restart_attempts: u32,
}

/// Hard cap on consecutive auto-restarts. After this many failed revivals
/// the supervisor gives up and leaves the tunnel in `Failed` so the user
/// can investigate.
const MAX_AUTO_RESTARTS: u32 = 3;
/// Backoff between auto-restart attempts. Short enough that a transient
/// crash heals before the user notices their phone stopped working.
const AUTO_RESTART_DELAY_SECS: u64 = 3;

impl TunnelManager {
    pub fn new(local_port: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: TunnelState::Idle,
                child: None,
                restart_attempts: 0,
            })),
            local_port,
        }
    }

    pub async fn status(&self) -> TunnelState {
        self.inner.lock().await.state.clone()
    }

    /// Spawn cloudflared. Returns immediately with `Starting`; the URL
    /// becomes available a second or two later once cloudflared logs
    /// the `*.trycloudflare.com` line.
    ///
    /// Takes `self: Arc<Self>` so the supervisor task spawned inside can
    /// hold a strong reference and call `start` again if cloudflared dies
    /// unexpectedly mid-session. Callers with a shared `Arc<TunnelManager>`
    /// (e.g. `state.tunnel`) should pass `state.tunnel.clone()`.
    pub async fn start(self: Arc<Self>) -> Result<TunnelState> {
        {
            let inner = self.inner.lock().await;
            if matches!(
                inner.state,
                TunnelState::Starting { .. } | TunnelState::Running { .. }
            ) {
                return Ok(inner.state.clone());
            }
        }

        // Probe for cloudflared on PATH first so we can give a clean
        // error instead of a subprocess-spawn failure.
        which_cloudflared()?;

        // Preflight the network. Without internet, cloudflared would sit
        // at "Requesting new quick Tunnel" indefinitely and the UI would
        // show a hung progress bar with no useful error. Held outside the
        // mutex so the captured guard doesn't poison the future's Send.
        if !has_internet().await {
            let mut inner = self.inner.lock().await;
            inner.state = TunnelState::Failed {
                error: "no internet — open to internet needs an outbound connection".into(),
            };
            return Ok(inner.state.clone());
        }

        // Flip state to Starting{Spawning} before we hand off — callers
        // see a fast transition and the supervisor loop owns the spawn.
        {
            let mut inner = self.inner.lock().await;
            if matches!(
                inner.state,
                TunnelState::Starting { .. } | TunnelState::Running { .. }
            ) {
                return Ok(inner.state.clone());
            }
            inner.state = TunnelState::Starting {
                stage: StartingStage::Spawning,
            };
        }

        // Single supervisor task: spawns cloudflared, reads its stderr,
        // and re-spawns on unexpected death. Looping inside one task
        // avoids the recursive-self.start() Send-trait gymnastics, keeps
        // the restart-attempt counter local to the loop, and gives us a
        // single place to react to stop() (which sets state to Idle).
        let supervisor_inner = self.inner.clone();
        let local_port = self.local_port;
        tokio::spawn(async move {
            supervisor_loop(supervisor_inner, local_port).await;
        });

        Ok(self.status().await)
    }

    pub async fn stop(&self) -> Result<TunnelState> {
        let mut inner = self.inner.lock().await;
        if let Some(mut child) = inner.child.take() {
            let _ = child.start_kill();
            // Don't await waiting — kill_on_drop already handles cleanup
            // and we don't want the handler to block on a slow exit.
        }
        inner.state = TunnelState::Idle;
        // User asked to stop, so any future tunnel start should get a
        // fresh restart budget. The supervisor in the reader task sees
        // state==Idle here and won't revive.
        inner.restart_attempts = 0;
        Ok(inner.state.clone())
    }
}

/// Supervisor loop: spawn cloudflared, read its stderr, and revive on
/// unexpected death up to [`MAX_AUTO_RESTARTS`] times. Runs as a single
/// long-lived tokio task per `start()` call. Exits when:
///   - the operator pressed stop (state == Idle) — clean handoff
///   - we ran out of restart attempts — state == Failed
///   - state == Failed for any other reason (e.g. spawn error)
async fn supervisor_loop(inner: Arc<Mutex<Inner>>, local_port: u16) {
    let target = format!("http://127.0.0.1:{}", local_port);
    loop {
        let mut cmd = Command::new("cloudflared");
        cmd.arg("tunnel")
            .arg("--no-autoupdate")
            .arg("--url")
            .arg(&target)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let mut guard = inner.lock().await;
                guard.state = TunnelState::Failed {
                    error: format!("failed to spawn cloudflared: {e}"),
                };
                guard.child = None;
                return;
            }
        };
        let stderr = match child.stderr.take() {
            Some(s) => s,
            None => {
                let mut guard = inner.lock().await;
                guard.state = TunnelState::Failed {
                    error: "cloudflared stderr unavailable".into(),
                };
                guard.child = None;
                return;
            }
        };
        {
            let mut guard = inner.lock().await;
            guard.child = Some(child);
            // Don't reset state to Starting{Spawning} on revival — keep
            // whatever the caller set (Running for revival path, Spawning
            // for first start) until cloudflared output advances it.
            if matches!(guard.state, TunnelState::Idle) {
                guard.state = TunnelState::Starting {
                    stage: StartingStage::Spawning,
                };
            }
        }

        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            tracing::debug!(line = %line, "cloudflared");
            if let Some(url) = extract_trycloudflare_url(&line) {
                let mut guard = inner.lock().await;
                guard.state = TunnelState::Running { url: url.clone() };
                guard.restart_attempts = 0;
                tracing::info!(url = %url, "cloudflared tunnel up");
                continue;
            }
            if let Some(next) = detect_stage(&line) {
                let mut guard = inner.lock().await;
                if let TunnelState::Starting { stage } = guard.state {
                    if next > stage {
                        guard.state = TunnelState::Starting { stage: next };
                    }
                }
            }
        }
        // stderr closed → process exited. Decide whether to revive.
        let mut should_revive = false;
        {
            let mut guard = inner.lock().await;
            guard.child = None;
            match guard.state {
                TunnelState::Running { .. } => {
                    if guard.restart_attempts < MAX_AUTO_RESTARTS {
                        guard.restart_attempts += 1;
                        let attempt = guard.restart_attempts;
                        guard.state = TunnelState::Starting {
                            stage: StartingStage::Spawning,
                        };
                        tracing::warn!(
                            attempt,
                            delay_secs = AUTO_RESTART_DELAY_SECS,
                            "cloudflared exited unexpectedly — reviving"
                        );
                        should_revive = true;
                    } else {
                        let attempts = guard.restart_attempts;
                        tracing::error!(
                            attempts,
                            "cloudflared crashed too many times — giving up"
                        );
                        guard.state = TunnelState::Failed {
                            error: format!(
                                "cloudflared crashed {attempts} times in a row — check the binary"
                            ),
                        };
                    }
                }
                TunnelState::Starting { .. } => {
                    guard.state = TunnelState::Failed {
                        error: "cloudflared exited before producing a url".into(),
                    };
                }
                // Idle (stop pressed) or Failed: leave the state, exit
                // the supervisor.
                _ => {}
            }
        }
        if !should_revive {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(AUTO_RESTART_DELAY_SECS)).await;
    }
}

/// Fast preflight: are we online enough to reach Cloudflare? Returns true
/// if a HEAD to cloudflare.com completes within 1.5s. We probe the same
/// vendor we're about to tunnel through so a working result actually
/// implies the tunnel will reach somebody.
async fn has_internet() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .head("https://www.cloudflare.com")
        .send()
        .await
        .map(|r| r.status().is_success() || r.status().is_redirection())
        .unwrap_or(false)
}

fn which_cloudflared() -> Result<()> {
    // Lightweight PATH check using which::which would be nicer but we
    // don't want a new dep; Command::spawn already fails clearly. This
    // is here so the caller can return a friendlier error before we
    // touch the process tree.
    let from_env = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&from_env) {
        let candidate = dir.join("cloudflared");
        if candidate.exists() {
            return Ok(());
        }
        #[cfg(windows)]
        {
            let with_exe = dir.join("cloudflared.exe");
            if with_exe.exists() {
                return Ok(());
            }
        }
    }
    anyhow::bail!(
        "cloudflared not found on PATH — install it first (e.g. `brew install cloudflared`)"
    );
}

/// Classify a cloudflared log line into a [`StartingStage`] if it matches
/// one of the known milestone substrings. Returns `None` for lines that
/// don't move the state machine.
fn detect_stage(line: &str) -> Option<StartingStage> {
    // Order matters: check the latest stage first so a line that
    // mentions multiple keywords (rare) classifies as the furthest one.
    if line.contains("Registered tunnel connection")
        || line.contains("Generated Connector ID")
        || line.contains("Starting metrics server")
        || line.contains("Initial protocol")
    {
        Some(StartingStage::Connecting)
    } else if line.contains("Requesting new quick Tunnel")
        || line.contains("Thank you for trying Cloudflare Tunnel")
    {
        Some(StartingStage::Requesting)
    } else {
        None
    }
}

/// Find a `*.trycloudflare.com` URL inside a cloudflared log line.
/// Format is roughly `... |  https://<words>.trycloudflare.com  |` but
/// version-to-version it shifts; we just grep for the substring.
fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let needle = "trycloudflare.com";
    let pos = line.find(needle)?;
    // Walk backwards to the start of "https://" or "http://".
    let prefix = &line[..pos];
    let scheme_pos = prefix.rfind("https://").or_else(|| prefix.rfind("http://"))?;
    let after = &line[scheme_pos..];
    // Cut at the first whitespace or pipe.
    let end = after
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(after.len());
    Some(after[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_url_from_log_line() {
        let line = "2025-01-01T00:00:00Z INF |  https://random-words-xyz.trycloudflare.com  |";
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://random-words-xyz.trycloudflare.com")
        );
    }

    #[test]
    fn extracts_url_when_inline() {
        let line = "INF Your tunnel: https://foo-bar.trycloudflare.com please wait";
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://foo-bar.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_unrelated_lines() {
        let line = "INF Starting tunnel";
        assert!(extract_trycloudflare_url(line).is_none());
    }

    #[test]
    fn detects_requesting_stage() {
        let line = "INF Requesting new quick Tunnel on trycloudflare.com...";
        assert_eq!(detect_stage(line), Some(StartingStage::Requesting));
    }

    #[test]
    fn detects_connecting_stage() {
        assert_eq!(
            detect_stage("INF Initial protocol quic"),
            Some(StartingStage::Connecting)
        );
        assert_eq!(
            detect_stage("INF Registered tunnel connection connIndex=0"),
            Some(StartingStage::Connecting)
        );
    }

    #[test]
    fn stage_ordering_is_monotonic() {
        // The state-machine relies on `Spawning < Requesting < Connecting`
        // so progress can only move forward.
        assert!(StartingStage::Spawning < StartingStage::Requesting);
        assert!(StartingStage::Requesting < StartingStage::Connecting);
    }
}
