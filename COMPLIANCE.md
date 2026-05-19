# Compliance posture

This document describes the project's posture toward the major regulatory regimes that touch open-source AI infrastructure. **It is not legal advice.** Every operator of the daemon is responsible for compliance with the laws of their own jurisdiction; this page is what the project is doing at the source-tree level so operators don't have to start from zero.

## Roles, briefly

Read every regulation through the lens of three roles:

- **The project** — the contributors who write and release this source code. Distributing open-source software is generally not a regulated activity in most jurisdictions, though sanctions and export controls apply.
- **The operator** — the natural or legal person running the daemon. This is the regulated role for GDPR (controller), AI Act (provider or deployer, depending on what they do with outputs), money transmission, KYC, taxes.
- **The user** — anyone whose prompts or data the daemon processes. Often the same person as the operator, in local mode.

Unless explicitly noted, every duty below falls on the **operator**, not the project.

## Sanctions

### Defaults shipped in the daemon

- The `PeerPaymentPolicy` default is `closed` (accepts nothing). An operator must opt in to anything broader.
- When the operator opts into public mode, the recommended baseline `blocked_countries` set includes the U.S. OFAC comprehensively-sanctioned jurisdictions: **KP, IR, SY, CU**. See [ACCEPTABLE_USE.md](ACCEPTABLE_USE.md#public-mode-host-duties) for the operator's duty to maintain this.
- The sanctions list is not static. OFAC, OFSI (UK), the EU, and the UN Security Council update theirs on different cadences. Operators should subscribe to alerts from the regimes relevant to their jurisdiction. The project does not currently publish a synced list.

### What the project will not do

- Knowingly accept commits, releases, or distribution to parties on a sanctions list.
- Process payments (none today) without rail-level KYC/AML obligations being honored by both sides.

## Export controls

The cryptographic primitives the daemon uses (Ed25519 for signatures, ChaCha20-Poly1305 / AES-GCM via TLS, BLAKE3 hashes via dependencies) are subject to the U.S. Export Administration Regulations (EAR) and equivalents.

- The repository qualifies as **publicly available source code under EAR §740.17(b)(2)(i)(B)**, which has historically been treated as the de facto safe harbor for FOSS cryptographic software with a one-time notification to BIS and NSA. The project will file that notification when a stable v0.1.0 release ships; until then, every release is a pre-alpha public-domain-style distribution.
- The Wassenaar Arrangement's general technology note covers the cryptography here.
- **Embargoed destinations** (currently the OFAC comprehensive set above) are not permitted to download, run, or redistribute the software regardless of the EAR exemption.

If you re-distribute the project's binaries through a download mirror, app store, package registry, or container registry, you take on whatever export-classification duty that channel imposes.

## EU AI Act

The EU AI Act (Regulation (EU) 2024/1689) applies to providers and deployers of AI systems placed on the EU market.

### The project's role

The project distributes general-purpose AI **runner** code, not models. The Act's GPAI (general-purpose AI model) provisions apply to model **publishers** (Meta for Llama, Alibaba for Qwen, etc.), not to runners. The project's distribution does not, on its own, place a model on the EU market.

### The operator's role

If you, as an operator, run a model and offer outputs to others (trusted peers; public mode; an internal product; a customer-facing chatbot), you are a **deployer** under Article 26 and, in some flows, a **provider** under Article 16. Specifically:

- **Annex III high-risk uses** (employment, credit, healthcare, criminal justice, biometric ID, education, critical infrastructure) require a conformity assessment, post-market monitoring, human oversight, and a logged risk-management process. These obligations apply to **you**, not the project. The project disclaims fitness for these uses (see [TERMS.md §6](TERMS.md#6-no-medical-legal-financial-or-safety-critical-use)).
- **Transparency obligations** (Article 50): if you generate synthetic media or interact with humans, you must disclose that the user is interacting with an AI. Build this into your application around the daemon.
- **Limited-risk uses** (everything not Annex III and not prohibited) have minimal duties beyond transparency.
- **Prohibited uses** (Article 5: social scoring, real-time biometric ID in public spaces, subliminal manipulation, etc.) — running the daemon for these is a violation by you, not by the project.

### Codes of practice

The Commission's voluntary GPAI code of practice (finalized 2025) is targeted at GPAI model providers, not runners. Until and unless the project itself trains a model, it isn't a signatory.

## GDPR and other data-protection regimes

### Data the daemon processes

See [PRIVACY.md](PRIVACY.md) for the data inventory. Briefly: prompts, responses, chat history, optional memory entries, identity keys, paired-peer registry, optional policy file. Default location: the operator's machine.

### The project's role

The project's official channels (the repository, the release artifacts, the documentation) do not collect personal data from users of the software. No analytics, no telemetry, no installer ping (see [MANIFESTO.md](MANIFESTO.md)). The project is therefore generally **not a data controller** under Article 4(7) GDPR with respect to operators of the daemon.

### The operator's role

If you run the daemon and process the personal data of EU/UK/Swiss/Brazilian/etc. residents, you are the **controller**. Specifically:

- **Lawful basis** (GDPR Art. 6): you must have one — consent, legitimate interest, contract, etc. Local mode (your own data, on your own machine) is generally Art. 6(1)(f) legitimate-interest. Public mode (someone else's data crosses your machine) is closer to Art. 6(1)(b) contract performance — you've taken payment to process their prompt.
- **Data subject rights** (Art. 15–22): access, erasure, portability, etc. Your duty to honor them when an EU/UK/CH/BR resident asks. The daemon's `/v1/memory` DELETE endpoint, `/v1/chats/<id>` DELETE endpoint, and the on-disk JSON files are the tools.
- **Transfers** (Art. 44–49): if your daemon talks to a non-EU upstream (most are), and the upstream is processing EU personal data, you need standard contractual clauses or an adequacy decision in place with that upstream. **This is your contract with the upstream, not ours.**
- **DPIA** (Art. 35): high-risk processing requires a data-protection impact assessment. Public mode at scale is probably high-risk. Local mode is probably not.

Equivalent regimes — **CCPA/CPRA** (California), **LGPD** (Brazil), **PIPEDA** (Canada), **APPI** (Japan), **PDPA** (Singapore), **POPIA** (South Africa), the UK GDPR — impose similar but not identical duties on the operator. The principles are the same; the specifics differ.

## Money transmission, MiCA, and crypto regulation

**No rails are wired in this repo as of v0.0.50.** The `PeerPaymentPolicy` and `/v1/public-mode/quote` endpoint are policy-only; nothing settles. The rail integration plan is sketched in [`unhosted-payments/design/0011-payment-rail-integration-plan.md`](https://github.com/unhosted-ai/unhosted-payments/blob/main/design/0011-payment-rail-integration-plan.md), with a per-rail compliance map. This section is forward-looking.

### Design intent

When rails ship, payments will be **peer-to-peer between the payer's wallet and the host's wallet**, on whichever rail both sides agreed to. The project does not take custody, does not aggregate funds, does not run an escrow service (a smart contract on Base may eventually act as escrow; the contract is the custodian, not the project). This is the same posture as a freelancer accepting USDC on Base directly — the freelancer is not a money transmitter; their wallet is.

### Money transmission

- **United States.** Receiving payment for one's own services (compute time) and not transmitting funds between third parties is not money transmission under FinCEN's interpretation (31 CFR 1010.100(ff)). State-level mileage varies (NY BitLicense; California DFPI). An operator selling compute for USDC in their own state is generally fine; building a service that aggregates many operators' payments would not be.
- **EU MiCA** (Regulation (EU) 2023/1114). The host accepting a stablecoin in exchange for compute is conducting "an exchange of crypto-assets for funds, goods, or services" — the service-leg of which is MiCA-exempt as a non-financial provision. The receipt of USDC is exempt for non-issuers performing fewer than €1M in payments per year (Article 21 thresholds; verify when rail ships). Past that threshold the operator may need crypto-asset service provider (CASP) authorization.
- **UK Money Laundering Regulations.** Cryptoasset business registration with the FCA is required for "exchange providers." A host taking USDC for compute is not, on its face, an exchange.

### Sanctions on the rail

USDC has a centralized issuer (Circle) that freezes wallets at OFAC's request. Lightning is harder to police but its routes do not implicate the issuer. Operators choosing a rail inherit its sanctions posture.

## Model licenses

Each model the daemon pulls has its own license. The daemon is a runner; the weights are not the project's to relicense. Common ones the `unhosted pull` short-names point to:

- **Meta Llama family** (3.1, 3.2 — `llama3.2:1b`, `llama3.2:3b`, `llama3.1:8b`): **Llama Community License** — permissive for commercial use up to 700M MAUs, attribution required, prohibited uses listed in the license's AUP. Operators with derivative products must include "Built with Llama" attribution.
- **Qwen 2.5 family** (`qwen2.5:0.5b`, `qwen2.5-coder:7b`): **Apache 2.0** (most), some larger ones have non-commercial restrictions. Verify per release.
- **TinyLlama-1.1B-Chat** (used as the distillation base): **Apache 2.0**.

If you distill an adapter from one of these and re-publish, the adapter's license is constrained by the base's license. The project's [model card template](models/distill/model-card.template.md) preserves the AGPL header for the recipe code but says the adapter inherits the base's license — fill that in honestly when you publish.

## Content liability

### The project's role

The project does not host inference outputs, does not run a "model gallery," does not curate prompts. The project distributes a runner. **Section 230 (U.S.)** and **Article 14 e-Commerce Directive / DSA (EU)** safe-harbor protections do not directly apply because the project does not act as an information society service — but the underlying reasoning (the distributor is not a publisher of third-party content) is the project's posture by analogy.

### The operator's role

If you run a public-mode host and one of your customers prompts the model into producing copyright-infringing, defamatory, or illegal output, the legal posture depends on your jurisdiction:

- **U.S.** Section 230 protects you as a provider of an interactive computer service for the output you didn't materially contribute to. Generated outputs are a hard case; the current case law is not settled.
- **EU.** DSA Article 6 hosting safe-harbor applies to providers acting as intermediaries. A public-mode host probably qualifies; an operator who tuned the model for a specific output probably doesn't.
- **UK.** Similar to EU.
- **Most other jurisdictions.** Less developed; assume the operator is on the hook and operate accordingly.

The duty to discontinue service on signal in [ACCEPTABLE_USE.md](ACCEPTABLE_USE.md#public-mode-host-duties) is the operator's practical way to preserve any intermediary defense available.

## Children and age

See [PRIVACY.md § Children](PRIVACY.md#children) and [TERMS.md § 7](TERMS.md#7-age). The minimum `KycTier::Email` policy requirement for public-mode hosts is a weak age signal; operators wanting stronger guarantees should require `IdVerified`.

## Tax

This is a jurisdiction-specific operator obligation. Briefly:

- **U.S.** Public-mode income is self-employment income; pay quarterly estimates if it exceeds the threshold; report on Schedule SE.
- **EU.** VAT on the compute-as-a-service may apply, depending on volume and the customer's location.
- **Many jurisdictions** treat crypto-asset receipt at fair market value as taxable income on the day of receipt.

The project provides no tax advice and no tax forms.

## Intellectual property

[IP_POSTURE.md](IP_POSTURE.md) is the standing IP policy: license (AGPL-3.0-or-later), DCO requirement, defensive-publication register of mechanisms we believe are non-obvious, and an explicit list of what we do not claim is novel.

Operators preparing a commercial deployment, an acquisition, an audit, or a patent filing should read it. The novelty register is a living document; entries are added in the same PR as the code they describe, and moved to "not claimed" if later prior-art searches anticipate them.

## Reporting and contact

| Topic | Address |
| --- | --- |
| Security vulnerabilities | `security@unhosted.dev` (see [SECURITY.md](SECURITY.md)) |
| Privacy | `security@unhosted.dev` with `[privacy]` subject |
| Abuse | `abuse@unhosted.dev` |
| Legal / compliance / DMCA | `legal@unhosted.dev` |
| Sanctions concerns about a host | Report to the relevant sanctions authority directly (OFAC, OFSI, etc.) |

## Disclaimer

This document is provided **as-is**, may be inaccurate, may be out of date, and is **not legal advice**. Cross-check every claim against a current source for your jurisdiction. The project will accept pull requests correcting errors with a citation; an operator preparing for a real audit should retain qualified counsel.
