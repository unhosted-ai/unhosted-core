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

use anyhow::{Context, Result};
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
}

impl TunnelManager {
    pub fn new(local_port: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                state: TunnelState::Idle,
                child: None,
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
    pub async fn start(&self) -> Result<TunnelState> {
        let mut inner = self.inner.lock().await;
        if matches!(
            inner.state,
            TunnelState::Starting { .. } | TunnelState::Running { .. }
        ) {
            return Ok(inner.state.clone());
        }

        // Probe for cloudflared on PATH first so we can give a clean
        // error instead of a subprocess-spawn failure.
        which_cloudflared()?;

        // Preflight the network. Without internet, cloudflared would sit at
        // "Requesting new quick Tunnel" indefinitely and the UI would show
        // a hung progress bar with no useful error. A 1.5s HEAD to
        // cloudflare.com fails fast and lets us surface a clear message
        // instead.
        if !has_internet().await {
            inner.state = TunnelState::Failed {
                error: "no internet — open to internet needs an outbound connection".into(),
            };
            return Ok(inner.state.clone());
        }

        let target = format!("http://127.0.0.1:{}", self.local_port);
        let mut cmd = Command::new("cloudflared");
        cmd.arg("tunnel")
            .arg("--no-autoupdate")
            .arg("--url")
            .arg(&target)
            // cloudflared writes the trycloudflare URL to stderr at INFO.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().context("spawning cloudflared")?;
        let stderr = child.stderr.take().context("cloudflared stderr unavailable")?;
        inner.state = TunnelState::Starting {
            stage: StartingStage::Spawning,
        };
        inner.child = Some(child);
        let inner_arc = self.inner.clone();
        drop(inner);

        // Background task: scan stderr for the public URL and update
        // state. Also surfaces fatal errors as `Failed`, and bumps the
        // starting-stage so the UI progress bar reflects real progress.
        //
        // Important ordering: cloudflared announces the `trycloudflare.com`
        // URL *several seconds* before the QUIC connection to Cloudflare's
        // edge is registered. If we flipped to `Running` the instant the URL
        // appeared, users would tap the link on their phone and hit 502s
        // because the tunnel wasn't live yet. So we capture the URL but
        // stay in `Starting` until we also see "Registered tunnel connection".
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut pending_url: Option<String> = None;
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(line = %line, "cloudflared");
                if let Some(url) = extract_trycloudflare_url(&line) {
                    pending_url = Some(url);
                }
                if line.contains("Registered tunnel connection") {
                    if let Some(url) = pending_url.take() {
                        let mut guard = inner_arc.lock().await;
                        guard.state = TunnelState::Running { url: url.clone() };
                        tracing::info!(url = %url, "cloudflared tunnel up");
                        continue;
                    }
                }
                if let Some(next) = detect_stage(&line) {
                    let mut guard = inner_arc.lock().await;
                    // Stages only move forward — ignore a late "Requesting"
                    // line after we've already advanced.
                    if let TunnelState::Starting { stage } = guard.state {
                        if next > stage {
                            guard.state = TunnelState::Starting { stage: next };
                        }
                    }
                }
            }
            // stderr closed → process exited.
            let mut guard = inner_arc.lock().await;
            if matches!(guard.state, TunnelState::Starting { .. }) {
                guard.state = TunnelState::Failed {
                    error: "cloudflared exited before producing a url".into(),
                };
            } else if matches!(guard.state, TunnelState::Running { .. }) {
                guard.state = TunnelState::Idle;
            }
            guard.child = None;
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
        Ok(inner.state.clone())
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
