//! QUIC transport for encrypted peer-to-peer requests in trusted mode.
//!
//! What this module is:
//!
//! - A thin wrapper over `quinn` that lets two daemons exchange opaque
//!   byte streams over a TLS-1.3-encrypted QUIC connection.
//! - Identity-bound: every daemon presents a self-signed X.509 cert
//!   whose Subject Public Key Info embeds its existing Ed25519 pubkey.
//!   The peer verifier accepts the connection iff the cert's SPKI
//!   matches a pubkey in the local trusted-peer registry.
//!
//! What it is *not*:
//!
//! - A request router. This is the byte-pipe; `lib.rs` decides what to
//!   send over it (run requests, eventually).
//! - A migration layer. v0.0.4 keeps HTTP+signed-headers as the working
//!   peer-to-peer path; this transport runs in parallel and exposes a
//!   `/v1/quic/ping` diagnostic until we're confident it works on real
//!   networks. Migration of `/v1/run` happens in a follow-up.
//!
//! Why no separate Noise layer:
//!
//! QUIC mandates TLS 1.3, which gives us AEAD encryption + forward
//! secrecy already. Adding Noise on top would be double-encryption
//! without a meaningful security upgrade for our threat model. The
//! identity binding (Ed25519 → cert SPKI → registry check) replaces the
//! PKI machinery that TLS normally relies on. See ADR 0008.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ed25519_dalek::VerifyingKey;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::DigitallySignedStruct;

use crate::identity::Identity;
use crate::peer::PeerRegistry;

/// Application-layer protocol identifier for ALPN. QUIC requires one;
/// using a versioned string lets us evolve the framing later without
/// silently mis-interpreting old peers.
pub const ALPN: &[u8] = b"unhosted/0";

/// Self-signed X.509 cert + private key derived from a daemon's Ed25519
/// identity. The cert's signing key *is* the Ed25519 key — TLS 1.3
/// supports Ed25519 natively (RFC 8410).
pub struct PeerIdentityCert {
    pub cert: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    /// The Ed25519 public key embedded in the cert, raw 32 bytes.
    pub pubkey: [u8; 32],
}

/// Generate a self-signed cert from the daemon's existing Ed25519 key.
/// Same key on every run → same cert every run; the cert serial is
/// derived from the pubkey so identical daemons produce identical bytes.
pub fn cert_from_identity(identity: &Identity) -> Result<PeerIdentityCert> {
    let secret_bytes = identity.secret_bytes();
    let signing = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
    let pubkey = signing.verifying_key().to_bytes();

    // Build a PKCS#8 encoding of the Ed25519 private key, which rcgen
    // accepts via `KeyPair::from_pkcs8_der`. Ed25519's PKCS#8 wrapper
    // is the 16-byte fixed prefix below + the 32-byte secret.
    //
    //   SEQUENCE (4 bytes header + version + alg + key)
    //   30 2e 02 01 00 30 05 06 03 2b 65 70 04 22 04 20 <32 bytes>
    let mut pkcs8 = Vec::with_capacity(16 + 32);
    pkcs8.extend_from_slice(&[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ]);
    pkcs8.extend_from_slice(&secret_bytes);

    let pkcs8_der = rustls_pki_types::PrivatePkcs8KeyDer::from(pkcs8);
    let key_pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8_der, &rcgen::PKCS_ED25519)
        .context("rcgen: building Ed25519 key pair")?;

    let mut params = rcgen::CertificateParams::new(vec!["unhosted".to_string()])
        .context("rcgen: cert params")?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "unhosted-peer");
    // Long-lived cert. Identity rotation = generate a new Ed25519 key
    // and re-pair, not "rotate the TLS cert."
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(2099, 12, 31);

    let cert = params
        .self_signed(&key_pair)
        .context("rcgen: self-signing")?;
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("private key DER: {e}"))?;

    Ok(PeerIdentityCert {
        cert: cert_der,
        key: key_der,
        pubkey,
    })
}

