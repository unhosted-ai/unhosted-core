# ADR 0012: In-core agent runtime

Status: Proposed (2026-05-21). First implementation lands as the slice that follows this ADR.

## TL;DR

`unhosted-core` runs an LLM in isolation today: prompt in, tokens out, one round-trip per request. Add an in-core **agent runtime** — a new module + a new endpoint `POST /v1/agents/run` — that lets the model drive a tool-call loop until it produces a final answer or hits a guardrail. Tools are a small allow-listed registry; every tool call emits an audit event and goes through the same DLP / sanctions hooks chat does. Stays in-core (not a separate repo) so the agent layer inherits the daemon's auth, audit, metrics, and policy boundary for free.

## Context

The daemon's current capabilities are inference-only:

- Chat completions through `/v1/chat/completions` (OpenAI-compatible).
- Local conversation history (`chats.rs`).
- Private memory — past-chat summaries retrievable as system context (`memory.rs`).
- Web fetch with private-IP blocking (`web_fetch.rs`) — but only the CLI / browser can call it. The model cannot.
- MCP plugin shim (`unhosted-plugins`) — but this is *inbound*: it lets external MCP hosts (Claude Desktop, Cursor) use unhosted as their model provider. The model running on unhosted has no MCP tools available *to it*.

Net effect: the model is a passenger, not the driver. It can answer questions but cannot fetch a page to inform the answer, look up its own past conversations, or compose multi-step work.

The pieces required to close that gap are:

1. **Tool-call loop.** The model emits OpenAI-style `tool_calls`; the daemon executes them and feeds results back as `role:"tool"` messages; the conversation iterates until a final answer.
2. **Tool registry.** A small allow-listed set of side-effects the agent can take.
3. **Guardrails.** Max steps, max tokens, max wall-clock, max tool-calls per step.
4. **Audit + metrics + DLP integration** — inherited if the runtime lives in-core.

## Decision

### Where it lives

**In-core**, as a new module `agent.rs` and a new endpoint `POST /v1/agents/run`. Not a separate `unhosted-agents` crate or repo.

Reasoning:

- **Auth boundary already exists.** Caller is classified as loopback / bearer / signed-peer the same way every other handler classifies. An agent run is privileged like a chat completion; the agent module reuses `require_auth(_, false)`.
- **Audit substrate already exists.** Each tool call emits an `AuditEvent::AgentToolCall`; each run emits `AgentRunStarted` and `AgentRunCompleted`. No new SIEM integration to build.
- **DLP hook already exists.** Inputs to the model pass through `dlp::check` the same way chat completions do.
- **Metrics already exist.** `agent_runs_total`, `agent_steps_total`, `agent_tool_calls_total` slot into the existing Prometheus surface.
- **Sanctions defaults already exist.** Agents running on a host with public-mode-policy.json apply the same comprehensive-OFAC defaults.

A separate-repo architecture would force every one of those bridges to be built explicitly. Not worth the cost for a project where the daemon is the trust boundary.

### Wire shape

```text
POST /v1/agents/run
Content-Type: application/json

{
  "goal":       "summarize the changelog from https://example.com/CHANGELOG.md",
  "tools":      ["web_fetch", "search_memory"],
  "max_steps":  8,
  "max_tokens": 4096,
  "max_seconds": 60,
  "model":      "auto"
}

→ 200 OK
Content-Type: application/json

{
  "final_answer": "...",
  "steps": [
    { "step": 0, "kind": "model_message",  "content": "..." },
    { "step": 1, "kind": "tool_call",      "tool": "web_fetch", "args_hash": "...", "result_chars": 4823 },
    { "step": 2, "kind": "model_message",  "content": "..." }
  ],
  "stopped_because": "final_answer",
  "tokens_used":   1834
}
```

`stopped_because` is one of `final_answer | max_steps | max_tokens | max_seconds | tool_error | dlp_blocked`.

