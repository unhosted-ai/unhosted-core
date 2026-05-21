//! Data Loss Prevention (DLP) hook.
//!
//! A `before_completion` policy callout: the daemon POSTs the
//! about-to-be-forwarded chat-completion body to a configured DLP
//! endpoint and proceeds only if the endpoint's response says
//! `allow`. Required for SOC 2 CC6.7, ISO 27001 A.8.11 / A.8.12,
//! and EU AI Act Article 26 monitoring obligations for high-risk
//! deployer use.
//!
//! ### Wire format
//!
//! POST `<dlp_endpoint>` with the chat-completion request body
//! unchanged (preserves `messages`, `model`, `stream`, etc.).
//! Optional bearer:
//!
//! ```text
//! Authorization: Bearer <dlp_bearer>
//! Content-Type: application/json
//! ```
//!
//! The DLP service responds with one of:
//!
//! ```json
//! { "decision": "allow" }
//! { "decision": "block", "reason": "PII detected: SSN pattern" }
//! ```
//!
//! Any other response shape, a non-2xx status, or a timeout is
//! handled per the `fail_mode` config:
//!
//! - `fail_open` (default) — daemon allows the request; logs warn.
//! - `fail_closed` — daemon refuses with 502, logs error.
//!
//! Most enterprises start fail-open (no DLP outage blocks chat)
//! and tighten once they trust the DLP integration.
//!
//! ### Config
//!
//! Read from `~/.config/unhosted/dlp.toml`:
//!
//! ```toml
//! endpoint = "https://dlp.internal.example/scan"
//! bearer = "..."        # optional
//! timeout_ms = 800
//! fail_mode = "fail_open"   # or "fail_closed"
//! ```
//!
//! Absent file = no DLP integration; chat path runs unchanged.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

/// What the operator stores in the config file.
#[derive(Debug, Clone, Deserialize)]
pub struct DlpConfig {
    pub endpoint: String,
    #[serde(default)]
    pub bearer: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fail_mode")]
    pub fail_mode: FailMode,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailMode {
    /// Allow the request when DLP is unreachable / errors. Safer
    /// for production reliability; weaker for compliance.
    FailOpen,
    /// Refuse the request when DLP is unreachable / errors. Stricter
    /// for compliance; risks chat outage if DLP service flaps.
    FailClosed,
}

fn default_timeout_ms() -> u64 {
    800
}
fn default_fail_mode() -> FailMode {
    FailMode::FailOpen
}

/// The DLP service's response shape. Keep it small and explicit —
/// any deviation from this shape is treated as "unparseable" and
/// goes through the fail-mode handler.
#[derive(Debug, Deserialize)]
struct DlpResponse {
    decision: String,
    #[serde(default)]
    reason: Option<String>,
}

/// What `dlp_check` returns. The chat handler matches on this:
/// `Allow` → forward upstream; `Block { reason }` → refuse with
/// 422 + the reason body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlpDecision {
    Allow,
    Block { reason: String },
}

pub fn config_path() -> Result<std::path::PathBuf> {
    paths::config_file("dlp.toml")
}

/// Load the DLP config from `~/.config/unhosted/dlp.toml`. Returns
/// `Ok(None)` if absent — that's the "no DLP integration" path and
/// chat runs unchanged. Returns `Err` only on a present-but-invalid
/// config (the daemon logs and skips, doesn't refuse to boot).
pub fn load() -> Result<Option<DlpConfig>> {
    let path = config_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let cfg: DlpConfig = toml::from_str(&text)
        .with_context(|| format!("parsing {} as TOML", path.display()))?;
    Ok(Some(cfg))
}

