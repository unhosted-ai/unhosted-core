# 0014 — Swarm model distribution (the torrent-shaped slice)

**Status:** Draft. No code yet. ADR is the contract before implementation.
**Captured:** 2026-06-14
**Target:** First slice in v0.1.0-class release; multi-slice feature.

## Motivation

unhosted is already a peer-to-peer compute mesh — symmetric daemons
(`transport.rs`'s combined client+server `PeerEndpoint`), Ed25519
identity-as-node-id (`identity.rs`), LAN discovery (`discovery.rs`),
and dual seed/peer roles in VRAM-pooling (`vram_pool.rs`'s orchestrator
vs. layer-host split). In spirit it is the closest thing in the
local-AI space to BitTorrent.

But one path is *not* peer-to-peer yet: **getting the model weights onto
the machine.** Today `model_manager.rs` fetches every GGUF over HTTPS
from `huggingface.co` and nowhere else (`validate_download_url` hard-codes
the host). That means:

- Every node independently re-downloads the same multi-GB file from a
  single origin. A LAN of five machines that all want `Qwen2.5-7B`
  pulls ~23 GB across the WAN uplink instead of ~4.7 GB once.
- The origin is a single point of failure and a single point of
  censorship/rate-limiting. If the hub 429s or the repo is pulled, the
  catalog entry is dead for everyone.
- It contradicts the project's own framing ("AI on hardware you own,
  pooled with the hardware your friends own"). The compute is pooled;
  the *weights* are not.

Model weights are the one part of unhosted that is **literally
torrent-able**: a GGUF is a static, content-addressable blob with a
stable hash. Unlike inference — which is sequential and stateful and so
can be *distributed* but never content-addressed and deduplicated like
file pieces — a weights file is exactly the interchangeable, verifiable
"piece" BitTorrent is built around. This ADR makes weight distribution
peer-to-peer. It does **not** touch inference routing.

## Decision

Add a **content-addressed, peer-to-peer model fetch** that sits in front
of the existing HTTPS download in `model_manager.rs`. The HTTPS origin
becomes the fallback "initial seed," not the only source.

### Content addressing

A model is identified by the SHA-256 of its GGUF bytes — call it the
**model digest**, rendered lowercase hex, e.g.
`blake-style` `sha256:9f86d0…`. The curated `CatalogEntry` gains a
`sha256: &'static str` field (we already pin exact `size_bytes`; the
digest is the same kind of integrity constant). Custom HuggingFace URLs
get their digest computed on first successful download and cached.

The digest is the swap key: any peer holding bytes whose SHA-256 matches
is a valid source, exactly like a torrent piece hash. This also closes a
real gap today — the current download verifies only `Content-Length`, not
content, so a corrupted or MITM'd body that happens to be the right
length is accepted. Digest verification fixes that regardless of source.

### Chunking + verification

Split each model into fixed **4 MiB chunks**. The manifest for a model is:

```
ModelManifest {
    digest:      String,      // sha256 of the whole file
    size_bytes:  u64,
    chunk_size:  u32,         // 4 MiB
    chunks:      Vec<[u8;32]>,// sha256 per chunk, in order
}
```

A chunk is accepted iff its SHA-256 matches `chunks[i]`. The whole file
is accepted iff every chunk verifies AND the reassembled file's SHA-256
matches `digest`. This is the Merkle-list model BitTorrent v1 uses; it
lets a node fetch chunk *i* from peer A and chunk *j* from peer B and
trust both without trusting the peers. The manifest itself is tiny
(a 7 GB model → ~1750 chunk hashes → ~56 KB) and is fetched whole before
any chunk transfer.

### Who can be a source

Peer roles reuse the existing trust tiers verbatim — no new trust model:

- **Trusted peers** (`PeerRegistry` entries with a `pubkey`): a node
  asks each paired peer "do you have manifest `<digest>`?" over the
  existing signed channel and pulls chunks from any that say yes.
- **LAN peers** (mDNS-discovered, `discovery.rs`): same protocol over the
  LAN address. The common "five machines on one LAN" case is served
  here with zero WAN traffic after the first node seeds.
- **HTTPS origin** (today's path): always the fallback seed. If no peer
  has a chunk, fetch that chunk's byte range from the origin via an HTTP
  `Range` request (the hub supports ranges), verify it, and now *this*
  node can serve it to the next peer. Bootstrap with zero seeds therefore
  still works and is no slower than today.

Public-swarm (stranger) sourcing is **out of scope** for this ADR — see
"Out of scope." This slice is trusted + LAN + origin only.

### Wire protocol

No new transport. Three message types ride the **existing peer channels**:
the QUIC `PeerEndpoint` for trusted/LAN peers (currently only carries the
ping diagnostic — this gives it its first real payload) with the relay's
pubkey-addressed `Forward` framing as the NAT-blocked fallback.

```
HaveManifest { digest }                    -> Manifest | NotFound
GetChunk     { digest, index }             -> ChunkData{ index, bytes } | NotFound
ListModels   {}                            -> Vec<{ digest, file, size }>
```

`GetChunk` responses are verified against the manifest before a byte is
written to disk; a peer that returns a non-matching chunk is dropped for
this transfer and the chunk is re-requested elsewhere (origin if nobody
else has it). `ListModels` lets a node advertise what it can seed so the
UI can show "3 peers can seed this — pulling from LAN."

### Integration point in `model_manager.rs`

`start_download(url, file)` keeps its signature and its
`DownloadState` state machine (the UI polls it unchanged). Internally:

1. Resolve the model digest: from the `CatalogEntry.sha256` for catalog
   models, or from a small `HEAD`/first-fetch for custom URLs.
2. Fetch the manifest (from a peer that has it, else synthesize it from
   the origin by ranged hashing — see Open questions).
3. For each chunk, in order, pick a source: prefer a LAN peer, then a
   trusted peer, then the origin. Stream into `<file>.gguf.part` exactly
   like `download_to` does today, verifying each chunk.
4. On completion, verify the whole-file digest, then the existing
   atomic `rename(part → dest)`. A failed whole-file check deletes the
   part and surfaces `DownloadState::Failed` — same failure surface the
   UI already renders.

`DownloadState::Downloading` gains an optional `source: "lan" | "peer" |
"origin"` hint so the UI can show where bytes are coming from. The
`bytes_done`/`bytes_total` fields are unchanged, so the progress bar
keeps working with no UI change required for the first cut.

`safe_model_filename` and the models-dir confinement are untouched — all
the existing path-traversal guards still gate every write.

## Alternatives considered

| Option | Why not chosen |
|--------|----------------|
| Embed a real BitTorrent client (`cratetorrent`, `rqbit`) | Drags in a whole second swarm/tracker/DHT stack and a second identity/trust model parallel to our Ed25519 one. We already have authenticated peer channels; reuse them. Reconsider only if we want public-swarm seeding (separate ADR). |
| IPFS / content-addressed store as the substrate | Same objection — a large external dependency and a second network identity. The value (content addressing + chunk dedup) is ~200 lines on top of what we have. |
| Whole-file P2P transfer, no chunking | No multi-source download, no resume, no per-chunk verification. A 7 GB transfer that dies at 90% restarts from zero. Chunking is what makes "pull from LAN + fall back to origin mid-file" possible. |
| Trust peers' bytes without per-chunk hashing | A malicious or buggy peer could poison weights. Per-chunk SHA-256 against a manifest makes source trust irrelevant — the math verifies, not the peer. Cheap and non-negotiable. |
| Keep HTTPS-only, just add a LAN HTTP mirror | Solves the LAN-redownload case but not integrity, not resume, not trusted-WAN peers, and bolts on a second ad-hoc protocol. The chunk protocol subsumes it. |

## Implementation sketch

1. **Manifest + digest types** (`model_manager.rs` or a new
   `swarm.rs`): `ModelManifest`, chunk hashing, whole-file verify.
   Pure functions, fully unit-testable with synthetic byte buffers.
   Add `sha256` to `CatalogEntry` and backfill the constant for each
   catalog model; extend `catalog_entries_are_consistent` to assert the
   digest is well-formed hex of length 64.
2. **Origin ranged fetch**: a `download_chunk_from_origin(url, range)`
   built on the existing `reqwest` client with a `Range` header;
   verify against the manifest. This alone (no peers yet) already
   upgrades today's download to per-chunk-verified + resumable.
3. **Peer chunk protocol**: `HaveManifest`/`GetChunk`/`ListModels` over
   `PeerEndpoint`. Server side answers from the local models dir; client
   side is the source-selection loop. Smoke test: two daemons on
   loopback, one seeds a small synthetic GGUF, the other pulls it
   peer-only and the digests match.
4. **Source selection** wired into `start_download`: LAN → trusted →
   origin per chunk, with drop-and-retry on a bad chunk. `DownloadState`
   gains the `source` hint.
5. **UI + CLI surface**: snapshot already carries `download`; add the
   `source` field display and a "seeding: N models" line. `unhosted
   model seed-status` (or fold into existing `model` subcommands).

Per the project's iteration style, steps 1–2 are independently
shippable and valuable on their own (verified, resumable downloads with
zero peer work) before the peer protocol in 3–4 lands.

## Open questions

- [ ] **Manifest origin for custom (non-catalog) URLs.** Catalog models
  ship a pinned `sha256`, so the manifest is derivable offline. A custom
  HuggingFace URL has no pre-shared digest. Options: (a) fetch the whole
  file from origin once, hash it, build the manifest, and only *then*
  this node can seed it to peers — correct but no multi-source benefit on
  the very first pull; (b) trust the first peer's manifest if a quorum of
  peers agree on the same digest. Lean (a) for the first slice; it's
  honest and the second puller already benefits.
- [ ] **Chunk size.** 4 MiB is a guess balancing manifest size vs.
  request overhead. Measure on a real LAN before committing; BitTorrent
  scales piece size with file size, which we may want for 70B-class files.
- [ ] **Concurrency.** First cut fetches chunks sequentially (mirrors
  today's single stream). Parallel multi-peer chunk fetch is the obvious
  throughput win but adds scheduling; defer to a later slice.
- [ ] **Eviction / "what do I seed."** A node seeds whatever GGUFs are in
  its models dir. Do we ever decline to seed (metered connection, user
  opt-out)? Probably a single `seed: bool` config, default on for LAN,
  default off for WAN/relay. Decide before the relay-fallback path ships.

## Out of scope

*Explicitly not decided here, so reviewers don't argue about it.*

- **Public-swarm seeding from strangers.** This ADR is trusted + LAN +
  HTTPS-origin only. Pulling chunks from unpaired public peers needs the
  payment/escrow + sanctions-gating machinery (`public_mode.rs`, ADR-0001/
  0006) and almost certainly a DHT for discovery — that is a separate ADR
  (the "0015 — DHT swarm discovery" follow-up).
- **A Kademlia/DHT discovery layer.** Source discovery here is limited to
  the peers a node already knows (registry + mDNS). Trackerless,
  whole-internet "who has digest X" lookup is the next ADR, not this one.
- **Distributed/streamed inference as a torrent.** Out by nature:
  inference is sequential and KV-cache-dependent, so a token is not an
  interchangeable, independently-verifiable piece the way a chunk is.
  VRAM-pooling (ADR-0009) is the right model for splitting *computation*;
  this ADR splits *weights* only.
- **Replacing the HTTPS origin.** The origin stays as the guaranteed
  initial seed and the zero-peer fallback. We are adding sources, not
  removing one.
- **Cross-quantization dedup.** Two different quants of the same model
  are different files with different digests. No attempt to share chunks
  between them.
