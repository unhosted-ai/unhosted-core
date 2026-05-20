//! Core engine for Unhosted.
//!
//! - v0.0.1: single-machine inference — proxies to a local llama-server.
//! - v0.0.2 (in progress): multi-node — the daemon round-robins requests
//!   across `Local` + configured peers, with loop prevention and per-request
//!   fallback to local on peer failure.
//!
//! Peer protocol is the same HTTP API the CLI uses (`POST /v1/run`), so a
//! peer is just another `unhosted serve` process. No new transport.

pub mod auth;
pub mod chats;
pub mod discovery;
pub mod identity;
pub mod memory;
pub mod paths;
pub mod peer;
pub mod public_mode;
pub mod relay_client;
pub mod router;
pub mod transport;
pub mod tunnel;
pub mod upstream;
pub mod vram_pool;
mod web;
pub mod web_fetch;

pub use auth::{AuthOutcome, LocalToken, ReplayGuard};
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
    /// Eagerly start the Cloudflare tunnel at daemon boot, so the public
    /// URL is already live by the time the user clicks "open to internet".
    /// Off by default — exposing the daemon publicly should require explicit
    /// opt-in even when starting the daemon.
    pub eager_tunnel: bool,
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
            // Eager-tunnel intent comes from two independent sources:
            //   1. UNHOSTED_EAGER_TUNNEL env var — for operators running
            //      `unhosted serve` from systemd / a config file, who
            //      want the tunnel up unconditionally.
            //   2. The persisted user-click state at
            //      `~/.config/unhosted/tunnel-autostart.txt`, written by
            //      `TunnelManager::start/stop`. Lets the desktop user
            //      enable the tunnel once and have it come back on every
            //      .app cold-start without re-clicking.
            // Either source opting in is enough. Persisted "disabled"
            // never overrides an operator's env var, which matches the
            // env var being "operator policy" and the file being "user
            // preference".
            eager_tunnel: std::env::var("UNHOSTED_EAGER_TUNNEL")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false)
                || crate::tunnel::load_autostart(),
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
    /// Local bearer token. Required by sensitive endpoints when the
    /// caller isn't on loopback and isn't a signed peer.
    local_token: LocalToken,
    /// Replay-protection store for signed peer requests. Keeps a
    /// (pubkey, ts, sig_prefix) set with TTL == verify-window.
    replay_guard: Arc<std::sync::Mutex<ReplayGuard>>,
    /// Shared QUIC endpoint for outbound peer dials. `None` if the
    /// daemon couldn't bind a UDP port for QUIC at startup.
    quic: Option<Arc<transport::PeerEndpoint>>,
    /// Server-side chat history. Loaded at startup from
    /// `~/.config/unhosted/chats.json`. Lets any device paired to this
    /// daemon see the same conversation list — replaces the per-browser
    /// `localStorage` store that diverged across origins.
    chats: chats::ChatStore,
    /// Cloudflare Tunnel control. Spawns `cloudflared` as a subprocess
    /// when the user clicks "open to internet" in the UI; lets the
    /// phone PWA reach this daemon from any network.
    tunnel: Arc<tunnel::TunnelManager>,
    /// Shared HTTP client for upstream (llama-server / Ollama / LM Studio)
    /// proxy calls. One client = one connection pool = HTTP keep-alive
    /// across chat requests instead of TCP handshake per turn. Reqwest
    /// itself is internally an Arc, so cloning it is cheap.
    http: reqwest::Client,
    /// Per-chat in-flight summarizer task handles, keyed by `chat_id`.
    /// Used to debounce auto-summarization: every `chats_upsert_handler`
    /// call cancels the previous task for the same chat (if any) and
    /// spawns a new one that sleeps before summarizing. The net effect
    /// is "summarize the chat ~30 s after the user stops typing", so
    /// long conversations don't trigger N upstream-LLM round trips,
    /// just one. Memory feature is the only consumer today.
    summarize_inflight:
        Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// VRAM-pool orchestration manager. Owns the `rpc-server` +
    /// `llama-server --rpc=…` child processes when a pool is active.
    /// `None` would be possible if we wanted to gate the feature on
    /// startup capability, but we instead keep the manager always
    /// present and let `start()` fail with a clear error when the
    /// machine isn't RPC-capable.
    vram_pool: Arc<vram_pool::PoolManager>,
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
    /// OpenAI-compatible servers vary on whether this field is required.
    /// llama-server ignores it (serves whatever's loaded); Ollama and
    /// LM Studio return 400 without it. We populate it from
    /// `upstream::select_live` when a model id is discoverable.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
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

    let local_token = LocalToken::load_or_create().context("loading local API token")?;
    let replay_guard = Arc::new(std::sync::Mutex::new(ReplayGuard::new()));

    // QUIC endpoint on the next port up from HTTP. Best-effort: if the
    // port is taken we log and continue with HTTP-only routing. The
    // accept loop is wired AFTER state is built so it can dispatch
    // inbound run requests against the daemon's inference path.
    let quic_bind = SocketAddr::new(node.addr.ip(), node.addr.port().saturating_add(1));
    let registry_for_quic = registry.clone();
    let (quic, quic_endpoint_for_accept) = match transport::PeerEndpoint::bind(
        quic_bind,
        &identity,
        registry_for_quic,
    ) {
        Ok(ep) => {
            tracing::info!(addr = %quic_bind, "quic peer endpoint listening");
            let handle = ep.handle();
            (Some(Arc::new(ep)), Some(handle))
        }
        Err(e) => {
            tracing::warn!(error = %e, addr = %quic_bind, "quic: failed to bind — peer encryption disabled");
            (None, None)
        }
    };

    let chat_store = chats::ChatStore::load_or_create().context("loading chat store")?;
    let tunnel_mgr = Arc::new(tunnel::TunnelManager::new(node.addr.port()));

    // Shared HTTP client. No total-request timeout (chat streams can run
    // for minutes), but a generous tcp_keepalive so idle connections in
    // the pool stay alive between user turns.
    let http = reqwest::Client::builder()
        .tcp_keepalive(Some(std::time::Duration::from_secs(60)))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let state = NodeState {
        node: Arc::new(node.clone()),
        router: router.clone(),
        registry,
        discovery,
        identity: identity.clone(),
        pairing_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        relay,
        local_token: local_token.clone(),
        replay_guard,
        quic,
        chats: chat_store,
        tunnel: tunnel_mgr,
        http,
        summarize_inflight: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        vram_pool: Arc::new(vram_pool::PoolManager::new(vram_pool::probe())),
    };

    // Eager tunnel: if the operator opted in (--eager-tunnel /
    // UNHOSTED_EAGER_TUNNEL=1), kick off cloudflared right away in the
    // background. By the time the user clicks "open to internet" in
    // the UI, the public URL is already live — 0s perceived latency.
    // Failures (no internet, missing cloudflared binary) surface in the
    // tunnel status the same way a manual click would.
    if node.eager_tunnel {
        let tunnel = state.tunnel.clone();
        let upstream_url = node.llama_server_url.clone();
        tokio::spawn(async move {
            // Don't burn a public URL on a daemon that can't serve chat.
            // Wait for an LLM backend (configured upstream, Ollama, LM
            // Studio, or llama-server) to be reachable before spawning
            // cloudflared. The watchdog below keeps polling for it.
            if upstream::select_live(&upstream_url).await.is_none() {
                tracing::info!(
                    "eager tunnel: skipping at boot — no LLM backend reachable yet (watchdog will retry once one comes up)"
                );
                return;
            }
            tracing::info!("eager tunnel: starting cloudflared at boot");
            match tunnel.clone().start().await {
                Ok(s) => tracing::info!(state = ?s, "eager tunnel: kicked off"),
                Err(e) => tracing::warn!(error = %e, "eager tunnel: start failed"),
            }
        });
        // Stickiness: a long-running watchdog revives the tunnel from
        // Idle/Failed (e.g. supervisor budget exhausted, accidental stop)
        // unless the user explicitly clicked off. Survives any bug that
        // strands the state machine in a "should be running but isn't"
        // state — which is exactly what users were hitting before.
        // Also picks up the case where the LLM came up *after* the
        // eager-boot skip above.
        let watchdog_upstream = node.llama_server_url.clone();
        let watchdog_tunnel = state.tunnel.clone();
        watchdog_tunnel.spawn_eager_watchdog_gated(watchdog_upstream);
    }

    // Spawn the QUIC accept loop now that NodeState is ready. Each
    // incoming connection gets dispatched to `quic_inbound_handler`,
    // which routes by request kind (run / ping / future).
    if let Some(endpoint) = quic_endpoint_for_accept {
        let state_for_quic = state.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let state = state_for_quic.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => quic_inbound_handler(conn, state).await,
                        Err(e) => tracing::debug!(error = %e, "quic: peer handshake refused"),
                    }
                });
            }
        });
    }

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
        .route("/v1/punch", post(punch_handler))
        .route("/v1/quic/ping", post(quic_ping_handler))
        .route("/v1/identity", get(identity_handler))
        .route("/v1/auth/token", get(auth_token_handler))
        // OpenAI-compatible endpoints — any client that speaks OpenAI's HTTP
        // API (Delta, LangChain, LlamaIndex, OpenWebUI, …) can point at
        // http://127.0.0.1:7777 instead of OpenAI / Ollama / llama-server.
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/models", get(models_handler))
        // Server-side chat history — same store regardless of which paired
        // device opens the UI.
        .route(
            "/v1/chats",
            get(chats_list_handler)
                .post(chats_upsert_handler)
                .delete(chats_clear_handler),
        )
        .route(
            "/v1/chats/{id}",
            get(chats_get_handler)
                .put(chats_upsert_handler)
                .delete(chats_delete_handler),
        )
        // Cloudflare Tunnel control — one-click "make this daemon reachable
        // from the public internet" for phones on cellular / coffee-shop wifi.
        .route("/v1/tunnel", get(tunnel_status_handler))
        .route("/v1/tunnel/start", post(tunnel_start_handler))
        .route("/v1/tunnel/stop", post(tunnel_stop_handler))
        // Private memory — RAG over the user's own past chats. Toggleable
        // from the sidebar; default off. See `memory.rs`.
        .route(
            "/v1/memory",
            get(memory_list_handler).post(memory_add_handler),
        )
        .route(
            "/v1/memory/{id}",
            axum::routing::delete(memory_delete_handler),
        )
        .route("/v1/memory/clear", post(memory_clear_handler))
        .route("/v1/memory/enable", post(memory_enable_handler))
        // Web fetch tool — exposed so external agents (and eventually
        // the LLM itself via a tool-use loop) can pull web pages
        // through the daemon. SSRF-guarded; only HTTPS to public IPs.
        .route("/v1/tools/web_fetch", post(web_fetch_handler))
        // VRAM-pool orchestration (ADR 0009 phase 2b). Spawns
        // rpc-server + llama-server --rpc=… subprocesses to make
        // this machine the orchestrator of a layer-split inference
        // cluster. Local-user-only — spawning subprocesses on the
        // user's box is consequential.
        .route("/v1/vram-pool", get(vram_pool_status_handler))
        .route("/v1/vram-pool/start", post(vram_pool_start_handler))
        .route("/v1/vram-pool/stop", post(vram_pool_stop_handler))
        // Layer-host endpoints (ADR 0009 phase 2c). Called by a
        // remote paired orchestrator to ask this machine to spawn
        // an rpc-server as a layer host. Auth: signed peer request
        // (not loopback or bearer-token like the orchestrator-side
        // endpoints — the caller is by definition a peer).
        .route(
            "/v1/vram-pool/layer-host/start",
            post(vram_pool_layer_host_start_handler),
        )
        .route(
            "/v1/vram-pool/layer-host/stop",
            post(vram_pool_layer_host_stop_handler),
        )
        // Public-mode policy (ADR-0010 slice 2). GET reads the
        // currently advertised PeerPaymentPolicy; PUT replaces it.
        // Both are local-user-only — only the daemon owner decides
        // which rails / KYC tiers / countries the node accepts.
        // Quoting (slice 3) will land alongside the rail adapters.
        .route(
            "/v1/public-mode/policy",
            get(public_mode_policy_get_handler).put(public_mode_policy_put_handler),
        )
        // Inspect: given a hypothetical PayerContext, return the
        // policy decision without quoting. Lets the UI (and tests)
        // ask "would this peer accept a US payer paying via
        // lightning?" before any real quote/job exists. Local-only.
        .route(
            "/v1/public-mode/policy/inspect",
            post(public_mode_policy_inspect_handler),
        )
        // Sign a UsageReport with this daemon's Ed25519 identity.
        // Local-only. The caller supplies everything except the
        // host_pubkey; we fill it from the daemon's identity so the
        // claimed signer can't disagree with the actual signer.
        .route(
            "/v1/public-mode/receipt/sign",
            post(public_mode_receipt_sign_handler),
        )
        // Quote: a stranger asks "what would this cost?" with a
        // signed body proving they hold the payer pubkey they claim.
        // NOT loopback-gated — payers are by definition external.
        // The ed25519 sig over the canonical body *is* the auth.
        .route("/v1/public-mode/quote", post(public_mode_quote_handler))
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

    // Probe the configured upstream + the two other backends we know
    // how to talk to. If nothing answers, the daemon still starts —
    // but we print install hints so the user isn't left wondering why
    // their first prompt 502s.
    print_upstream_banner(&node.llama_server_url).await;

    // Loud advisory when bound to a non-loopback addr: the LAN can reach
    // sensitive endpoints, so the user needs the bearer token to drive
    // the UI from another device. Loopback callers (the desktop shell,
    // the CLI on the same machine) don't need it.
    if !node.addr.ip().is_loopback() {
        print_lan_security_banner(node.addr, local_token.value());
    }

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Probe the configured upstream and the other known backends, then
/// print a single banner summarizing what's reachable. When the
/// configured upstream is dead, we surface whichever alternative
/// backend is actually running (Ollama, LM Studio) and tell the user
/// how to switch. When nothing is running, we print install hints.
async fn print_upstream_banner(configured_url: &str) {
    let configured_ok = upstream::probe_configured(configured_url).await;
    if configured_ok {
        eprintln!();
        eprintln!(" upstream reachable: {configured_url}");
        eprintln!();
        return;
    }

    // Configured upstream is down. Probe the three standard local
    // backends to see if the user has *something* running on a
    // different port — most common cause of "it didn't work" is
    // Ollama on :11434 while we look at :8080.
    let report = upstream::probe_all().await;

    eprintln!();
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!(" upstream check: {configured_url} did not respond");
    eprintln!();
    for r in &report.results {
        let status = if r.reachable { "ok    " } else { "absent" };
        eprintln!("   [{}] {:<13} {}", status, r.backend.name(), r.url);
    }
    eprintln!();

    match report.first_reachable() {
        Some(found) => {
            eprintln!(" a local backend is running on a different port.");
            eprintln!(" to use it, restart with:");
            eprintln!();
            eprintln!("   UNHOSTED_LLAMA_SERVER_URL={} unhosted serve", found.url);
            eprintln!();
            eprintln!(" all three speak openai-compatible /v1, so unhosted will");
            eprintln!(" proxy requests through transparently.");
        }
        None => {
            eprintln!("{}", upstream::install_hints());
        }
    }
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!();
}

