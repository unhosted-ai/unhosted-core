//! Core engine for Unhosted.
//!
//! - v0.0.1: single-machine inference — proxies to a local llama-server.
//! - v0.0.2 (in progress): multi-node — the daemon round-robins requests
//!   across `Local` + configured peers, with loop prevention and per-request
//!   fallback to local on peer failure.
//!
//! Peer protocol is the same HTTP API the CLI uses (`POST /v1/run`), so a
//! peer is just another `unhosted serve` process. No new transport.

pub mod discovery;
pub mod identity;
pub mod peer;
pub mod relay_client;
pub mod router;
mod web;

pub use discovery::{default_node_name, DiscoveredPeer, Discovery};
pub use identity::Identity;
pub use peer::{Peer, PeerRegistry};
pub use relay_client::{InboundRequest, RelayClient, RelayState, ResponseEvent};
pub use router::{Router as RouteRouter, Target};

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router as AxumRouter,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// Default upstream llama-server URL when no override is configured.
pub const DEFAULT_LLAMA_SERVER_URL: &str = "http://127.0.0.1:8080";

/// Default address the local Unhosted node listens on.
pub const DEFAULT_NODE_ADDR: &str = "127.0.0.1:7777";

/// Header used to mark a request as already forwarded from another peer.
/// Daemons that receive this header skip the router and serve locally.
const FORWARDED_HEADER: &str = "x-unhosted-forwarded";

/// `X-Unhosted-Auth: <pubkey>:<ts>:<sig>` — present on requests from trusted
/// peers. Receiver verifies the signature against the registry. If the
/// header is present but invalid, the request is rejected outright. If
/// absent, requests are accepted (back-compat with LAN/v0.0.x callers).
const AUTH_HEADER: &str = "x-unhosted-auth";

/// Default system prompt — anchors the assistant's voice. Plain, direct,
/// no "as an AI" padding, length matched to the question.
const DEFAULT_SYSTEM_PROMPT: &str = "you are the assistant inside unhosted, open-source software that runs ai on hardware the user owns. answer plainly and directly. do not begin with disclaimers like \"as an ai\" or \"i'm an artificial intelligence\". do not use marketing words (\"exciting\", \"powerful\", \"leverage\", \"empower\"). match the length of your answer to the length the question needs — short questions get short answers. if you do not know something, say so.";

#[derive(Clone, Debug)]
pub struct Node {
    pub addr: SocketAddr,
    pub llama_server_url: String,
    /// Peers reachable from this node. Loaded from the peer registry at
    /// startup; empty means single-node operation (v0.0.1 behavior).
    pub peers: Vec<Peer>,
    /// Human-readable name used for mDNS announcement and the served-by tag.
    pub name: String,
    /// Optional relay URL (`wss://...` or `ws://...`). When set, the daemon
    /// connects to the relay, registers with its identity, and (eventually)
    /// routes off-LAN peer traffic through it.
    pub relay_url: Option<String>,
}

impl Node {
    pub fn local() -> Self {
        Self {
            addr: DEFAULT_NODE_ADDR.parse().expect("valid default addr"),
            llama_server_url: std::env::var("UNHOSTED_LLAMA_SERVER_URL")
                .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_URL.to_string()),
            peers: Vec::new(),
            name: default_node_name(),
            relay_url: std::env::var("UNHOSTED_RELAY").ok(),
        }
    }
}

