# 0007 — Security hardening pass

**Status:** Accepted (implemented in v0.0.4 / Phase A)
**Captured:** 2026-05-11
**Targets:** Close the soft spots in the v0.0.x threat-model answer.

A user asked "make sure this is non-hackable." Nothing is unhackable. What we *can* do is close the four gaps that were trivially exploitable in v0.0.3, and make subsequent levelling-up incremental rather than rewrite-driven.

## What was wrong

The Phase-A pre-audit found:

1. **Daemon had no local auth.** `X-Unhosted-Auth` was checked only if *present*. Anyone on the same LAN could hit `/v1/run`, `/v1/peers add`, `/v1/pair/*`, `/v1/punch` when the daemon was bound to `0.0.0.0` (the common case for phone access). Inference theft, peer injection, prompt redirection — all unauthenticated.
2. **No replay defense.** The signature window is 5 minutes; without a nonce store an attacker who sniffed one signed peer request could replay it ~300× before expiry.
3. **4-letter codes were brute-forceable.** ~20 bits of entropy with no rate limit on the relay's `/v1/codes/{code}` lookup. The whole space takes seconds to enumerate.
4. **Relay was unbounded.** No total session cap, no per-IP cap. One actor opens 10⁶ connections, FD exhaustion drops the service.

## What changed

### Daemon — bearer token + replay store

New `auth.rs` module:

- **`LocalToken`** — 256-bit random token at `~/.config/unhosted/api-token.txt` (0o600). Persists across restarts so the web UI's localStorage cache stays valid. Constant-time comparison on check.
- **`ReplayGuard`** — `HashMap<(pubkey, ts, sig_prefix), Instant>` with a 65k-entry cap and TTL-based eviction. Any signed peer request whose `(pubkey, ts, first-16-bytes-of-sig)` triple has been seen in the verify window is rejected.
- **`classify()`** — single decision point. Returns one of `Peer(pk)` / `Local` / `LoopbackUnauthed` / `Rejected(reason)` / `Missing`. Handlers map outcomes to allow/deny based on sensitivity.

Endpoints, after this pass:

| Endpoint | Loopback unauthed | Local bearer | Paired peer | None of the above |
|---|---|---|---|---|
| `/health` | ✓ | ✓ | ✓ | ✓ (intentional liveness probe) |
| `/v1/auth/token` | ✓ | (n/a) | (n/a) | **403** |
| `/v1/status`, `/v1/identity`, `/v1/models` | ✓ | ✓ | ✓ | **401** |
| `/v1/run`, `/v1/chat/completions` | ✓ | ✓ | ✓ | **401** |
| `/v1/peers` POST, `/v1/peers/:n` DELETE | ✓ | ✓ | **403** (peers can't add peers on our behalf) | **401** |
| `/v1/pair/offer`, `/short-offer`, `/connect`, `/use-code`, `/v1/punch` | ✓ | ✓ | **403** | **401** |
| `/v1/pair/accept` | ✓ | ✓ | ✓ | ✓ (uses its own one-time token) |

### Web UI — token bootstrap, monkey-patched fetch

- Reads `?t=<token>` query param on first load → localStorage → URL-bar cleaned via `history.replaceState`.
- Falls back to `GET /v1/auth/token` (which succeeds only from loopback — desktop shell + same-machine browsers get the token automatically).
- `window.fetch` patched to attach `Authorization: Bearer <token>` to every `/v1/*` call (except `/v1/auth/token` itself).
- Phone-from-LAN flow: daemon startup banner prints `http://<machine-ip>:7777/?t=<token>` — user opens it once, token persists in localStorage from then on.

### Relay — caps + per-IP rate limit

- **`MAX_SESSIONS = 10_000`** total. Hard reject beyond.
- **`MAX_SESSIONS_PER_IP = 8`**. Per-IP counter incremented on accept, decremented on close + on register-failure. Prevents one bad actor from monopolizing.
- **Code-lookup rate limit**: 8 attempts per 60s sliding window, per source IP, on `/v1/codes/{code}`. Returns `429` past that. With 20 bits of entropy and 8 lookups/min/IP, brute-forcing one code in expectation takes >2 years from a single source.

## What we still haven't done

Acknowledged-and-deferred:

- **Transport encryption between LAN peers.** Trusted-mode requests are *signed* but not *encrypted*. Anyone passive on the wire sees prompts in cleartext. QUIC + Noise is the next Phase A item; it's a real protocol rewrite, not a one-session patch.
- **Windows identity.toml ACLs.** Unix gets 0o600; Windows currently relies on the user-profile directory ACL inheritance, which is usually right but not enforced by us. Fix is a small `winapi` ACL call in [identity.rs](../crates/unhosted-core/src/identity.rs) when implementing.
- **External security audit.** `cargo audit` is clean; that's a sanity check, not an audit. Real audit is a v0.2.0/v0.3.0 milestone, not v0.0.x. Auditing a moving codebase before trusted mode is stable is wasted spend.
- **Per-pubkey relay rate limits.** Per-IP catches one-host floods; a botnet evades it. Adding per-pubkey message-rate limits is straightforward but waits until we have real abuse data.
- **Per-request prompt encryption to public-mode providers.** Out of scope until v0.3.0 (ADR 0001 / 0006).

## Threat model after this pass

- **LAN attacker on the same wifi without the bearer:** locked out of every sensitive endpoint. Can hit `/health`. That's it.
- **LAN attacker who shoulder-surfs the bearer:** has full local-user access. Token can be rotated by `rm ~/.config/unhosted/api-token.txt && systemctl --user restart unhosted` (or equivalent). We do not auto-rotate; users who need that level of paranoia should pin the daemon to loopback and use SSH port-forward instead.
- **Internet attacker who hits the relay:** session cap + per-IP cap mean they cannot exhaust the service from one host. Code brute-force is rate-limited to economically infeasible. They still see traffic *metadata* (who's paired with whom), as documented in ADR 0005.
- **Internet attacker who intercepts a signed peer request:** replay window is 5 min; replay attempt rejected by the nonce store. Cannot mutate the body without invalidating the signature.
- **Passive attacker on your LAN reading peer-to-peer traffic:** can read prompts/responses in cleartext. This is the unfixed gap until QUIC+Noise lands. Surfaces in the threat-model docs.

In one line: v0.0.4 is "no one on your LAN gets free access just because you turned on phone mode."
