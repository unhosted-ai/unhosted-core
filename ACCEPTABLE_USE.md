# Acceptable use

This policy applies to anyone who runs the Unhosted daemon, especially in **trusted-peer** or **public** mode where the daemon's compute serves someone else, or the daemon's traffic crosses someone else's machine. It also applies to anyone publishing fine-tuned adapters or distilled models built with the recipe in [`models/distill/`](models/distill/).

Read it as **what you may not do with this software** — independent of and on top of any legal duty in your jurisdiction.

## Hard prohibitions

The following are prohibited in any mode (local, trusted, public). Violation is grounds for immediate de-pairing, refusal to serve, and reporting to law enforcement where required:

1. **CSAM and any sexual content involving minors.** Including generation, distribution, request, or processing. Hosts who detect such activity in workloads they serve **must** discontinue service to the requester and may report it to NCMEC (United States), IWF (United Kingdom), or the equivalent in their jurisdiction.
2. **Generating or facilitating non-consensual sexual imagery** of any real person, including "deepfake" image, audio, or video.
3. **Operational support for violence against people.** Includes weapons design (chemical, biological, radiological, nuclear, autonomous, or otherwise capable of mass casualties), targeting, surveillance for planned harm, or content intended to incite imminent violence.
4. **Mass-influence operations.** Generating coordinated political disinformation, election interference content, or impersonation of public officials.
5. **Cyber-offense at scale.** Writing malware for distribution, exploits for unpatched zero-days, ransomware, phishing kits, credential stuffing tools, or any pen-test artifact for systems you do not own and do not have written authorization to test.
6. **Fraud.** Generating fake identity documents, forged signatures, fraudulent invoices, or anything intended to deceive a financial, governmental, or healthcare system.
7. **Stalking, harassment, doxxing.** Generating or aggregating personal information about a private individual against their will.
8. **Discriminatory automation.** Using outputs in decisions about employment, credit, housing, healthcare, insurance, education, or law enforcement in any jurisdiction where such use is regulated (EU AI Act, Colorado AI Act, NYC Local Law 144, etc.) without the required impact assessment, transparency, and opt-out the law mandates.
9. **Sanctioned-party service.** Knowingly running inference for, hosting tools for, or accepting payment from individuals or entities on:
   - the U.S. OFAC SDN list,
   - the EU consolidated sanctions list,
   - HM Treasury (UK) sanctions list,
   - UN Security Council Consolidated List,
   - your own country's equivalent.
10. **Circumventing the law of either party's jurisdiction.** Including jurisdiction-shopping to defeat a court order, regulatory bar, age-of-majority requirement, or content classification rule.

## Public-mode host duties

If you set a `PeerPaymentPolicy` that accepts any payment rail (`accepted_rails` non-empty), you become a **host** under this policy. Hosts must:

1. **Configure a sanctions block-list.** As a minimum, block the countries currently under comprehensive U.S. OFAC sanctions: `KP` (North Korea), `IR` (Iran), `SY` (Syria), `CU` (Cuba), and the occupied regions of Ukraine (`UA`-specific subdivisions are out of `Country`'s ISO-2 scope; if you're concerned, do not accept payers from `RU` or `UA` and add `BY`). The daemon's default policy is `closed` — *you* must opt in to anything broader, and the default block-list ships in the daemon when you do. Add to it.
2. **Require KYC tier `email` or higher** unless your jurisdiction explicitly permits anonymous compute-for-pay (most don't, especially in payment-rail-touched flows).
3. **Set a price ceiling on what your machine will accept per quote.** This is the host's call, not a policy field — it lives in your `unit_price_micros` quote response. A misconfiguration that serves trillion-token jobs for free is a denial-of-service against your own electricity bill, not a legal issue, but the rest of this list is.
4. **Discontinue service on signal.** If a payer's quote or job shows signs of any prohibition above, **stop**. Cancel the in-flight job. Don't issue a receipt. Don't return tokens to the rail.
5. **Keep signed receipts.** When rails are wired, every served job ends in a `SignedReceipt` (see [`unhosted-payments/core/src/receipt.rs`](https://github.com/unhosted-ai/unhosted-payments/blob/main/core/src/receipt.rs)). Keep them for at least the limitation period of your jurisdiction — they're the only audit trail a host has.

## Payer duties

If you call a host's `/v1/public-mode/quote` endpoint, you are a **payer** under this policy. Payers must:

1. **Not lie in `PayerContext`.** The `kyc` tier and `country` you sign into the quote body bind you to whatever assertions the host's policy depends on. A signed payer context misrepresenting your country is fraud against the host and, depending on the host's jurisdiction, against the host's regulators.
2. **Pay only via rails you're allowed to use.** USDC on Base is not legal for everyone everywhere. KYC tier is host-asserted; your duty to be eligible for the rail is your own.
3. **Not use compute for prohibited workloads.** All ten prohibitions above apply to you. The receipt the host signs is evidence of what they served you; it is not a defense for what you did with the output.

## Model and adapter publication

If you publish a distilled adapter to Hugging Face Hub via `unhosted distill push` (or by hand), the model card the recipe generates is **a representation** about the adapter. Don't lie in it:

- The base model must be the real base.
- The training data provenance must be honest. "Synthetic, gen_data.py against gpt-4o-mini" is a different legal posture than "scraped from copyrighted sources without notice"; do not conflate them.
- The eval numbers must come from a real eval. Cherry-picking is dishonest; cooking the eval set so the adapter wins is fraud.
- Mark adapters likely to produce restricted output (medical, legal, biosecurity-adjacent) with the appropriate tags so consumers know what they're loading.

## Enforcement

Unhosted is open-source software. There is no Unhosted Inc. with a Trust & Safety team and an inbox. Enforcement against violators is each host's own duty to their own peers, plus whatever the violator's jurisdiction does on its own. The Unhosted project's role is:

- This policy is what a contribution to the repo represents about its author and reviewers.
- A user reported to be operating in violation of this policy may be de-listed from the project's discovery / pairing index (when one exists) and excluded from any future managed services (none planned).
- Pull requests that materially weaken sanctions screening, age checks, or the prohibitions above will be closed without merge.

## Reporting

Suspected abuse using the software:

- If it involves child safety: report directly to NCMEC ([report.cybertip.org](https://report.cybertip.org/)) or IWF ([iwf.org.uk/report](https://www.iwf.org.uk/report)) **first**, then notify **abuse@unhosted.dev** with the report ID.
- If it involves sanctions: report to the relevant sanctions authority (OFAC, OFSI, etc.) directly.
- Anything else: **abuse@unhosted.dev**.

See [SECURITY.md](SECURITY.md) for vulnerabilities (separate intake).

## Changes

This file is part of the repo. Changes are commits. The version of the policy that applies to a given build is the version in that build's `git log`. Operators of public-mode hosts are responsible for tracking commits to this file and applying changes — there is no push notification.
