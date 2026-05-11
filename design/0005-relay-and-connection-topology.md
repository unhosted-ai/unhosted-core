# 0005 — Connection topology for paired peers across the internet

**Status:** Accepted (relay binary scaffolded; daemon integration follows)
**Captured:** 2026-05-11
**Targets:** v0.1.0-beta lands the relay binary. v0.1.0-stable wires the daemon to use it, plus hole-punching when both peers have permissive NAT.

v0.1.0-alpha gave us pubkey-based pairing. Two paired peers know each other's keys but can only actually reach each other if they share a LAN or one has a public IP. This ADR is how we close that gap.

## The three connection attempts

For every request to a trusted peer, try in order:

1. **Direct** — connect to the peer's last-known IP:port. Lowest latency, no third party. Works when at least one peer has a public IP or is port-forwarded.
2. **Hole-punched** — both peers register their external IP:port with a coordinator, exchange those mappings, and send packets simultaneously. ~85% of home connections cooperate with this.
3. **Relay** — coordinator also acts as a TURN-style byte forwarder. Higher latency, eats bandwidth, but always works (symmetric NAT, CGNAT, locked-down corporate networks).

## The relay service

A small public service that does three jobs:

```
┌──────────────────────────────────────┐
│   unhosted-relay (Rust binary)       │
│                                      │
│   1. Rendezvous:                     │
│      WebSocket sessions keyed by     │
│      pubkey. Peers register on       │
│      connect, prove they hold the    │
│      private key.                    │
│                                      │
│   2. Hole-punch coordination:        │
│      Tell paired peers each other's  │
│      current external IP:port; both  │
│      send simultaneously.            │
│                                      │
│   3. Relay fallback:                 │
│      If 2 fails, forward encrypted   │
│      bytes between sessions.         │
│      Cannot decrypt — keys never     │
│      leave the peers.                │
└──────────────────────────────────────┘
```

Runs anywhere a Rust binary runs. Single port (TCP/443 with TLS). No database — all state in-memory, sessions live as long as the websocket. Designed to be cheap to run: one `$5/mo` VPS handles thousands of light users.

## Protocol (v1)

WebSocket-based JSON messages. Binary CBOR is a later optimization.

### Client → relay

```json
{ "type": "register", "pubkey": "base64-ed25519", "sig": "base64-sig-over-server-challenge" }
{ "type": "punch_request", "peer_pubkey": "base64", "my_external": "ip:port" }
{ "type": "forward", "peer_pubkey": "base64", "payload": "base64-bytes" }
```

### Relay → client

```json
{ "type": "registered", "session_id": "uuid" }
{ "type": "punch_target", "peer_pubkey": "base64", "addr": "ip:port" }
{ "type": "inbound", "from_pubkey": "base64", "payload": "base64-bytes" }
{ "type": "error", "code": "...", "message": "..." }
```

The relay sees:
- Which pubkeys are online
- Which pubkeys send to which (graph metadata)
- IP addresses

The relay does NOT see:
- Prompts
- Responses
- Model names
- Any plaintext

This is enforced because the encrypted transport (QUIC+Noise in the daemon, planned for next sprint) puts the byte stream behind keys the relay never holds.

## Federation + self-hosting

- **Default coordinator** at `relay.unhosted.dev` (when domain is registered). Free for the community to use.
- **Self-hosted** by running `unhosted-relay` on your own VPS / homelab. Single command. Configure `UNHOSTED_RELAY=wss://relay.example.com` in the daemon.
- **Per-pair-circle** is possible: a closed group can run their own relay and never touch a public one.

Metadata leak: the relay learns *who pairs with whom*. Same property as Tailscale's coordination server. Documented as a tradeoff in the threat model.

## What's not in the relay

- No payment/identity layer (that's v0.3.0+ public mode)
- No content moderation
- No retention beyond live sessions — when both peers disconnect, the session is gone
- No logs of payloads (only connection events, opt-in)

## Open questions for daemon-side integration

Resolved during the next sprint:

- How long to attempt direct + hole-punching before falling back to relay (target: 1.5s total)
- Where to store the relay address in `peers.toml` — per-peer override vs daemon-wide default
- Multiplexing: one websocket per peer, or one connection multiplexed for all peers?
- Heartbeats and reconnection on websocket drop