`args_hash` is a SHA-256 prefix of the canonical-JSON tool args — not the args themselves, so the response body doesn't leak (e.g.) URLs the user fetched. Full args are in the audit feed for operators with SIEM access.

### Initial tool registry

Three tools, narrow on purpose:

| Tool | Purpose | Existing code reused |
|---|---|---|
| `web_fetch` | Fetch a single URL, return its text body (private IPs blocked) | `web_fetch::fetch` — already shipped, already SSRF-safe |
| `search_memory` | Query private memory for relevant past-chat summaries | `memory::query` — already shipped |
| `list_models` | Return the list of model names available on the daemon's configured upstreams | Read from the existing upstream probe |

Notes:

- No `read_file` or `run_command` in slice 1. Filesystem and shell access materially expand the blast radius; they need a separate ADR addressing sandboxing.
- `web_fetch` already blocks private IPs (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, link-local, loopback). The agent layer inherits that.
- The model receives only tools listed in the request. The daemon never injects a tool the caller didn't ask for. Operators wanting to permanently disable a tool patch the registry in code.

### Guardrails

Hard caps enforced inside `run_agent`:

| Cap | Default | Hard limit |
|---|---|---|
| `max_steps` | 8 | 32 |
| `max_tokens` (sum across steps) | 4096 | 32 768 |
| `max_seconds` (wall clock) | 60 | 600 |
| `max_tool_calls_per_step` | 4 | 8 |

Caller-supplied values above the hard limit are silently clamped down. Hard limits exist so a misconfigured run can't burn a GPU for an hour.

### Authentication

Identical to `/v1/chat/completions`:

- Loopback → free pass.
- Off-loopback non-peer → bearer token required.
- Paired peer → Ed25519 signed-request required.

Agent runs are not categorically more privileged than chat completions — they're a different *shape* of LLM call, not a privileged operation. A caller who can chat-completion can also agent-run.

### DLP integration

The agent's first model call carries the user's goal as a user-role message; subsequent model calls carry tool results as `role:"tool"` messages. If a DLP config is loaded:

1. The initial goal goes through `dlp::check` before the first model call. Block → 422.
2. Tool results that include text (e.g., a `web_fetch` body) go through `dlp::check` before being fed to the model. Block → step terminates with `stopped_because: dlp_blocked`.

A `Block` decision at any point ends the run, emits `AgentRunCompleted { stopped_because: "dlp_blocked", reason }`, and returns 422.

### Audit events

Three new variants on `AuditEvent`:

```rust
AgentRunStarted   { ts, caller, goal_hash, tools, max_steps, max_tokens, max_seconds }
AgentToolCall     { ts, run_id, step, tool, args_hash, result_chars, error }
AgentRunCompleted { ts, run_id, stopped_because, steps_used, tokens_used }
```

`goal_hash` and `args_hash` are SHA-256 prefixes — the audit feed must not leak the actual prompt content to a SIEM operator. `result_chars` is enough for "this tool returned a large/small result" without leaking content.

The full goal + full tool args are logged at debug level via `tracing` (off by default; operators opt in via `RUST_LOG=unhosted_core::agent=debug`). This matches how the platform already handles raw chat content — never in the audit feed, only behind explicit debug-level tracing.

### Metrics

Three new counters added to `metrics.rs`:

```text
unhosted_agent_runs_total          counter
unhosted_agent_steps_total         counter (sum across all runs)
unhosted_agent_tool_calls_total    counter (sum across all runs)
unhosted_agent_runs_stopped_by{reason="final_answer|max_steps|..."}  counter (5 values)
```

The `stopped_by` family lets an operator alert on "agent runs blowing past max_steps faster than usual" — a signal of a model getting stuck in a tool-call loop.

### Model selection

`model: "auto"` (default) routes through the existing upstream-selection logic that chat completions use — vram-pool when running, otherwise the configured `llama_server_url`, otherwise auto-detect.

