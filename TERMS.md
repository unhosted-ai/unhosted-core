# Terms of use

Unhosted is open-source software distributed under the [GNU Affero General Public License v3.0 or later](LICENSE) ("AGPL-3.0-or-later"). This document is **not a service contract** — there is no service. It is the project's statement of how the software is intended to be used, what it does not promise, and which risks the user takes on by running it.

If you download or use any binary, build, or release artifact of this repository, you accept the AGPL-3.0-or-later license **and** the following points.

## 1. No service, no provider

There is no "Unhosted Inc.", no managed cloud, no hosted endpoint, no paid tier, no support obligation. The project consists of source code and the right to run it. When the daemon talks to anything off your machine, it is talking to **a third party you chose** (your upstream LLM, Hugging Face for model downloads, Cloudflare for tunneling, a paired peer, a payment rail). Those third parties' terms govern those interactions, not these.

## 2. No warranty of any kind

The license text is binding; this is the plain-English restatement:

The software is provided **as-is**. There is **no warranty** of merchantability, fitness for a particular purpose, non-infringement, accuracy, completeness, reliability, security, or non-interruption. Specifically:

- **Model outputs may be wrong.** Large language models hallucinate. Outputs may be factually inaccurate, defamatory, infringing, or unsafe. **Do not** rely on output for medical, legal, financial, safety-critical, or life-decision purposes.
- **The cluster may drop requests.** Peers go offline. mDNS announces stale addresses. Cloudflare tunnels rotate. Routing is best-effort.
- **The payment-mode scaffolding is pre-production.** No rails are wired as of v0.0.45. Quote endpoints exist; payment endpoints do not. Anyone treating any current build as a settlement system is in error.

## 3. Limitation of liability

To the maximum extent permitted by law, contributors to this repository — including the original author, current maintainers, and any other commit author — are **not liable** for any damages of any kind arising from the use or inability to use the software, including direct, indirect, incidental, consequential, exemplary, or punitive damages. This includes (without limitation) lost profits, lost data, electricity cost, GPU wear, reputational harm, regulatory penalties, and damages caused by third-party services the daemon was configured to use.

This is the AGPL liability disclaimer restated. Where applicable law does not permit a full disclaimer (some consumer-protection regimes), liability is limited to the maximum extent that law permits.

## 4. You are the operator

When you run the daemon, **you** are the operator of that node, including any compute it offers to others in trusted or public mode. Specifically:

- **You** are the data controller of any personal data the daemon processes (yours or anyone else's). See [PRIVACY.md](PRIVACY.md) and [COMPLIANCE.md](COMPLIANCE.md#gdpr-and-other-data-protection-regimes).
- **You** are responsible for the legality of the workloads your daemon serves under your jurisdiction's law. See [ACCEPTABLE_USE.md](ACCEPTABLE_USE.md).
- **You** are responsible for the licenses of the models you pull. The daemon is a runner; the weights are not ours. See [COMPLIANCE.md](COMPLIANCE.md#model-licenses).
- **You** are responsible for any taxes due on income received from running public-mode inference for pay.
- **You** are responsible for KYC, AML, sanctions screening, and money-transmission registration to the extent your jurisdiction requires them. The `PeerPaymentPolicy.blocked_countries` field is a tool, not a complete compliance program.

## 5. Public mode is peer-to-peer

If and when payment rails ship, payment in public mode is **between the payer and the host's chosen rail** — between two independent parties using the same software. No part of the Unhosted project takes custody of funds, brokers a transaction, or acts as an intermediary. The project does not run a money-transmitter business and does not intend to. If your jurisdiction would classify what *you* are doing as money transmission, that is on you to comply with or to not do.

## 6. No medical, legal, financial, or safety-critical use

The software is not certified for, designed for, or intended for use in:

- Medical diagnosis, treatment, or any clinical decision-support context (regulated under FDA, EU MDR, etc.).
- Legal advice or court filings without a licensed attorney reviewing every output.
- Financial advice, investment recommendation, or automated trading where regulators require human supervision.
- Aviation, automotive, industrial control, life support, or any application where failure could result in death or serious injury.
- Hiring, lending, housing, insurance, healthcare, education, or criminal-justice decisions in jurisdictions where those uses are regulated (EU AI Act high-risk Annex III; U.S. state-level AI laws; etc.) without the impact assessment and human oversight those regimes require.

Running the daemon to assist with any of the above is your decision and your liability. The project disclaims any duty of care that would arise from such use.

## 7. Age

You must be old enough to enter into a binding contract in your jurisdiction to use the software (18 in most places; sometimes 16, sometimes 21). The software is not intended for users under 13 in any jurisdiction (COPPA-equivalent baseline). If you operate a host in public mode, your `min_kyc` policy must be at minimum `email` to provide a weak age signal — most major email providers have their own age floor.

## 8. Sanctions and export controls

The software is published from the United States. Use of it is subject to:

- U.S. Export Administration Regulations (EAR). The cryptography it uses (Ed25519, AES-GCM, TLS) qualifies for license exception ENC under 15 CFR 740.17 as publicly available source-code, but **redistribution from a comprehensively sanctioned jurisdiction is prohibited**.
- U.S. OFAC sanctions. Do not download, run, distribute, or accept payment from individuals or entities on the SDN list or persons ordinarily resident in comprehensively sanctioned jurisdictions (currently Cuba, Iran, North Korea, Syria, the Crimea / Donetsk / Luhansk regions of Ukraine, and any others added during your use).
- Equivalent regimes in your jurisdiction (EU, UK, UN, Switzerland, Japan, Australia, etc.). The strictest set wins.

The project will not knowingly accept contributions from, or distribute releases to, sanctioned parties.

## 9. Trademark

"Unhosted" and the lockup at [`assets/lockup.svg`](assets/lockup.svg) are project marks of the Unhosted project. Use of the marks to identify modified forks ("based on Unhosted, with my own modifications") is permitted. Use of the marks to imply endorsement of an unrelated product or service is not. AGPL governs the **code**; it does not grant trademark rights.

## 10. Governing law and forum

Where law allows the project to choose: this document and any non-AGPL dispute about the software are governed by the laws of the United States and the State of California, without regard to conflict-of-laws principles. Any AGPL dispute is governed by the AGPL's own terms. Where local consumer-protection law gives a user a non-waivable right (EU, UK, Australia, others), that right is unaffected.

## 11. Changes

This document is part of the source tree. The version that governs a given build is the version in that build's `git log`. There is no centralized notification of changes. If you operate a public-mode host, you accept the duty to track this file.

## 12. Contact

Legal questions: **legal@unhosted.dev**. Privacy: **security@unhosted.dev** with `[privacy]` subject. Abuse: **abuse@unhosted.dev**. These are best-effort intakes operated by the maintainers; treat reply times as a few business days.
