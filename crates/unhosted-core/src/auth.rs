//! Request authentication policy for the daemon.
//!
//! Two auth tracks:
//! - **Peer auth** via the `X-Unhosted-Auth` Ed25519 header — for
//!   inter-daemon requests between paired trusted peers.
//! - **Local auth** via `Authorization: Bearer <token>` — for the web
//!   UI and the user's own clients when the daemon is bound to a
//!   non-loopback address (so phone/LAN access doesn't accidentally
//!   open inference + state mutation to anyone on the same wifi).
//!
//! Plus a **replay guard**: an in-memory set of recently-seen
//! `(pubkey, ts, sig_digest)` triples so a captured signed request
//! can't be re-played within the 5-minute window.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use rand::RngCore;

use crate::identity::Identity;
use crate::peer::PeerRegistry;

const AUTH_HEADER: &str = "X-Unhosted-Auth";
const BEARER_HEADER: &str = "Authorization";
/// Hard cap on the replay table. Older entries get evicted on insert
/// once we cross this. With a 5-minute window, 65k slots holds ~218 req/s
/// across all peers before evicting — plenty for v0.1.x.
const REPLAY_MAX_ENTRIES: usize = 65_536;

/// How an incoming request authenticated. Each handler decides what to
/// allow based on its sensitivity (state-mutating endpoints reject
/// `LoopbackUnauthed`; read-only endpoints accept it).
#[derive(Debug, Clone)]
pub enum AuthOutcome {
    /// Verified signed request from a paired peer. Pubkey is base64.
    Peer(String),
    /// Verified local bearer token (user's own web UI / CLI on a non-
    /// loopback bind).
    Local,
    /// No auth header presented, but the request came from 127.0.0.1 /
    /// ::1 / the unix-socket path. Allowed for the user's own machine.
    LoopbackUnauthed,
    /// Header present but invalid (bad sig, expired ts, replay, wrong
    /// bearer, etc.) → 401.
    Rejected(&'static str),
    /// No auth header, not from loopback → 401.
    Missing,
}

impl AuthOutcome {
    pub fn is_authed(&self) -> bool {
        matches!(self, AuthOutcome::Peer(_) | AuthOutcome::Local)
    }

    /// True when the caller is the local user (loopback or valid local
    /// bearer). Used to gate endpoints that should be unreachable to
    /// paired peers (e.g. unpair, identity disclosure).
    pub fn is_local_user(&self) -> bool {
        matches!(self, AuthOutcome::Local | AuthOutcome::LoopbackUnauthed)
    }
}

/// Replay defense. Stores `(pubkey, ts, sig_digest)` keys with a TTL.
/// Anything within the verify-window that's been seen before is rejected.
#[derive(Default)]
pub struct ReplayGuard {
    seen: HashMap<ReplayKey, Instant>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct ReplayKey {
    pubkey: String,
    ts: u64,
    /// First 16 bytes of the signature — enough to disambiguate without
    /// storing the full 88-byte base64.
    sig_prefix: [u8; 16],
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if this is a *new* request, `false` if it's a replay.
    /// On `true`, the key is recorded for the duration of the window.
    pub fn record(&mut self, pubkey: &str, ts: u64, sig: &str, window: Duration) -> bool {
        let now = Instant::now();

        // Evict stale entries lazily.
        if self.seen.len() >= REPLAY_MAX_ENTRIES {
            self.seen.retain(|_, seen_at| now.duration_since(*seen_at) < window);
            // If still full, drop the oldest. Picking the oldest is O(n);
            // at 65k entries this is once per ~20k inserts max, fine.
            if self.seen.len() >= REPLAY_MAX_ENTRIES {
                if let Some((k, _)) = self
                    .seen
                    .iter()
                    .min_by_key(|(_, t)| *t)
                    .map(|(k, t)| (k.clone(), *t))
                {
                    self.seen.remove(&k);
                }
            }
        }

        let mut sig_prefix = [0u8; 16];
        let bytes = sig.as_bytes();
        let n = bytes.len().min(16);
        sig_prefix[..n].copy_from_slice(&bytes[..n]);

        let key = ReplayKey {
            pubkey: pubkey.to_string(),
            ts,
            sig_prefix,
        };

        if let Some(seen_at) = self.seen.get(&key) {
            if now.duration_since(*seen_at) < window {
                return false; // replay
            }
        }
        self.seen.insert(key, now);
        true
    }
}

/// The daemon's local bearer token. Read from / persisted to
/// `~/.config/unhosted/api-token.txt` (0600) so it survives restarts —
/// the web UI stores it in localStorage and would otherwise have to be
/// re-paired on every daemon restart.
#[derive(Clone)]
pub struct LocalToken {
    value: Arc<String>,
}

impl LocalToken {
    pub fn load_or_create() -> Result<Self> {
        Self::load_or_create_at(&token_path()?)
    }

