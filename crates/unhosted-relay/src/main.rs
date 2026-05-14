//! Unhosted relay — rendezvous + byte-forwarding service for trusted-mode
//! peers that can't reach each other directly. See
//! `design/0005-relay-and-connection-topology.md` for the full protocol.
//!
//! What it does:
//!   - Accepts WebSocket sessions at `/v1/tunnel`
//!   - Each client registers with its Ed25519 pubkey, proving possession
//!     of the private key by signing a server-issued challenge
//!   - Forwards `forward` messages between registered peers, addressed by
//!     pubkey
//!   - Holds no decryption keys; payloads are opaque bytes
//!
//! What it does NOT do (yet):
//!   - Hole-punch coordination — that's the next sprint
//!   - Persistent state — restart drops all sessions, peers reconnect

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::Engine;
use clap::Parser;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::{sink::SinkExt, stream::StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};

const PROTOCOL_VERSION: u8 = 1;

#[derive(Parser, Debug)]
#[command(
    name = "unhosted-relay",
    version,
    about = "Rendezvous + relay for Unhosted trusted peers."
)]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:7780")]
    addr: SocketAddr,
}

type Tx = mpsc::UnboundedSender<Message>;

// Resource caps — set conservatively, override per deploy via env if needed.
const MAX_SESSIONS: usize = 10_000;
const MAX_SESSIONS_PER_IP: usize = 8;
/// Token-bucket window for `/v1/codes/{code}` lookups, per source IP.
/// At 8 attempts / 60s, a 32^4 (~1M) space takes >2 years to brute-force.
const CODE_LOOKUP_BURST: u32 = 8;
const CODE_LOOKUP_WINDOW_SECS: u64 = 60;

#[derive(Clone)]
struct AppState {
    /// Map of registered pubkey (base64) → outbound channel.
    sessions: Arc<Mutex<HashMap<String, Tx>>>,
    /// 4-letter pair codes → (offer URI, expiry). One-time, 5min TTL.
    codes: Arc<Mutex<HashMap<String, (String, std::time::Instant)>>>,
    /// Pending hole-punch offers, keyed by (asker_pubkey, target_pubkey).
    /// When the symmetric entry (target,asker) arrives, both sides are
    /// notified and the pair is cleared. 30s TTL — entries get garbage-
    /// collected on the next coordinate call.
    pending_punches: Arc<Mutex<HashMap<(String, String), PendingPunch>>>,
    /// Per-IP session count, used to enforce MAX_SESSIONS_PER_IP.
    /// Decremented when a session ends.
    ip_sessions: Arc<Mutex<HashMap<std::net::IpAddr, u32>>>,
    /// Code-lookup rate-limit state. `(window_start, count)` per IP.
    /// Cheap manual sliding window; fancier limiter not worth pulling.
    code_rate: Arc<Mutex<HashMap<std::net::IpAddr, (std::time::Instant, u32)>>>,
}

/// Half of a hole-punch handshake — recorded when one side asks before
/// the other has shown up. `external` is what the relay will tell the
/// peer to dial: the WS-observed IP and the locally-bound UDP port.
struct PendingPunch {
    external: SocketAddr,
    created: std::time::Instant,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("unhosted_relay=info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        codes: Arc::new(Mutex::new(HashMap::new())),
        pending_punches: Arc::new(Mutex::new(HashMap::new())),
        ip_sessions: Arc::new(Mutex::new(HashMap::new())),
        code_rate: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/tunnel", get(tunnel_handler))
        .route("/v1/info", get(info_handler))
        .route("/v1/codes", axum::routing::post(create_code_handler))
        .route("/v1/codes/{code}", get(consume_code_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cli.addr).await?;
    tracing::info!(addr = %cli.addr, version = PROTOCOL_VERSION, "unhosted-relay listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn info_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "service": "unhosted-relay",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL_VERSION,
    }))
}

// ----- short pair codes -----------------------------------------------------

const CODE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

#[derive(serde::Deserialize)]
struct CreateCodeRequest {
    offer: String,
}

