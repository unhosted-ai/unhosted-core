//! Core engine for Unhosted.
//!
//! - v0.0.1: single-machine inference — proxies to a local llama-server.
//! - v0.0.2 (in progress): multi-node — the daemon round-robins requests
//!   across `Local` + configured peers, with loop prevention and per-request
//!   fallback to local on peer failure.
//!
//! Peer protocol is the same HTTP API the CLI uses (`POST /v1/run`), so a
//! peer is just another `unhosted serve` process. No new transport.

pub mod peer;
pub mod router;
mod web;

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

#[derive(Clone, Debug)]
pub struct Node {
    pub addr: SocketAddr,
    pub llama_server_url: String,
    /// Peers reachable from this node. Loaded from the peer registry at
    /// startup; empty means single-node operation (v0.0.1 behavior).
    pub peers: Vec<Peer>,
}

impl Node {
    pub fn local() -> Self {
        Self {
            addr: DEFAULT_NODE_ADDR.parse().expect("valid default addr"),
            llama_server_url: std::env::var("UNHOSTED_LLAMA_SERVER_URL")
                .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_URL.to_string()),
            peers: Vec::new(),
        }
    }
}

/// Runtime state shared by all request handlers.
#[derive(Clone)]
struct NodeState {
    node: Arc<Node>,
    router: Arc<RouteRouter>,
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
struct UpstreamCompletion<'a> {
    prompt: &'a str,
    n_predict: u32,
    stream: bool,
    cache_prompt: bool,
}

pub async fn serve(node: Node) -> Result<()> {
    let router = Arc::new(RouteRouter::new(&node.peers));
    let state = NodeState {
        node: Arc::new(node.clone()),
        router: router.clone(),
    };

    let api = AxumRouter::new()
        .route("/health", get(health))
        .route("/v1/run", post(run_handler))
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
    let upstream_url = format!("{}/completion", node.llama_server_url);
    let client = reqwest::Client::new();

    let upstream_resp = client
        .post(&upstream_url)
        .json(&UpstreamCompletion {
            prompt: &req.prompt,
            n_predict: req.max_tokens,
            stream: true,
            cache_prompt: false,
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
        if let Some(content) = json.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty()
                && tx
                    .send(Ok(bytes::Bytes::copy_from_slice(content.as_bytes())))
                    .await
                    .is_err()
            {
                return false;
            }
        }
        if json.get("stop").and_then(|v| v.as_bool()) == Some(true) {
            return false;
        }
    }
    true
}