/// Runtime state shared by all request handlers.
#[derive(Clone)]
struct NodeState {
    node: Arc<Node>,
    router: Arc<RouteRouter>,
    registry: Arc<std::sync::Mutex<PeerRegistry>>,
    discovery: Option<Discovery>,
    identity: Identity,
    /// Outstanding pairing tokens issued by `POST /v1/pair/offer`. Each
    /// expires 5 minutes after issuance. In-memory only — restart drops
    /// them, which is the right behavior for one-time secrets.
    pairing_tokens: Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    relay: RelayClient,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RunRequest {
    pub prompt: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_max_tokens() -> u32 {
    256
}

#[derive(Serialize, Debug)]
struct ChatRequest<'a> {
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Serialize, Debug)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

pub async fn serve(node: Node) -> Result<()> {
    let router = Arc::new(RouteRouter::new(&node.peers));

    // mDNS: announce ourselves and start browsing for peers. Best-effort —
    // if it fails, the daemon still works, you just don't get auto-discovery.
    // Identity first — we want the pubkey to flow into mDNS announcements.
    let identity = Identity::load_or_create().context("loading node identity")?;
    tracing::info!(pubkey = %identity.public_b64(), "node identity loaded");
    let pubkey_for_mdns = identity.public_b64();

    let discovery = match Discovery::start(
        &node.name,
        node.addr,
        env!("CARGO_PKG_VERSION"),
        Some(&pubkey_for_mdns),
    ) {
        Ok(d) => {
            tracing::info!(name = %node.name, "mdns discovery active");
            Some(d)
        }
        Err(e) => {
            tracing::warn!(error = %e, "mdns discovery disabled — peers won't auto-discover");
            None
        }
    };

    let registry = Arc::new(std::sync::Mutex::new(PeerRegistry {
        peers: node.peers.clone(),
    }));

    let (relay, inbound_rx) = if let Some(url) = node.relay_url.clone() {
        tracing::info!(relay = %url, "starting relay client");
        let (client, rx) = RelayClient::spawn(identity.clone(), url);
        (client, Some(rx))
    } else {
        (RelayClient::disabled(), None)
    };

    let state = NodeState {
        node: Arc::new(node.clone()),
        router: router.clone(),
        registry,
        discovery,
        identity,
        pairing_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        relay,
    };

    // Dispatcher for inbound relay requests: peer sent us a request via the
    // relay; run it locally and stream chunks back through the relay's
    // response channel.
    if let Some(mut rx) = inbound_rx {
        let state_for_inbound = state.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let state = state_for_inbound.clone();
                tokio::spawn(async move {
                    dispatch_inbound_relay_request(state, req).await;
                });
            }
        });
    }

    let api = AxumRouter::new()
        .route("/health", get(health))
        .route("/v1/run", post(run_handler))
        .route("/v1/status", get(status_handler))
        .route("/v1/peers", post(pair_handler))
        .route("/v1/peers/{name}", axum::routing::delete(unpair_handler))
        .route("/v1/pair/offer", post(pair_offer_handler))
        .route("/v1/pair/accept", post(pair_accept_handler))
        .route("/v1/pair/connect", post(pair_connect_handler))
        .route("/v1/pair/short-offer", post(pair_short_offer_handler))
        .route("/v1/pair/use-code", post(pair_use_code_handler))
        .route("/v1/identity", get(identity_handler))
        // OpenAI-compatible endpoints — any client that speaks OpenAI's HTTP
        // API (Delta, LangChain, LlamaIndex, OpenWebUI, …) can point at
        // http://127.0.0.1:7777 instead of OpenAI / Ollama / llama-server.
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/models", get(models_handler))
        .with_state(state);

    let app = api
        .route("/", get(web::serve_index))
        .fallback(web::serve_static)
        .layer(cors_layer());

    let listener = tokio::net::TcpListener::bind(node.addr).await?;
    tracing::info!(
        addr = %node.addr,
        upstream = %node.llama_server_url,
        peers = router.target_count() - 1,
        ui = "enabled",
        "unhosted node listening — open http://{} in a browser",
        node.addr
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// CORS policy. Default is local-only — explicit allow-list extends it to
/// browser-based clients (e.g. a Delta extension served from a non-loopback
/// origin).
///
///   UNHOSTED_CORS_ORIGINS=""         only localhost / 127.0.0.1 origins
///   UNHOSTED_CORS_ORIGINS="*"        allow any origin (use with care)
///   UNHOSTED_CORS_ORIGINS="https://delta.local,https://x.unhosted.dev"
fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let raw = std::env::var("UNHOSTED_CORS_ORIGINS").unwrap_or_default();
    let trimmed = raw.trim();

    let base = CorsLayer::new()
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::Any)
        .max_age(std::time::Duration::from_secs(600));

    if trimmed.is_empty() {
        // Default: allow loopback origins so the embedded web UI works
        // and any tool on the same machine reaches us, but nothing else.
        return base.allow_origin(AllowOrigin::predicate(|origin, _req| {
            origin.as_bytes().starts_with(b"http://127.0.0.1")
                || origin.as_bytes().starts_with(b"http://localhost")
                || origin.as_bytes().starts_with(b"https://127.0.0.1")
                || origin.as_bytes().starts_with(b"https://localhost")
                || origin.as_bytes().starts_with(b"tauri://")
                || origin.as_bytes().starts_with(b"file://")
        }));
    }

    if trimmed == "*" {
        return base.allow_origin(AllowOrigin::any());
    }

    let origins: Vec<axum::http::HeaderValue> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    base.allow_origin(origins)
}

#[derive(Serialize)]
struct StatusResponse {
    node: NodeStatus,
    upstream: UpstreamStatus,
    peers: Vec<PeerStatus>,
    routing: RoutingStatus,
    discovered: Vec<DiscoveredPeer>,
    relay: RelayStatus,
}

#[derive(Serialize)]
struct RelayStatus {
    /// "disabled" | "connecting" | "registered" | "error"
    state: &'static str,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct NodeStatus {
    addr: String,
    name: String,
    version: &'static str,
}

#[derive(Serialize)]
struct UpstreamStatus {
    url: String,
    reachable: bool,
    model: Option<String>,
}

#[derive(Serialize)]
struct PeerStatus {
    name: String,
    addr: String,
    priority: u8,
    /// True when the peer is paired with a known Ed25519 pubkey. Used by
    /// the UI to badge trusted peers vs. unauthenticated LAN ones.
    trusted: bool,
}

#[derive(Serialize)]
struct RoutingStatus {
    targets: usize,
    mode: &'static str,
}

