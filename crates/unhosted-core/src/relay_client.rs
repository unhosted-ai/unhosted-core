//! Daemon-side relay client.
//!
//! Opens a WebSocket to the configured `unhosted-relay`, registers using
//! the local Ed25519 identity (signs a server-issued challenge), then runs
//! as a multiplexed request/response transport between paired peers.
//!
//! What this module owns:
//! - the WebSocket lifecycle (connect, register, reconnect with backoff)
//! - protocol framing on top of the relay's `forward` envelope:
//!   `{ kind: "req_start" | "resp_chunk" | "resp_end" | "err", id, ... }`
//! - matching response chunks back to outstanding outbound requests by id
//! - lifting inbound requests up to an `InboundRequest` channel for the
//!   daemon to dispatch (the daemon owns the `/v1/run` logic; we just
//!   wire bytes back and forth)
//!
//! What this module does NOT own:
//! - the model. Inbound requests are handed off to the daemon's run
//!   handler via the InboundRequest mpsc receiver returned at spawn time.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::Identity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayState {
    Disabled,
    Connecting,
    Registered,
    Error(String),
}

/// A response chunk delivered from a relay `Inbound` whose payload was a
/// `resp_chunk` / `resp_end` / `err` frame for an outbound request we sent.
#[derive(Debug)]
pub enum ResponseEvent {
    Chunk(Bytes),
    End,
    Error(String),
}

/// An inbound request from a peer, lifted up for the daemon to dispatch.
/// `response_tx` is how the daemon streams chunks back; closing it (drop)
/// signals end-of-response.
pub struct InboundRequest {
    pub from_pubkey: String,
    pub req_id: String,
    pub body: serde_json::Value,
    pub response_tx: mpsc::UnboundedSender<ResponseEvent>,
}

#[derive(Clone)]
pub struct RelayClient {
    state: Arc<Mutex<RelayState>>,
    /// `None` until the websocket is connected + registered. After that,
    /// outbound payloads go through this channel to the writer task.
    out_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>>,
    pending: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<ResponseEvent>>>>,
}