fn print_lan_security_banner(bind: SocketAddr, token: &str) {
    eprintln!();
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!(" unhosted is reachable on the LAN at {bind}");
    eprintln!();
    eprintln!(" sensitive endpoints (/v1/run, /v1/peers, /v1/pair/*, …)");
    eprintln!(" require either a paired-peer signature OR this bearer:");
    eprintln!();
    eprintln!("   {token}");
    eprintln!();
    eprintln!(" to reach this node from your phone, open:");
    eprintln!("   http://<this-machine-ip>:{}?t={token}", bind.port());
    eprintln!(" (the UI stashes the token in localStorage after the first load)");
    eprintln!();
    eprintln!(" rotate the token by deleting ~/.config/unhosted/api-token.txt");
    eprintln!(" and restarting the daemon.");
    eprintln!("───────────────────────────────────────────────────────────────");
    eprintln!();
}

async fn health() -> &'static str {
    "ok"
}

impl NodeState {
    /// Classify an incoming request. Handlers decide what to do with the
    /// outcome — read-only endpoints accept `LoopbackUnauthed`; state-
    /// mutating ones require `is_authed()`.
    fn classify(
        &self,
        headers: &HeaderMap,
        peer_addr: Option<std::net::IpAddr>,
        body: &[u8],
    ) -> AuthOutcome {
        auth::classify(
            headers,
            peer_addr,
            body,
            &self.registry,
            &self.local_token,
            &self.replay_guard,
        )
    }
}

