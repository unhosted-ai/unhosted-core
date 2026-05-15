//! Server-side web-page fetcher for the LLM tool-use loop.
//!
//! Exposed at `POST /v1/tools/web_fetch`. Callers — the UI, an agent
//! pointing at this daemon, or eventually the model itself via the
//! tool-use scaffold — pass a URL and get back the page's plain text
//! body, capped, with the worst HTML stripped.
//!
//! Today: the endpoint stands alone; callers drive it explicitly.
//! Later: a tool-use loop inside `proxy_chat_local` will let the model
//! emit `<fetch>https://…</fetch>` mid-stream and have the daemon
//! resolve it for the next turn. The endpoint contract here is what
//! that loop will call internally, so getting it right now pays
//! forward.
//!
//! Threat model:
//! - **SSRF**: the daemon is reachable from outside the box once "open
//!   to internet" is on. An attacker who got the bearer token could
//!   ask the daemon to fetch `http://127.0.0.1:11434` (the LLM
//!   backend), `http://192.168.1.1` (the user's router), or AWS
//!   metadata endpoints. We refuse loopback, link-local, and the
//!   three RFC-1918 ranges by default. An `allow_local` flag is
//!   reserved for future intra-cluster fetches but is not exposed
//!   to the public API.
//! - **Bytes blow**: an LLM context window is small. We cap at
//!   `DEFAULT_MAX_BYTES` (200 KB), well under what any reasonable
//!   page is *useful* for after stripping. The HTTP layer also
//!   sets a deadline so a slow-loris source can't tie up the
//!   handler.
//! - **Privacy**: the fetch comes from the user's IP. We send a
//!   recognizable User-Agent so target sites can robots.txt us if
//!   they want. We do NOT pass through cookies, auth headers, or
//!   any of the request's incoming headers — the fetcher is a
//!   fresh outbound HTTPS client.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use url::Url;

/// Default cap on bytes returned to the caller. 200 KB is well past
/// any reasonable article body after stripping, but small enough that
/// an entire response fits in a 4k-token context with room to spare.
pub const DEFAULT_MAX_BYTES: usize = 200_000;

/// Network-level timeout for the whole request (connect + read).
/// Short enough that a hung remote doesn't tie up the chat loop;
/// long enough for the "slow blog over LTE" case.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// User-Agent we identify as. Lets target sites filter us with
/// robots.txt if they want — we're explicit rather than hiding
/// behind a vanilla curl UA. The trailing URL points at the
/// project so a target site's logs can tell what's calling.
fn user_agent() -> String {
    format!(
        "unhosted/{} (+https://github.com/unhosted-ai/unhosted-core)",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Deserialize, Debug)]
pub struct WebFetchRequest {
    pub url: String,
    /// Caller can request a smaller cap than [`DEFAULT_MAX_BYTES`].
    /// Larger requests are clamped at the default.
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Serialize, Debug)]
pub struct WebFetchResponse {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    pub truncated: bool,
    /// Plain text extracted from the response body. For HTML pages,
    /// `<script>`/`<style>` blocks are dropped, tags are stripped, and
    /// runs of whitespace are collapsed. For text/* responses we keep
    /// the body as-is (still truncated). For other types we return an
    /// empty string and let the caller decide what to do with the
    /// `content_type`.
    pub content: String,
}

#[derive(Debug)]
pub enum WebFetchError {
    InvalidUrl(String),
    SchemeRejected(String),
    HostRejected(String),
    DnsFailed(String),
    Network(String),
    UpstreamStatus(u16),
}

impl std::fmt::Display for WebFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(s) => write!(f, "invalid url: {s}"),
            Self::SchemeRejected(s) => write!(f, "scheme rejected: {s} (only https is allowed)"),
            Self::HostRejected(s) => write!(f, "host rejected: {s} (loopback / private / link-local addresses are blocked to prevent SSRF)"),
            Self::DnsFailed(s) => write!(f, "dns lookup failed: {s}"),
            Self::Network(s) => write!(f, "network error: {s}"),
            Self::UpstreamStatus(c) => write!(f, "upstream returned status {c}"),
        }
    }
}

impl std::error::Error for WebFetchError {}

