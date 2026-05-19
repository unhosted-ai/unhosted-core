# Privacy

Unhosted is open-source software you run on your own machine. There is no "Unhosted Inc." that hosts a service. Nothing about installing or running the daemon sends data to us. This page tells you, with file paths and endpoints, exactly what stays local and exactly what doesn't.

## TL;DR

| Data | Where it lives | Who sees it |
| --- | --- | --- |
| Prompts and responses | Your daemon process and its upstream (llama.cpp / Ollama / LM Studio) | You. Plus paired peers if the daemon routes a request through them. |
| Chat history | `~/.config/unhosted/chats.json` on the machine running the daemon | You. Phones / browsers / paired peers reading your daemon over LAN or your own tunnel see the same store. |
| Memory entries | `~/.config/unhosted/memories.json` | You. Opt-in (default off). |
| Node identity (Ed25519 keys) | `~/.config/unhosted/identity.toml` (mode 0600) | You. Peers you choose to pair with see only the public half. |
| Paired-peer registry | `~/.config/unhosted/peers.toml` | You. |
| Public-mode policy | `~/.config/unhosted/public-mode-policy.json` | You — plus anyone who calls `GET /v1/status`, because the file's contents are advertised so callers know what you accept. |
| Web/UI assets | Embedded in the binary; served on loopback | You. |

Nothing in this list ever leaves your machine unless **you** invoke a feature that crosses a trust boundary. The next sections describe each of those features.

## What sends data off your machine

These are the features that, by design, send data somewhere other than the daemon process. None are on by default; each requires an explicit action.

### Upstream LLM (always)

The daemon proxies inference requests to a separate process you chose: `llama-server`, Ollama, LM Studio, or another OpenAI-compatible endpoint. The default upstream is `http://127.0.0.1:8080`, which is loopback — same machine. **If you set `UNHOSTED_LLAMA_SERVER_URL` to a non-loopback host, prompts and responses go there.** That host's privacy posture is its own.

### Cloudflare Tunnel (opt-in, off by default)

`unhosted serve --eager-tunnel`, or the "open to internet" button in the sidebar, runs the `cloudflared` subprocess and registers a Cloudflare Quick Tunnel. Once active:

- Cloudflare proxies all HTTP traffic between phones/callers and your daemon. They can observe encrypted-at-rest TLS metadata (request size, timing, the public URL); the request bodies are end-to-end TLS but terminate at Cloudflare's edge before re-encryption to you.
- The public URL is rotated on each tunnel start. It is not authenticated by default — anyone with the URL can reach your daemon. Pair this with a bearer token or use it only for short-lived shares.
- Off persists across restarts unless you re-enable.

If this matters for your threat model, run the daemon on a LAN and pair phones over WireGuard or Tailscale instead. The tunnel is for convenience, not for privacy.

### Trusted-peer routing (opt-in, requires explicit pairing)

If you've paired with a peer (`unhosted pair …`), the request router may forward an inference request to that peer when the local upstream is busy. The peer's daemon runs the model on its hardware and streams tokens back over QUIC. The peer's daemon sees:

- The prompt you sent
- The response it generated
- Your daemon's Ed25519 public key (the request is signed)

It does **not** see your other chats, your memory entries, or any keys other than the request signature. Trust is reciprocal — your daemon will only forward to peers you explicitly added to the registry.

### Public mode / quotes (opt-in, off by default, currently no-payment scaffold only)

If you set a `PeerPaymentPolicy` that accepts any rails, strangers can call `POST /v1/public-mode/quote` against your daemon. They send a signed payer context (rail, KYC tier, country); your daemon runs the policy filter and returns a quote or a rejection. No inference happens at quote time. **No rails are wired up in this repo as of v0.0.45** — quotes are a price commitment, not a settlement.

When rails do land (slice 3 of ADR-0010), payment metadata will cross trust boundaries by definition. That metadata's scope is the payment rail's, not unhosted's. See [`unhosted-payments/design/0010-transactional-public-mode.md`](https://github.com/unhosted-ai/unhosted-payments/blob/main/design/0010-transactional-public-mode.md) for the receipt shape.

### Memory (opt-in, off by default)

When you toggle private memory on in the sidebar, the daemon records short summaries of past chats and injects relevant ones into the system prompt of new chats. Storage is `~/.config/unhosted/memories.json` on your machine. **Nothing leaves your machine.** The summaries are produced by the same upstream LLM as your normal chat — so if your upstream is non-loopback, memory generation hits it like any other prompt.

### Embeddings (one-time, opt-in)

The first time memory is enabled, the daemon downloads a ~33 MB ONNX embedder (`bge-small-en-v1.5`) from Hugging Face into `~/.cache/fastembed/`. This is the only network call the daemon ever initiates on its own, and only when you turn memory on. The download is a stock HTTP GET — Hugging Face logs your IP like any download mirror would.

### Plugins (opt-in, separate repo)

The MCP server in [`unhosted-plugins`](https://github.com/unhosted-ai/unhosted-plugins) is a separate process you start manually. It speaks MCP to a host (Claude Desktop, Cursor, Zed) and proxies tool calls to your daemon. Anything the host model decides to call (e.g., `unhosted_web_fetch`) goes through your daemon — so whatever the model sees, your daemon sees too. The host vendor (Anthropic, etc.) sees what their own UI logs say they see; that's their privacy policy, not ours.

## What never leaves

- **Your prompts.** The daemon does not log them, batch them, send anonymized samples, or train on them. The only place they go is the upstream you configured.
- **Your model files.** Pulled with `unhosted pull`, cached at `~/.cache/unhosted/`. Inference runs locally; tokens are streamed by `llama-server` (or whatever upstream you chose) — they don't pass through any unhosted-ai service because no such service exists.
- **Your identity key's secret half.** `~/.config/unhosted/identity.toml` is mode 0600. The public half travels with signed peer requests (paired peers, public-mode quotes); the secret half stays put.
- **Telemetry of any kind.** We do not collect crash reports, usage analytics, feature flags, or installer pings. If we ever want any of those, [`MANIFESTO.md`](MANIFESTO.md) says they will be opt-in with a plain-English explanation. That promise is unchanged.

## Models and the data they were trained on

When you `unhosted pull llama3.2:3b`, the file comes from Hugging Face mirrors of Meta's release. The training data of those models is whatever the original publisher said it was. Unhosted does not modify the weights, do not fine-tune on your data, and do not host an opaque "system model." See [COMPLIANCE.md](COMPLIANCE.md#model-licenses) for what governs each pulled model.

## Children

The software is not designed for, marketed to, or intended for children under 13. Public mode (when rails are wired) will require KYC at a minimum tier of "email" for any non-zero policy, which most age-restricted email providers won't grant.

## Changes

This file is part of the repo. Changes are commits. To see what changed and when, run `git log PRIVACY.md`.

## Contact

Privacy issues: **security@unhosted.dev** (same address as security disclosures — separate intake doesn't exist at this scale). Use a subject line that starts with `[privacy]`.