/// Convert an auth outcome into either a pass-through (Ok) or an HTTP error.
/// `require_local_user_only` rejects authenticated paired-peer requests too —
/// used for endpoints that should never be reachable to peers (e.g. unpair).
fn require_auth(outcome: &AuthOutcome, require_local_user_only: bool) -> Result<(), StatusCode> {
    match outcome {
        AuthOutcome::Peer(_) if !require_local_user_only => Ok(()),
        AuthOutcome::Peer(_) => Err(StatusCode::FORBIDDEN),
        AuthOutcome::Local | AuthOutcome::LoopbackUnauthed => Ok(()),
        AuthOutcome::Rejected(why) => {
            tracing::warn!(reason = %why, "auth rejected");
            Err(StatusCode::UNAUTHORIZED)
        }
        AuthOutcome::Missing => {
            tracing::warn!("auth missing — LAN access without bearer or signed peer");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

#[derive(Serialize)]
struct AuthTokenResponse {
    token: String,
}

/// `GET /v1/auth/token` — returns the local bearer token. Strictly
/// loopback-only; nothing else gets to read it. The web UI calls this
/// on first load (when it has no cached token) so the embedded shell
/// + browser tabs on the same machine just work.
async fn auth_token_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
) -> Result<axum::Json<AuthTokenResponse>, StatusCode> {
    if !remote.ip().is_loopback() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(axum::Json(AuthTokenResponse {
        token: state.local_token.value().to_string(),
    }))
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
            axum::http::Method::PUT,
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
    /// VRAM-pooling readiness probe (ADR 0009). Tells consumers
    /// whether this machine has an RPC-capable llama.cpp build,
    /// and therefore whether it can participate in the cluster as
    /// orchestrator or layer host. `null` when probing is skipped
    /// (e.g., daemon started in a mode where llama.cpp isn't
    /// relevant); a populated value is cheap to compute.
    #[serde(skip_serializing_if = "Option::is_none")]
    vram_pool: Option<vram_pool::RpcCapability>,
    /// Public-mode posture (ADR-0010 slice 2). A peer can read this
    /// once and know whether to even attempt a quote against this
    /// node, without an extra round-trip to /v1/public-mode/policy.
    /// Always populated — the "closed" policy is a valid answer.
    public_mode: PublicModeStatus,
}

#[derive(Serialize, Clone)]
struct PublicModeStatus {
    /// True iff at least one rail is accepted. The fast "do you
    /// even take customers" signal — UIs and routing logic can
    /// short-circuit on this without parsing the full policy.
    open: bool,
    /// The full advertised policy. Cheap to ship (under 200 bytes)
    /// and lets a peer pre-filter quote attempts locally.
    policy: public_mode::PeerPaymentPolicy,
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
    /// Per-backend probe results so the UI can suggest switching when
    /// the configured upstream is down but another runtime is alive
    /// on its default port (the "you have ollama running on :11434"
    /// case from the v0.0.4 UX work).
    backends: Vec<BackendProbe>,
}

#[derive(Serialize, Clone)]
struct BackendProbe {
    name: &'static str,
    url: &'static str,
    reachable: bool,
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

/// Decide whether a newly-discovered peer address is worth
/// overwriting the one we already have on file.
///
/// Auto-restore is supposed to handle the "peer's IP changed
/// after a router reboot" case. It is **not** supposed to swap
/// a working address for a broken one — which is exactly what
/// happened when mDNS on macOS advertised an IPv6 link-local
/// (`fe80::*`) for a peer that was already reachable at, say,
/// `192.168.1.42:7777`. Link-locals require a zone identifier
/// (`%en0`) which `SocketAddr` doesn't carry, so any connect
/// against `[fe80::*]:port` from outside the discovery path
/// returns "Connection refused".
///
/// Heuristic: only accept the swap if the new address is at
/// least as "good" as the current one by this rank:
///
///   loopback (127.0.0.1, ::1)         best  (already-running test setup)
///   private IPv4 (10.*, 192.168.*)    good  (LAN reachable)
///   global IPv6                       good
///   non-private IPv4                  ok    (public, may NAT)
///   link-local IPv4 (169.254.*)       avoid
///   link-local IPv6 (fe80::*)         avoid (needs zone id)
///
/// Returns true only when `new` is strictly better than or equal
/// to `old`; otherwise we keep `old`.
fn discovered_is_better(old: &SocketAddr, new: &SocketAddr) -> bool {
    fn rank(addr: &SocketAddr) -> u8 {
        use std::net::IpAddr;
        match addr.ip() {
            IpAddr::V4(v4) if v4.is_loopback() => 5,
            IpAddr::V6(v6) if v6.is_loopback() => 5,
            IpAddr::V4(v4) if v4.is_private() => 4,
            IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00 => 4, // unique-local
            IpAddr::V4(v4) if v4.is_link_local() => 1,
            IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => 1, // link-local
            IpAddr::V4(_) => 3,
            IpAddr::V6(_) => 3,
        }
    }
    rank(new) >= rank(old)
}

async fn status_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<StatusResponse>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, false)?;

    // VRAM-pool takes precedence as the upstream when it's running —
    // matches the routing decision in `resolve_upstream` so the UI's
    // "node ready" indicator stays in sync with where chats actually
    // go. When the pool is anything other than Running, fall back to
    // the configured-upstream probe.
    let pool_state = state.vram_pool.status().await;
    let (upstream_url, reachable, model) =
        if let vram_pool::PoolState::Running { endpoint, plan, .. } = &pool_state {
            (endpoint.clone(), true, Some(plan.model.clone()))
        } else {
            let url = state.node.llama_server_url.clone();
            let (r, m) = probe_upstream(&url).await;
            (url, r, m)
        };
    // Probe all three known local backends in parallel so the UI can
    // suggest a switch when the configured upstream is down but, say,
    // ollama is running on :11434. Cheap (~750ms timeout each, in
    // parallel) and runs once per status poll.
    let backend_report = upstream::probe_all().await;
    let backends: Vec<BackendProbe> = backend_report
        .results
        .iter()
        .map(|r| BackendProbe {
            name: r.backend.name(),
            url: r.backend.upstream_url(),
            reachable: r.reachable,
        })
        .collect();

    // Public-mode posture. Cheap: if the policy file is missing
    // (the common case for a peer that hasn't opted in) we return
    // PeerPaymentPolicy::closed() without touching disk. A parse
    // error on a present-but-corrupt file degrades to "closed" too,
    // because every status caller deserves *some* answer and the
    // safe answer is "we accept nothing".
    let public_mode_status = {
        let policy = public_mode::load().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "public_mode: load failed in status, returning closed");
            public_mode::PeerPaymentPolicy::closed()
        });
        let open = !policy.accepted_rails.is_empty();
        PublicModeStatus { open, policy }
    };

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
                return Ok(axum::Json(StatusResponse {
                    node: NodeStatus {
                        addr: state.node.addr.to_string(),
                        name: state.node.name.clone(),
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    upstream: UpstreamStatus {
                        url: upstream_url,
                        reachable,
                        model,
                        backends: backends.clone(),
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
                    vram_pool: None,
                    public_mode: public_mode_status.clone(),
                }));
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
                // Don't overwrite a reachable address with an
                // unreachable one. mDNS on macOS frequently
                // advertises an IPv6 link-local (`fe80::*`) for
                // peers on the local interface, and on this
                // platform a link-local address without a zone
                // identifier (`%en0`) isn't routable from a fresh
                // `connect()` call. Previously this code blindly
                // replaced the registry's `127.0.0.1:7778`
                // (perfectly reachable) with the link-local
                // (broken), and every peer-to-peer call after the
                // first status poll failed with "Connection
                // refused". Skip the swap when the discovered
                // address looks worse than what we already have.
                if p.addr != d.addr && discovered_is_better(&p.addr, &d.addr) {
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

    Ok(axum::Json(StatusResponse {
        node: NodeStatus {
            addr: state.node.addr.to_string(),
            name: state.node.name.clone(),
            version: env!("CARGO_PKG_VERSION"),
        },
        upstream: UpstreamStatus {
            url: upstream_url,
            reachable,
            model,
            backends,
        },
        peers,
        routing: RoutingStatus {
            targets: state.router.target_count(),
            mode: "round-robin",
        },
        discovered,
        relay,
        // Cheap probe (~30 ms for the llama-server --help call when
        // present, less when absent). Run per-request rather than
        // cached so the user sees changes immediately after a brew
        // install / tap install without restarting the daemon.
        vram_pool: Some(vram_pool::probe()),
        public_mode: public_mode_status,
    }))
}

#[derive(Deserialize)]
struct PairRequest {
    name: String,
    addr: SocketAddr,
    #[serde(default = "default_pair_priority")]
    priority: u8,
    /// Optional Ed25519 public key (base64 no-pad) for the peer being
    /// added. The /v1/pair/accept handler discovers the pubkey via the
    /// pairing token exchange; this field exists so the CLI's accept
    /// flow can pass the pubkey through to the daemon's registry in
    /// one call instead of patching `peers.toml` on disk (which
    /// breaks when the CLI's XDG_CONFIG_HOME differs from the
    /// daemon's). Background: pair-accept bug surfaced by the
    /// 2026-05-20 vrampool-loopback benchmark.
    #[serde(default)]
    pubkey: Option<String>,
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PairRequest>,
) -> Result<axum::Json<PairResponse>, StatusCode> {
    // Body irrelevant: this endpoint is local-user-only, so peer-signed
    // requests get rejected by require_auth(_, true) regardless.
    let _ = &req;
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;

    let new_peer = Peer {
        name: req.name.clone(),
        addr: req.addr,
        priority: req.priority,
        models: vec![],
        // None for LAN-discovered peers; Some for trusted pairing
        // where the CLI already learned the peer's pubkey from the
        // /v1/pair/accept reply and is registering it in one call.
        pubkey: req.pubkey.clone(),
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<PairOfferResponse>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;

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

/// Pick the address to put in a pair offer.
///
/// Rule: be honest about where the daemon actually listens.
/// - Unspecified bind (0.0.0.0, ::): the daemon listens on every
///   interface but the bind addr itself isn't dial-able. Replace
///   with a LAN IP so the offer is usable from elsewhere on the
///   network.
/// - Loopback bind (127.0.0.1, ::1): the daemon listens *only* on
///   loopback. Advertise loopback. Anyone scanning the offer from
///   off-machine genuinely can't reach this daemon — telling them
///   to dial a LAN IP would be a lie.
/// - Specific IP bind: pass through unchanged.
///
/// Earlier versions advertised a LAN IP for loopback binds too,
/// which made loopback multi-daemon test setups (e.g. v0.0.53's
/// vrampool-loopback benchmark) require manual URI rewriting from
/// `addr=10.x.x.x:port` to `addr=127.0.0.1:port`. Surfaced in
/// runs/2026-05-20-vrampool-loopback/notes.md.
fn advertised_addr(bind: SocketAddr) -> SocketAddr {
    if !bind.ip().is_unspecified() {
        // Loopback or specific IP — pass through.
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<ShortOfferResponse>, (StatusCode, String)> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if require_auth(&outcome, true).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }

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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<UseCodeRequest>,
) -> Result<axum::Json<PairConnectResponse>, (StatusCode, String)> {
    let _ = &req;
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if require_auth(&outcome, true).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }

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
    do_pair_connect(state, offer).await
}

#[derive(Deserialize)]
struct PunchRequest {
    /// Pubkey of an already-paired peer to attempt a hole-punch with.
    /// Must match an entry in the peer registry (we only punch peers
    /// we've already verified).
    peer: String,
    /// Overall coordination timeout (seconds). Defaults to 8.
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Serialize)]
struct PunchResponse {
    /// Was the relay able to coordinate matching PunchRequests from both
    /// sides within the timeout?
    coordinated: bool,
    /// Was a UDP packet from the peer's external addr actually observed?
    /// `false` typically means symmetric NAT on one side; we'll need to
    /// continue using the relay fallback for that peer.
    bidirectional: bool,
    /// External `ip:port` the relay told us about, if coordination succeeded.
    peer_addr: Option<String>,
    /// Local UDP port we bound for the attempt (informational).
    local_port: Option<u16>,
    /// Human-readable error, when coordination failed.
    error: Option<String>,
}

/// `POST /v1/punch` — diagnostic. Asks the relay to coordinate a UDP
/// hole-punch with `peer` and reports whether bidirectional UDP was
/// observed. Used to validate that a direct path is feasible before we
/// commit a real transport (QUIC) to using it. Both sides must call this
/// roughly simultaneously; otherwise one side will be the "first half"
/// and time out.
async fn punch_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PunchRequest>,
) -> Result<axum::Json<PunchResponse>, (StatusCode, String)> {
    let _ = &req;
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if require_auth(&outcome, true).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }

    // The peer must be in our registry — we only punch trusted peers.
    let peer_pubkey: String = {
        let reg = state.registry.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "registry poisoned".into(),
            )
        })?;
        let matched = reg
            .peers
            .iter()
            .find(|p| p.name == req.peer || p.pubkey.as_deref() == Some(req.peer.as_str()))
            .ok_or((StatusCode::NOT_FOUND, format!("no peer named {}", req.peer)))?;
        matched.pubkey.clone().ok_or((
            StatusCode::PRECONDITION_FAILED,
            "peer has no pubkey on file — re-pair to enable punch".into(),
        ))?
    };

    if !matches!(state.relay.current_state().await, RelayState::Registered) {
        return Err((
            StatusCode::PRECONDITION_FAILED,
            "relay not connected — start the daemon with --relay ws://... to enable punching"
                .into(),
        ));
    }

    let timeout = std::time::Duration::from_secs(req.timeout_secs.unwrap_or(8));
    match state.relay.try_punch(&peer_pubkey, timeout).await {
        Ok(outcome) => Ok(axum::Json(PunchResponse {
            coordinated: true,
            bidirectional: outcome.bidirectional,
            peer_addr: Some(outcome.peer_addr.to_string()),
            local_port: Some(outcome.local_port),
            error: None,
        })),
        Err(e) => Ok(axum::Json(PunchResponse {
            coordinated: false,
            bidirectional: false,
            peer_addr: None,
            local_port: None,
            error: Some(e.to_string()),
        })),
    }
}