impl RelayClient {
    pub fn disabled() -> Self {
        Self {
            state: Arc::new(Mutex::new(RelayState::Disabled)),
            out_tx: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Connect, stay registered, reconnect on drop. Returns the client
    /// handle plus an `InboundRequest` stream the daemon must consume.
    pub fn spawn(
        identity: Identity,
        relay_url: String,
    ) -> (Self, mpsc::UnboundedReceiver<InboundRequest>) {
        let state = Arc::new(Mutex::new(RelayState::Connecting));
        let out_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>> = Arc::new(Mutex::new(None));
        let pending: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<ResponseEvent>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel::<InboundRequest>();

        let state_for_task = state.clone();
        let out_for_task = out_tx.clone();
        let pending_for_task = pending.clone();

        tokio::spawn(async move {
            let mut backoff_secs: u64 = 1;
            loop {
                let res = run_once(
                    &identity,
                    &relay_url,
                    state_for_task.clone(),
                    out_for_task.clone(),
                    pending_for_task.clone(),
                    inbound_tx.clone(),
                )
                .await;

                match res {
                    Ok(()) => {
                        tracing::warn!(relay = %relay_url, "relay session closed cleanly; reconnecting");
                        backoff_secs = 1;
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        tracing::warn!(relay = %relay_url, error = %msg, "relay session failed");
                        {
                            let mut s = state_for_task.lock().await;
                            *s = RelayState::Error(msg);
                        }
                    }
                }

                // Disconnected: drop the out_tx so any in-flight callers see EOF.
                {
                    let mut o = out_for_task.lock().await;
                    *o = None;
                }
                {
                    let mut p = pending_for_task.lock().await;
                    for (_, tx) in p.drain() {
                        let _ = tx.send(ResponseEvent::Error("relay disconnected".into()));
                    }
                }

                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        });

        (
            Self {
                state,
                out_tx,
                pending,
            },
            inbound_rx,
        )
    }

    pub async fn current_state(&self) -> RelayState {
        self.state.lock().await.clone()
    }

    /// Send an outbound request to `peer_pubkey` via the relay. Returns a
    /// receiver of `ResponseEvent` items (`Chunk` … `End` | `Error`).
    /// Errors immediately if the relay isn't registered.
    pub async fn call(
        &self,
        peer_pubkey: &str,
        body: serde_json::Value,
    ) -> Result<mpsc::UnboundedReceiver<ResponseEvent>> {
        let out = {
            let guard = self.out_tx.lock().await;
            guard
                .clone()
                .ok_or_else(|| anyhow::anyhow!("relay not connected"))?
        };

        let req_id = uuid_simple();
        let (resp_tx, resp_rx) = mpsc::unbounded_channel::<ResponseEvent>();
        {
            let mut p = self.pending.lock().await;
            p.insert(req_id.clone(), resp_tx);
        }

        let payload = RelayPayload::ReqStart {
            id: req_id.clone(),
            body,
        };
        let payload_str = serde_json::to_string(&payload)?;
        let outer = ClientMessage::Forward {
            peer_pubkey,
            payload: &payload_str,
        };
        let frame = serde_json::to_string(&outer)?;
        out.send(Message::Text(frame.into()))
            .map_err(|_| anyhow::anyhow!("relay writer is gone"))?;

        Ok(resp_rx)
    }
}

// ---------------------------------------------------------------- protocol

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage<'a> {
    Register {
        pubkey: &'a str,
        challenge: &'a str,
        signature: &'a str,
    },
    Forward {
        peer_pubkey: &'a str,
        payload: &'a str,
    },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Hello {
        challenge: String,
        #[allow(dead_code)]
        protocol: u8,
    },
    Registered {
        #[allow(dead_code)]
        pubkey: String,
    },
    Inbound {
        from_pubkey: String,
        payload: String,
    },
    Error {
        code: String,
        message: String,
    },
}

/// Application-layer envelope carried inside a relay `forward.payload`.
/// JSON-encoded. Keeps every multiplexed request distinguishable by `id`.
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RelayPayload {
    ReqStart {
        id: String,
        body: serde_json::Value,
    },
    RespChunk {
        id: String,
        data: String, // text chunk; preserves the daemon's text/plain stream
    },
    RespEnd {
        id: String,
    },
    Err {
        id: String,
        msg: String,
    },
}

// ---------------------------------------------------------------- session

#[allow(clippy::too_many_arguments)]
async fn run_once(
    identity: &Identity,
    relay_url: &str,
    state: Arc<Mutex<RelayState>>,
    out_slot: Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>>,
    pending: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<ResponseEvent>>>>,
    inbound_tx: mpsc::UnboundedSender<InboundRequest>,
) -> Result<()> {
    {
        let mut s = state.lock().await;
        *s = RelayState::Connecting;
    }

    let tunnel_url = format!("{}/v1/tunnel", relay_url.trim_end_matches('/'));
    tracing::info!(url = %tunnel_url, "connecting to relay");

    let (socket, _resp) = tokio_tungstenite::connect_async(&tunnel_url)
        .await
        .with_context(|| format!("dialing {tunnel_url}"))?;

    let (mut writer, mut reader) = socket.split();

    // Hello.
    let hello = next_text(&mut reader).await.context("reading hello")?;
    let parsed: ServerMessage = serde_json::from_str(&hello).context("parsing hello")?;
    let challenge = match parsed {
        ServerMessage::Hello { challenge, .. } => challenge,
        ServerMessage::Error { code, message } => {
            anyhow::bail!("relay rejected with {code}: {message}")
        }
        _ => anyhow::bail!("relay sent unexpected first frame"),
    };

    // Register.
    let sig = identity.sign(challenge.as_bytes());
    let pubkey = identity.public_b64();
    let register = serde_json::to_string(&ClientMessage::Register {
        pubkey: &pubkey,
        challenge: &challenge,
        signature: &sig,
    })?;
    writer.send(Message::Text(register.into())).await?;

    let confirm = next_text(&mut reader)
        .await
        .context("reading register confirm")?;
    let parsed: ServerMessage = serde_json::from_str(&confirm)?;
    match parsed {
        ServerMessage::Registered { .. } => {}
        ServerMessage::Error { code, message } => {
            anyhow::bail!("relay refused registration ({code}): {message}")
        }
        _ => anyhow::bail!("relay sent unexpected confirmation frame"),
    }

    // Multiplex: spawn a writer task driven by an mpsc.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    {
        let mut slot = out_slot.lock().await;
        *slot = Some(out_tx.clone());
    }
    {
        let mut s = state.lock().await;
        *s = RelayState::Registered;
    }
    tracing::info!(relay = %relay_url, pubkey = %pubkey, "registered with relay");

    let writer_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if writer.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop.
    while let Some(frame) = reader.next().await {
        match frame {
            Ok(Message::Ping(p)) => {
                let _ = out_tx.send(Message::Pong(p));
            }
            Ok(Message::Text(t)) => {
                let parsed: Result<ServerMessage, _> = serde_json::from_str(&t);
                match parsed {
                    Ok(ServerMessage::Inbound {
                        from_pubkey,
                        payload,
                    }) => {
                        handle_inbound_payload(
                            &from_pubkey,
                            &payload,
                            &pending,
                            &inbound_tx,
                            &out_tx,
                        )
                        .await;
                    }
                    Ok(ServerMessage::Error { code, message }) => {
                        tracing::warn!(%code, %message, "relay error frame");
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "relay socket error");
                break;
            }
        }
    }

    writer_task.abort();
    Ok(())
}

async fn handle_inbound_payload(
    from_pubkey: &str,
    payload: &str,
    pending: &Arc<Mutex<HashMap<String, mpsc::UnboundedSender<ResponseEvent>>>>,
    inbound_tx: &mpsc::UnboundedSender<InboundRequest>,
    out_tx: &mpsc::UnboundedSender<Message>,
) {
    let parsed: Result<RelayPayload, _> = serde_json::from_str(payload);
    let Ok(payload) = parsed else {
        tracing::debug!("malformed relay payload, ignoring");
        return;
    };

    match payload {
        RelayPayload::ReqStart { id, body } => {
            // Hand the inbound request up to the daemon. Build a response
            // channel; spawn a forwarder that turns ResponseEvents into
            // RespChunk/RespEnd/Err Forward messages back to the caller.
            let (resp_tx, mut resp_rx) = mpsc::unbounded_channel::<ResponseEvent>();
            let req = InboundRequest {
                from_pubkey: from_pubkey.to_string(),
                req_id: id.clone(),
                body,
                response_tx: resp_tx,
            };
            if inbound_tx.send(req).is_err() {
                tracing::warn!("daemon dropped inbound channel; ignoring relayed request");
                return;
            }

            let out_tx = out_tx.clone();
            let from = from_pubkey.to_string();
            tokio::spawn(async move {
                while let Some(event) = resp_rx.recv().await {
                    let p = match event {
                        ResponseEvent::Chunk(b) => RelayPayload::RespChunk {
                            id: id.clone(),
                            data: String::from_utf8_lossy(&b).into_owned(),
                        },
                        ResponseEvent::End => RelayPayload::RespEnd { id: id.clone() },
                        ResponseEvent::Error(msg) => RelayPayload::Err {
                            id: id.clone(),
                            msg,
                        },
                    };
                    let Ok(payload_str) = serde_json::to_string(&p) else {
                        continue;
                    };
                    let outer = ClientMessage::Forward {
                        peer_pubkey: &from,
                        payload: &payload_str,
                    };
                    let Ok(frame) = serde_json::to_string(&outer) else {
                        continue;
                    };
                    if out_tx.send(Message::Text(frame.into())).is_err() {
                        break;
                    }
                    if matches!(p, RelayPayload::RespEnd { .. } | RelayPayload::Err { .. }) {
                        break;
                    }
                }
            });
        }
        RelayPayload::RespChunk { id, data } => {
            let tx = {
                let p = pending.lock().await;
                p.get(&id).cloned()
            };
            if let Some(tx) = tx {
                let _ = tx.send(ResponseEvent::Chunk(Bytes::from(data.into_bytes())));
            }
        }
        RelayPayload::RespEnd { id } => {
            let tx = {
                let mut p = pending.lock().await;
                p.remove(&id)
            };
            if let Some(tx) = tx {
                let _ = tx.send(ResponseEvent::End);
            }
        }
        RelayPayload::Err { id, msg } => {
            let tx = {
                let mut p = pending.lock().await;
                p.remove(&id)
            };
            if let Some(tx) = tx {
                let _ = tx.send(ResponseEvent::Error(msg));
            }
        }
    }
}

async fn next_text(
    reader: &mut futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<String> {
    let msg = reader
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("websocket closed"))?
        .map_err(|e| anyhow::anyhow!("ws error: {e}"))?;
    match msg {
        Message::Text(t) => Ok(t.to_string()),
        _ => anyhow::bail!("expected text frame"),
    }
}

/// 22-char URL-safe random id. Avoids pulling in the `uuid` crate.
fn uuid_simple() -> String {
    use base64::Engine;
    let mut buf = [0u8; 16];
    rand::Rng::fill(&mut rand::thread_rng(), &mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}
