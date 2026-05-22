# Use cases

The use cases below are the canonical targets unhosted is built for. They share three properties: the data must not leave the user's hardware, the workload is high enough that per-token SaaS pricing would dominate, and the workflow benefits from an auditable agent loop rather than a single chat turn.

This document is the project's product reference. It groups the use cases by how well unhosted's architecture serves them today, names the tool surface each one needs, and is explicit about cases where a SaaS API is genuinely a better fit. Each entry's "tools needed" column reflects the live tool registry as of the latest tagged release.

Tier 1 cases are the lead targets — the project's near-term roadmap is organized around making them feel polished. Tier 2 cases are real fits but need work outside the open-source core (commercial-tier legal surface, signed contracts, attestations) before they can be responsibly demoed. Tier 3 cases are out of scope; they're listed so the project doesn't chase the wrong audience.

## Tier 1 — lead targets

### 1. Codebase exploration and refactoring

A developer points an agent at a local repository and asks structural questions: where authentication flows, which modules depend on which crate, what changed in the last release. The agent crawls source files, follows references, and produces an answer locally.

**Why it fits**: a codebase is privacy-sensitive enough that engineers refuse to forward it to a third-party assistant by default. Volume is high (many files, many tokens). Tool needs are read-only filesystem plus lightweight shell — exactly the slice 4 of the agent runtime roadmap.

**Tools needed**: `read_file` (shipped v0.0.71), `list_dir` (slice 4b), `grep` (small follow-up), `git_log` (read-only). All read-only by construction; no shell escape required for the typical query.

**Demo target**: "Explain how authentication flows through this codebase" — the agent crawls from `main.rs`, follows the call graph via `grep`, summarizes. The output is internally testable against the codebase the developer can see for themselves.

### 2. Personal research assistant

Academics and independent researchers maintain a corpus of papers, notes, and half-finished drafts that they will not upload to a cloud assistant. The agent reads from that corpus on demand, fetches public sources to fill gaps, and tracks citations rigorously.

**Why it fits**: the corpus is privacy-sensitive; citation accuracy is mandatory; the use is high-volume enough that per-token SaaS pricing would be prohibitive.

**Tools needed**: `read_file` for text artifacts; `web_fetch` for arXiv abstracts and citation chains (shipped); `search_memory` for "what have I read before about this topic" (shipped); `extract_text` for PDFs (follow-up — UTF-8 only today); `cite` for structured `{ claim, source_url, retrieved_at }` emissions (follow-up).

**Demo target**: "Find papers I've already read that discuss diffusion-model alignment, summarize the disagreements between them, and identify the bibliography gaps."

### 3. Regulatory and compliance monitoring

Monitor SEC filings, FDA actions, sanctions updates, or regulatory news for a defined set of companies or topics. Outputs are structured (claim plus source plus retrieved-at) and the audit trail of each agent run is itself a compliance artifact.

**Why it fits**: high-volume polling, structured outputs, audit-trail mandatory by the user's own profession. The signed-receipts substrate plus the audit-log SSE feed are first-class differentiators for compliance teams' own paper trails.

**Tools needed**: `web_fetch` (shipped); a structured-source tool such as `fetch_sec_filing(cik, type, period)` (lives in the planned market-research MCP plugin); `now()` injection so the agent understands "this quarter"; scheduled-run capability (cron-style) above the agent endpoint.

**Demo target**: "Every Monday at 07:00, fetch new 10-Qs from these 50 tickers, summarize material risk factor changes versus the last quarter, and flag anything that triggers our risk matrix." Self-hosted; every output traceable.

### 4. Internal knowledge base agent

An organization with engineering wikis, runbooks, post-mortems, and customer notes wants an agent that can search and summarize without exposing any of it to a third party. The audit feed and DLP hook are the differentiators against cloud-hosted assistants for this use case.

**Why it fits**: the corpus is the organization's institutional knowledge; the user wants the privacy guarantee in writing; the workload is high enough to make per-token pricing painful.