async fn status_handler(State(state): State<NodeState>) -> axum::Json<StatusResponse> {
    let upstream_url = state.node.llama_server_url.clone();
    let (reachable, model) = probe_upstream(&upstream_url).await;

    let mut discovered = state
        .discovery
        .as_ref()
        .map(|d| d.snapshot())
        .unwrap_or_default();

    // Auto-restore: if a discovered peer's pubkey matches one of our paired
    // peers but the addr has drifted (IP change after a router reboot, e.g.),
    // update the registry in-place so direct routing works without a fresh
    // pairing round.
    {
        let mut reg = match state.registry.lock() {
            Ok(r) => r,
            Err(_) => {
                return axum::Json(StatusResponse {
                    node: NodeStatus {
                        addr: state.node.addr.to_string(),
                        name: state.node.name.clone(),
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    upstream: UpstreamStatus {
                        url: upstream_url,
                        reachable,
                        model,
                    },
                    peers: vec![],
                    routing: RoutingStatus {
                        targets: state.router.target_count(),
                        mode: "round-robin",
                    },
                    discovered: vec![],
                    relay: RelayStatus {
                        state: "error",
                        url: None,
                        error: Some("registry lock poisoned".into()),
                    },
                });
            }
        };
        let mut changed = false;
        for d in &discovered {
            let Some(dpk) = d.pubkey.as_deref() else {
                continue;
            };
            if let Some(p) = reg
                .peers
                .iter_mut()
                .find(|p| p.pubkey.as_deref() == Some(dpk))
            {
                if p.addr != d.addr {
                    tracing::info!(
                        peer = %p.name,
                        old = %p.addr,
                        new = %d.addr,
                        "auto-restoring paired peer addr from mDNS"
                    );
                    p.addr = d.addr;
                    changed = true;
                }
            }
        }
        if changed {
            let _ = reg.save();
            state.router.replace_peers(&reg.peers);
        }
    }

    let peers = state
        .registry
        .lock()
        .map(|r| {
            r.peers
                .iter()
                .map(|p| PeerStatus {
                    name: p.name.clone(),
                    addr: p.addr.to_string(),
                    priority: p.priority,
                    trusted: p.pubkey.is_some(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Hide peers that are already paired — match by name OR by advertised
    // pubkey. Either signal means "we already trust this device" and we
    // don't want to show a redundant pair button.
    let paired_names: std::collections::HashSet<String> =
        peers.iter().map(|p| p.name.clone()).collect();
    let paired_pubkeys: std::collections::HashSet<String> = state
        .registry
        .lock()
        .map(|r| r.peers.iter().filter_map(|p| p.pubkey.clone()).collect())
        .unwrap_or_default();
    discovered.retain(|d| {
        !paired_names.contains(&d.name)
            && !d
                .pubkey
                .as_ref()
                .map(|pk| paired_pubkeys.contains(pk))
                .unwrap_or(false)
    });

    let relay_state = state.relay.current_state().await;
    let relay = match relay_state {
        RelayState::Disabled => RelayStatus {
            state: "disabled",
            url: None,
            error: None,
        },
        RelayState::Connecting => RelayStatus {
            state: "connecting",
            url: state.node.relay_url.clone(),
            error: None,
        },
        RelayState::Registered => RelayStatus {
            state: "registered",
            url: state.node.relay_url.clone(),
            error: None,
        },
        RelayState::Error(msg) => RelayStatus {
            state: "error",
            url: state.node.relay_url.clone(),
            error: Some(msg),
        },
    };

    axum::Json(StatusResponse {
        node: NodeStatus {
            addr: state.node.addr.to_string(),
            name: state.node.name.clone(),
            version: env!("CARGO_PKG_VERSION"),
        },
        upstream: UpstreamStatus {
            url: upstream_url,
            reachable,
            model,
        },
        peers,
        routing: RoutingStatus {
            targets: state.router.target_count(),
            mode: "round-robin",
        },
        discovered,
        relay,
    })
}

#[derive(Deserialize)]
struct PairRequest {
    name: String,
    addr: SocketAddr,
    #[serde(default = "default_pair_priority")]
    priority: u8,
}

fn default_pair_priority() -> u8 {
    10
}

#[derive(Serialize)]
struct PairResponse {
    ok: bool,
    peers: Vec<PeerStatus>,
}

async fn pair_handler(
    State(state): State<NodeState>,
    Json(req): Json<PairRequest>,
) -> Result<axum::Json<PairResponse>, StatusCode> {
    let new_peer = Peer {
        name: req.name.clone(),
        addr: req.addr,
        priority: req.priority,
        models: vec![],
        pubkey: None, // LAN-discovered; trusted pairing flows through /v1/pair/accept
    };

    {
        let mut reg = state
            .registry
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        reg.add(new_peer).map_err(|e| {
            tracing::error!(error = %e, "pair: persisting peer failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        state.router.replace_peers(&reg.peers);
        tracing::info!(name = %req.name, addr = %req.addr, "peer paired and live");
    }

    let peers = state
        .registry
        .lock()
        .map(|r| {
            r.peers
                .iter()
                .map(|p| PeerStatus {
                    name: p.name.clone(),
                    addr: p.addr.to_string(),
                    priority: p.priority,
                    trusted: p.pubkey.is_some(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(axum::Json(PairResponse { ok: true, peers }))
}

// ---------------------------------------------------------------- trusted pairing (v0.1.0)

const PAIRING_TOKEN_TTL: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Serialize)]
struct PairOfferResponse {
    /// One-time token the acceptor presents to /v1/pair/accept. 5min TTL.
    token: String,
    /// Compact share URI containing the addr + token; can be copy-pasted out
    /// of band (Signal, email, paper) to the other party.
    offer: String,
    expires_in_seconds: u64,
    /// How the other side will reach us:
    ///   "relay"          — works behind NAT, via the relay
    ///   "lan"            — only works on the same LAN (no relay registered)
    ///   "loopback_only"  — only works on the same machine (bind addr is
    ///                      loopback and we couldn't detect a LAN IP)
    reachability: String,
}

async fn pair_offer_handler(
    State(state): State<NodeState>,
) -> Result<axum::Json<PairOfferResponse>, StatusCode> {
    use base64::Engine;
    let mut buf = [0u8; 9];
    rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);

    let now = std::time::Instant::now();
    {
        let mut tokens = state
            .pairing_tokens
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tokens.retain(|_, t| now.duration_since(*t) < PAIRING_TOKEN_TTL);
        tokens.insert(token.clone(), now);
    }

    // Include relay info if we're registered, so the accepting side can
    // reach us when neither end has a public IP.
    let relay = match state.relay.current_state().await {
        RelayState::Registered => state.node.relay_url.clone(),
        _ => None,
    };
    let pubkey = state.identity.public_b64();

    // If the daemon is bound to loopback (127.0.0.1), the other side can't
    // reach it from anywhere else. Substitute the LAN IP so the offer is
    // usable from at least the same network.
    let advertised_addr = advertised_addr(state.node.addr);
    let only_loopback = advertised_addr.ip().is_loopback();

    let has_relay = relay.is_some();
    let offer = match &relay {
        Some(url) => format!(
            "unhosted://pair?addr={}&pk={}&relay={}&token={}",
            advertised_addr,
            urlencode(&pubkey),
            urlencode(url),
            token
        ),
        None => format!(
            "unhosted://pair?addr={}&pk={}&token={}",
            advertised_addr,
            urlencode(&pubkey),
            token
        ),
    };

    let reachability = if has_relay {
        "relay".to_string()
    } else if only_loopback {
        "loopback_only".to_string()
    } else {
        "lan".to_string()
    };

    Ok(axum::Json(PairOfferResponse {
        token,
        offer,
        expires_in_seconds: PAIRING_TOKEN_TTL.as_secs(),
        reachability,
    }))
}

/// Pick the address to put in a pair offer. If the bind address is loopback,
/// fall back to whatever interface the OS would use to reach an external host
/// (the standard "open a UDP socket and ask" trick — no packets sent).
fn advertised_addr(bind: SocketAddr) -> SocketAddr {
    if !bind.ip().is_loopback() {
        return bind;
    }
    if let Some(ip) = local_lan_ip() {
        return SocketAddr::new(ip, bind.port());
    }
    bind
}

fn local_lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // Doesn't actually send packets — just resolves the routing table.
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct PairAcceptRequest {
    /// Token from the offer URI.
    token: String,
    /// Acceptor's own identity, name, and reachable address.
    peer_name: String,
    peer_pubkey: String,
    peer_addr: SocketAddr,
}

#[derive(Serialize)]
struct PairAcceptResponse {
    ok: bool,
    /// The offerer's pubkey + name, so the acceptor can save them locally
    /// as a trusted peer in turn.
    name: String,
    pubkey: String,
    addr: String,
}

async fn pair_accept_handler(
    State(state): State<NodeState>,
    Json(req): Json<PairAcceptRequest>,
) -> Result<axum::Json<PairAcceptResponse>, StatusCode> {
    // Consume the token. One-time use.
    {
        let mut tokens = state
            .pairing_tokens
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let now = std::time::Instant::now();
        tokens.retain(|_, t| now.duration_since(*t) < PAIRING_TOKEN_TTL);
        if tokens.remove(&req.token).is_none() {
            tracing::warn!(token_prefix = %&req.token.chars().take(4).collect::<String>(), "pair accept: unknown or expired token");
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // Token valid → save the requester as a trusted peer.
    let new_peer = Peer {
        name: req.peer_name.clone(),
        addr: req.peer_addr,
        priority: 5, // trusted peers are preferred over plain LAN peers (priority 10)
        models: vec![],
        pubkey: Some(req.peer_pubkey.clone()),
    };
    {
        let mut reg = state
            .registry
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        reg.add(new_peer).map_err(|e| {
            tracing::error!(error = %e, "pair accept: persisting peer failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        state.router.replace_peers(&reg.peers);
    }
    tracing::info!(
        name = %req.peer_name,
        addr = %req.peer_addr,
        "trusted peer paired"
    );

    Ok(axum::Json(PairAcceptResponse {
        ok: true,
        name: state.node.name.clone(),
        pubkey: state.identity.public_b64(),
        addr: state.node.addr.to_string(),
    }))
}

// ---------------------------------------------------------------- short pair codes

/// Convert the relay's WebSocket URL (ws:// or wss://) to its HTTP base
/// for hitting `/v1/codes` etc. Treats `ws://host` like `http://host`.
fn relay_http_base(relay_url: &str) -> String {
    relay_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

#[derive(Serialize)]
struct ShortOfferResponse {
    code: String,
    expires_in_seconds: u64,
}

/// `POST /v1/pair/short-offer` — generates an offer URI internally, asks the
/// relay to store it under a 4-letter code, returns the code. The other side
/// types the 4 letters into `pair/use-code` and the rest is automatic.
async fn pair_short_offer_handler(
    State(state): State<NodeState>,
) -> Result<axum::Json<ShortOfferResponse>, (StatusCode, String)> {
    let relay_url = state.node.relay_url.clone().ok_or((
        StatusCode::PRECONDITION_FAILED,
        "no relay configured. start daemon with --relay ws://... to enable short codes".into(),
    ))?;

    // Generate a fresh offer URI (reuses the long-form code path).
    let token = {
        use base64::Engine;
        let mut buf = [0u8; 9];
        rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
    };

    let now = std::time::Instant::now();
    {
        let mut tokens = state
            .pairing_tokens
            .lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "lock".into()))?;
        tokens.retain(|_, t| now.duration_since(*t) < PAIRING_TOKEN_TTL);
        tokens.insert(token.clone(), now);
    }

    let advertised = advertised_addr(state.node.addr);
    let pubkey = state.identity.public_b64();
    let offer = format!(
        "unhosted://pair?addr={}&pk={}&relay={}&token={}",
        advertised,
        urlencode(&pubkey),
        urlencode(&relay_url),
        token
    );

    // Hand the offer to the relay, get back a short code.
    let http_base = relay_http_base(&relay_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let resp = client
        .post(format!("{}/v1/codes", http_base.trim_end_matches('/')))
        .json(&serde_json::json!({ "offer": offer }))
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("relay: {e}")))?;

    if !resp.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("relay HTTP {}", resp.status()),
        ));
    }
    let parsed: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let code = parsed
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or((StatusCode::BAD_GATEWAY, "relay reply missing code".into()))?
        .to_string();
    let exp = parsed
        .get("expires_in_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(300);

    Ok(axum::Json(ShortOfferResponse {
        code,
        expires_in_seconds: exp,
    }))
}

#[derive(Deserialize)]
struct UseCodeRequest {
    code: String,
}

/// `POST /v1/pair/use-code` — accepts a 4-letter code, fetches the offer from
/// the relay, completes the pairing via the existing connect flow.
async fn pair_use_code_handler(
    State(state): State<NodeState>,
    Json(req): Json<UseCodeRequest>,
) -> Result<axum::Json<PairConnectResponse>, (StatusCode, String)> {
    let relay_url = state.node.relay_url.clone().ok_or((
        StatusCode::PRECONDITION_FAILED,
        "no relay configured. start daemon with --relay ws://... to use short codes".into(),
    ))?;

    let code = req.code.trim().to_ascii_uppercase();
    if code.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "code is empty".into()));
    }

    let http_base = relay_http_base(&relay_url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let resp = client
        .get(format!(
            "{}/v1/codes/{}",
            http_base.trim_end_matches('/'),
            code
        ))
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("relay: {e}")))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err((
            StatusCode::NOT_FOUND,
            "code not found — already used, expired, or mistyped".into(),
        ));
    }
    if !resp.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("relay HTTP {}", resp.status()),
        ));
    }

    let parsed: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    let offer = parsed
        .get("offer")
        .and_then(|v| v.as_str())
        .ok_or((StatusCode::BAD_GATEWAY, "relay reply missing offer".into()))?
        .to_string();

    // Delegate to pair_connect logic.
    pair_connect_handler(State(state), Json(PairConnectRequest { offer })).await
}

// Server-side equivalent of `unhosted pair accept`, callable from the UI.
// Parses an offer URI, contacts the offerer, and registers both sides
// locally + remotely. Reuses the existing HTTP-based handshake.
#[derive(Deserialize)]
struct PairConnectRequest {
    offer: String,
}

#[derive(Serialize)]
struct PairConnectResponse {
    ok: bool,
    name: String,
    pubkey: String,
    addr: String,
}

async fn pair_connect_handler(
    State(state): State<NodeState>,
    Json(req): Json<PairConnectRequest>,
) -> Result<axum::Json<PairConnectResponse>, (StatusCode, String)> {
    let parsed = parse_offer_uri(&req.offer)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad offer: {e}")))?;

    let body = serde_json::json!({
        "token": parsed.token,
        "peer_name": state.node.name,
        "peer_pubkey": state.identity.public_b64(),
        "peer_addr": state.node.addr.to_string(),
    });

    // Try direct HTTP first. Short timeout — if the offerer's addr is
    // unreachable (both behind NAT), we want to fail fast and fall back to
    // relay rather than wait 30s.
    let accept_url = format!("http://{}/v1/pair/accept", parsed.addr);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let direct_attempt = client.post(&accept_url).json(&body).send().await;

    let confirmation: serde_json::Value = match direct_attempt {
        Ok(resp) if resp.status().is_success() => resp
            .json()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("bad offerer reply: {e}")))?,
        Ok(resp) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("offerer rejected: HTTP {}", resp.status()),
            ));
        }
        Err(direct_err) => {
            // Direct didn't work. If the offer carries a relay URL AND that
            // relay matches the one we're registered with AND the peer
            // pubkey is in the offer, try the pair-accept-via-relay path.
            let relay_url = parsed.relay.as_deref();
            let our_relay = state.node.relay_url.as_deref();
            let same_relay = match (relay_url, our_relay) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let peer_pk = parsed.pubkey.as_deref();
            let relay_ready = matches!(state.relay.current_state().await, RelayState::Registered);

            if let (Some(pk), true, true) = (peer_pk, same_relay, relay_ready) {
                tracing::info!(peer_pubkey = %pk, "pair direct failed; trying via relay");
                pair_accept_via_relay(&state.relay, pk, &body)
                    .await
                    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("relay pair: {e}")))?
            } else {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("offerer unreachable: {direct_err}"),
                ));
            }
        }
    };

    let their_name = confirmation
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)")
        .to_string();
    let their_pubkey = confirmation
        .get("pubkey")
        .and_then(|v| v.as_str())
        .ok_or((
            StatusCode::BAD_GATEWAY,
            "offerer omitted pubkey".to_string(),
        ))?
        .to_string();
    let their_addr_str = confirmation
        .get("addr")
        .and_then(|v| v.as_str())
        .ok_or((StatusCode::BAD_GATEWAY, "offerer omitted addr".to_string()))?
        .to_string();
    let their_addr: SocketAddr = their_addr_str
        .parse()
        .map_err(|e: std::net::AddrParseError| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Register locally with the pubkey set, so we treat this peer as trusted.
    {
        let mut reg = state
            .registry
            .lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "lock".into()))?;
        reg.add(Peer {
            name: their_name.clone(),
            addr: their_addr,
            priority: 5,
            models: vec![],
            pubkey: Some(their_pubkey.clone()),
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        state.router.replace_peers(&reg.peers);
    }

    tracing::info!(name = %their_name, addr = %their_addr, "trusted peer paired via UI");

    Ok(axum::Json(PairConnectResponse {
        ok: true,
        name: their_name,
        pubkey: their_pubkey,
        addr: their_addr_str,
    }))
}

