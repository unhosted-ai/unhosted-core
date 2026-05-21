//! Audit-log feed.
//!
//! Emits structured JSON events for every operation that touches
//! authentication, configuration, or peer state. Two access paths:
//!
//! - **`GET /v1/audit/recent?limit=N`** — returns the last N events
//!   from the in-memory ring buffer (default 1000 events). Cheap;
//!   one snapshot.
//! - **`GET /v1/audit/stream`** — Server-Sent-Events stream that
//!   begins with the ring buffer, then live-tails. Each event is one
//!   `data: <json>\n\n` frame.
//!
//! Both paths require off-loopback auth (bearer token or paired-peer
//! signature) — the audit feed is sensitive and must not leak
//! pubkeys / policy mutations to the local network.
//!
//! The feed is intentionally simple: no on-disk retention, no
//! filtering, no aggregation. Operators ship to a SIEM (Splunk,
//! Datadog, Vector) and retain there. The platform's job is to
//! emit the events; the operator's job is to keep them.
//!
//! Why a ring buffer plus a broadcast channel: the ring buffer
//! lets late subscribers see the last few minutes of activity
//! without missing it; the broadcast channel delivers live events
//! to every active stream. Subscribers that fall behind get
//! `lagged` errors and have to reconnect — by design, the audit
//! feed never blocks the emitter.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

/// Default ring-buffer size. 1000 events at ~200 bytes each = 200KB
/// — small relative to the daemon's working set, large enough to
/// span a few minutes of activity at moderate load.
const RING_DEFAULT: usize = 1000;

/// Default broadcast-channel capacity. Sized so a slow consumer
/// (browser SSE tab with the WebSocket in a backgrounded tab) can
/// fall ~256 events behind before being dropped.
const BROADCAST_DEFAULT: usize = 256;

/// What we actually emit. Each variant carries the structured
/// fields the operator's SIEM will index on. Top-level `kind` is
/// serialized as a discriminator so the wire format is
/// `{"kind":"chat_completion_started","ts":...,...}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEvent {
    /// A chat completion request entered the daemon. Emitted before
    /// the upstream call so a stuck request still leaves a trace.
    ChatCompletionStarted {
        ts: u64,
        /// `loopback`, `bearer`, or `peer:<pubkey>` — who the
        /// daemon authenticated the caller as.
        caller: String,
        /// The upstream the daemon will forward to: `local`,
        /// `peer:<name>`, or a URL.
        upstream: String,
        /// Best-effort: the model name from the request body, if
        /// present. May be empty for OpenAI-compatible clients that
        /// omit `model`.
        model: String,
    },
    /// A chat completion finished cleanly. `error` variants emit
    /// `ChatCompletionFailed` instead.
    ChatCompletionFinished {
        ts: u64,
        caller: String,
        upstream: String,
        model: String,
        /// Approximate completion-token count if the runtime reported
        /// one in its final `usage` field; -1 if unknown.
        completion_tokens: i64,
    },
    /// A new peer was paired. Adds an Ed25519 pubkey to the
    /// receiver's authorized-peer registry.
    PeerPaired {
        ts: u64,
        peer_name: String,
        peer_pubkey: String,
        /// `offered` (we issued the token) or `accepted` (we
        /// consumed someone else's token).
        direction: String,
    },
    /// An existing peer was removed.
    PeerUnpaired {
        ts: u64,
        peer_name: String,
        peer_pubkey: String,
    },
    /// The public-mode policy was changed via PUT
    /// `/v1/public-mode/policy`. Lets a SIEM alert on policy
    /// drift (a host that flipped to accepting an unexpected
    /// rail).
    PolicyChanged {
        ts: u64,
        /// `loopback`, `bearer`, or `peer:<pubkey>`.
        caller: String,
        /// The full policy after enforcement (sanctions defaults
        /// auto-merged), serialized as a JSON object. Used by a
        /// SIEM diff alert against the previous PolicyChanged event.
        policy: serde_json::Value,
    },
    /// A chat completion was blocked by the DLP integration. The
    /// `reason` mirrors what the DLP endpoint returned (e.g.
    /// "PII: SSN-pattern matched"). Same caller / model fields as
    /// ChatCompletionStarted so a SIEM can correlate.
    DlpBlocked {
        ts: u64,
        caller: String,
        model: String,
        reason: String,
    },
    /// The Cloudflare tunnel transitioned from off to starting,
    /// from starting to live, or from live to stopped. Three
    /// sub-states represented by `state`: `starting`, `live`,
    /// `stopped`.
    TunnelStateChanged {
        ts: u64,
        state: String,
        /// The public URL if `state == "live"`, otherwise empty.
        url: String,
    },
}