/// Fetch a URL using the shared HTTP client and return a sanitized
/// snippet for the caller (UI, agent, or model tool-use loop).
///
/// This is the core unit: no auth, no routing — those live in the
/// HTTP handler in `lib.rs`. Pure function over (client, request) →
/// response, easy to unit-test without the axum surface.
pub async fn fetch(
    client: &reqwest::Client,
    req: WebFetchRequest,
) -> Result<WebFetchResponse, WebFetchError> {
    let parsed = Url::parse(&req.url).map_err(|e| WebFetchError::InvalidUrl(e.to_string()))?;
    // HTTPS only at the gateway. We could allow http to localhost
    // for testing, but the only safe localhost target is the daemon
    // itself, which the caller can reach directly.
    if parsed.scheme() != "https" {
        return Err(WebFetchError::SchemeRejected(parsed.scheme().to_string()));
    }
    // SSRF guard. Resolve the host and check every returned address;
    // a host like `attacker.com` that resolves to 192.168.1.1 must be
    // rejected, not just `192.168.1.1` typed literally.
    let host = parsed
        .host_str()
        .ok_or_else(|| WebFetchError::InvalidUrl("missing host".to_string()))?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let lookup_results: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| WebFetchError::DnsFailed(e.to_string()))?
            .collect();
    let mut any_safe = false;
    for sockaddr in &lookup_results {
        if is_private(&sockaddr.ip()) {
            return Err(WebFetchError::HostRejected(host));
        }
        any_safe = true;
    }
    if !any_safe {
        // No A/AAAA records. Refuse — better to give a clear error
        // than to let reqwest re-resolve and possibly bypass our check.
        return Err(WebFetchError::DnsFailed("no addresses returned".to_string()));
    }

    let cap = req
        .max_bytes
        .map(|n| n.min(DEFAULT_MAX_BYTES))
        .unwrap_or(DEFAULT_MAX_BYTES);

    let resp = client
        .get(parsed.as_str())
        .header(reqwest::header::USER_AGENT, user_agent())
        .header(reqwest::header::ACCEPT, "text/html,text/plain;q=0.9,*/*;q=0.5")
        .timeout(DEFAULT_TIMEOUT)
        .send()
        .await
        .map_err(|e| WebFetchError::Network(e.to_string()))?;

    let final_url = resp.url().to_string();
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(WebFetchError::UpstreamStatus(status));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    // Streaming read with a hard cap. Without this a multi-MB page
    // would buffer fully and then be discarded — better to stop at
    // the byte we know we can't show the model anyway.
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(cap.min(64 * 1024));
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| WebFetchError::Network(e.to_string()))?;
        if buf.len() + chunk.len() > cap {
            let remaining = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    let bytes_read = buf.len();
    let content = extract_text(&content_type, &buf);

    Ok(WebFetchResponse {
        url: req.url,
        final_url,
        status,
        content_type,
        bytes: bytes_read,
        truncated,
        content,
    })
}

/// IP-level SSRF guard. Returns true for any address we don't want
/// the fetcher to reach: loopback, link-local, RFC-1918 ranges,
/// CGNAT (100.64.0.0/10), and unspecified. The list is conservative
/// — we'd rather reject a legitimate edge case than let the daemon
/// be turned into a port scanner.
fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                || matches!(v4.octets(), [100, n, ..] if (64..=127).contains(&n)) // CGNAT
                || v4.octets()[0] == 0 // 0.0.0.0/8 "this network"
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique-local (fc00::/7)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local (fe80::/10)
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Extract plain text from an HTTP response body for LLM consumption.
/// Very intentional minimalism: strip `<script>` and `<style>` blocks
/// (their content is never useful and burns tokens), drop the rest of
/// the tags, collapse whitespace. No HTML5 parser, no DOM construction
/// — just enough to give the model something readable for the price
/// of a tiny dep surface.
fn extract_text(content_type: &str, body: &[u8]) -> String {
    let lower = content_type.to_ascii_lowercase();
    if lower.starts_with("text/html") || lower.starts_with("application/xhtml") {
        let raw = String::from_utf8_lossy(body);
        strip_html(&raw)
    } else if lower.starts_with("text/") || lower.contains("json") || lower.contains("xml") {
        String::from_utf8_lossy(body).to_string()
    } else {
        // Binary / unknown — return empty content; caller has the
        // `content_type` and `bytes` fields if they want to react.
        String::new()
    }
}