/// Extract the raw Ed25519 public key bytes from a peer's leaf cert.
/// Looks for the SubjectPublicKeyInfo's BIT STRING containing the
/// 32-byte key. Returns `None` if the cert isn't Ed25519-shaped.
fn extract_ed25519_pubkey(cert: &CertificateDer<'_>) -> Option<[u8; 32]> {
    // ASN.1 walk just deep enough to locate the SPKI. Ed25519 SPKI is:
    //   SEQUENCE {
    //     SEQUENCE { OID 1.3.101.112 }            -- alg id "Ed25519"
    //     BIT STRING { 0x00, <32 raw pubkey> }
    //   }
    // The OID encodes as: 06 03 2b 65 70.
    let bytes = cert.as_ref();
    let needle: &[u8] = &[0x06, 0x03, 0x2b, 0x65, 0x70];
    let mut start = 0;
    while let Some(pos) = find_subseq(&bytes[start..], needle) {
        let abs = start + pos + needle.len();
        // After the OID we expect: BIT STRING tag (0x03), length, 0x00, then 32 bytes of key.
        if abs + 2 < bytes.len() {
            // Skip past the enclosing AlgorithmIdentifier SEQUENCE wrapper.
            // The next BIT STRING (tag 0x03) holds the key.
            if let Some(bs_pos) = bytes[abs..].iter().position(|&b| b == 0x03) {
                let bs_start = abs + bs_pos;
                if bs_start + 35 <= bytes.len()
                    && bytes[bs_start] == 0x03
                    && bytes[bs_start + 1] == 0x21
                    && bytes[bs_start + 2] == 0x00
                {
                    let mut out = [0u8; 32];
                    out.copy_from_slice(&bytes[bs_start + 3..bs_start + 35]);
                    return Some(out);
                }
            }
        }
        start = abs;
    }
    None
}

fn find_subseq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// rustls verifier that accepts any cert whose embedded Ed25519 pubkey
/// matches a peer in the trusted-peer registry. Skips X.509 PKI checks
/// entirely — we don't have a CA, the cert *is* the identity.
#[derive(Debug)]
struct PeerKeyVerifier {
    registry: Arc<std::sync::Mutex<PeerRegistry>>,
    /// Default signature algorithms quinn/rustls accept. Reused for
    /// both TLS 1.2 (won't happen — QUIC mandates 1.3) and 1.3 paths.
    schemes: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl PeerKeyVerifier {
    fn new(registry: Arc<std::sync::Mutex<PeerRegistry>>) -> Self {
        let provider = rustls::crypto::ring::default_provider();
        Self {
            registry,
            schemes: provider.signature_verification_algorithms,
        }
    }

    fn check_known_peer(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let Some(pk) = extract_ed25519_pubkey(cert) else {
            return Err(rustls::Error::General(
                "peer cert is not Ed25519 — refusing".into(),
            ));
        };
        // Sanity: the embedded key must round-trip through ed25519-dalek.
        let _ = VerifyingKey::from_bytes(&pk)
            .map_err(|e| rustls::Error::General(format!("peer cert Ed25519 key invalid: {e}")))?;

        let pk_b64 =
            base64::engine::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, pk);
        let trusted = self
            .registry
            .lock()
            .map(|r| {
                r.peers
                    .iter()
                    .any(|p| p.pubkey.as_deref() == Some(pk_b64.as_str()))
            })
            .unwrap_or(false);
        if !trusted {
            return Err(rustls::Error::General(format!(
                "peer cert key {pk_b64} is not a paired peer",
            )));
        }
        Ok(())
    }
}

impl ServerCertVerifier for PeerKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check_known_peer(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // QUIC mandates TLS 1.3; this should never be called. Accept
        // anything if it somehow is, since we already verified identity.
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

impl ClientCertVerifier for PeerKeyVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check_known_peer(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.supported_schemes()
    }
}

/// Build the rustls ServerConfig for a quinn endpoint. Requires client
/// auth — peers we don't know get rejected at handshake.
fn build_server_config(
    id_cert: &PeerIdentityCert,
    registry: Arc<std::sync::Mutex<PeerRegistry>>,
) -> Result<ServerConfig> {
    let verifier = Arc::new(PeerKeyVerifier::new(registry));
    let mut rustls_cfg =
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![id_cert.cert.clone()], id_cert.key.clone_key())
            .context("rustls server config")?;
    rustls_cfg.alpn_protocols = vec![ALPN.to_vec()];

    let quic_crypto = QuicServerConfig::try_from(rustls_cfg)
        .context("wrapping rustls server config for quinn")?;
    Ok(ServerConfig::with_crypto(Arc::new(quic_crypto)))
}

