//! Core engine for Unhosted: a local node that proxies inference requests to
//! a llama.cpp `llama-server` instance and streams tokens back to the caller.
//!
//! v0.0.1 is single-machine only. Multi-node orchestration arrives in v0.0.2.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// Default upstream llama-server URL when no override is configured.
pub const DEFAULT_LLAMA_SERVER_URL: &str = "http://127.0.0.1:8080";

/// Default address the local Unhosted node listens on.
pub const DEFAULT_NODE_ADDR: &str = "127.0.0.1:7777";

#[derive(Clone, Debug)]
pub struct Node {
    pub addr: SocketAddr,
    pub llama_server_url: String,
}

impl Node {
    pub fn local() -> Self {
        Self {
            addr: DEFAULT_NODE_ADDR.parse().expect("valid default addr"),
            llama_server_url: std::env::var("UNHOSTED_LLAMA_SERVER_URL")
                .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_URL.to_string()),
        }
    }
}

#[derive(Deserialize, Debug)]
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
    let state = Arc::new(node.clone());
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/run", post(run_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(node.addr).await?;
    tracing::info!(addr = %node.addr, upstream = %node.llama_server_url, "unhosted node listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn run_handler(
    State(node): State<Arc<Node>>,
    Json(req): Json<RunRequest>,
) -> Result<Response, StatusCode> {
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

            // SSE events are separated by "\n\n"; each carries one or more
            // "data: <payload>" lines. We accumulate, split, and forward the
            // `content` field of each JSON payload as plain text.
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