fn strip_html(raw: &str) -> String {
    // Drop scripts and styles wholesale.
    let mut out = String::with_capacity(raw.len() / 4);
    let bytes = raw.as_bytes();
    let mut i = 0;
    let mut in_tag = false;
    let mut last_space = true;
    while i < bytes.len() {
        // Look for `<script` / `<style` and skip to their closing tag.
        if !in_tag
            && bytes[i] == b'<'
            && (lower_starts(&bytes[i + 1..], b"script") || lower_starts(&bytes[i + 1..], b"style"))
        {
            let needle: &[u8] = if lower_starts(&bytes[i + 1..], b"script") {
                b"</script"
            } else {
                b"</style"
            };
            if let Some(close) = find_ci(&bytes[i..], needle) {
                if let Some(gt) = bytes[i + close..]
                    .iter()
                    .position(|&b| b == b'>')
                {
                    i += close + gt + 1;
                    continue;
                }
            }
            // Couldn't find a close tag — bail and stop processing.
            break;
        }
        match bytes[i] {
            b'<' => in_tag = true,
            b'>' if in_tag => {
                in_tag = false;
                // A tag boundary acts like whitespace for readability.
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            c if !in_tag => {
                if (c as char).is_whitespace() {
                    if !last_space {
                        out.push(' ');
                        last_space = true;
                    }
                } else {
                    out.push(c as char);
                    last_space = false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let trimmed = out.trim().to_string();
    // Decode a handful of the most common HTML entities so the text
    // doesn't look broken to the model. Full entity decoding is its
    // own dep; this covers > 95% of what real pages emit.
    trimmed
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn lower_starts(hay: &[u8], needle: &[u8]) -> bool {
    if hay.len() < needle.len() {
        return false;
    }
    hay[..needle.len()].eq_ignore_ascii_case(needle)
}

fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    for i in 0..=hay.len() - needle.len() {
        if lower_starts(&hay[i..], needle) {
            return Some(i);
        }
    }
    None
}

/// Convenience: convert a [`WebFetchError`] into the HTTP status we
/// surface from the handler. Keeps the route handler in `lib.rs`
/// concise.
pub fn error_status(err: &WebFetchError) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    match err {
        WebFetchError::InvalidUrl(_) => StatusCode::BAD_REQUEST,
        WebFetchError::SchemeRejected(_) => StatusCode::BAD_REQUEST,
        WebFetchError::HostRejected(_) => StatusCode::FORBIDDEN,
        WebFetchError::DnsFailed(_) => StatusCode::BAD_GATEWAY,
        WebFetchError::Network(_) => StatusCode::BAD_GATEWAY,
        WebFetchError::UpstreamStatus(_) => StatusCode::BAD_GATEWAY,
    }
}

// Silence unused-import warning when nothing in the file pulls in
// `anyhow` directly (we re-export it for callers that want to chain).
#[allow(dead_code)]
fn _re_export_anyhow() -> Result<()> {
    Err(anyhow!("placeholder"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ipv4_blocked() {
        assert!(is_private(&"127.0.0.1".parse().unwrap()));
        assert!(is_private(&"10.0.0.1".parse().unwrap()));
        assert!(is_private(&"192.168.1.1".parse().unwrap()));
        assert!(is_private(&"172.16.0.1".parse().unwrap()));
        assert!(is_private(&"169.254.1.1".parse().unwrap())); // link-local
        assert!(is_private(&"100.64.0.1".parse().unwrap())); // CGNAT
        assert!(is_private(&"0.0.0.0".parse().unwrap()));
        // Public addresses must pass through.
        assert!(!is_private(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private(&"1.1.1.1".parse().unwrap()));
        assert!(!is_private(&"172.32.0.1".parse().unwrap())); // just outside RFC 1918
    }

    #[test]
    fn private_ipv6_blocked() {
        assert!(is_private(&"::1".parse().unwrap()));
        assert!(is_private(&"::".parse().unwrap()));
        assert!(is_private(&"fe80::1".parse().unwrap())); // link-local
        assert!(is_private(&"fc00::1".parse().unwrap())); // unique-local
        // Public addresses pass through.
        assert!(!is_private(&"2606:4700:4700::1111".parse().unwrap())); // Cloudflare DNS
    }

    #[test]
    fn html_strips_scripts_and_tags() {
        let html = r#"<html><head><title>Hi</title><script>alert("x")</script></head>
        <body><p>Hello <b>world</b>!</p><style>body{}</style></body></html>"#;
        let out = strip_html(html);
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
        assert!(!out.contains("<"));
        assert!(!out.contains("alert"));
        assert!(!out.contains("body{}"));
    }

    #[test]
    fn html_collapses_whitespace_and_decodes_entities() {
        let html = "<p>a   &amp;   b &nbsp; c</p>";
        let out = strip_html(html);
        assert!(out.contains("a & b"));
        assert!(!out.contains("&amp;"));
    }

    #[test]
    fn extract_text_passes_text_plain_through() {
        let body = b"hello\nworld";
        let out = extract_text("text/plain; charset=utf-8", body);
        assert_eq!(out, "hello\nworld");
    }

    #[test]
    fn extract_text_empties_binary() {
        let body = &[0x89, 0x50, 0x4e, 0x47]; // PNG magic
        let out = extract_text("image/png", body);
        assert_eq!(out, "");
    }
}