#[derive(Deserialize)]
struct QuicPingRequest {
    /// Name of an already-paired peer in the registry.
    peer: String,
}

#[derive(Serialize)]
struct QuicPingResponse {
    /// True when both sides completed the QUIC handshake and exchanged
    /// the ping/pong stream. False with `error` set otherwise.
    ok: bool,
    /// Round-trip in milliseconds.
    rtt_ms: Option<u64>,
    /// Address dialed (`<peer-addr-ip>:<peer-port+1>`).
    target_addr: Option<String>,
    error: Option<String>,
}

/// `POST /v1/quic/ping` — diagnostic. Dials the peer's QUIC endpoint
/// (UDP, port+1 from their HTTP addr), runs the cert-key check, and
/// times a round-trip on a single bidi stream. Confirms the encrypted
/// peer-to-peer path works end-to-end on this network. v0.0.4 uses
/// QUIC only for this diagnostic; `/v1/run` still rides HTTP.
async fn quic_ping_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<QuicPingRequest>,
) -> Result<axum::Json<QuicPingResponse>, (StatusCode, String)> {
    let _ = &req;
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if require_auth(&outcome, true).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }

    let Some(quic) = state.quic.clone() else {
        return Ok(axum::Json(QuicPingResponse {
            ok: false,
            rtt_ms: None,
            target_addr: None,
            error: Some("quic endpoint failed to bind at startup".into()),
        }));
    };

    let peer_http_addr = {
        let reg = state.registry.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "registry poisoned".into(),
            )
        })?;
        reg.peers
            .iter()
            .find(|p| p.name == req.peer)
            .map(|p| p.addr)
            .ok_or((StatusCode::NOT_FOUND, format!("no peer named {}", req.peer)))?
    };
    let target = SocketAddr::new(peer_http_addr.ip(), peer_http_addr.port().saturating_add(1));

    match quic.ping(target, &state.identity.public_b64()).await {
        Ok(rtt) => Ok(axum::Json(QuicPingResponse {
            ok: true,
            rtt_ms: Some(rtt.as_millis() as u64),
            target_addr: Some(target.to_string()),
            error: None,
        })),
        Err(e) => Ok(axum::Json(QuicPingResponse {
            ok: false,
            rtt_ms: None,
            target_addr: Some(target.to_string()),
            error: Some(e.to_string()),
        })),
    }
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PairConnectRequest>,
) -> Result<axum::Json<PairConnectResponse>, (StatusCode, String)> {
    let _ = &req;
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if require_auth(&outcome, true).is_err() {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".into()));
    }
    do_pair_connect(state, req.offer).await
}

/// Underlying connect logic. Separate from the HTTP handler so internal
/// callers (e.g. pair_use_code_handler, which already authed) can reuse
/// it without re-authing.
async fn do_pair_connect(
    state: NodeState,
    offer: String,
) -> Result<axum::Json<PairConnectResponse>, (StatusCode, String)> {
    let parsed = parse_offer_uri(&offer)
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &body);
    require_auth(&outcome, false)?;

    let already_forwarded = headers.get(FORWARDED_HEADER).is_some();
    let target = if already_forwarded {
        Target::Local
    } else {
        state.router.next()
    };

    match target {
        Target::Local => proxy_chat_local(&state.node, &state.http, &state.vram_pool, body).await,
        Target::Peer { ref name, addr } => match proxy_chat_peer(name, addr, &body).await {
            Ok(r) => Ok(r),
            Err(e) => {
                tracing::warn!(peer = %name, error = %e, "chat: peer unreachable, falling back to local");
                proxy_chat_local(&state.node, &state.http, &state.vram_pool, body).await
            }
        },
    }
}

/// Pick which upstream the chat-completions proxy talks to for this
/// request. When the VRAM-pool is `Running`, route through it; the
/// whole point of starting the pool is to use it. Otherwise fall
/// back to the configured-or-probed backend list. The pool's plan
/// carries the model path that was passed to `llama-server -m`, so
/// we surface that as the `model` field for the placeholder-model
/// substitution downstream.
async fn resolve_upstream(
    node: &Node,
    pool: &vram_pool::PoolManager,
) -> Option<upstream::LiveUpstream> {
    if let vram_pool::PoolState::Running { endpoint, plan, .. } = pool.status().await {
        tracing::debug!(endpoint = %endpoint, model = %plan.model, "chat: routing through vram-pool");
        return Some(upstream::LiveUpstream {
            url: endpoint,
            model: Some(plan.model),
        });
    }
    upstream::select_live(&node.llama_server_url).await
}

/// If the chat-completions body's `model` field is the documented
/// placeholder ("local" / "default" / "auto"), swap it for the
/// upstream's actual model id. llama-server doesn't care about the
/// model name, but Ollama and LM Studio strictly resolve it against
/// their loaded set — sending the placeholder to those backends 404s.
///
/// Falls through unchanged when:
///
/// - the body isn't valid JSON (let upstream's error speak)
/// - the model field is already a real name (assume user knows)
/// - we don't know the upstream's model (no probe data — let upstream
///   error properly so the user sees a real message)
fn rewrite_placeholder_model(body: bytes::Bytes, upstream_model: Option<&str>) -> bytes::Bytes {
    let Some(real_model) = upstream_model else {
        return body;
    };
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return body;
    };
    let Some(obj) = v.as_object_mut() else {
        return body;
    };
    let needs_swap = match obj.get("model") {
        Some(serde_json::Value::String(s)) => {
            matches!(s.as_str(), "local" | "default" | "auto" | "")
        }
        None => true,
        _ => false,
    };
    if !needs_swap {
        return body;
    }
    obj.insert(
        "model".to_string(),
        serde_json::Value::String(real_model.to_string()),
    );
    match serde_json::to_vec(&v) {
        Ok(b) => bytes::Bytes::from(b),
        Err(_) => body,
    }
}

/// Inject retrieved memory summaries into the chat-completions request's
/// system message. No-op when:
///   - memory is disabled (the toggle is off → privacy-preserving default)
///   - the store is empty (nothing to inject)
///   - the body isn't valid JSON or doesn't look like a chat request
///
/// When applied, picks the top-3 keyword-overlap matches against the
/// latest user message in the request and either prepends them to the
/// existing system message or inserts a new one at the head of the
/// messages array. Caller's other fields (model, max_tokens, etc.)
/// are untouched.
///
/// Three is small enough that even with our 1k-character summary cap
/// it can't blow past a 4k-token system prompt. If the user grows the
/// memory store large, the keyword ranker still picks the three most
/// relevant — quality of context matters more than quantity here.
const MEMORY_TOP_K: usize = 3;

/// Ensure the request has a system message anchoring the assistant's
/// voice. If the caller already supplied one (anywhere in the messages
/// array — most chat libraries put it at index 0 but the spec allows
/// it anywhere), leave it alone. Otherwise prepend a system message
/// with `DEFAULT_SYSTEM_PROMPT` at index 0.
///
/// Why this exists: `/v1/run` always injects this prompt (the CLI's
/// `unhosted run "..."` flow), but `/v1/chat/completions` did not.
/// External callers — curl one-liners, agents, OpenAI-API-compatible
/// libraries — would get whatever the upstream model's default
/// behavior is, which on most fine-tuned chat models is the
/// marketing-toned "I'm an AI assistant here to help!" opener that
/// the project's voice explicitly rejects.
///
/// No-op when the body isn't valid JSON or doesn't have a `messages`
/// array (let upstream's error speak), and when a system message
/// already exists (respect the caller).
fn ensure_default_system_prompt(body: bytes::Bytes) -> bytes::Bytes {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return body;
    };
    let Some(messages) = v.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return body;
    };
    let already_has_system = messages
        .iter()
        .any(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"));
    if already_has_system {
        return body;
    }
    messages.insert(
        0,
        serde_json::json!({ "role": "system", "content": DEFAULT_SYSTEM_PROMPT }),
    );
    tracing::debug!("chat: injected default system prompt (caller sent none)");
    serde_json::to_vec(&v)
        .map(bytes::Bytes::from)
        .unwrap_or(body)
}