struct ParsedOfferUri {
    addr: SocketAddr,
    token: String,
    #[allow(dead_code)] // wired up when pair-over-relay lands
    pubkey: Option<String>,
    #[allow(dead_code)] // wired up when pair-over-relay lands
    relay: Option<String>,
}

fn parse_offer_uri(s: &str) -> Result<ParsedOfferUri> {
    let s = s.trim();
    let rest = s
        .strip_prefix("unhosted://pair?")
        .or_else(|| s.strip_prefix("unhosted://pair/"))
        .context("offer must start with 'unhosted://pair?'")?;

    let mut addr: Option<String> = None;
    let mut token: Option<String> = None;
    let mut pubkey: Option<String> = None;
    let mut relay: Option<String> = None;
    for kv in rest.split('&') {
        let mut it = kv.splitn(2, '=');
        let key = it.next().unwrap_or("");
        let raw = it.next().unwrap_or("");
        let val = urldecode(raw);
        match key {
            "addr" => addr = Some(val),
            "token" => token = Some(val),
            "pk" => pubkey = Some(val),
            "relay" => relay = Some(val),
            _ => {}
        }
    }
    Ok(ParsedOfferUri {
        addr: addr
            .context("offer missing addr=")?
            .parse()
            .context("addr not valid host:port")?,
        token: token.context("offer missing token=")?,
        pubkey,
        relay,
    })
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = char_to_hex(bytes[i + 1]);
            let lo = char_to_hex(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn char_to_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------- OpenAI compatibility

/// `POST /v1/chat/completions` — passes the request body through to the
/// upstream llama-server's identical endpoint and streams the response back
/// verbatim. The OpenAI request/response shape is preserved end-to-end, so
/// any tool that speaks OpenAI's API can use Unhosted as its backend
/// (Delta, LangChain, LlamaIndex, OpenWebUI, …).
///
/// Routing: same multi-node policy as `/v1/run`. If the router picks a
/// peer, the request is proxied to that peer's `/v1/chat/completions`.
/// Loop prevention via `X-Unhosted-Forwarded` works the same way.
async fn chat_completions_handler(
    State(state): State<NodeState>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let already_forwarded = headers.get(FORWARDED_HEADER).is_some();
    let target = if already_forwarded {
        Target::Local
    } else {
        state.router.next()
    };

    match target {
        Target::Local => proxy_chat_local(&state.node, body).await,
        Target::Peer { ref name, addr } => match proxy_chat_peer(name, addr, &body).await {
            Ok(r) => Ok(r),
            Err(e) => {
                tracing::warn!(peer = %name, error = %e, "chat: peer unreachable, falling back to local");
                proxy_chat_local(&state.node, body).await
            }
        },
    }
}

async fn proxy_chat_local(node: &Node, body: bytes::Bytes) -> Result<Response, StatusCode> {
    let url = format!("{}/v1/chat/completions", node.llama_server_url);
    let client = reqwest::Client::new();
    let upstream = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "chat: upstream call failed");
            StatusCode::BAD_GATEWAY
        })?;

    let status = upstream.status();
    if !status.is_success() {
        tracing::error!(%status, "chat: upstream non-success");
        return Err(StatusCode::BAD_GATEWAY);
    }
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let stream = upstream.bytes_stream().map(|c| match c {
        Ok(b) => Ok::<_, std::io::Error>(b),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    });
    Ok(Response::builder()
        .header("content-type", content_type)
        .header("x-unhosted-served-by", "local")
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

async fn proxy_chat_peer(name: &str, addr: SocketAddr, body: &bytes::Bytes) -> Result<Response> {
    let url = format!("http://{addr}/v1/chat/completions");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()?;
    let upstream = client
        .post(&url)
        .header("content-type", "application/json")
        .header(FORWARDED_HEADER, "1")
        .body(body.clone())
        .send()
        .await?;
    if !upstream.status().is_success() {
        anyhow::bail!("peer {} returned {}", name, upstream.status());
    }
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let stream = upstream.bytes_stream().map(|c| match c {
        Ok(b) => Ok::<_, std::io::Error>(b),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    });
    Ok(Response::builder()
        .header("content-type", content_type)
        .header("x-unhosted-served-by", format!("peer:{name}"))
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

/// `GET /v1/models` — proxies the upstream's identical endpoint so OpenAI
/// clients can auto-discover what model is being served.
async fn models_handler(State(state): State<NodeState>) -> Result<Response, StatusCode> {
    let url = format!("{}/v1/models", state.node.llama_server_url);
    let upstream = reqwest::Client::new().get(&url).send().await.map_err(|e| {
        tracing::error!(error = %e, "models: upstream call failed");
        StatusCode::BAD_GATEWAY
    })?;
    if !upstream.status().is_success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body_bytes = upstream
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Response::builder()
        .header("content-type", content_type)
        .body(Body::from(body_bytes))
        .expect("valid response"))
}

async fn identity_handler(State(state): State<NodeState>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "name": state.node.name,
        "pubkey": state.identity.public_b64(),
        "addr": state.node.addr.to_string(),
    }))
}