#[derive(serde::Serialize)]
struct CreateCodeResponse {
    code: String,
    expires_in_seconds: u64,
}

async fn create_code_handler(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<CreateCodeRequest>,
) -> Result<axum::Json<CreateCodeResponse>, axum::http::StatusCode> {
    let mut codes = state.codes.lock().await;

    // gc expired codes opportunistically
    let now = std::time::Instant::now();
    codes.retain(|_, (_, exp)| now < *exp);

    // generate a unique 4-letter code (collision retry up to 10x)
    let mut tries = 0;
    let code = loop {
        let mut buf = [0u8; 4];
        for b in buf.iter_mut() {
            *b = CODE_ALPHABET
                [(rand::Rng::gen::<u8>(&mut rand::thread_rng()) as usize) % CODE_ALPHABET.len()];
        }
        let candidate = String::from_utf8(buf.to_vec()).unwrap();
        if !codes.contains_key(&candidate) {
            break candidate;
        }
        tries += 1;
        if tries > 10 {
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    codes.insert(code.clone(), (req.offer, now + CODE_TTL));
    Ok(axum::Json(CreateCodeResponse {
        code,
        expires_in_seconds: CODE_TTL.as_secs(),
    }))
}

#[derive(serde::Serialize)]
struct ConsumeCodeResponse {
    offer: String,
}

async fn consume_code_handler(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    axum::extract::Path(code): axum::extract::Path<String>,
) -> Result<axum::Json<ConsumeCodeResponse>, axum::http::StatusCode> {
    // Rate-limit per source IP. 4-letter codes only have ~20 bits of
    // entropy; without throttling, the space is hammerable in seconds.
    {
        let mut rates = state.code_rate.lock().await;
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(CODE_LOOKUP_WINDOW_SECS);
        let entry = rates.entry(remote.ip()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > CODE_LOOKUP_BURST {
            tracing::warn!(
                ip = %remote.ip(),
                count = entry.1,
                "rate-limited code lookup"
            );
            return Err(axum::http::StatusCode::TOO_MANY_REQUESTS);
        }
        // Opportunistic GC of stale entries.
        if rates.len() > 4096 {
            rates.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
    }

    let code = code.to_ascii_uppercase();
    let mut codes = state.codes.lock().await;

    let now = std::time::Instant::now();
    codes.retain(|_, (_, exp)| now < *exp);

    let entry = codes.remove(&code);
    match entry {
        Some((offer, exp)) if now < exp => Ok(axum::Json(ConsumeCodeResponse { offer })),
        _ => Err(axum::http::StatusCode::NOT_FOUND),
    }
}

async fn tunnel_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, remote))
}

// ----- protocol types --------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    /// First message after connect. Pubkey + signature over the server-issued challenge.
    Register {
        pubkey: String,
        challenge: String,
        signature: String,
    },
    /// Send opaque bytes to a registered peer.
    Forward {
        peer_pubkey: String,
        payload: String,
    },
    /// Ask the relay to coordinate a UDP hole-punch with `peer_pubkey`.
    /// `udp_port` is the local UDP port the requester has bound and is
    /// ready to receive on. Relay tells both sides each other's external
    /// addr (IP from the WS connection + the supplied UDP port).
    PunchRequest { peer_pubkey: String, udp_port: u16 },
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    /// Sent on connect; client must sign this challenge to register.
    Hello {
        challenge: &'a str,
        protocol: u8,
    },
    /// Sent after successful Register.
    Registered {
        pubkey: &'a str,
    },
    /// Forwarded payload from another peer.
    Inbound {
        from_pubkey: &'a str,
        payload: &'a str,
    },
    /// Both sides of a hole-punch get this with each other's external
    /// addr. Recipient should immediately send UDP packets to that addr
    /// while the NAT mapping is fresh on both ends.
    PunchTarget {
        peer_pubkey: &'a str,
        addr: &'a str,
    },
    Error {
        code: &'a str,
        message: &'a str,
    },
}

// ----- session ---------------------------------------------------------------