impl AuditEvent {
    /// Wall-clock timestamp at event emission. We capture once at
    /// construction so all sinks (ring buffer, broadcast, SSE
    /// stream) see the same value.
    pub fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// The shared audit broadcaster. Held inside `Arc` on NodeState so
/// handlers can clone and emit cheaply. `emit` is non-blocking:
/// dropped events because the channel is full are not propagated as
/// errors — the buffer survives, and any active SSE stream will see
/// the next event after the lag.
pub struct AuditBroadcaster {
    ring: Mutex<VecDeque<AuditEvent>>,
    ring_cap: usize,
    tx: broadcast::Sender<AuditEvent>,
}

impl AuditBroadcaster {
    pub fn new() -> Self {
        Self::with_capacity(RING_DEFAULT, BROADCAST_DEFAULT)
    }

    pub fn with_capacity(ring_cap: usize, broadcast_cap: usize) -> Self {
        let (tx, _) = broadcast::channel(broadcast_cap);
        Self {
            ring: Mutex::new(VecDeque::with_capacity(ring_cap)),
            ring_cap,
            tx,
        }
    }

    /// Append to the ring buffer + broadcast. Cheap: a mutex
    /// acquire + a single push. Never blocks the emitter.
    pub fn emit(&self, event: AuditEvent) {
        if let Ok(mut ring) = self.ring.lock() {
            if ring.len() == self.ring_cap {
                ring.pop_front();
            }
            ring.push_back(event.clone());
        }
        // Errors from `send` mean no active subscribers — fine.
        let _ = self.tx.send(event);
    }

    /// Return the most recent `n` events. If `n` exceeds the ring
    /// size, returns the whole ring.
    pub fn recent(&self, n: usize) -> Vec<AuditEvent> {
        let ring = match self.ring.lock() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let start = ring.len().saturating_sub(n);
        ring.iter().skip(start).cloned().collect()
    }

    /// Subscribe to the live broadcast. Each subscriber owns its
    /// own receiver; falling-behind subscribers are dropped with
    /// a `RecvError::Lagged` rather than blocking the sender.
    pub fn subscribe(&self) -> broadcast::Receiver<AuditEvent> {
        self.tx.subscribe()
    }
}

impl Default for AuditBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_respects_cap() {
        let b = AuditBroadcaster::with_capacity(3, 16);
        for i in 0..5 {
            b.emit(AuditEvent::ChatCompletionStarted {
                ts: i as u64,
                caller: "loopback".into(),
                upstream: "local".into(),
                model: format!("m{i}"),
            });
        }
        let recent = b.recent(10);
        assert_eq!(recent.len(), 3, "ring should be capped at 3");
        match &recent[0] {
            AuditEvent::ChatCompletionStarted { ts, .. } => {
                assert_eq!(*ts, 2, "oldest in ring should be index 2 after 5 pushes");
            }
            _ => panic!("expected ChatCompletionStarted"),
        }
    }

    #[test]
    fn recent_zero_returns_empty() {
        let b = AuditBroadcaster::new();
        b.emit(AuditEvent::PeerPaired {
            ts: 1,
            peer_name: "a".into(),
            peer_pubkey: "k".into(),
            direction: "offered".into(),
        });
        assert!(b.recent(0).is_empty());
    }

    #[test]
    fn recent_larger_than_ring_returns_all() {
        let b = AuditBroadcaster::with_capacity(2, 16);
        b.emit(AuditEvent::PeerPaired {
            ts: 1,
            peer_name: "a".into(),
            peer_pubkey: "k".into(),
            direction: "offered".into(),
        });
        b.emit(AuditEvent::PeerPaired {
            ts: 2,
            peer_name: "b".into(),
            peer_pubkey: "k".into(),
            direction: "accepted".into(),
        });
        assert_eq!(b.recent(100).len(), 2);
    }

    #[tokio::test]
    async fn subscribers_see_live_events() {
        let b = AuditBroadcaster::new();
        let mut rx = b.subscribe();
        b.emit(AuditEvent::TunnelStateChanged {
            ts: 1,
            state: "live".into(),
            url: "https://x.trycloudflare.com".into(),
        });
        match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(AuditEvent::TunnelStateChanged { state, .. })) => {
                assert_eq!(state, "live");
            }
            other => panic!("expected TunnelStateChanged, got {other:?}"),
        }
    }

    #[test]
    fn event_serializes_with_kind_discriminator() {
        let e = AuditEvent::ChatCompletionStarted {
            ts: 42,
            caller: "loopback".into(),
            upstream: "local".into(),
            model: "llama3.2".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"kind\":\"chat_completion_started\""));
        assert!(json.contains("\"ts\":42"));
        assert!(json.contains("\"model\":\"llama3.2\""));
    }
}
