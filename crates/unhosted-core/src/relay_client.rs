//! Daemon-side relay client.
//!
//! Opens a WebSocket to the configured `unhosted-relay`, registers using the
//! local Ed25519 identity (signs a server-issued challenge), and stays
//! connected. Once the daemon-side router learns to use it (next sprint),
//! outbound peer requests for unreachable peers route through this socket;
//! inbound payloads delivered by the relay get executed locally.
//!
//! For v0.1.0-beta-alpha this module is connection-only: it proves the
//! relay is reachable and we are registered. Routing integration follows.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::Identity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayState {
    Disabled,
    Connecting,
    Registered,
    Error(String),
}

#[derive(Clone)]
pub struct RelayClient {
    state: Arc<Mutex<RelayState>>,
}

impl RelayClient {
    pub fn disabled() -> Self {
        Self {
            state: Arc::new(Mutex::new(RelayState::Disabled)),
        }
    }

    /// Connect to `relay_url` and stay registered. Reconnects with backoff
    /// on disconnect. Returns immediately; the connection runs in a
    /// background task.
    pub fn spawn(identity: Identity, relay_url: String) -> Self {
        let state = Arc::new(Mutex::new(RelayState::Connecting));
        let state_for_task = state.clone();

        tokio::spawn(async move {
            let mut backoff_secs: u64 = 1;
            loop {
                match run_once(&identity, &relay_url, state_for_task.clone()).await {
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
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        });

        Self { state }
    }

    pub async fn current_state(&self) -> RelayState {
        self.state.lock().await.clone()
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
    #[allow(dead_code)] // wired up in the next sprint
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
    #[allow(dead_code)] // wired up in the next sprint
    Inbound {
        from_pubkey: String,
        payload: String,
    },
    Error {
        code: String,
        message: String,
    },
}

async fn run_once(
    identity: &Identity,
    relay_url: &str,
    state: Arc<Mutex<RelayState>>,
) -> Result<()> {
    {
        let mut s = state.lock().await;
        *s = RelayState::Connecting;
    }

    let tunnel_url = format!("{}/v1/tunnel", relay_url.trim_end_matches('/'));
    tracing::info!(url = %tunnel_url, "connecting to relay");

    let (mut socket, _resp) = tokio_tungstenite::connect_async(&tunnel_url)
        .await
        .with_context(|| format!("dialing {tunnel_url}"))?;

    // Wait for Hello.
    let hello_msg = socket
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("relay closed before hello"))?
        .context("reading hello")?;
    let hello_text = match hello_msg {
        Message::Text(t) => t.to_string(),
        _ => anyhow::bail!("relay sent non-text hello frame"),
    };
    let parsed: ServerMessage = serde_json::from_str(&hello_text).context("parsing relay hello")?;
    let challenge = match parsed {
        ServerMessage::Hello { challenge, .. } => challenge,
        ServerMessage::Error { code, message } => {
            anyhow::bail!("relay rejected with {code}: {message}")
        }
        _ => anyhow::bail!("relay sent unexpected first frame"),
    };

    // Sign the challenge with our identity.
    let signature_b64 = identity.sign(challenge.as_bytes());
    let pubkey_b64 = identity.public_b64();

    let register = ClientMessage::Register {
        pubkey: &pubkey_b64,
        challenge: &challenge,
        signature: &signature_b64,
    };
    let register_text = serde_json::to_string(&register)?;
    socket
        .send(Message::Text(register_text.into()))
        .await
        .context("sending register frame")?;

    // Expect Registered.
    let confirm_msg = socket
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("relay closed during register"))?
        .context("reading register confirmation")?;
    let confirm_text = match confirm_msg {
        Message::Text(t) => t.to_string(),
        _ => anyhow::bail!("relay sent non-text confirmation frame"),
    };
    let parsed: ServerMessage =
        serde_json::from_str(&confirm_text).context("parsing relay confirmation")?;
    match parsed {
        ServerMessage::Registered { .. } => {}
        ServerMessage::Error { code, message } => {
            anyhow::bail!("relay refused registration ({code}): {message}")
        }
        _ => anyhow::bail!("relay sent unexpected confirmation frame"),
    }

    {
        let mut s = state.lock().await;
        *s = RelayState::Registered;
    }
    tracing::info!(relay = %relay_url, pubkey = %pubkey_b64, "registered with relay");

    // Until routing integration ships in the next sprint, all we do here is
    // drain inbound frames + reply to pings so the session stays alive.
    while let Some(frame) = socket.next().await {
        match frame {
            Ok(Message::Ping(payload)) => {
                let _ = socket.send(Message::Pong(payload)).await;
            }
            Ok(Message::Text(t)) => {
                let parsed: Result<ServerMessage, _> = serde_json::from_str(&t);
                match parsed {
                    Ok(ServerMessage::Inbound { from_pubkey, .. }) => {
                        tracing::debug!(
                            from = %from_pubkey,
                            "relay inbound payload received (routing pending)"
                        );
                        // TODO(next sprint): decode payload, dispatch as a
                        // local /v1/run, stream the response back over the
                        // socket as Forward messages.
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

    Ok(())
}

// Tests for the wire protocol live in the relay binary; this module is
// thin glue + a state machine that's best exercised end-to-end.