fn inject_memory_context(body: bytes::Bytes) -> bytes::Bytes {
    if !memory::is_enabled() {
        return body;
    }
    let store = memory::load();
    if store.entries.is_empty() {
        return body;
    }
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return body;
    };
    let Some(messages) = v.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return body;
    };
    // Extract the latest user message as the retrieval query. Walking
    // backwards rather than taking the last entry covers the case where
    // the most recent message is a tool/assistant turn.
    let query = messages
        .iter()
        .rev()
        .find_map(|m| {
            let obj = m.as_object()?;
            if obj.get("role").and_then(|r| r.as_str()) == Some("user") {
                obj.get("content")
                    .and_then(|c| c.as_str())
                    .map(String::from)
            } else {
                None
            }
        })
        .unwrap_or_default();
    let hits = memory::retrieve(&store, &query, MEMORY_TOP_K);
    if hits.is_empty() {
        return body;
    }
    let context = build_memory_block(&hits);

    // Prepend to existing system message, or insert a new one at index 0.
    let existing_system_idx = messages
        .iter()
        .position(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"));
    match existing_system_idx {
        Some(i) => {
            let prev_content = messages[i]
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            messages[i].as_object_mut().map(|o| {
                o.insert(
                    "content".into(),
                    serde_json::Value::String(format!("{context}\n\n{prev_content}")),
                )
            });
        }
        None => {
            messages.insert(
                0,
                serde_json::json!({ "role": "system", "content": context }),
            );
        }
    }
    serde_json::to_vec(&v)
        .map(bytes::Bytes::from)
        .unwrap_or(body)
}

fn build_memory_block(hits: &[&memory::MemoryEntry]) -> String {
    let mut out = String::from(
        "you have private memory of previous conversations with this user. \
         use these as background context when they're relevant — do not \
         repeat them back verbatim or mention you are reading them:\n",
    );
    for h in hits {
        out.push_str("- ");
        out.push_str(h.summary.trim());
        out.push('\n');
    }
    out
}

async fn proxy_chat_local(
    node: &Node,
    client: &reqwest::Client,
    pool: &vram_pool::PoolManager,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let Some(live) = resolve_upstream(node, pool).await else {
        return Ok(upstream_offline_response(&node.llama_server_url));
    };
    let base = live.url;
    let url = format!("{base}/v1/chat/completions");
    // Substitute the documented placeholder model "local" with the
    // upstream's actual model id when we know it. llama-server ignores
    // the model field entirely, but Ollama (and LM Studio) reject
    // unknown model names with a 404 — that's what was surfacing as a
    // bare 502 to anyone copying the docs snippet against Ollama.
    let body = rewrite_placeholder_model(body, live.model.as_deref());
    // Ensure every chat-completion has a system message anchoring the
    // assistant's voice. /v1/run already does this; /v1/chat/completions
    // historically didn't, so a curl one-liner or an external agent
    // hitting the chat endpoint got whatever the upstream's default
    // happened to be — usually a marketing-toned "I'm an AI assistant
    // here to help!" opener that the project's voice explicitly rejects.
    // Caller-supplied system messages are left intact; this only fills
    // the gap when none was provided.
    let body = ensure_default_system_prompt(body);
    // If the user has private memory enabled, retrieve the most
    // relevant past summaries and prepend them to the system prompt.
    // Runs AFTER the default-system step so memory text always lands
    // ahead of the voice anchor — the model reads "context first, then
    // voice" in order. No-op when memory is off or the store is empty,
    // so the bytes flow through untouched in the common path.
    let body = inject_memory_context(body);
    let upstream = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, %url, "chat: upstream call failed");
            StatusCode::BAD_GATEWAY
        })?;

    let status = upstream.status();
    if !status.is_success() {
        tracing::error!(%status, %url, "chat: upstream non-success");
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
        .header("x-unhosted-upstream", base)
        .body(Body::from_stream(stream))
        .expect("valid response"))
}

/// Build the structured "no upstream reachable" response. Returned as
/// HTTP 503 with a JSON body the web UI parses to render a friendly
/// message + install hint. Falls back to a plain 502 if JSON
/// serialization somehow fails (should be unreachable).
fn upstream_offline_response(configured: &str) -> Response {
    let body = upstream::offline_error_json(configured);
    let bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from("upstream offline"))
                .expect("valid response");
        }
    };
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header("content-type", "application/json")
        .header("x-unhosted-served-by", "local")
        .header("x-unhosted-error", "upstream_offline")
        .body(Body::from(bytes))
        .expect("valid response")
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
async fn models_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, false)?;

    let Some(live) = upstream::select_live(&state.node.llama_server_url).await else {
        return Ok(upstream_offline_response(&state.node.llama_server_url));
    };
    let url = format!("{}/v1/models", live.url);
    let upstream = state.http.get(&url).send().await.map_err(|e| {
        tracing::error!(error = %e, %url, "models: upstream call failed");
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

async fn identity_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, false)?;

    Ok(axum::Json(serde_json::json!({
        "name": state.node.name,
        "pubkey": state.identity.public_b64(),
        "addr": state.node.addr.to_string(),
    })))
}

// ─── chat history endpoints ────────────────────────────────────────────────
// All four chat endpoints are local-user-only: paired peers can call /v1/run
// against this daemon's hardware, but they don't get to read or mutate the
// owner's chat history. Auth must be loopback or a valid local bearer token.

async fn chats_list_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    Ok(axum::Json(
        serde_json::json!({ "chats": state.chats.list() }),
    ))
}

