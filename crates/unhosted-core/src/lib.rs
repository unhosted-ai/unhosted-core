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
pub mod peer;
pub mod router;
mod web;

pub use discovery::{default_node_name, DiscoveredPeer, Discovery};
pub use peer::{Peer, PeerRegistry};
pub use router::{Router as RouteRouter, Target};

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
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
}

impl Node {
    pub fn local() -> Self {
        Self {
            addr: DEFAULT_NODE_ADDR.parse().expect("valid default addr"),
            llama_server_url: std::env::var("UNHOSTED_LLAMA_SERVER_URL")
                .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_URL.to_string()),
            peers: Vec::new(),
            name: default_node_name(),
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
    let discovery = match Discovery::start(&node.name, node.addr, env!("CARGO_PKG_VERSION")) {
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

    let state = NodeState {
        node: Arc::new(node.clone()),
        router: router.clone(),
        registry,
        discovery,
    };

    let api = AxumRouter::new()
        .route("/health", get(health))
        .route("/v1/run", post(run_handler))
        .route("/v1/status", get(status_handler))
        .route("/v1/peers", post(pair_handler))
        .route("/v1/peers/{name}", axum::routing::delete(unpair_handler))
        .with_state(state);

    let app = api
        .route("/", get(web::serve_index))
        .fallback(web::serve_static);

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

#[derive(Serialize)]
struct StatusResponse {
    node: NodeStatus,
    upstream: UpstreamStatus,
    peers: Vec<PeerStatus>,
    routing: RoutingStatus,
    discovered: Vec<DiscoveredPeer>,
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
}

#[derive(Serialize)]
struct RoutingStatus {
    targets: usize,
    mode: &'static str,
}

async fn status_handler(State(state): State<NodeState>) -> axum::Json<StatusResponse> {
    let upstream_url = state.node.llama_server_url.clone();
    let (reachable, model) = probe_upstream(&upstream_url).await;

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
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut discovered = state
        .discovery
        .as_ref()
        .map(|d| d.snapshot())
        .unwrap_or_default();

    // Hide peers that are already in the registry — we only want to show
    // *unpaired* discoveries in the UI.
    let paired_names: std::collections::HashSet<String> =
        peers.iter().map(|p| p.name.clone()).collect();
    discovered.retain(|d| !paired_names.contains(&d.name));

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
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(axum::Json(PairResponse { ok: true, peers }))
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
            match run_peer(name, addr, &req).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    tracing::warn!(
                        peer = %name,
                        error = %e,
                        "peer unreachable, falling back to local"
                    );
                    run_local(&state.node, req).await
                }
            }
        }
    }
}

/// Forward a request to a peer's `/v1/run`, streaming the response body back
/// to our caller unchanged. The peer is another `unhosted` daemon, so the
/// response is already text/plain token stream.
async fn run_peer(name: &str, addr: SocketAddr, req: &RunRequest) -> Result<Response> {
    let url = format!("http://{addr}/v1/run");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(anyhow::Error::from)?;

    let upstream_resp = client
        .post(&url)
        .header(FORWARDED_HEADER, "1")
        .json(req)
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
