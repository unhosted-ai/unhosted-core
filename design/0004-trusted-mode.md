# 0004 — Trusted mode (v0.1.0)

**Status:** Accepted (alpha shipped — identity + pairing; encryption deferred)
**Captured:** 2026-05-11
**Targets:** v0.1.0-alpha lands explicit identity + pairing on top of the existing v0.0.3 architecture. v0.1.0-beta adds encrypted transport and NAT traversal.

This ADR covers the second ring of the trust radius: devices belonging to people you've explicitly paired with — roommate, family, teammate — that may live outside your LAN.

## Three concerns vs local mode

| concern | local (v0.0.x) | trusted (v0.1.0) |
|---|---|---|
| identity | none — same LAN assumed | Ed25519 keypair per node |
| pairing | mDNS auto-discovery + click | explicit out-of-band exchange |
| transport | HTTP, unencrypted | HTTP signed for alpha, QUIC+Noise for beta |
| reachability | LAN | LAN/VPN for alpha; native NAT traversal via relay for beta |

## v0.1.0-alpha (this milestone)

### 1. Persistent Ed25519 identity per node

On first `unhosted serve`, generate an Ed25519 keypair and persist to
`~/.config/unhosted/identity.toml` (alongside `peers.toml`). The keypair is
the node's stable identity — surviving restarts, model swaps, IP changes.

### 2. Extend `Peer` with an optional pubkey

Existing `peers.toml` entries stay LAN-only. Trusted peers carry an additional
`pubkey` field (base64-encoded Ed25519 public key). Presence of `pubkey`
distinguishes a trusted peer from an unauthenticated LAN peer.

### 3. Pairing flow — one-step, out-of-band token

```
device A:  unhosted pair offer
  → prints: unhosted://pair?pk=BASE64&addr=192.168.1.42:7777&t=A1B2C3
  → that string is shared with device B via Signal / SMS / paper

device B:  unhosted pair accept "unhosted://pair?…"
  → POST to A's /v1/pair with B's pubkey + the token
  → A verifies the token (one-time, 5min lifetime), saves B as trusted
  → A's response contains A's pubkey; B saves A as trusted
  → mutual pairing complete, one HTTP round trip
```

One-time tokens are sufficient because the only attacker who can use them is
someone who has the offer string — at which point they have the addr too, and
already-paired-by-design is the threat model we want.

### 4. Signed requests

Every request to a trusted peer carries `X-Unhosted-Auth: <pubkey>:<sig>` —
Ed25519 signature over the request body + a Unix-timestamp nonce. The
receiver verifies the signature against its trusted-peer table; unsigned or
invalid-sig requests from non-local paths are rejected.

This authenticates *who* is making the request, even before transport
encryption ships in beta. Stops a stranger who somehow obtains your peer's
IP from masquerading as that peer.

### 5. Reachability for alpha

Alpha works on a LAN out of the box. Across the internet, both nodes need
to reach each other (one with a public IP, port-forwarded, or both behind
the same VPN). Beta adds the relay + hole-punching layer so this stops
being a user problem.

## v0.1.0-beta (later)

### 1. Encrypted transport — QUIC + Noise

Replace HTTP+signatures with **QUIC carrying the Noise IK handshake**
(quinn + snow Rust crates). The Ed25519 identity from alpha becomes the
static key on each side; pairing's already done the public-key exchange.
After handshake, all bytes are encrypted and authenticated end-to-end.
No TLS PKI, no certs to manage, no expiration.

Picking QUIC over plain TCP+TLS because:
- single round-trip handshake with already-known peer keys
- friendlier NAT behavior (UDP)
- the same socket can carry many streams (parallel requests)

### 2. Relay / NAT traversal

Optional small relay server for nodes that can't accept inbound (typical
home internet). STUN-style hole-punching tried first; falls back to a
TURN-style data relay if that fails. Community-runnable; nothing
proprietary. Not the protocol's critical path — direct connections work
when both nodes have public IPs or successful hole punching.

## What we will not do

- **No central pairing service.** Pairing tokens never leave the two devices.
- **No "trust on first use" auto-accept.** Every trusted peer requires
  explicit human action on both ends.
- **No revocation infrastructure beyond `unhosted peer remove`.** If you
  unpair, the peer's pubkey is removed; future requests fail. That's it.
- **No multi-user.** A node is identified by one keypair; "users" are an
  application-layer concern that doesn't exist in v0.1.x.

## Open questions

- Should the pairing offer be a URL (clickable in chat apps) or a
  human-readable code (typeable from paper)? Probably both; URL primary.
- What happens when a paired peer's IP changes? Tailscale handles this for
  alpha. Native version will need a "last seen at" probe.
- mDNS announcements should advertise the pubkey so paired peers can be
  rediscovered on the LAN without a fresh pairing round. Adds a `pk=` TXT
  record to the existing _unhosted._tcp.local. service.