/// Build the rustls ClientConfig for a quinn endpoint.
fn build_client_config(
    id_cert: &PeerIdentityCert,
    registry: Arc<std::sync::Mutex<PeerRegistry>>,
) -> Result<ClientConfig> {
    let verifier = Arc::new(PeerKeyVerifier::new(registry));
    let mut rustls_cfg =
        rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(vec![id_cert.cert.clone()], id_cert.key.clone_key())
            .context("rustls client config")?;
    rustls_cfg.alpn_protocols = vec![ALPN.to_vec()];

    let quic_crypto = QuicClientConfig::try_from(rustls_cfg)
        .context("wrapping rustls client config for quinn")?;
    Ok(ClientConfig::new(Arc::new(quic_crypto)))
}

/// A combined client+server QUIC endpoint. Listens on `bind_addr`,
/// accepts inbound peer connections, and can dial out to other peers
/// on the same registry.
pub struct PeerEndpoint {
    endpoint: Endpoint,
    /// Cached for outbound dial config. Server config is baked into
    /// the endpoint itself.
    client_cfg: ClientConfig,
}

impl PeerEndpoint {
    pub fn bind(
        bind_addr: SocketAddr,
        identity: &Identity,
        registry: Arc<std::sync::Mutex<PeerRegistry>>,
    ) -> Result<Self> {
        // Install the ring crypto provider once. Idempotent across
        // multiple Endpoints in the same process.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let id_cert = cert_from_identity(identity)?;
        let server_cfg = build_server_config(&id_cert, registry.clone())?;
        let client_cfg = build_client_config(&id_cert, registry)?;

        let endpoint = Endpoint::server(server_cfg, bind_addr).context("binding quinn endpoint")?;
        Ok(Self {
            endpoint,
            client_cfg,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Loop until `cancel` fires, accepting peer connections and
    /// handing each one off to `handler`. The handler is responsible
    /// for protocol on top of the stream.
    pub async fn accept_loop<F, Fut>(&self, mut handler: F)
    where
        F: FnMut(quinn::Connection) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        while let Some(incoming) = self.endpoint.accept().await {
            match incoming.await {
                Ok(conn) => {
                    let fut = handler(conn);
                    tokio::spawn(fut);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "quic: handshake failed (likely unknown peer)");
                }
            }
        }
    }

    /// Dial a peer at `addr`, returning the live connection on success.
    /// The peer verifier rejects unknown pubkeys, so a successful return
    /// means the remote presented a cert keyed to a registered peer.
    pub async fn connect(&self, addr: SocketAddr) -> Result<quinn::Connection> {
        // `server_name` is the TLS SNI; we don't route by it, so any
        // value works. The peer verifier never inspects it.
        let conn = self
            .endpoint
            .connect_with(self.client_cfg.clone(), addr, "unhosted")?
            .await
            .context("quinn connect")?;
        Ok(conn)
    }

    /// Round-trip ping: open a bidi stream, send "ping\n<our pubkey>",
    /// read "pong\n<their pubkey>", return the elapsed time.
    /// Useful as the diagnostic check that the encrypted path works.
    pub async fn ping(&self, addr: SocketAddr, our_pubkey: &str) -> Result<Duration> {
        let conn = self.connect(addr).await?;
        let (mut send, mut recv) = conn.open_bi().await.context("open bi stream")?;
        let probe = format!("ping\n{our_pubkey}\n");
        let started = std::time::Instant::now();
        send.write_all(probe.as_bytes())
            .await
            .context("write ping")?;
        send.finish().context("finish ping stream")?;
        let mut buf = Vec::with_capacity(128);
        let _ = recv.read_to_end(1024).await.map(|b| buf = b);
        let elapsed = started.elapsed();
        conn.close(0u32.into(), b"done");
        if !buf.starts_with(b"pong\n") {
            anyhow::bail!(
                "expected 'pong' from peer, got: {:?}",
                String::from_utf8_lossy(&buf)
            );
        }
        Ok(elapsed)
    }

    pub fn handle(&self) -> Endpoint {
        self.endpoint.clone()
    }
}

/// Default handler: a small ping responder. Reads one stream's bytes,
/// writes "pong\n<our pubkey>" back, closes. Plumbed into the daemon's
/// QUIC listener as the v0.0.4 stand-in for real request routing.
pub async fn ping_responder(conn: quinn::Connection, our_pubkey: String) {
    let remote = conn.remote_address();
    loop {
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(quinn::ConnectionError::ApplicationClosed(_))
            | Err(quinn::ConnectionError::LocallyClosed) => return,
            Err(e) => {
                tracing::debug!(%remote, error = %e, "quic: stream end");
                return;
            }
        };
        let mut buf = Vec::with_capacity(128);
        if recv.read_to_end(1024).await.map(|b| buf = b).is_err() {
            return;
        }
        if buf.starts_with(b"ping\n") {
            let reply = format!("pong\n{our_pubkey}\n");
            let _ = send.write_all(reply.as_bytes()).await;
            let _ = send.finish();
            tracing::debug!(%remote, "quic: ping → pong");
        } else {
            tracing::debug!(%remote, "quic: unrecognized request, dropping");
            let _ = send.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::Peer;

    fn temp_identity(tag: &str) -> Identity {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "unhosted-quic-{tag}-{}-{stamp}",
            std::process::id()
        ));
        let path = dir.join("identity.toml");
        Identity::load_or_create_at(&path).unwrap()
    }

