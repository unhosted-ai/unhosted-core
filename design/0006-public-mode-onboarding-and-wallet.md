# 0006 — Public mode onboarding and wallet binding

**Status:** Accepted (design only — no code yet)
**Captured:** 2026-05-11
**Targets:** v0.3.0+, builds on [ADR 0001](0001-public-mode-architecture.md).

ADR 0001 settled *how* public mode pays (Base, signed-receipt escrow, no token). This ADR settles *how a user gets there from a fresh install* — the on-ramp to that mode without re-inventing accounts.

## The shape of "account"

Public mode does not introduce a new identity layer. The Ed25519 keypair already generated at `~/.config/unhosted/identity.toml` (local + trusted modes) is the account. What public mode adds is:

1. A **payment binding** — an EVM address (Base) linked to the Ed25519 pubkey by a signature.
2. An **on-chain footprint** — escrow deposit (buyer) or stake (provider). Exactly one tx per side, per cycle.

No usernames, no email, no server-side state. The matchmaker only learns `(ed25519_pubkey, evm_address, signed_link)` when the daemon registers, and the link is reproducible from local state.

## Decisions

### 1. Wallet binding: dual path, embedded default

`unhosted public enable` exposes both:

- **`--embedded`** (default): derive a secp256k1 key from a domain-separated extension of the existing Ed25519 seed, store next to `identity.toml` in `wallet.toml` (0600). Auto-funds via Coinbase Onramp redirect with the EVM address pre-filled.
- **`--external`**: open a localhost OAuth-style flow (`http://127.0.0.1:<port>/wallet/connect`) that prompts MetaMask / Coinbase Wallet / Rabby to sign a binding message. Key never touches our process.

Reasoning:

- Embedded covers the 80% case where a new user wants to spend $5 to try public mode without a 5-step wallet-install detour. The key derivation is reproducible (recovery = restore `identity.toml`), and the file lives only on the user's machine — not custodial, just feels that way.
- External covers the 20% case where stake size or earned balance crosses the "I'd want a hardware wallet" threshold. Hardware wallet support comes free with the external path.
- Storing both on one machine is allowed (binding is per-`(pubkey, role)`, not per-machine). Power users can earn into a Ledger while spending from embedded.

**Not chosen:**

- *Custodial wallets only* — kills the manifesto principle. We don't hold keys.
- *External wallets only* — five-step friction kills new-user conversion at the only point that matters: the first $1 deposit.
- *Per-request wallet prompts (MetaMask popup per inference)* — destroys streaming UX. The whole point of the signed-receipt model is one click per cycle, not one per token.

### 2. Funding: detect, route, never custody

`unhosted public deposit <amount>` does one of three things, picked automatically:

1. **Wallet has USDC + ETH for gas**: prompt the user to send `<amount>` USDC to the escrow contract. Embedded path signs locally; external path sends a WalletConnect request.
2. **Wallet has neither**: redirect to Coinbase Onramp with the destination address pre-filled to the *escrow address with a memo tag* so funds land in escrow directly, not the user's address. (One on-chain tx becomes zero from the user's POV — the onramp does it.)
3. **Wallet has USDC but no ETH**: surface the gas problem explicitly. Don't try to be clever — gas-abstraction relayers add a custodial leg.

Withdrawals (provider side) follow the inverse: USDC arrives in their wallet on `unhosted public withdraw`, and a Coinbase Offramp link is offered for cash-out.

### 3. Provider onboarding: stake, register, serve

`unhosted serve --public --stake <usdc>` is the provider entry point. The daemon:

1. Verifies the wallet binding exists (errors with a hint to run `unhosted public enable` first).
2. Posts `<usdc>` to the slashing contract as a single tx. Minimum stake parameter is set by the matchmaker; default `$50` at v0.3.0 launch (subject to change as Sybil cost economics get re-validated).
3. Announces `(pubkey, evm_address, model_list, stake_amount, geo_hint)` to the matchmaker.
4. Begins accepting matchmaker-routed inference requests, gated by:
   - **Acceptance toggle** — provider can pause without un-staking.
   - **Per-model opt-in** — explicit allow-list, not auto-everything-installed.
   - **Allow-list for prompts** — none at protocol layer; jurisdiction filter at the *application* layer is the provider's responsibility (see [ADR 0001 jurisdiction note](0001-public-mode-architecture.md)).

Un-staking returns the locked amount after the slashing dispute window (TBD, target 7 days).

### 4. The first-90-seconds buyer journey

The bar we're optimizing for. Annotated:

```
$ unhosted public enable
> Linked Ed25519 → 0x7e3c…a91d (embedded wallet at ~/.config/unhosted/wallet.toml)

$ unhosted public deposit 5
> No USDC in wallet. Open Coinbase Onramp to deposit $5 directly into escrow? [Y/n]
> [browser opens; user pays w/ card; funds land in escrow via Onramp]
> Escrow balance: $5.00 USDC

$ unhosted run "explain transformers in one paragraph"
> [stream from provider in Berlin; 0.018 USDC spent]
```

Three commands, one card payment, one minute. The first two are one-time. Future `unhosted run` calls without `--local`/`--trusted` and with `public_fallback = true` configured just work, debiting the escrow until empty.

### 5. The first-90-seconds provider journey

```
$ unhosted public enable
> Linked Ed25519 → 0x7e3c…a91d (embedded wallet)

$ unhosted public deposit 60   # stake + a little headroom for gas
> [Onramp deposits 60 USDC directly to wallet]

$ unhosted serve --public --stake 50
> Stake posted. Registered with matchmaker.
> Earning. (8.4 GB/s upload available; running 1 model: llama3.2:3b)
```

Provider opt-in is *louder* than buyer opt-in for a reason — they're putting capital and reputation on the line. The output names the stake, the bandwidth, and the models so the user can't accidentally enable something they didn't intend.

## What this ADR does not decide

- **Exact key derivation** for the embedded EVM key from the Ed25519 seed. Candidates: BIP-32 over a hardened-derivation root with a domain-separation salt (`"unhosted-evm-v1"`); or a HKDF expansion of the ed25519 secret bytes. Either works; pick when implementing.
- **Recovery UX.** Today `identity.toml` is the single source of truth and lost = lost. We may add an opt-in BIP-39 seed-phrase export for users who don't already back up their `~/.config`.
- **Reputation portability.** Whether a provider's reputation is bound to the EVM address (so wallet rotation costs reputation) or the Ed25519 pubkey (so it survives wallet rotation). Probably the latter, with a one-time signed handoff for wallet rotation.
- **Onramp partner.** Coinbase Onramp is the launch choice on Base; Stripe Crypto and MoonPay are fallbacks if Coinbase has regional gaps.
- **Mobile wallet flow.** External-path connect on phone uses WalletConnect, but the buyer UX on iOS/Android is its own design problem the desktop ADR doesn't solve.

## What we are explicitly not building

- Email signup, password reset, "forgot account" flows.
- A custodial mode where Unhosted holds keys "for convenience."
- A `$UNHOSTED` token. Public mode pays in USDC, end of story.
- Per-request wallet popups.
- Subscriptions or recurring auth at the protocol layer. (An application built on top is free to abstract this.)

## Migration path from trusted to public

A user already running trusted mode upgrades cleanly:

- Their `identity.toml` stays. No re-key.
- Their paired peer list stays. Public mode is purely *additive*.
- `unhosted run` with a fallback chain (`local → trusted → public`) starts using strangers only when the first two can't serve a request, gated by a per-request `--public-ok` flag or a config opt-in. We do not silently start spending money for a user who paired with friends.