/// Route an inbound relay request by its `kind` to the right local handler.
async fn dispatch_inbound_relay_request(state: NodeState, req: InboundRequest) {
    match req.kind.as_str() {
        "pair_accept" => dispatch_inbound_pair_accept(state, req).await,
        _ /* "run" or unspecified */ => dispatch_inbound_run(state, req).await,
    }
}

/// Run an inference request that arrived over the relay against the local
/// llama-server, streaming chunks back through the response channel.
async fn dispatch_inbound_run(state: NodeState, req: InboundRequest) {
    let run_req: RunRequest = match serde_json::from_value(req.body.clone()) {
        Ok(r) => r,
        Err(e) => {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error(format!("bad request body: {e}")));
            return;
        }
    };

    tracing::info!(from = %req.from_pubkey, "relay-inbound /v1/run dispatch");

    match run_local(&state.node, run_req).await {
        Err(code) => {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error(format!("local upstream: {code}")));
        }
        Ok(resp) => {
            let mut body = resp.into_body().into_data_stream();
            while let Some(chunk) = body.next().await {
                match chunk {
                    Ok(b) => {
                        if req.response_tx.send(ResponseEvent::Chunk(b)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = req
                            .response_tx
                            .send(ResponseEvent::Error(format!("local stream: {e}")));
                        return;
                    }
                }
            }
            let _ = req.response_tx.send(ResponseEvent::End);
        }
    }
}