    #[test]
    fn cert_embeds_correct_pubkey() {
        let id = temp_identity("cert");
        let pic = cert_from_identity(&id).unwrap();
        let extracted = extract_ed25519_pubkey(&pic.cert).expect("Ed25519 SPKI present");
        assert_eq!(pic.pubkey, extracted, "cert SPKI matches identity key");
    }

    #[tokio::test]
    async fn two_peers_complete_ping_when_paired() {
        let id_a = temp_identity("a");
        let id_b = temp_identity("b");

        let pk_a = id_a.public_b64();
        let pk_b = id_b.public_b64();

        // Each side trusts the other in its registry.
        let reg_a = Arc::new(std::sync::Mutex::new(PeerRegistry {
            peers: vec![Peer {
                name: "b".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                priority: 5,
                models: vec![],
                pubkey: Some(pk_b.clone()),
            }],
        }));
        let reg_b = Arc::new(std::sync::Mutex::new(PeerRegistry {
            peers: vec![Peer {
                name: "a".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                priority: 5,
                models: vec![],
                pubkey: Some(pk_a.clone()),
            }],
        }));

        let ep_a = PeerEndpoint::bind("127.0.0.1:0".parse().unwrap(), &id_a, reg_a).unwrap();
        let ep_b = PeerEndpoint::bind("127.0.0.1:0".parse().unwrap(), &id_b, reg_b).unwrap();

        let addr_b = ep_b.local_addr().unwrap();
        let pk_b_for_responder = pk_b.clone();

        // Spawn B's accept loop with a ping responder.
        let endpoint_b = ep_b.handle();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint_b.accept().await {
                if let Ok(conn) = incoming.await {
                    let pk = pk_b_for_responder.clone();
                    tokio::spawn(async move {
                        ping_responder(conn, pk).await;
                    });
                }
            }
        });

        let rtt = ep_a.ping(addr_b, &pk_a).await.expect("ping succeeds");
        assert!(
            rtt.as_secs() < 2,
            "ping should complete quickly on loopback"
        );
    }

    #[tokio::test]
    async fn unknown_peer_handshake_rejected() {
        let id_a = temp_identity("a-strict");
        let id_stranger = temp_identity("stranger");

        // A's registry is empty: it trusts nobody.
        let reg_empty = Arc::new(std::sync::Mutex::new(PeerRegistry { peers: vec![] }));
        // Stranger's registry trusts A, but A doesn't trust stranger.
        let reg_stranger = Arc::new(std::sync::Mutex::new(PeerRegistry {
            peers: vec![Peer {
                name: "a".into(),
                addr: "127.0.0.1:0".parse().unwrap(),
                priority: 5,
                models: vec![],
                pubkey: Some(id_a.public_b64()),
            }],
        }));

        let ep_a = PeerEndpoint::bind("127.0.0.1:0".parse().unwrap(), &id_a, reg_empty).unwrap();
        let ep_s =
            PeerEndpoint::bind("127.0.0.1:0".parse().unwrap(), &id_stranger, reg_stranger).unwrap();

        let endpoint_a = ep_a.handle();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint_a.accept().await {
                let _ = incoming.await; // expect this to fail
            }
        });

        let addr_a = ep_a.local_addr().unwrap();
        let res = ep_s.ping(addr_a, &id_stranger.public_b64()).await;
        assert!(
            res.is_err(),
            "stranger must not be able to ping a non-pairing peer"
        );
    }
}