    pub fn load_or_create_at(path: &Path) -> Result<Self> {
        if path.exists() {
            let token = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?
                .trim()
                .to_string();
            if !token.is_empty() {
                return Ok(Self {
                    value: Arc::new(token),
                });
            }
        }

        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        let token = B64URL.encode(buf);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, &token)
            .with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self {
            value: Arc::new(token),
        })
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Constant-time comparison. Avoids timing-side-channel leaks when an
    /// attacker brute-forces the bearer.
    pub fn check(&self, candidate: &str) -> bool {
        let a = self.value.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        let mut diff: u8 = 0;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }
}

pub fn token_path() -> Result<PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").context("HOME env var not set")?;
        PathBuf::from(home).join(".config")
    };
    Ok(dir.join("unhosted").join("api-token.txt"))
}

/// Classify an incoming request. Pure-ish — only mutates the replay
/// guard. Handlers decide what to allow based on the returned variant.
pub fn classify(
    headers: &HeaderMap,
    peer_addr: Option<IpAddr>,
    body: &[u8],
    registry: &Arc<std::sync::Mutex<PeerRegistry>>,
    local_token: &LocalToken,
    replay: &Arc<Mutex<ReplayGuard>>,
) -> AuthOutcome {
    // 1. Peer-authed via X-Unhosted-Auth?
    if let Some(hv) = headers.get(AUTH_HEADER) {
        let Ok(auth_str) = hv.to_str() else {
            return AuthOutcome::Rejected("malformed auth header");
        };
        let Some(sender_pk) = Identity::verify_request(auth_str, body) else {
            return AuthOutcome::Rejected("signature invalid or expired");
        };

        // Parse out ts + sig for the replay guard. Same split as verify_request.
        let mut parts = auth_str.splitn(3, ':');
        let _pk = parts.next();
        let ts_str = parts.next();
        let sig = parts.next();
        if let (Some(ts_str), Some(sig)) = (ts_str, sig) {
            if let Ok(ts) = ts_str.parse::<u64>() {
                let fresh = replay
                    .lock()
                    .map(|mut g| g.record(&sender_pk, ts, sig, Duration::from_secs(300)))
                    .unwrap_or(true);
                if !fresh {
                    return AuthOutcome::Rejected("replay detected");
                }
            }
        }

        let is_trusted = registry
            .lock()
            .map(|r| {
                r.peers
                    .iter()
                    .any(|p| p.pubkey.as_deref() == Some(sender_pk.as_str()))
            })
            .unwrap_or(false);
        if !is_trusted {
            return AuthOutcome::Rejected("signing pubkey is not a paired peer");
        }
        return AuthOutcome::Peer(sender_pk);
    }

    // 2. Local bearer?
    if let Some(hv) = headers.get(BEARER_HEADER) {
        let Ok(bearer_str) = hv.to_str() else {
            return AuthOutcome::Rejected("malformed bearer header");
        };
        let candidate = bearer_str
            .strip_prefix("Bearer ")
            .or_else(|| bearer_str.strip_prefix("bearer "))
            .unwrap_or(bearer_str);
        if local_token.check(candidate) {
            return AuthOutcome::Local;
        }
        return AuthOutcome::Rejected("bad bearer token");
    }

    // 3. No auth — only allow if from loopback.
    match peer_addr {
        Some(ip) if ip.is_loopback() => AuthOutcome::LoopbackUnauthed,
        Some(_) => AuthOutcome::Missing,
        None => AuthOutcome::Missing, // unknown source, fail closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_guard_rejects_duplicate() {
        let mut g = ReplayGuard::new();
        let window = Duration::from_secs(60);
        assert!(g.record("pk", 100, "sig", window));
        assert!(!g.record("pk", 100, "sig", window));
        // Different ts is fine.
        assert!(g.record("pk", 101, "sig", window));
        // Different pubkey is fine.
        assert!(g.record("pk2", 100, "sig", window));
    }

    #[test]
    fn local_token_check_constant_time_correctness() {
        // Use an explicit path to avoid touching the real config dir.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("unhosted-tok-{}-{stamp}", std::process::id()));
        let path = dir.join("api-token.txt");
        let t = LocalToken::load_or_create_at(&path).unwrap();
        assert!(t.check(t.value()));
        assert!(!t.check(""));
        assert!(!t.check("nope"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
