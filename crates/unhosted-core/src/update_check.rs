//! Startup update check for the CLI / daemon.
//!
//! Desktop users get update prompts via tauri-plugin-updater (see
//! `unhosted-desktop/tauri.conf.json`). CLI users who installed via
//! `curl … install.sh | sh` have no equivalent — once the binary's
//! on disk, nothing tells them v0.0.62 is out. This module fills
//! that gap with a single lightweight check at daemon startup:
//!
//!   1. Wait ~5s after startup so we don't compete with the cluster
//!      probes that happen first.
//!   2. Hit GitHub's public releases API for the latest tag.
//!   3. Compare with `CARGO_PKG_VERSION`.
//!   4. If newer, log one info-level line with the upgrade command.
//!
//! No telemetry: the only request is outbound to api.github.com,
//! and we send no headers identifying the host. Operators who
//! disagree with even *that* outbound call can set
//! `UNHOSTED_NO_UPDATE_CHECK=1` to skip it entirely.
//!
//! Failure modes (no network, GitHub rate-limit, parse error) are
//! all silent at warn level — the daemon must not refuse to boot
//! because the update check is unreachable.

use anyhow::{Context, Result};
use serde::Deserialize;

/// What the GitHub releases API returns (subset). We only need the
/// tag name; the rest of the payload is large and unstable.
#[derive(Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

const LATEST_URL: &str = "https://api.github.com/repos/unhosted-ai/unhosted-core/releases/latest";

/// Run the check unless disabled. Returns `Ok(Some(tag))` when a
/// newer version is published, `Ok(None)` when up-to-date, and
/// `Err` on any network / parse / GitHub-side failure.
pub async fn check(http: &reqwest::Client) -> Result<Option<String>> {
    if std::env::var("UNHOSTED_NO_UPDATE_CHECK")
        .ok()
        .as_deref()
        .map(|v| matches!(v, "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let resp = http
        .get(LATEST_URL)
        // GitHub requires a User-Agent on API requests. We send the
        // crate name + version; no hostname, no user identifier.
        .header(
            "User-Agent",
            concat!("unhosted-core/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("github releases request")?;

    if !resp.status().is_success() {
        anyhow::bail!("github responded {}", resp.status());
    }

    let info: ReleaseInfo = resp
        .json()
        .await
        .context("github releases response parse")?;

    // tag_name is like "v0.0.62"; strip the leading "v" if present.
    let latest = info.tag_name.strip_prefix('v').unwrap_or(&info.tag_name);
    let current = env!("CARGO_PKG_VERSION");

    if is_newer(latest, current) {
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

/// Lexicographic-by-component semver comparison. Deliberately simple
/// — we only ship x.y.z tags, no pre-release labels, no build
/// metadata. If either side has a non-numeric component, we treat
/// the comparison as "equal" rather than guess.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<(u32, u32, u32)> {
        let mut it = s.split('.');
        let a = it.next()?.parse::<u32>().ok()?;
        let b = it.next()?.parse::<u32>().ok()?;
        let c = it.next()?.parse::<u32>().ok()?;
        Some((a, b, c))
    };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// The "fire and forget" entrypoint the daemon calls from `run()`.
/// Runs the check after a short delay so it doesn't slow startup,
/// emits a single tracing line on result, and never propagates an
/// error.
pub fn spawn_background(http: reqwest::Client) {
    tokio::spawn(async move {
        // Give the daemon a few seconds to finish its own startup
        // pipeline (cert generation, mDNS announce, upstream probe)
        // before the network call competes for resources.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        match check(&http).await {
            Ok(Some(latest)) => {
                tracing::info!(
                    current = env!("CARGO_PKG_VERSION"),
                    latest = %latest,
                    "update available — run `unhosted upgrade` to install"
                );
            }
            Ok(None) => {
                tracing::debug!("update check: up to date");
            }
            Err(e) => {
                tracing::debug!(error = %e, "update check failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_basic_semver() {
        assert!(is_newer("0.0.62", "0.0.61"));
        assert!(is_newer("0.1.0", "0.0.61"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.0.61", "0.0.61"));
        assert!(!is_newer("0.0.60", "0.0.61"));
    }

    #[test]
    fn is_newer_rejects_garbage() {
        // Non-numeric component on either side → treat as equal so
        // we never spam the user with a false "update available"
        // line because GitHub returned something unexpected.
        assert!(!is_newer("foo", "0.0.61"));
        assert!(!is_newer("0.0.61", "bar"));
        assert!(!is_newer("0.0.62-rc.1", "0.0.61"));
    }
}