Caller can specify a concrete model id (`"llama3.1:8b"`) and that name flows through to the upstream the same way a chat-completions body's `model` field does.

The model needs to support OpenAI-style tool calling. Most modern open-weights models do (Llama 3.1+, Qwen 2.5, Mistral 7B v0.3+). For a model that doesn't, the daemon returns 422 `model_does_not_support_tool_use` after the first turn.

## Implementation slices

### Slice 1 — minimum agent runtime

What ships:

- `agent.rs` module with `run_agent(...)`.
- `POST /v1/agents/run` endpoint, auth-gated like chat.
- Tool registry covering `web_fetch`, `search_memory`, `list_models`.
- Guardrails: all four caps enforced.
- DLP integration: initial goal + tool results both run through `dlp::check`.
- Audit emit at run start, per tool call, at run end.
- Metrics for runs, steps, tool calls, stopped-by reason.
- Unit tests covering each tool, each guardrail, the DLP block path, and a wiremock-backed end-to-end run.

What's out:

- No CLI subcommand yet. Callers hit the endpoint directly.
- No Web UI surface yet. Agents are headless in slice 1.
- No persistent run-id / resumption. Each `POST /v1/agents/run` is one-shot.

### Slice 2 — `unhosted run --goal` subcommand

CLI wrapper around the endpoint. Streams the step trace to stdout so the operator sees the model thinking.

### Slice 3 — Web UI surface

New tab in the chat UI: "Agent mode" toggle. Goal field, tool checklist, max-steps slider, step-by-step trace renderer.

### Slice 4 — additional tools (separate ADR)

Filesystem (`read_file`, `list_dir`), shell (`run_command`), and MCP-as-tool (let the agent call MCP tools exposed by `unhosted-plugins`) each warrant their own ADR because they expand the blast radius and need sandboxing analysis.

## Consequences

- **Pro:** the LLM stops being a passenger. The daemon can perform multi-step work without the caller wiring it in their own agent framework.
- **Pro:** every existing guarantee (audit, DLP, sanctions, metrics, auth) extends to agent runs without code duplication.
- **Pro:** the agent runtime is a thin wrapper — most of the work is reusing existing modules. Slice 1 is ~500-700 LOC.
- **Con:** in-core agent runtime means model authors who want to swap to a different runtime (planning-based, ReAct-style, AutoGen-style) have to fork the daemon. Mitigated by `POST /v1/agents/run` being one path of many — callers wanting their own runtime continue to use `POST /v1/chat/completions` and orchestrate externally.
- **Con:** tool execution inside the daemon means a bug in a tool can affect the daemon process. Mitigated by the narrow initial tool set (web fetch + memory query are read-only, no shell, no filesystem).
- **Con:** running an agent with tools enabled has roughly 2-10× the token cost of a single chat completion. Operators should be aware of the cost shape; the metrics counter `unhosted_agent_runs_total` plus existing token-counting puts that in dashboards.

## Open questions

- **Cross-step memory between runs.** If a user calls `/v1/agents/run` twice in succession, the second run has no knowledge of the first. Is that the right default, or should agent runs inherit the same conversation thread chat completions use? Likely no for v1 (each run is one shot), but worth revisiting once usage shows whether it matters.
- **Tool error semantics.** Today's plan: a tool error becomes a `role:"tool"` message containing the error, and the model decides whether to retry / recover / give up. Alternative: a tool error aborts the run with `stopped_because: tool_error`. Default to the former (model recovers); add a `strict_tool_errors: true` flag for callers wanting the latter.
- **MCP-as-tool.** Letting the agent call MCP tools exposed by `unhosted-plugins` would massively expand its reach. Defer to a separate ADR — needs analysis of which MCP servers are trustworthy enough to expose to an autonomous loop.

## License

This ADR is AGPL-3.0-or-later, same as the rest of `unhosted-core`. The author timestamps it via the git commit that adds it.