async fn handle_socket(socket: WebSocket, state: AppState, remote: SocketAddr) {
    // Enforce connection caps before doing any work. Total sessions and
    // per-IP sessions. A floods-from-one-source DoS attempts to exhaust
    // FD / memory; per-IP caps keep one bad actor from monopolizing.
    {
        let sessions = state.sessions.lock().await;
        if sessions.len() >= MAX_SESSIONS {
            tracing::warn!(%remote, "rejecting: relay at session cap");
            drop(sessions);
            return;
        }
        drop(sessions);

        let mut ip_count = state.ip_sessions.lock().await;
        let current = ip_count.entry(remote.ip()).or_insert(0);
        if *current >= MAX_SESSIONS_PER_IP as u32 {
            tracing::warn!(%remote, count = %current, "rejecting: per-IP session cap");
            return;
        }
        *current += 1;
    }

    let (mut sender, mut receiver) = socket.split();

    // Issue a random challenge the client must sign with their pubkey.
    let mut challenge_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut challenge_bytes);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(challenge_bytes);

    if let Ok(json) = serde_json::to_string(&ServerMessage::Hello {
        challenge: &challenge,
        protocol: PROTOCOL_VERSION,
    }) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    // Wait for Register.
    let (pubkey, tx, mut rx) = match register(&mut receiver, &mut sender, &challenge, &state).await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(%remote, error = %e, "register failed");
            let _ = sender
                .send(Message::Text(
                    serde_json::to_string(&ServerMessage::Error {
                        code: "register_failed",
                        message: &e.to_string(),
                    })
                    .unwrap_or_default()
                    .into(),
                ))
                .await;
            // Release the per-IP slot we tentatively reserved.
            let mut ip_count = state.ip_sessions.lock().await;
            if let Some(c) = ip_count.get_mut(&remote.ip()) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    ip_count.remove(&remote.ip());
                }
            }
            return;
        }
    };

    tracing::info!(%remote, %pubkey, "peer registered");

    // Forward outbound messages from our channel to the websocket.
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Read inbound from the websocket, dispatch to the peer's session.
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                let parsed: Result<ClientMessage, _> = serde_json::from_str(&text);
                match parsed {
                    Ok(ClientMessage::Forward {
                        peer_pubkey,
                        payload,
                    }) => {
                        let sessions = state.sessions.lock().await;
                        if let Some(peer_tx) = sessions.get(&peer_pubkey) {
                            let out = ServerMessage::Inbound {
                                from_pubkey: &pubkey,
                                payload: &payload,
                            };
                            if let Ok(json) = serde_json::to_string(&out) {
                                let _ = peer_tx.send(Message::Text(json.into()));
                            }
                        } else {
                            let err = serde_json::to_string(&ServerMessage::Error {
                                code: "peer_offline",
                                message: &format!("peer {peer_pubkey} not registered"),
                            })
                            .unwrap_or_default();
                            let _ = tx.send(Message::Text(err.into()));
                        }
                    }
                    Ok(ClientMessage::PunchRequest {
                        peer_pubkey,
                        udp_port,
                    }) => {
                        // Both sides must submit a PunchRequest naming the
                        // other; only when both halves arrive do we tell
                        // each side where to send packets. This lets the
                        // simultaneous-open work — both NATs see an
                        // outbound UDP packet within the same window.
                        let my_external = SocketAddr::new(remote.ip(), udp_port);

                        let mut pending = state.pending_punches.lock().await;
                        // GC stale entries.
                        let now = std::time::Instant::now();
                        pending.retain(|_, p| {
                            now.duration_since(p.created) < std::time::Duration::from_secs(30)
                        });

                        // Is the other side already waiting on us?
                        let counterpart_key = (peer_pubkey.clone(), pubkey.clone());
                        if let Some(other) = pending.remove(&counterpart_key) {
                            drop(pending);
                            let sessions = state.sessions.lock().await;
                            let peer_tx = sessions.get(&peer_pubkey).cloned();
                            let my_tx_for_msg = tx.clone();

                            // Tell me where to dial them.
                            let to_me = serde_json::to_string(&ServerMessage::PunchTarget {
                                peer_pubkey: &peer_pubkey,
                                addr: &other.external.to_string(),
                            })
                            .unwrap_or_default();
                            let _ = my_tx_for_msg.send(Message::Text(to_me.into()));

                            // Tell them where to dial me.
                            if let Some(peer_tx) = peer_tx {
                                let to_them = serde_json::to_string(&ServerMessage::PunchTarget {
                                    peer_pubkey: &pubkey,
                                    addr: &my_external.to_string(),
                                })
                                .unwrap_or_default();
                                let _ = peer_tx.send(Message::Text(to_them.into()));
                            }

                            tracing::info!(
                                from = %pubkey,
                                to = %peer_pubkey,
                                me = %my_external,
                                them = %other.external,
                                "punch coordinated"
                            );
                        } else {
                            // First half — record and wait.
                            pending.insert(
                                (pubkey.clone(), peer_pubkey.clone()),
                                PendingPunch {
                                    external: my_external,
                                    created: now,
                                },
                            );
                            tracing::debug!(
                                from = %pubkey,
                                to = %peer_pubkey,
                                me = %my_external,
                                "punch first half recorded"
                            );
                        }
                    }
                    Ok(ClientMessage::Register { .. }) => {
                        let err = serde_json::to_string(&ServerMessage::Error {
                            code: "already_registered",
                            message: "session is already registered",
                        })
                        .unwrap_or_default();
                        let _ = tx.send(Message::Text(err.into()));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "malformed client message");
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup.
    {
        let mut sessions = state.sessions.lock().await;
        sessions.remove(&pubkey);
    }
    {
        let mut ip_count = state.ip_sessions.lock().await;
        if let Some(c) = ip_count.get_mut(&remote.ip()) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                ip_count.remove(&remote.ip());
            }
        }
    }
    send_task.abort();
    tracing::info!(%remote, %pubkey, "peer disconnected");
}

