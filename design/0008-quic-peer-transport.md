# 0008 — QUIC transport for encrypted peer-to-peer

**Status:** Accepted — diagnostic landed in v0.0.4; `/v1/run` migration follows.
**Captured:** 2026-05-11
**Targets:** Closes the last big v0.0.x gap: trusted-mode requests between paired peers are *signed* but currently transit in cleartext on the LAN.

## Why now

The hardening pass (ADR 0007) closed the four bear-trap attack surfaces (LAN auth, replay, code brute-force, relay DoS) but explicitly punted on transport encryption: trusted peers verify each other's signatures, but the request body itself is in the clear over HTTP. Anyone passive on the wire reads prompts. That's the next gap.

## What was considered, and what didn't fit

We had three candidate architectures:

1. **QUIC + TLS 1.3 with self-signed certs derived from Ed25519 identities.** QUIC mandates TLS 1.3, so encryption + forward secrecy come built in. We bind peer identity to the cert by embedding the existing Ed25519 pubkey in the SubjectPublicKeyInfo (TLS 1.3 supports Ed25519 directly via RFC 8410). The cert verifier accepts the connection iff the SPKI matches a pubkey in the trusted-peer registry.
2. **QUIC over a Noise-framed channel.** Layered: QUIC for transport, Noise for app-layer key agreement. Matches Wireguard/Tailscale's approach in some sense.
3. **Custom TCP + Noise.** Skip QUIC entirely.

We picked (1) for v0.0.4. The reasoning:

- QUIC's TLS 1.3 with Ed25519 + ChaCha20-Poly1305 is the encryption. Adding Noise on top is double-encryption without a meaningful security upgrade in our threat model.
- The identity binding (Ed25519 SPKI in cert → registry check) replaces the X.509 PKI machinery we don't have. No CAs, no DNS-based trust, no expiry chasing.
- QUIC over UDP fits naturally with the hole-punching work from ADR 0005: once the relay-coordinated UDP punch succeeds, we can run QUIC over the punched socket. (This isn't wired in yet — the punch is still diagnostic.) The CGNAT fallback path stays on the relay's WebSocket as today.
- This is what Iroh / Cloudflare's tunnel auth / a growing fraction of the Rust networking ecosystem already does. Mature libraries (`quinn` + `rustls`).

**Not chosen:**

- Noise-on-top — see above. We can revisit if we want post-quantum hybrid key agreement, but that's a 2027+ concern.
- TCP+Noise — loses QUIC's stream multiplexing, native 0-RTT resumption, and the path-validation primitives we'll want for migrating across NAT remappings.
- mTLS with a real CA — would need cert provisioning. The "Ed25519 *is* the identity" model is simpler.

## What landed in v0.0.4

A new `transport` module (`crates/unhosted-core/src/transport.rs`):

- **`cert_from_identity(&Identity)`** — builds a self-signed X.509 cert from the existing Ed25519 keypair via `rcgen`. The Ed25519 pubkey appears in the SPKI as raw bytes (RFC 8410 encoding). Same identity → same cert across restarts.
- **`PeerKeyVerifier`** — rustls verifier (both `ServerCertVerifier` and `ClientCertVerifier`). Extracts the 32 bytes of Ed25519 SPKI from the peer's leaf cert, checks the registry for a paired peer with the matching base64'd pubkey, accepts or rejects. Skips X.509 chain validation entirely — we don't have a CA.
- **`PeerEndpoint`** — `quinn` endpoint that both serves and dials. Bound to `<bind-ip>:<http-port+1>` UDP. Default daemon: HTTP on `127.0.0.1:7777`, QUIC on `127.0.0.1:7778`.
- **`ping_responder`** — single-stream pong handler installed by the daemon.

Wired into the daemon startup: best-effort bind, log+continue on failure. New diagnostic endpoint `POST /v1/quic/ping { "peer": "<name>" }` dials the peer's QUIC endpoint, completes the handshake (proves cert verification works), runs a single bidi stream round-trip, returns RTT. CLI shortcut: `unhosted quic-ping <peer>`.

Three tests cover the load-bearing pieces:
- Cert SPKI round-trips through the parser to the original 32-byte key.
- Two paired peers complete a ping in the same process.
- A stranger (each side's registry omits the other) fails handshake.

## What's deliberately not in v0.0.4

- **`/v1/run` does not yet route over QUIC.** Inference traffic still rides HTTP + the signed-header path. Reason: we want the new transport observable on real LANs before we move the load-bearing path. Migration to QUIC for run requests happens in v0.0.5 once we have field-test data that connections survive long-running streams.
- **No QUIC-over-hole-punch yet.** The transport binds a fixed UDP port; using the hole-punched socket from `relay_client::try_punch` requires lifting the endpoint to accept a pre-bound UDP socket. Straightforward but distinct work — depends on a real two-NAT test environment, not solvable from a single machine.
- **No QUIC fallback to relay-routed run requests.** The relay WebSocket path (ADR 0005) still carries inference for symmetric-NAT pairs. QUIC will eventually live on top of the punched socket *or* defer to relay; we won't tunnel QUIC over WebSockets.
- **No model attestation / per-model auth.** All paired peers can use any model the responder serves. Per-pair allow-lists are a v0.2.0 concern.

## Migration plan for `/v1/run`

Once two-machine testing confirms the v0.0.4 ping works across the network shapes that matter (LAN, hole-punched, relay-fallback), the migration is:

1. Add a `request` stream protocol on top of `PeerEndpoint`: client sends a JSON header + body on one bidi stream; server streams response chunks back on the same stream.
2. In the daemon's existing `run_peer` path, when the peer is a trusted (pubkey-bearing) registry entry, attempt QUIC first with a short timeout. Fall back to HTTP-signed path on any QUIC failure.
3. After one ship of co-existence, the HTTP-signed path becomes legacy-only for v0.0.x peers; current peers always use QUIC.

This staging keeps the network observable: if a real network breaks QUIC, the HTTP path is right there to roll back to without a release.

## Threat model after this lands

(Updates the ADR 0007 table.)

| Attack | Before v0.0.4 | After /v1/run on QUIC |
|---|---|---|
| Passive sniff prompts on LAN | trivial | requires breaking TLS 1.3 + ChaCha20-Poly1305 |
| MITM modify request body | mitigated by signature, but tampering visible to operator | rejected at transport (TLS integrity) before reaching app layer |
| Impersonate a paired peer | requires stealing their `identity.toml` | unchanged — same root identity |
| DoS the QUIC endpoint | n/a | quinn enforces per-connection limits; we still need a real connection cap (followup) |
| Inject a malicious peer cert | n/a | rejected by `PeerKeyVerifier` — SPKI must match registry |

## Open questions deferred

- **Per-peer connection caps on the QUIC endpoint.** quinn has knobs; we haven't set them. Followup.
- **Connection migration when a peer's IP changes.** quinn supports it; we haven't tested.
- **TLS 1.3 0-RTT.** Tempting for short requests; brings replay concerns. Defer.
- **Public-mode trust radius.** Strangers don't get into the registry, so they can't establish a QUIC connection. The public-mode v0.3.0 design will need a separate trust mechanism (signed attestation or stake-based, per ADR 0001).