/// Handle a pair-accept request that arrived over the relay (used when the
/// other peer can't reach us directly). Performs the same logic as the
/// HTTP /v1/pair/accept handler and emits the response as a single chunk
/// + End on the relay's response channel.
async fn dispatch_inbound_pair_accept(state: NodeState, req: InboundRequest) {
    let accept: PairAcceptRequest = match serde_json::from_value(req.body.clone()) {
        Ok(r) => r,
        Err(e) => {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error(format!("bad pair_accept body: {e}")));
            return;
        }
    };

    tracing::info!(
        from = %req.from_pubkey,
        peer = %accept.peer_name,
        "relay-inbound /v1/pair/accept"
    );

    // Same flow as pair_accept_handler — consume the one-time token, then
    // save the requester as a trusted peer.
    {
        let mut tokens = match state.pairing_tokens.lock() {
            Ok(t) => t,
            Err(_) => {
                let _ = req
                    .response_tx
                    .send(ResponseEvent::Error("token lock poisoned".into()));
                return;
            }
        };
        let now = std::time::Instant::now();
        tokens.retain(|_, t| now.duration_since(*t) < PAIRING_TOKEN_TTL);
        if tokens.remove(&accept.token).is_none() {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error("token expired or unknown".into()));
            return;
        }
    }

    let new_peer = Peer {
        name: accept.peer_name.clone(),
        addr: accept.peer_addr,
        priority: 5,
        models: vec![],
        pubkey: Some(accept.peer_pubkey.clone()),
    };
    {
        let mut reg = match state.registry.lock() {
            Ok(r) => r,
            Err(_) => {
                let _ = req
                    .response_tx
                    .send(ResponseEvent::Error("registry lock poisoned".into()));
                return;
            }
        };
        if let Err(e) = reg.add(new_peer) {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error(format!("persisting peer: {e}")));
            return;
        }
        state.router.replace_peers(&reg.peers);
    }

    let reply = serde_json::json!({
        "ok": true,
        "name": state.node.name,
        "pubkey": state.identity.public_b64(),
        "addr": state.node.addr.to_string(),
    });
    let reply_str = match serde_json::to_string(&reply) {
        Ok(s) => s,
        Err(e) => {
            let _ = req
                .response_tx
                .send(ResponseEvent::Error(format!("serializing reply: {e}")));
            return;
        }
    };
    let _ = req
        .response_tx
        .send(ResponseEvent::Chunk(bytes::Bytes::from(reply_str)));
    let _ = req.response_tx.send(ResponseEvent::End);
}

