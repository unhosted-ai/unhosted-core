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
    Starting,
    Running { url: String },
    Failed { error: String },
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
        if matches!(inner.state, TunnelState::Starting | TunnelState::Running { .. }) {
            return Ok(inner.state.clone());
        }

        // Probe for cloudflared on PATH first so we can give a clean
        // error instead of a subprocess-spawn failure.
        which_cloudflared()?;

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
        inner.state = TunnelState::Starting;
        inner.child = Some(child);
        let inner_arc = self.inner.clone();
        drop(inner);

        // Background task: scan stderr for the public URL and update
        // state. Also surfaces fatal errors as `Failed`.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(line = %line, "cloudflared");
                if let Some(url) = extract_trycloudflare_url(&line) {
                    let mut guard = inner_arc.lock().await;
                    guard.state = TunnelState::Running { url: url.clone() };
                    tracing::info!(url = %url, "cloudflared tunnel up");
                }
            }
            // stderr closed → process exited.
            let mut guard = inner_arc.lock().await;
            if matches!(guard.state, TunnelState::Starting) {
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
}
