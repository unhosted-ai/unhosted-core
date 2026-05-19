# IP posture

How this project handles intellectual property — license clarity, defensive publication of novel mechanisms, and what we deliberately don't claim.

**This is not legal advice.** It is the project's standing policy. Operators preparing to file patents, defend infringement claims, or commercialize forks need real counsel.

## License

All source under this organization is **AGPL-3.0-or-later**. See [LICENSE](LICENSE). The license applies to:

- The daemon binary, CLI, and web UI (`unhosted-core`).
- Settlement primitives and rail adapters when they land (`unhosted-payments`).
- Plugins, including the MCP shim (`unhosted-plugins`).

When a sub-package crosses an ecosystem boundary that has a different convention (e.g., the Solidity escrow contract that will land in `unhosted-payments/contracts/`), the file header may carry **dual licensing** ("AGPL-3.0-or-later OR MIT", say) only when:

1. The chosen second license is strictly more permissive than AGPL,
2. The dual license is necessary for the ecosystem to consume it (Solidity audit tooling expects MIT/Apache), and
3. The decision is recorded in the file's own ADR or commit message.

The default is single-license AGPL.

## Developer Certificate of Origin (DCO)

[CONTRIBUTING.md](CONTRIBUTING.md) requires `git commit -s` on every contribution. This is the [DCO](https://developercertificate.org/) — the contributor asserts they have the right to license their contribution under the project's terms. PRs without sign-off won't be merged. This keeps the IP chain clean if the project is ever audited, sold, sublicensed, or asked to defend itself.

## Trademark

"Unhosted" and the lockup at [`assets/lockup.svg`](assets/lockup.svg) are project marks. Restated from [TERMS.md § 9](TERMS.md#9-trademark): use of the marks to identify modified forks is permitted; use to imply endorsement of an unrelated product is not. AGPL governs code, not marks.

Filing a registered trademark in the United States and EU is a likely follow-up step once the project has a stable v0.1.0 release; until then the marks are unregistered common-law marks, distinguished by first-use and visible public attribution in this repo's history.

## Patents

### What this project will not do

- File offensive patent claims for the purpose of extracting royalties from other implementers of the same general patterns.
- Accept contributions that come with patent grants narrower than what AGPL § 11 already grants (i.e., contributors implicitly grant a license to any patent claims their contribution practices).
- Distribute code that the project has actual knowledge infringes a valid third-party patent without that holder's consent. If notified of a credible claim, the response is to redesign around it, not to litigate.

### What this project may do

Defensive patent filings are an option if:

1. A novel mechanism (per the register below) faces a credible threat of being patented offensively by another party.
2. Counsel advises that defensive filing is the right move (e.g., to back into an Open Invention Network covenant, or as deterrent for cross-licensing).

No defensive filings exist as of this commit. If any are made they will be listed publicly in this file with the application/issuance numbers.

### Novelty register — what we believe is non-obvious

The items below are documented here, dated to this commit, AGPL-3.0-or-later licensed, and publicly available. They constitute **defensive publication**: if another party tries to patent these mechanisms after this commit, this file and the cited source code are cited prior art. We do not claim *exclusive* novelty — only that we are practicing these openly and on the record.

#### 1. Sanctions-default block-list auto-merged at daemon save

`PeerPaymentPolicy` saved via the daemon's HTTP `PUT` endpoint has the comprehensively-sanctioned OFAC jurisdictions (KP, IR, SY, CU) merged into `blocked_countries` before the file is written to disk. The operator cannot, through the API, save a policy that omits them. Removing one of these codes requires editing the constant `SANCTIONS_DEFAULT_BLOCKED` in `crates/unhosted-core/src/public_mode.rs` and rebuilding — a deliberate friction.

What's possibly novel: the *merge-on-save* pattern that makes the sanctions block-list inviolable from the API while keeping the policy file otherwise user-controlled. Sanctions screening as a static input is everywhere; "the operator cannot disable it short of recompiling" is the unusual bit.

#### 2. Signed receipt with the host public key embedded inside the signed body

A `SignedReceipt`'s `UsageReport.host_pubkey` is part of the bytes the signature covers. A verifier reads the pubkey from inside the signed body, not from a separate envelope field. This prevents an envelope swap where an attacker substitutes a different claimed signer.

What's possibly novel: most signed-message schemes carry the signer's identity in an outer envelope or rely on a separate identity-binding step. Including the signer's pubkey *inside* the canonical body, with no separate identity binding, is a small but meaningful design choice for the verifier's trust model. Source: [`unhosted-payments/core/src/receipt.rs`](https://github.com/unhosted-ai/unhosted-payments/blob/main/core/src/receipt.rs).

#### 3. Cross-language canonical-JSON contract verified by a shared fixture

The Rust `canonical_json` in `unhosted-payments-core` and the TypeScript `canonicalJson` in `@unhosted-ai/wallet-js` produce byte-identical output for the same input, verified by a fixture (`{"a":2,"m":{"b":4,"y":3},"z":1}`) tested in both implementations. The contract is: object keys sorted recursively, no whitespace, JSON.stringify-default number/string encoding.

What's possibly novel: not the canonicalization itself (RFC 8785, JCS, has prior art), but the specific minimal canonicalization plus the explicit cross-language fixture test as the canary. Sources: [`unhosted-payments/core/src/receipt.rs`](https://github.com/unhosted-ai/unhosted-payments/blob/main/core/src/receipt.rs), [`unhosted-payments/wallet-js/test/wallet.test.ts`](https://github.com/unhosted-ai/unhosted-payments/blob/main/wallet-js/test/wallet.test.ts).

#### 4. Trust-radius routing with per-ring opt-in

`Local → Trusted → Public` routing where the operator opts in to each ring independently (`PeerRegistry`, `PeerPaymentPolicy`), and the router consults the rings in order with per-ring policy. Local works offline. Trusted requires explicit pairing. Public requires a non-empty `accepted_rails` AND a payer's signed quote. Each ring has its own auth posture (loopback / paired-peer signed request / payer-signed body).

What's possibly novel: the three-tier opt-in with each tier having its own cryptographic auth mechanism, rather than a single "auth or not" toggle. The concrete pattern is in [`unhosted-core/crates/unhosted-core/src/auth.rs`](crates/unhosted-core/src/auth.rs) + [`crates/unhosted-core/src/router.rs`](crates/unhosted-core/src/router.rs).

### What we do not claim is novel

For completeness, so any reader knows where we are *not* exposed and where we expect no defense:

- Ed25519 signatures (RFC 8032; decades of prior art).
- Multi-machine inference via layer splitting (llama.cpp's RPC mode, MIT-licensed; we orchestrate, we don't invent).
- Trusted-peer pairing via short-code + signed offer (similar to WireGuard's posture, well-trodden).
- mDNS / DNS-SD service discovery (RFC 6762/6763, prior art > 20 years).
- Cloudflare Quick Tunnel usage (CF's own product).
- LoRA / QLoRA fine-tuning recipes (open-source academic prior art).
- Cargo / npm package layout patterns.

## How to add to this file

When you ship code that you believe contains a non-obvious mechanism worth documenting as prior art:

1. In the same PR that lands the code, edit this file. Add an entry under "Novelty register" with: a one-line summary, what's possibly novel, and a link to the source.
2. Commit and tag the release. The git commit's timestamp is the citation date.
3. Do not over-claim. "Possibly novel" is the right hedge. If counsel later determines the mechanism is actually trivially anticipated, the entry can be moved to "What we do not claim is novel."

## How to remove from this file

If a prior-art search later finds an entry was anticipated, move it from "Novelty register" to "What we do not claim is novel" with a brief note pointing at the anticipating work. Don't silently delete — the historical claim should remain visible in git history.

## Contact

License or patent matters: **legal@unhosted.dev**.