async fn chats_get_handler(
    State(state): State<NodeState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<chats::Chat>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    state
        .chats
        .get(&id)
        .map(axum::Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Insert-or-replace. Used by both `POST /v1/chats` (id in body) and
/// `PUT /v1/chats/{id}` (id in path). When the path id is present it
/// overrides whatever the body says — clients shouldn't rely on the
/// body id matching, but if they get it wrong we don't surprise them
/// by writing under a different key.
async fn chats_upsert_handler(
    State(state): State<NodeState>,
    path: Option<axum::extract::Path<String>>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<chats::Chat>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &body);
    require_auth(&outcome, true)?;
    let mut chat: chats::Chat = serde_json::from_slice(&body).map_err(|e| {
        tracing::warn!(error = %e, "chats upsert: bad body");
        StatusCode::BAD_REQUEST
    })?;
    if let Some(axum::extract::Path(path_id)) = path {
        chat.id = path_id;
    }
    if chat.id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let saved = state.chats.upsert(chat).map_err(|e| {
        tracing::error!(error = %e, "chats upsert: write failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Auto-summarize feed: if the user has private memory turned on,
    // kick off a debounced summarizer for this chat. The summarizer
    // calls the local LLM ~30 s after the last upsert, so we only
    // pay one round-trip per "burst" of activity instead of one per
    // turn. Cheap no-op when memory is off.
    if memory::is_enabled() && saved.messages.len() >= 2 {
        tracing::info!(chat_id = %saved.id, msgs = saved.messages.len(), "memory: scheduling summarizer");
        schedule_chat_summarize(&state, &saved.id);
    }
    Ok(axum::Json(saved))
}

/// How long to wait after the last chat upsert before kicking the
/// summarizer. Long enough to skip the per-token saves a streaming
/// chat does, short enough that closing the app or starting a new
/// chat catches the summary on disk.
const SUMMARIZE_DEBOUNCE_SECS: u64 = 30;

/// Cap on chat-message slice we feed the summarizer. Older context
/// matters less for "what does this chat reveal about the user", and
/// the local LLM has limited context — 10 turns (so ~5 user + 5
/// assistant) covers the salient bits without blowing the budget.
const SUMMARIZE_MESSAGE_TAIL: usize = 10;

fn schedule_chat_summarize(state: &NodeState, chat_id: &str) {
    let chat_id = chat_id.to_string();

    // Cancel any in-flight summarizer for this chat — every new
    // message resets the debounce timer.
    {
        let mut inflight = match state.summarize_inflight.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(prev) = inflight.remove(&chat_id) {
            prev.abort();
        }
    }

    // Clone the bits the task needs into separately-owned values so
    // we can still touch `state` after spawning to insert the handle.
    let state_for_task = state.clone();
    let inflight_for_task = state.summarize_inflight.clone();
    let chat_id_for_task = chat_id.clone();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(SUMMARIZE_DEBOUNCE_SECS)).await;
        if !memory::is_enabled() {
            return;
        }
        let Some(chat) = state_for_task.chats.get(&chat_id_for_task) else {
            return;
        };
        if chat.messages.len() < 2 {
            return;
        }
        match summarize_chat_via_upstream(&state_for_task, &chat).await {
            Ok(Some(summary)) => {
                let trimmed = summary.trim().to_string();
                if !trimmed.is_empty() {
                    if let Err(e) = memory::upsert_for_chat(chat_id_for_task.clone(), trimmed) {
                        tracing::warn!(error = %e, chat_id = %chat_id_for_task, "memory: summary write failed");
                    } else {
                        tracing::info!(chat_id = %chat_id_for_task, "memory: chat summary updated");
                    }
                }
            }
            Ok(None) => {
                // Upstream didn't return usable text — skip silently.
            }
            Err(e) => {
                tracing::warn!(error = %e, chat_id = %chat_id_for_task, "memory: summarize call failed");
            }
        }
        // Self-remove from the inflight map so the next upsert spawns fresh.
        if let Ok(mut g) = inflight_for_task.lock() {
            g.remove(&chat_id_for_task);
        }
    });

    match state.summarize_inflight.lock() {
        Ok(mut g) => {
            g.insert(chat_id, handle);
        }
        Err(p) => {
            p.into_inner().insert(chat_id, handle);
        }
    }
}

/// Ask the configured local LLM to summarize a chat into the format
/// the memory store expects: third-person, focused on the user (not
/// the topic), 1–2 sentences. Returns `Ok(None)` if upstream is
/// unreachable or returns a non-JSON body — those are recoverable
/// "skip this round" cases, not hard errors.
async fn summarize_chat_via_upstream(
    state: &NodeState,
    chat: &chats::Chat,
) -> anyhow::Result<Option<String>> {
    let Some(live) = upstream::select_live(&state.node.llama_server_url).await else {
        return Ok(None);
    };
    let mut transcript = String::new();
    let tail = chat
        .messages
        .iter()
        .rev()
        .take(SUMMARIZE_MESSAGE_TAIL)
        .collect::<Vec<_>>();
    for msg in tail.iter().rev() {
        let role = if msg.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        transcript.push_str(role);
        transcript.push_str(": ");
        transcript.push_str(msg.text.trim());
        transcript.push_str("\n\n");
    }
    let system = "you summarize chats for a private memory store. your output is read later by the same model as background context. write 1–2 sentences, third person, about the USER — their interests, preferences, technical level, or any persistent facts they shared. do not summarize the topic of the conversation itself, do not include greetings, do not say \"the user asks about X\". return only the summary text, no preamble.";
    let body = serde_json::json!({
        "model": live.model.clone().unwrap_or_else(|| "local".to_string()),
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": format!("transcript:\n\n{transcript}\nsummary:") }
        ],
        "max_tokens": 200,
        "stream": false,
    });
    let url = format!("{}/v1/chat/completions", live.url);
    let resp = state
        .http
        .post(&url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let json: serde_json::Value = resp.json().await?;
    let content = json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    Ok(content)
}

async fn chats_delete_handler(
    State(state): State<NodeState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    match state.chats.delete(&id) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(error = %e, "chats delete: write failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn chats_clear_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    let cleared = state.chats.clear().map_err(|e| {
        tracing::error!(error = %e, "chats clear: write failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(axum::Json(serde_json::json!({ "cleared": cleared })))
}

// ─── tunnel (cloudflare) endpoints ────────────────────────────────────────
// All three are local-user-only. Spawning a public tunnel is consequential —
// only the owner of this daemon should be able to flip it on, and only from
// loopback or with the bearer token.

async fn tunnel_status_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<tunnel::TunnelState>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    Ok(axum::Json(state.tunnel.status().await))
}

async fn tunnel_start_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<tunnel::TunnelState>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    match state.tunnel.clone().start().await {
        Ok(s) => Ok(axum::Json(s)),
        Err(e) => {
            tracing::warn!(error = %e, "tunnel start failed");
            Ok(axum::Json(tunnel::TunnelState::Failed {
                error: e.to_string(),
            }))
        }
    }
}

async fn tunnel_stop_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<tunnel::TunnelState>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    // Log the caller so we can identify who keeps killing the tunnel.
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let referer = headers
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let cf_ip = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let confirm = headers
        .get("x-unhosted-confirm")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    tracing::warn!(
        remote = %remote,
        cf_connecting_ip = %cf_ip,
        user_agent = %ua,
        referer = %referer,
        has_confirm = !confirm.is_empty(),
        "POST /v1/tunnel/stop — identifying caller"
    );
    // Server-side guard: require an explicit X-Unhosted-Confirm: yes header
    // before honoring a stop. Stale tabs running pre-confirm JS, accidental
    // scripts, and anything other than the current UI cannot kill the
    // tunnel anymore. The active web/ui.js attaches this header from the
    // toggle handler immediately after the user confirms the in-app modal.
    if confirm != "yes" {
        tracing::warn!(
            remote = %remote,
            user_agent = %ua,
            "tunnel stop refused: missing X-Unhosted-Confirm: yes header (caller is likely stale UI)"
        );
        return Err(StatusCode::PRECONDITION_REQUIRED);
    }
    let s = state.tunnel.stop().await.map_err(|e| {
        tracing::error!(error = %e, "tunnel stop failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(axum::Json(s))
}

// ─── private memory endpoints ─────────────────────────────────────────────
// Local-user-only. The memory store is the user's own past chat summaries;
// no peer or unauthenticated caller should be able to read or mutate it.
// Same auth posture as the /v1/chats CRUD endpoints just above.

#[derive(serde::Serialize)]
struct MemoryListResponse {
    enabled: bool,
    entries: Vec<memory::MemoryEntry>,
}

async fn memory_list_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<MemoryListResponse>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    let store = memory::load();
    Ok(axum::Json(MemoryListResponse {
        enabled: memory::is_enabled(),
        entries: store.entries,
    }))
}

#[derive(serde::Deserialize)]
struct MemoryAddRequest {
    summary: String,
    chat_id: Option<String>,
}

async fn memory_add_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<MemoryAddRequest>,
) -> Result<axum::Json<memory::MemoryEntry>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    let summary = req.summary.trim();
    if summary.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    memory::add(summary.to_string(), req.chat_id)
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "memory: add failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn memory_delete_handler(
    State(state): State<NodeState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    match memory::remove(&id) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(error = %e, "memory: remove failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn memory_clear_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    memory::clear().map_err(|e| {
        tracing::error!(error = %e, "memory: clear failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct MemoryEnableRequest {
    enabled: bool,
}

async fn memory_enable_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<MemoryEnableRequest>,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    memory::set_enabled(req.enabled);
    Ok(axum::Json(serde_json::json!({ "enabled": req.enabled })))
}

// ─── tool: web_fetch ──────────────────────────────────────────────────────
// Server-side URL fetcher for the LLM tool-use loop. SSRF + scheme +
// size guards live in `web_fetch::fetch`; this handler is the thin
// auth + JSON layer in front of it. Local-user-only, same posture as
// /v1/memory and /v1/tunnel — only the daemon owner should be able to
// drive outbound fetches through their machine.

async fn web_fetch_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<web_fetch::WebFetchRequest>,
) -> Result<axum::Json<web_fetch::WebFetchResponse>, axum::http::Response<axum::body::Body>> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if let Err(status) = require_auth(&outcome, true) {
        // Return a structured-but-empty body so the caller's JSON
        // parser doesn't choke on a raw status.
        let body = serde_json::json!({ "error": "unauthorized" });
        return Err(axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("valid response"));
    }
    tracing::info!(url = %req.url, "web_fetch: requested");
    match web_fetch::fetch(&state.http, req).await {
        Ok(resp) => {
            tracing::info!(
                final_url = %resp.final_url,
                status = resp.status,
                bytes = resp.bytes,
                truncated = resp.truncated,
                "web_fetch: ok"
            );
            Ok(axum::Json(resp))
        }
        Err(e) => {
            tracing::warn!(error = %e, "web_fetch: rejected");
            let status = web_fetch::error_status(&e);
            let body = serde_json::json!({ "error": e.to_string() });
            Err(axum::http::Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("valid response"))
        }
    }
}

// ─── public-mode policy ───────────────────────────────────────────────────
// ADR-0010 slice 2. Read/replace the on-disk PeerPaymentPolicy that
// this node will advertise to peers asking for a quote. Both routes
// are local-user-only: the policy is part of the node's identity, and
// only the local user should decide what their machine accepts.

async fn public_mode_policy_get_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<public_mode::PeerPaymentPolicy>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    match public_mode::load() {
        Ok(policy) => Ok(axum::Json(policy)),
        Err(e) => {
            tracing::error!(error = %e, "public_mode: load failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn public_mode_policy_put_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(mut policy): axum::Json<public_mode::PeerPaymentPolicy>,
) -> Result<axum::Json<public_mode::PeerPaymentPolicy>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    // `save` merges the sanctions block-list into the policy before
    // writing. The response is the policy as actually persisted, so a
    // caller who PUT with KP missing sees KP present in the response.
    if let Err(e) = public_mode::save(&mut policy) {
        tracing::error!(error = %e, "public_mode: save failed");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(axum::Json(policy))
}

#[derive(serde::Serialize)]
struct PolicyInspectResponse {
    accepted: bool,
    /// Human-readable rejection reason. `None` when accepted.
    /// Stable enough to render in the UI without parsing.
    reason: Option<String>,
}

// ─── public-mode quote ────────────────────────────────────────────────────
// Slice 2b: the actual "tell me what this would cost" call. A payer
// signs a canonical body with their Ed25519 key, the daemon verifies
// the sig, runs policy.accepts(), and returns a Quote on success or
// a structured rejection on failure. Pricing is a fixed default for
// now (10 micros per unit) — slice 3 will plumb in real rail-side
// quotes. The point of shipping the endpoint shape early is so the
// payer-side helpers (wallet-js, slice 4) can be written against a
// stable wire format.

#[derive(serde::Deserialize, serde::Serialize, Clone)]
struct QuoteRequestBody {
    payer: public_mode::PayerContext,
    payer_pubkey: String,
    requested_units: u64,
}

#[derive(serde::Deserialize)]
struct QuoteRequest {
    body: QuoteRequestBody,
    /// Base64 (no-pad) Ed25519 sig over canonical_json(body), produced
    /// with the secret matching payer_pubkey. The sig is the auth —
    /// no bearer / loopback / paired-peer check applies.
    sig: String,
}

#[derive(serde::Serialize)]
struct Quote {
    job_id: String,
    host_pubkey: String,
    unit_price_micros: u64,
    quoted_units: u64,
    /// Unix seconds. Quote is valid until this timestamp; after that
    /// the payer must re-quote. Short window (5 min) — quotes that
    /// linger are a footgun for sudden policy changes.
    expires_at: u64,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum QuoteResponse {
    Quote(Quote),
    Rejected { reason: String },
}

/// Fixed-price placeholder until rail-aware pricing lands in slice 3.
/// 10 micros = $0.00001 — small enough that a quote against the
/// stub never looks like a real number to a UI tester.
const STUB_UNIT_PRICE_MICROS: u64 = 10;
const QUOTE_TTL_SECONDS: u64 = 300;

async fn public_mode_quote_handler(
    State(state): State<NodeState>,
    bytes: axum::body::Bytes,
) -> Result<axum::Json<QuoteResponse>, StatusCode> {
    // Parse the whole envelope first so we can fail-fast on
    // malformed JSON before touching the policy file.
    let req: QuoteRequest = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };
    // Verify the sig is over canonical_json(body) signed by the
    // claimed payer pubkey. We reject hard here — a stranger can't
    // even *attempt* a quote without proving they hold the key
    // they claim.
    let body_bytes = match unhosted_payments_core::receipt::canonical_json(&req.body) {
        Ok(b) => b,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };
    let key = match unhosted_payments_core::parse_pubkey(&req.body.payer_pubkey) {
        Ok(k) => k,
        Err(_) => return Err(StatusCode::UNAUTHORIZED),
    };
    if unhosted_payments_core::verify_sig(&key, &body_bytes, &req.sig).is_err() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let policy = match public_mode::load() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "public_mode: load failed (quote)");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    match policy.accepts(&req.body.payer) {
        Ok(()) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Ok(axum::Json(QuoteResponse::Quote(Quote {
                job_id: format!("job_{now:x}"),
                host_pubkey: state.identity.public_b64(),
                unit_price_micros: STUB_UNIT_PRICE_MICROS,
                quoted_units: req.body.requested_units,
                expires_at: now + QUOTE_TTL_SECONDS,
            })))
        }
        Err(err) => Ok(axum::Json(QuoteResponse::Rejected {
            reason: err.to_string(),
        })),
    }
}