**Tools needed**: `read_file` (shipped, allow-list the docs directory); `search_memory` (shipped, indexes summaries); `vector_search` over the docs corpus for semantic retrieval (follow-up).

**Demo target**: "What did we decide about the v3 migration's rollback strategy? Show me the post-mortem from the last related incident."

### 5. Personal finance and tax preparation

Bank CSV exports, transaction histories, and tax forms are sensitive structured data the user has locally. The agent does deterministic arithmetic across CSVs and produces categorizations or summaries that go straight into the user's spreadsheet.

**Why it fits**: high accuracy stakes (a hallucinated number is meaningfully wrong), strict privacy, structured data that benefits from a real SQL engine rather than the model's own arithmetic.

**Tools needed**: `read_file` (shipped); `sql_query` over CSVs via embedded DuckDB (follow-up); `now()` injection so the agent knows the tax year and quarter.

**Demo target**: "Categorize last quarter's transactions, flag anything that looks like a deductible business expense I haven't already categorized, and total the gas plus mileage entries."

## Tier 2 — fit on paper, blocked on commercial-tier surface

These use cases are technically a strong fit for the open-source platform but should not be publicly demoed until the commercial enterprise tier ships the legal surface they require. The architecture supports them today; the contract surface does not.

### 6. Healthcare clinical-note assist

Clinical note drafting, medication reconciliation, patient summary refresh. The architecture maps cleanly to HIPAA Technical Safeguards (see [HIPAA_TECHNICAL_SAFEGUARDS.md](https://github.com/unhosted-ai/unhosted-enterprise/blob/main/legal/HIPAA_TECHNICAL_SAFEGUARDS.md) in the commercial tier). Demoing this responsibly requires a signed Business Associate Agreement with an incorporated legal entity. The open-source repo cannot sign one; the commercial tier is the path.

### 7. Legal contract review

Read clauses, compare to a known-good template, summarize delta. High-trust corpus, audit trail mandatory. Same blocker as healthcare: serious deployment requires a signed agreement and an indemnification clause that only a commercial entity can offer.

### 8. Customer-support draft generation

Read past tickets from a support tool's CSV export, draft a response in the same voice. Architectural fit is moderate; the differentiation against CRM-native AI features is weak unless the user organization is unusually privacy-sensitive about its support corpus.

## Tier 3 — out of scope

These use cases are listed so the project does not chase the wrong audience. A SaaS API is the better tool for the job:

| Use case | Why a SaaS API is the better choice |
|---|---|
| Single-turn quick Q&A on public knowledge | The model is the only meaningful surface; tool registry, audit, and sandbox add weight without value. ChatGPT or Claude.ai is faster to reach. |
| Image and video generation | Local hardware cannot keep pace with frontier image and video models. SaaS providers have purpose-built inference clusters. |
| Multi-language voice agents | Real-time voice requires interrupt handling and audio streaming outside the daemon's current shape. |
| One-shot writing assistance | The privacy stakes do not justify the setup friction. |
| Frontier-reasoning workloads on consumer hardware | Local hardware is fundamentally behind the closed-weights frontier; users on consumer machines will be disappointed. |

## How this list evolves

Tier 1 use cases drive the agent-runtime tool roadmap. Each new tool the agent registry exposes is justified by a tier-1 use case it unlocks; tools that do not connect to a tier-1 use case are deferred to a separate ADR per [ADR-0012](https://github.com/unhosted-ai/unhosted-core/blob/main/design/0012-agent-runtime.md)'s "slice 4 — additional tools" section.

Tier 2 use cases unlock as the commercial enterprise tier ships its legal surface. Each maps to a specific compliance gate (BAA for healthcare, indemnification for legal, signed support contract for any customer that needs one).

Tier 3 use cases are intentionally permanent — they are not a roadmap item; they are the project's "what we are not."

---

*Last reviewed: 2026-05-21. The tool-status column reflects the latest tagged release; check the README's status table for the canonical source of truth on what has actually shipped.*