/// Perform the DLP check. Sends the chat-completion body to the
/// configured endpoint and returns the policy decision.
///
/// Failure handling per `fail_mode`:
///   - FailOpen  — network / parse / timeout → `Allow`, warn-log.
///   - FailClosed → propagates as `Block`.
pub async fn check(
    http: &reqwest::Client,
    cfg: &DlpConfig,
    body: &[u8],
) -> DlpDecision {
    let mut req = http
        .post(&cfg.endpoint)
        .timeout(Duration::from_millis(cfg.timeout_ms))
        .header("Content-Type", "application/json")
        .body(body.to_vec());
    if let Some(token) = cfg.bearer.as_deref() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return fail_to_decision(cfg, format!("dlp request: {e}")),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        return fail_to_decision(cfg, format!("dlp http {status}"));
    }

    let parsed: DlpResponse = match resp.json().await {
        Ok(p) => p,
        Err(e) => return fail_to_decision(cfg, format!("dlp parse: {e}")),
    };

    match parsed.decision.as_str() {
        "allow" => DlpDecision::Allow,
        "block" => DlpDecision::Block {
            reason: parsed.reason.unwrap_or_else(|| "dlp blocked".into()),
        },
        other => fail_to_decision(cfg, format!("dlp unknown decision: {other}")),
    }
}

fn fail_to_decision(cfg: &DlpConfig, reason: String) -> DlpDecision {
    match cfg.fail_mode {
        FailMode::FailOpen => {
            tracing::warn!(error = %reason, "dlp: failing open");
            DlpDecision::Allow
        }
        FailMode::FailClosed => {
            tracing::error!(error = %reason, "dlp: failing closed — refusing chat");
            DlpDecision::Block {
                reason: format!("dlp unavailable: {reason}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(url: &str, fail: FailMode) -> DlpConfig {
        DlpConfig {
            endpoint: url.into(),
            bearer: None,
            timeout_ms: 500,
            fail_mode: fail,
        }
    }

    #[tokio::test]
    async fn allow_decision_is_allow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "allow"
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let decision = check(&http, &cfg(&server.uri(), FailMode::FailClosed), b"{}").await;
        assert_eq!(decision, DlpDecision::Allow);
    }

    #[tokio::test]
    async fn block_decision_with_reason_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "block",
                "reason": "PII: SSN-pattern matched"
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let decision = check(&http, &cfg(&server.uri(), FailMode::FailClosed), b"{}").await;
        match decision {
            DlpDecision::Block { reason } => {
                assert_eq!(reason, "PII: SSN-pattern matched");
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bearer_is_sent_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("Authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "allow"
            })))
            .mount(&server)
            .await;
        let mut c = cfg(&server.uri(), FailMode::FailOpen);
        c.bearer = Some("secret-token".into());
        let http = reqwest::Client::new();
        assert_eq!(check(&http, &c, b"{}").await, DlpDecision::Allow);
    }

    #[tokio::test]
    async fn fail_open_on_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let decision = check(&http, &cfg(&server.uri(), FailMode::FailOpen), b"{}").await;
        assert_eq!(decision, DlpDecision::Allow);
    }

    #[tokio::test]
    async fn fail_closed_on_500_blocks() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let decision = check(&http, &cfg(&server.uri(), FailMode::FailClosed), b"{}").await;
        match decision {
            DlpDecision::Block { reason } => assert!(reason.contains("http 500")),
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fail_open_on_unreachable_endpoint() {
        // Port 1 — guaranteed unbound. The connect will refuse fast.
        let http = reqwest::Client::new();
        let decision = check(
            &http,
            &cfg("http://127.0.0.1:1", FailMode::FailOpen),
            b"{}",
        )
        .await;
        assert_eq!(decision, DlpDecision::Allow);
    }

    #[tokio::test]
    async fn unknown_decision_falls_through_fail_mode() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "decision": "what?"
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        assert_eq!(
            check(&http, &cfg(&server.uri(), FailMode::FailOpen), b"{}").await,
            DlpDecision::Allow
        );
    }

    #[test]
    fn parse_toml_with_defaults() {
        let cfg: DlpConfig = toml::from_str(r#"endpoint = "https://x.example""#).unwrap();
        assert_eq!(cfg.endpoint, "https://x.example");
        assert_eq!(cfg.timeout_ms, 800);
        assert_eq!(cfg.fail_mode, FailMode::FailOpen);
    }

    #[test]
    fn parse_toml_explicit_fail_closed() {
        let cfg: DlpConfig = toml::from_str(
            r#"
            endpoint = "https://x.example"
            fail_mode = "fail_closed"
            timeout_ms = 1500
            "#,
        )
        .unwrap();
        assert_eq!(cfg.fail_mode, FailMode::FailClosed);
        assert_eq!(cfg.timeout_ms, 1500);
    }
}