#[derive(serde::Deserialize)]
struct ReceiptSignRequest {
    job_id: String,
    payer_pubkey: String,
    rail: public_mode::PaymentRail,
    units: u64,
    unit_price_micros: u64,
}

async fn public_mode_receipt_sign_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<ReceiptSignRequest>,
) -> Result<axum::Json<unhosted_payments_core::SignedReceipt>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    let report = unhosted_payments_core::UsageReport {
        job_id: req.job_id,
        host_pubkey: state.identity.public_b64(),
        payer_pubkey: req.payer_pubkey,
        rail: req.rail,
        units: req.units,
        unit_price_micros: req.unit_price_micros,
        issued_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    match unhosted_payments_core::sign_receipt(report, state.identity.signing_key()) {
        Ok(receipt) => Ok(axum::Json(receipt)),
        Err(e) => {
            tracing::error!(error = %e, "public_mode: sign_receipt failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn public_mode_policy_inspect_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(payer): axum::Json<public_mode::PayerContext>,
) -> Result<axum::Json<PolicyInspectResponse>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    let policy = match public_mode::load() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "public_mode: load failed (inspect)");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let resp = match policy.accepts(&payer) {
        Ok(()) => PolicyInspectResponse {
            accepted: true,
            reason: None,
        },
        Err(err) => PolicyInspectResponse {
            accepted: false,
            reason: Some(err.to_string()),
        },
    };
    Ok(axum::Json(resp))
}

// ─── vram-pool orchestration ──────────────────────────────────────────────
// Local-user-only — spawning subprocesses on the user's machine is
// exactly the kind of action that needs the auth classifier on it.
// Same posture as /v1/tunnel.

async fn vram_pool_status_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<vram_pool::PoolState>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;
    Ok(axum::Json(state.vram_pool.status().await))
}

#[derive(serde::Deserialize)]
struct VramPoolStartRequest {
    /// Pre-computed plan (built by the CLI from local capability +
    /// `--peers` + `--model`). Passing the whole plan from the
    /// caller keeps the daemon decision-free; the planner lives in
    /// pure Rust and a future client (UI, agent) can build plans
    /// without going through the CLI.
    plan: vram_pool::Plan,
}

async fn vram_pool_start_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<VramPoolStartRequest>,
) -> Result<axum::Json<vram_pool::PoolState>, axum::http::Response<axum::body::Body>> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    if let Err(status) = require_auth(&outcome, true) {
        let body = serde_json::json!({ "error": "unauthorized" });
        return Err(axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("valid response"));
    }

    // Phase 2c: for every non-local layer host in the plan, sign +
    // POST `/v1/vram-pool/layer-host/start` to its daemon BEFORE
    // we ask PoolManager to spawn local llama-server. PoolManager
    // is intentionally local-only; cluster coordination lives here
    // in the route handler so it can use NodeState's identity +
    // registry + http client without polluting the pool module.
    let remote_hosts: Vec<vram_pool::LayerHost> = req
        .plan
        .layer_hosts
        .iter()
        .filter(|h| h.name != "local")
        .cloned()
        .collect();

    // We accumulate successfully-started peers so a mid-loop
    // failure can tell those peers to stop hosting rather than
    // leak rpc-server processes on their boxes.
    let mut started: Vec<vram_pool::LayerHost> = Vec::new();
    for host in &remote_hosts {
        if let Err(e) = ask_peer_to_host(&state, &host.name, host.addr.port()).await {
            for prior in &started {
                let _ = ask_peer_to_stop_hosting(&state, &prior.name).await;
            }
            let body = serde_json::json!({
                "error": format!("peer `{}` rejected layer-host request: {e}", host.name)
            });
            return Err(axum::http::Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("valid response"));
        }
        started.push(host.clone());
    }

    match state.vram_pool.clone().start(req.plan).await {
        Ok(s) => Ok(axum::Json(s)),
        Err(e) => {
            // PoolManager rejected. Roll back the peers we asked to
            // host so they don't leak processes either.
            for prior in &started {
                let _ = ask_peer_to_stop_hosting(&state, &prior.name).await;
            }
            tracing::warn!(error = %e, "vram-pool: start failed");
            let body = serde_json::json!({ "error": e.to_string() });
            Err(axum::http::Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .expect("valid response"))
        }
    }
}

/// Send a signed `/v1/vram-pool/layer-host/start` to a paired peer.
/// Looks up the peer's daemon address from the registry, signs the
/// JSON body with this node's identity, and waits for the peer's
/// `PoolState::Hosting` response. The peer's daemon will TCP-probe
/// its rpc-server bind before replying, so a 200 means the peer is
/// actually accepting RPC connections.
async fn ask_peer_to_host(state: &NodeState, peer_name: &str, rpc_port: u16) -> anyhow::Result<()> {
    let peer_addr = peer_daemon_addr(state, peer_name)
        .ok_or_else(|| anyhow::anyhow!("peer `{peer_name}` not in registry"))?;
    let body = serde_json::json!({
        "port": rpc_port,
        "orchestrator": state.identity.public_b64(),
    });
    let body_bytes = serde_json::to_vec(&body)?;
    let auth = state.identity.sign_request(&body_bytes);
    let url = format!("http://{peer_addr}/v1/vram-pool/layer-host/start");
    tracing::info!(peer = peer_name, %url, "vram-pool: asking peer to host");
    let resp = state
        .http
        .post(&url)
        .header("content-type", "application/json")
        .header("X-Unhosted-Auth", auth)
        .body(body_bytes)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("peer returned HTTP {status}: {text}");
    }
    Ok(())
}

/// Reverse of `ask_peer_to_host`. Best-effort — if the peer is
/// unreachable we log and move on; the orphan rpc-server on the
/// peer will be cleaned up by its own supervisor if/when the
/// orchestrator restarts.
async fn ask_peer_to_stop_hosting(state: &NodeState, peer_name: &str) -> anyhow::Result<()> {
    let peer_addr = peer_daemon_addr(state, peer_name)
        .ok_or_else(|| anyhow::anyhow!("peer `{peer_name}` not in registry"))?;
    let body_bytes: Vec<u8> = b"{}".to_vec();
    let auth = state.identity.sign_request(&body_bytes);
    let url = format!("http://{peer_addr}/v1/vram-pool/layer-host/stop");
    let _ = state
        .http
        .post(&url)
        .header("content-type", "application/json")
        .header("X-Unhosted-Auth", auth)
        .body(body_bytes)
        .send()
        .await;
    Ok(())
}

fn peer_daemon_addr(state: &NodeState, peer_name: &str) -> Option<SocketAddr> {
    let registry = state.registry.lock().ok()?;
    registry
        .peers
        .iter()
        .find(|p| p.name == peer_name)
        .map(|p| p.addr)
}

async fn vram_pool_stop_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::Json<vram_pool::PoolState>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;

    // Snapshot the current plan BEFORE stopping locally — once
    // stop() flips state to Idle we lose the layer-host list.
    let remote_hosts: Vec<vram_pool::LayerHost> = match state.vram_pool.status().await {
        vram_pool::PoolState::Running { plan, .. }
        | vram_pool::PoolState::Starting { plan, .. } => plan
            .layer_hosts
            .into_iter()
            .filter(|h| h.name != "local")
            .collect(),
        _ => Vec::new(),
    };

    let result = state.vram_pool.stop().await.map_err(|e| {
        tracing::error!(error = %e, "vram-pool: stop failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Tell each remote layer host to stop hosting. Best-effort — a
    // peer that's already off / unreachable doesn't block local
    // stop from succeeding. Sequential rather than parallel: the
    // peer count is small and serial is easier to log.
    for host in &remote_hosts {
        if let Err(e) = ask_peer_to_stop_hosting(&state, &host.name).await {
            tracing::warn!(
                peer = %host.name,
                error = %e,
                "vram-pool: failed to tell peer to stop hosting (may leak its rpc-server)"
            );
        }
    }

    Ok(axum::Json(result))
}

#[derive(serde::Deserialize)]
struct VramPoolLayerHostStartRequest {
    /// Port the orchestrator wants this machine's rpc-server to
    /// listen on. The orchestrator will dial `<our-ip>:port` via
    /// `llama-server --rpc=…`.
    port: u16,
    /// Human-readable identifier of the orchestrator (peer name
    /// or address). Surfaced verbatim in `PoolState::Hosting` so
    /// the UI can show "hosting layers for <peer>".
    orchestrator: String,
}

async fn vram_pool_layer_host_start_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<vram_pool::PoolState>, axum::http::Response<axum::body::Body>> {
    // Auth posture: must be a paired peer's signed request. NOT
    // loopback/bearer like the orchestrator-side endpoints — the
    // whole point is that a remote daemon is calling in to host
    // layers for it. We're effectively letting a paired peer run
    // a subprocess on this machine, so the pairing requirement is
    // load-bearing.
    let outcome = state.classify(&headers, Some(remote.ip()), &body);
    let req: VramPoolLayerHostStartRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return Err(axum::http::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(axum::body::Body::from(format!("bad json: {e}")))
                .expect("valid response"));
        }
    };
    if !matches!(outcome, auth::AuthOutcome::Peer(_)) {
        return Err(axum::http::Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(axum::body::Body::from(
                "layer-host operations require a paired-peer signed request",
            ))
            .expect("valid response"));
    }
    match state
        .vram_pool
        .clone()
        .start_as_layer_host(req.port, req.orchestrator)
        .await
    {
        Ok(s) => Ok(axum::Json(s)),
        Err(e) => Err(axum::http::Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(axum::body::Body::from(e.to_string()))
            .expect("valid response")),
    }
}

