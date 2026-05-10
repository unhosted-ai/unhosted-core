# 0001 — Public mode: payment and verification architecture

**Status:** Accepted (design only — no code yet)
**Captured:** 2026-05-09
**Targets:** v0.3.0+, after local mode (v0.0.x) and trusted-peer mode (v0.1.0–0.2.0) ship.

This ADR captures the architectural shape of Unhosted's *public* mode — the third ring in the trust radius, where strangers' GPUs serve inference in exchange for stablecoin.

## Decisions

### 1. Settlement chain — Base first, Solana later

We ship public mode on **Base** first. Solana support follows in a later release.

Reasoning:

- Base is USDC-native, EVM-compatible, ~$0.01 per tx (acceptable when batched via signed receipts).
- Coinbase fiat onramp is one-click — critical for buyers who aren't already in crypto.
- EVM tooling is mature; we don't have the engineering bandwidth to do EVM and Anchor/Solana well at v0.3.0.

Solana ships later because:

- ~$0.0001 per tx is genuinely better for micropayments at scale.
- Worse fiat-onramp story as of 2026 (Solana Pay exists but the UX trails Coinbase Onramp).

The matchmaker and CLI are designed multi-chain from day one so adding Solana doesn't require a rebuild.

### 2. Verifiable inference — optimistic + redundancy

For v1 public mode, verification is **optimistic with N≥2 redundant queries**.

Mechanism:

1. Each request goes to N randomly-selected providers (default N=2).
2. Buyer's CLI compares outputs locally using deterministic decoding (temperature=0, fixed seed, identical sampling params).
3. Matching outputs → both providers paid pro-rata.
4. Diverging outputs → both providers slashed, request automatically retried against fresh providers.
5. Provider reputation accrues from clean settlements; reputation loss excludes a provider from matchmaker rotation.

Cost: 2–3× compute per query. Acceptable because public mode is opt-in, used only when local + trusted can't fulfill.

**Not chosen:**

- *TEE attestation only* — would require H100/Blackwell confidential compute. Kills the consumer-GPU pitch (the whole point is the 4090 in your buddy's basement).
- *ZK proofs of inference* — ~1000× slowdown today. Re-evaluate in 2027–2028.
- *Pure reputation* — too easy to game with sock puppets at scale.

### 3. Payment flow — pre-paid escrow + signed receipts

We do not put per-token payments on-chain. We do not run a custodial wallet. The flow is:

1. Buyer deposits USDC into the Unhosted escrow contract — *one* on-chain tx.
2. Per inference request, buyer signs an off-chain payment authorization referencing the escrow.
3. Provider streams tokens; both sides count locally and agree at end-of-stream.
4. Provider accumulates signed receipts off-chain; submits a batch on-chain to claim when balance exceeds a threshold.

One on-chain tx per deposit, one per withdrawal. Everything in between is cryptographic signatures.

**Not chosen for v1:**

- *State channels (Lightning-style)* — more efficient at scale, much harder to build correctly. Revisit when volume justifies the complexity.
- *x402 (HTTP-native payment)* — Coinbase's emerging standard for AI agent micropayments. Promising but bleeding edge in 2025–2026; we don't bet v1 on it.

## What we will not do

- **No token.** There is no `$UNHOSTED`. Manifesto rule.
- **No protocol cut to a private wallet.** If a fee is taken for funding development, it must be: explicit, capped, multisig-controlled, switchable, documented in this ADR.
- **No KYC at the protocol layer.** Whatever onramp the buyer uses (Coinbase, etc.) handles KYC; Unhosted itself is permissionless.
- **No prompts on-chain.** Prompts stay encrypted between buyer and provider.

## Open questions (deliberately not deciding yet)

- Exact escrow contract shape — single-buyer-many-provider escrow vs. shared pool.
- Provider stake amount, slashing parameters, slashing dispute window.
- Sybil resistance at the matchmaker / discovery layer.
- Prompt privacy on the provider side beyond "don't send sensitive things to public mode" — TEE? Garbled circuits? Just a clear warning?
- Tax / regulatory exposure for high-volume providers in different jurisdictions.

These become sub-ADRs as v0.3.0 design firms up.