async fn register(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    _sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    challenge: &str,
    state: &AppState,
) -> Result<(String, Tx, mpsc::UnboundedReceiver<Message>)> {
    let msg = receiver
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("websocket closed before register"))?
        .map_err(|e| anyhow::anyhow!("ws error: {e}"))?;

    let text = match msg {
        Message::Text(t) => t.to_string(),
        _ => anyhow::bail!("expected text register frame"),
    };

    let parsed: ClientMessage =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("malformed register: {e}"))?;
    let (pubkey_b64, sig_b64, sent_challenge) = match parsed {
        ClientMessage::Register {
            pubkey,
            challenge: c,
            signature,
        } => (pubkey, signature, c),
        _ => anyhow::bail!("first message must be register"),
    };
    if sent_challenge != challenge {
        anyhow::bail!("challenge mismatch");
    }

    // Verify the signature.
    let pk_bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(pubkey_b64.as_bytes())
        .map_err(|_| anyhow::anyhow!("pubkey not base64"))?;
    let pk_array: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("pubkey not 32 bytes"))?;
    let verifying =
        VerifyingKey::from_bytes(&pk_array).map_err(|e| anyhow::anyhow!("invalid pubkey: {e}"))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(sig_b64.as_bytes())
        .map_err(|_| anyhow::anyhow!("signature not base64"))?;
    let sig_array: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("signature not 64 bytes"))?;
    let signature = Signature::from_bytes(&sig_array);

    verifying
        .verify(challenge.as_bytes(), &signature)
        .map_err(|e| anyhow::anyhow!("bad signature: {e}"))?;

    // Register this pubkey.
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    {
        let mut sessions = state.sessions.lock().await;
        if sessions.contains_key(&pubkey_b64) {
            anyhow::bail!("pubkey already has an active session");
        }
        sessions.insert(pubkey_b64.clone(), tx.clone());
    }

    // Confirm.
    let confirmation = serde_json::to_string(&ServerMessage::Registered {
        pubkey: &pubkey_b64,
    })?;
    tx.send(Message::Text(confirmation.into()))
        .map_err(|_| anyhow::anyhow!("session channel closed"))?;

    Ok((pubkey_b64, tx, rx))
}