async fn vram_pool_layer_host_stop_handler(
    State(state): State<NodeState>,
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::Json<vram_pool::PoolState>, StatusCode> {
    // Same pairing-required posture as start.
    let outcome = state.classify(&headers, Some(remote.ip()), &body);
    if !matches!(outcome, auth::AuthOutcome::Peer(_)) {
        return Err(StatusCode::FORBIDDEN);
    }
    state.vram_pool.stop().await.map(axum::Json).map_err(|e| {
        tracing::error!(error = %e, "vram-pool: layer-host stop failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<axum::Json<PairResponse>, StatusCode> {
    let outcome = state.classify(&headers, Some(remote.ip()), &[]);
    require_auth(&outcome, true)?;

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
    axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RunRequest>,
) -> Result<Response, StatusCode> {
    // Auth: paired peer (signed header) OR local bearer OR loopback.
    // Same body the sender signed: serialized JSON of the request.
    let body_bytes = serde_json::to_vec(&req).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let outcome = state.classify(&headers, Some(remote.ip()), &body_bytes);
    require_auth(&outcome, false)?;

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
            let peer_pubkey = lookup_peer_pubkey(&state.registry, name);
            let quic_first = peer_pubkey.is_some()
                && state.quic.is_some()
                && std::env::var("UNHOSTED_QUIC_RUN")
                    .map(|v| v != "0" && !v.is_empty())
                    .unwrap_or(false);

            // QUIC path (opt-in via UNHOSTED_QUIC_RUN=1 during v0.0.4 →
            // v0.0.5 transition). Falls through to the HTTP-signed path
            // on any failure, preserving observability for whichever
            // network shape breaks the new transport.
            if quic_first {
                if let Some(ref quic) = state.quic {
                    let quic_target = SocketAddr::new(addr.ip(), addr.port().saturating_add(1));
                    match run_peer_via_quic(quic, quic_target, &req).await {
                        Ok(resp) => return Ok(resp),
                        Err(e) => {
                            tracing::info!(
                                peer = %name,
                                error = %e,
                                "quic peer path failed; falling back to HTTP"
                            );
                        }
                    }
                }
            }

            // Direct HTTP. If the peer has a pubkey (trusted), sign the
            // request so they can verify the sender.
            let signing = peer_pubkey.as_ref().map(|_| state.identity.clone());
            match run_peer(name, addr, &req, signing.as_ref()).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    tracing::warn!(peer = %name, error = %e, "peer direct failed");

                    // Fall back to relay if the peer has a pubkey AND we're
                    // currently registered with a relay. Otherwise local.
                    let relay_ready =
                        matches!(state.relay.current_state().await, RelayState::Registered);

                    match (peer_pubkey, relay_ready) {
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

/// QUIC peer transport, request-side. Opens one bidi stream, writes a
/// JSON header line + the serialized RunRequest, half-closes send, and
/// streams the response chunks back as the daemon's standard
/// `text/plain` response.
///
/// Wire format (v0):
///   line 1: `{"kind":"run","version":0}\n`
///   line 2: serialized `RunRequest` JSON + `\n`
///   (send-side closed)
///   chunks of `text/plain` until EOF (recv-side closed)
async fn run_peer_via_quic(
    quic: &Arc<transport::PeerEndpoint>,
    peer_quic_addr: SocketAddr,
    req: &RunRequest,
) -> Result<Response> {
    let conn = quic
        .connect(peer_quic_addr)
        .await
        .context("quic: connect to peer")?;
    let (mut send, mut recv) = conn.open_bi().await.context("quic: open bi stream")?;

    let header = b"{\"kind\":\"run\",\"version\":0}\n";
    send.write_all(header).await.context("quic: write header")?;
    let body = serde_json::to_vec(req).context("serializing run request")?;
    send.write_all(&body).await.context("quic: write body")?;
    send.write_all(b"\n").await.context("quic: terminator")?;
    send.finish().context("quic: finish send")?;

    // Drain the response stream into a channel that becomes the
    // axum response body. Bounded chunk size keeps memory tight on
    // long generations.
    let (tx, rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(32);
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match recv.read(&mut buf).await {
                Ok(Some(n)) if n > 0 => {
                    if tx
                        .send(Ok(bytes::Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(_) => break,
                Err(e) => {
                    let _ = tx
                        .send(Err(std::io::Error::other(format!("quic recv: {e}"))))
                        .await;
                    break;
                }
            }
        }
        // Holding `conn` until the stream finishes keeps the connection
        // alive for the duration of the response.
        drop(conn);
    });

    Ok(Response::builder()
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-unhosted-served-by", "peer:quic")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .expect("valid response"))
}

/// QUIC peer transport, server-side. Dispatches each inbound stream by
/// its JSON header `kind` field. v0.0.4 + v0.0.5 only handle "run".
async fn quic_inbound_handler(conn: quinn::Connection, state: NodeState) {
    let remote = conn.remote_address();
    loop {
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(quinn::ConnectionError::ApplicationClosed(_))
            | Err(quinn::ConnectionError::LocallyClosed)
            | Err(quinn::ConnectionError::ConnectionClosed(_)) => return,
            Err(e) => {
                tracing::debug!(%remote, error = %e, "quic: stream end");
                return;
            }
        };

        // Read header line. Cap at 4KB so a malicious peer can't make
        // us buffer arbitrary data before the LF.
        let mut header = Vec::with_capacity(128);
        let mut byte = [0u8; 1];
        let header_ok = loop {
            match recv.read(&mut byte).await {
                Ok(Some(_)) => {
                    if byte[0] == b'\n' {
                        break true;
                    }
                    header.push(byte[0]);
                    if header.len() > 4096 {
                        break false;
                    }
                }
                _ => break false,
            }
        };
        if !header_ok {
            let _ = send.finish();
            continue;
        }

        let kind = serde_json::from_slice::<serde_json::Value>(&header)
            .ok()
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string))
            .unwrap_or_default();

        match kind.as_str() {
            "run" => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_quic_run(&mut send, &mut recv, &state).await {
                        tracing::debug!(error = %e, "quic: run stream errored");
                    }
                });
            }
            other => {
                tracing::debug!(%remote, kind = %other, "quic: unknown stream kind");
                let _ = send.finish();
            }
        }
    }
}

async fn handle_quic_run(
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    state: &NodeState,
) -> Result<()> {
    // Read the request body until end-of-stream (capped at 256KB so a
    // bug or hostile peer can't exhaust memory).
    let body = recv
        .read_to_end(256 * 1024)
        .await
        .context("quic: read body")?;
    let req: RunRequest = serde_json::from_slice(&body).context("quic: parse run req")?;

    // Reuse the local-inference path. Build a fake axum Response and
    // stream its body into the QUIC send stream chunk-by-chunk.
    let resp = match run_local(&state.node, req).await {
        Ok(r) => r,
        Err(status) => {
            let msg = format!("local run failed: {status}");
            let _ = send.write_all(msg.as_bytes()).await;
            let _ = send.finish();
            return Ok(());
        }
    };

    let mut stream = resp.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                if send.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "quic: local stream errored");
                break;
            }
        }
    }
    let _ = send.finish();
    Ok(())
}

/// Serve a request locally by proxying to whichever model runtime is
/// actually reachable right now — the configured upstream first, then
/// ollama / lm studio / llama-server as fallbacks. Parses the SSE
/// stream from the chosen backend and emits plain-text tokens.
async fn run_local(node: &Node, req: RunRequest) -> Result<Response, StatusCode> {
    let Some(live) = upstream::select_live(&node.llama_server_url).await else {
        return Ok(upstream_offline_response(&node.llama_server_url));
    };
    let upstream_url = format!("{}/v1/chat/completions", live.url);
    let client = reqwest::Client::new();

    let upstream_resp = client
        .post(&upstream_url)
        .json(&ChatRequest {
            model: live.model.as_deref(),
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
            tracing::error!(error = %e, %upstream_url, "upstream request failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !upstream_resp.status().is_success() {
        tracing::error!(status = %upstream_resp.status(), %upstream_url, "upstream returned non-success");
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

#[cfg(test)]
mod lib_tests {
    use super::*;
    use std::net::SocketAddr;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn keep_reachable_addr_over_link_local() {
        // The bug this fix targets: a LAN-private addr should NOT
        // be replaced by an IPv6 link-local mDNS broadcast.
        let lan = sa("192.168.1.42:7777");
        let link_local = sa("[fe80::1]:7777");
        assert!(!discovered_is_better(&lan, &link_local));
    }

    #[test]
    fn keep_loopback_over_link_local() {
        // The synthetic two-daemon test setup hits this exact
        // case: 127.0.0.1:7778 should not be swapped for fe80:*.
        let loop_back = sa("127.0.0.1:7778");
        let link_local = sa("[fe80::abcd]:7778");
        assert!(!discovered_is_better(&loop_back, &link_local));
    }

    #[test]
    fn keep_lan_over_public_v4() {
        // A peer paired by LAN IP should keep that addr even if
        // mDNS somehow advertises a public IPv4.
        let lan = sa("192.168.1.42:7777");
        let public_v4 = sa("8.8.8.8:7777");
        assert!(!discovered_is_better(&lan, &public_v4));
    }

    #[test]
    fn swap_link_local_for_lan() {
        // The other direction: if the registry already has a
        // broken link-local (from a prior mDNS-only discovery)
        // and the peer comes back at a proper LAN address, we
        // should restore.
        let link_local = sa("[fe80::1]:7777");
        let lan = sa("192.168.1.42:7777");
        assert!(discovered_is_better(&link_local, &lan));
    }

    #[test]
    fn swap_for_same_rank_different_addr() {
        // Two LAN-private addrs of the same rank — the IP rotated
        // (router reboot, e.g.). Accept the swap. This is the
        // case the original auto-restore was written for.
        let a = sa("192.168.1.42:7777");
        let b = sa("192.168.1.99:7777");
        assert!(discovered_is_better(&a, &b));
    }
}