async fn unpair_handler(
    State(state): State<NodeState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<PairResponse>, StatusCode> {
    let removed = {
        let mut reg = state
            .registry
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let removed = reg
            .remove(&name)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if removed {
            state.router.replace_peers(&reg.peers);
            tracing::info!(%name, "peer unpaired");
        }
        removed
    };

    if !removed {
        return Err(StatusCode::NOT_FOUND);
    }

    let peers = state
        .registry
        .lock()
        .map(|r| {
            r.peers
                .iter()
                .map(|p| PeerStatus {
                    name: p.name.clone(),
                    addr: p.addr.to_string(),
                    priority: p.priority,
                    trusted: p.pubkey.is_some(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(axum::Json(PairResponse { ok: true, peers }))
}

/// Best-effort probe of llama-server: reachable check + currently loaded
/// model name from its OpenAI-compatible `/v1/models` endpoint. Times out
/// fast so the status request stays snappy when the upstream is down.
async fn probe_upstream(url: &str) -> (bool, Option<String>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (false, None),
    };

    let resp = match client.get(format!("{url}/v1/models")).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return (false, None),
    };

    let model = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
        v.get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|m| m.get("id"))
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
    });

    (true, model)
}

async fn run_handler(
    State(state): State<NodeState>,
    headers: HeaderMap,
    Json(req): Json<RunRequest>,
) -> Result<Response, StatusCode> {
    // Auth: if the caller presents an X-Unhosted-Auth header, verify it
    // against the trusted-peer registry. Header present but invalid →
    // 401. Header absent → accept (preserves LAN/local CLI behavior).
    if let Some(auth_hv) = headers.get(AUTH_HEADER) {
        let auth_str = match auth_hv.to_str() {
            Ok(s) => s,
            Err(_) => return Err(StatusCode::UNAUTHORIZED),
        };
        // Sign over the same body the sender signed: ts\n + JSON(req).
        let body_bytes = match serde_json::to_vec(&req) {
            Ok(b) => b,
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
        let sender_pk = match Identity::verify_request(auth_str, &body_bytes) {
            Some(pk) => pk,
            None => {
                tracing::warn!("auth: signature failed or expired");
                return Err(StatusCode::UNAUTHORIZED);
            }
        };
        // Confirm the sender is actually a trusted peer of ours.
        let is_trusted = state
            .registry
            .lock()
            .map(|r| {
                r.peers
                    .iter()
                    .any(|p| p.pubkey.as_deref() == Some(sender_pk.as_str()))
            })
            .unwrap_or(false);
        if !is_trusted {
            tracing::warn!(sender = %sender_pk, "auth: pubkey signed correctly but is not paired");
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // Loop prevention: if a peer already forwarded this request to us, we
    // serve it locally and don't bounce it back into the router.
    let already_forwarded = headers.get(FORWARDED_HEADER).is_some();

    let target = if already_forwarded {
        Target::Local
    } else {
        state.router.next()
    };

    match target {
        Target::Local => {
            tracing::debug!(
                target = "local",
                forwarded = already_forwarded,
                "routing request"
            );
            run_local(&state.node, req).await
        }
        Target::Peer { ref name, addr } => {
            tracing::debug!(target = %name, %addr, "routing request to peer");
            // Direct HTTP first. If the peer has a pubkey (trusted), sign
            // the request so they can verify the sender.
            let signing = lookup_peer_pubkey(&state.registry, name).map(|_| state.identity.clone());
            match run_peer(name, addr, &req, signing.as_ref()).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    tracing::warn!(peer = %name, error = %e, "peer direct failed");

                    // Fall back to relay if the peer has a pubkey AND we're
                    // currently registered with a relay. Otherwise local.
                    let pubkey = lookup_peer_pubkey(&state.registry, name);
                    let relay_ready =
                        matches!(state.relay.current_state().await, RelayState::Registered);

                    match (pubkey, relay_ready) {
                        (Some(pk), true) => {
                            tracing::info!(peer = %name, "trying via relay");
                            match run_peer_via_relay(&state.relay, &pk, name, &req).await {
                                Ok(resp) => Ok(resp),
                                Err(e) => {
                                    tracing::warn!(peer = %name, error = %e, "relay path failed; local");
                                    run_local(&state.node, req).await
                                }
                            }
                        }
                        _ => run_local(&state.node, req).await,
                    }
                }
            }
        }
    }
}

/// Find a registered peer's pubkey by name. None if the peer isn't in the
/// registry, or if it is but doesn't carry a pubkey (LAN-only / untrusted).
fn lookup_peer_pubkey(
    registry: &Arc<std::sync::Mutex<PeerRegistry>>,
    name: &str,
) -> Option<String> {
    let reg = registry.lock().ok()?;
    reg.peers
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.pubkey.clone())
}

/// Send a pair-accept payload to a trusted peer over the relay. Used by
/// `pair_connect_handler` as a fallback when the offerer's addr is
/// unreachable (e.g. both peers behind NAT). The peer's relay client
/// dispatches it through `dispatch_inbound_pair_accept` and returns the
/// same JSON shape the HTTP /v1/pair/accept handler does.
async fn pair_accept_via_relay(
    client: &RelayClient,
    peer_pubkey: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let mut rx = client
        .call_with_kind(peer_pubkey, "pair_accept", body.clone())
        .await?;

    let mut buf = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            ResponseEvent::Chunk(b) => buf.extend_from_slice(&b),
            ResponseEvent::End => break,
            ResponseEvent::Error(msg) => anyhow::bail!("peer rejected: {msg}"),
        }
    }
    if buf.is_empty() {
        anyhow::bail!("peer closed stream with no response");
    }
    let parsed: serde_json::Value =
        serde_json::from_slice(&buf).context("parsing peer reply as JSON")?;
    Ok(parsed)
}

/// Send a `/v1/run`-style request to a trusted peer over the relay rather
/// than direct HTTP. Used as a fallback when the direct HTTP call fails —
/// e.g. the peer is behind NAT.
async fn run_peer_via_relay(
    client: &RelayClient,
    peer_pubkey: &str,
    peer_name: &str,
    req: &RunRequest,
) -> Result<Response> {
    let body = serde_json::to_value(req).context("serializing run request")?;
    let mut rx = client.call(peer_pubkey, body).await?;

    let (tx, body_rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                ResponseEvent::Chunk(b) => {
                    if tx.send(Ok(b)).await.is_err() {
                        break;
                    }
                }
                ResponseEvent::End => break,
                ResponseEvent::Error(msg) => {
                    let _ = tx.send(Err(std::io::Error::other(msg))).await;
                    break;
                }
            }
        }
    });

    let stream = ReceiverStream::new(body_rx);
    Ok(Response::builder()
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-unhosted-served-by", format!("peer:{peer_name}:relay"))
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

/// Forward a request to a peer's `/v1/run`, streaming the response body back
/// to our caller unchanged. The peer is another `unhosted` daemon, so the
/// response is already text/plain token stream.
///
/// If `signing` is `Some`, attach an `X-Unhosted-Auth` header signed with
/// our identity — the receiving peer can verify we are who we claim to be.
async fn run_peer(
    name: &str,
    addr: SocketAddr,
    req: &RunRequest,
    signing: Option<&Identity>,
) -> Result<Response> {
    let url = format!("http://{addr}/v1/run");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(anyhow::Error::from)?;

    let body_bytes = serde_json::to_vec(req).context("serializing run request")?;

    let mut builder = client
        .post(&url)
        .header(FORWARDED_HEADER, "1")
        .header("content-type", "application/json");
    if let Some(id) = signing {
        builder = builder.header(AUTH_HEADER, id.sign_request(&body_bytes));
    }

    let upstream_resp = builder
        .body(body_bytes)
        .send()
        .await
        .map_err(anyhow::Error::from)?;

    if !upstream_resp.status().is_success() {
        anyhow::bail!("peer {} returned {}", name, upstream_resp.status());
    }

    let stream = upstream_resp.bytes_stream().map(|chunk| match chunk {
        Ok(b) => Ok::<_, std::io::Error>(b),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    });

    Ok(Response::builder()
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-unhosted-served-by", format!("peer:{name}"))
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

/// Serve a request locally by proxying to this node's upstream llama-server,
/// parsing its SSE stream, and emitting plain-text tokens.
async fn run_local(node: &Node, req: RunRequest) -> Result<Response, StatusCode> {
    let upstream_url = format!("{}/v1/chat/completions", node.llama_server_url);
    let client = reqwest::Client::new();

    let upstream_resp = client
        .post(&upstream_url)
        .json(&ChatRequest {
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: DEFAULT_SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: &req.prompt,
                },
            ],
            max_tokens: req.max_tokens,
            stream: true,
        })
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "upstream request failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !upstream_resp.status().is_success() {
        tracing::error!(status = %upstream_resp.status(), "upstream returned non-success");
        return Err(StatusCode::BAD_GATEWAY);
    }

    let (tx, rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);

    tokio::spawn(async move {
        let mut byte_stream = upstream_resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "upstream stream error");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(boundary) = buffer.find("\n\n") {
                let event: String = buffer.drain(..boundary + 2).collect();
                if !forward_event(&event, &tx).await {
                    return;
                }
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Response::builder()
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-unhosted-served-by", "local")
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

async fn forward_event(
    event: &str,
    tx: &mpsc::Sender<Result<bytes::Bytes, std::io::Error>>,
) -> bool {
    for line in event.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload == "[DONE]" {
            return false;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        // OpenAI-compatible /v1/chat/completions stream shape:
        //   { "choices": [{ "delta": { "content": "..." }, "finish_reason": null }] }
        let choice = json
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first());

        if let Some(content) = choice
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(|v| v.as_str())
        {
            if !content.is_empty()
                && tx
                    .send(Ok(bytes::Bytes::copy_from_slice(content.as_bytes())))
                    .await
                    .is_err()
            {
                return false;
            }
        }
        if choice
            .and_then(|c| c.get("finish_reason"))
            .map(|v| !v.is_null())
            == Some(true)
        {
            return false;
        }
    }
    true
}
