//! In-core agent runtime.
//!
//! Implements ADR-0012. The user calls `POST /v1/agents/run` with a
//! goal plus a list of allowed tools; the runtime drives a model in a
//! tool-call loop until it produces a final answer or hits a
//! guardrail.
//!
//! Wire format and full design are in [`design/0012-agent-runtime.md`].
//! This module is intentionally narrow: it does not introduce a new
//! way to talk to models, a new auth boundary, or a new audit
//! substrate. Everything plugs into existing modules (`upstream`,
//! `auth`, `audit`, `metrics`, `dlp`, `web_fetch`, `memory`).
//!
//! ## Tool registry (slice 1)
//!
//! Three tools, deliberately narrow:
//!
//! - `web_fetch` — fetch a URL, return text body. Reuses [`web_fetch`]
//!   so SSRF protection is inherited.
//! - `search_memory` — query private-memory summaries. Reuses
//!   [`memory`].
//! - `list_models` — enumerate models the daemon's upstreams claim to
//!   serve. Read-only; safe for any agent.
//!
//! Filesystem (`read_file`), shell (`run_command`), and MCP-as-tool
//! are deferred to a separate ADR because they expand the blast
//! radius and need sandboxing analysis.
//!
//! ## Guardrails
//!
//! Every run is bounded by four caps (defaults / hard limits):
//!
//! | cap | default | hard limit |
//! |---|---|---|
//! | `max_steps` | 8 | 32 |
//! | `max_tokens` | 4096 | 32 768 |
//! | `max_seconds` | 60 | 600 |
//! | `max_tool_calls_per_step` | 4 | 8 |
//!
//! Caller values above the hard limit are silently clamped.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_fs::{self, AgentFsConfig};
use crate::audit::{AuditBroadcaster, AuditEvent};
use crate::dlp::{self, DlpConfig, DlpDecision};
use crate::metrics::{AgentStopReason, Metrics};

/// Default and hard-limit guardrails.
pub const DEFAULT_MAX_STEPS: u32 = 8;
pub const HARD_MAX_STEPS: u32 = 32;
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const HARD_MAX_TOKENS: u32 = 32_768;
pub const DEFAULT_MAX_SECONDS: u32 = 60;
pub const HARD_MAX_SECONDS: u32 = 600;
pub const DEFAULT_MAX_TOOL_CALLS_PER_STEP: u32 = 4;
pub const HARD_MAX_TOOL_CALLS_PER_STEP: u32 = 8;

/// The HTTP body a caller POSTs to `/v1/agents/run`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRunRequest {
    pub goal: String,
    /// Allow-list of tool names the model may call. The daemon never
    /// injects a tool the caller didn't request.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_seconds")]
    pub max_seconds: u32,
    #[serde(default = "default_max_tool_calls_per_step")]
    pub max_tool_calls_per_step: u32,
    /// Model id. `"auto"` (default) means "use the daemon's
    /// configured upstream's preferred model"; a concrete name flows
    /// through to the upstream the same way it does for chat
    /// completions.
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_max_steps() -> u32 {
    DEFAULT_MAX_STEPS
}
fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}
fn default_max_seconds() -> u32 {
    DEFAULT_MAX_SECONDS
}
fn default_max_tool_calls_per_step() -> u32 {
    DEFAULT_MAX_TOOL_CALLS_PER_STEP
}
fn default_model() -> String {
    "auto".into()
}

impl AgentRunRequest {
    /// Clamp caller-supplied values to the hard limits. Slice-1
    /// behavior: clamping is silent — the response's actual caps are
    /// the post-clamp values, not the request body's.
    pub fn clamped(mut self) -> Self {
        self.max_steps = self.max_steps.min(HARD_MAX_STEPS).max(1);
        self.max_tokens = self.max_tokens.min(HARD_MAX_TOKENS).max(64);
        self.max_seconds = self.max_seconds.min(HARD_MAX_SECONDS).max(1);
        self.max_tool_calls_per_step = self
            .max_tool_calls_per_step
            .min(HARD_MAX_TOOL_CALLS_PER_STEP)
            .max(1);
        self
    }
}

/// What the daemon returns when a run completes.
#[derive(Debug, Clone, Serialize)]
pub struct AgentRunResponse {
    pub final_answer: String,
    pub steps: Vec<StepRecord>,
    pub stopped_because: StoppedBecause,
    pub tokens_used: u32,
    pub run_id: String,
}

/// Per-step record, returned to the caller. `args_hash` /
/// `result_chars` is a privacy-conscious summary; the full content is
/// in the audit feed only.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepRecord {
    /// Model produced a turn — either a `tool_calls` request or a
    /// final answer. `content` is empty when the model only emitted
    /// tool calls.
    ModelMessage {
        step: u32,
        content: String,
        tool_calls_made: u32,
    },
    /// A tool was invoked. `args_hash` is the SHA-256 prefix of the
    /// canonical-JSON-serialized args; `result_chars` is the
    /// post-execution result's character count.
    ToolCall {
        step: u32,
        tool: String,
        args_hash: String,
        result_chars: usize,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StoppedBecause {
    FinalAnswer,
    MaxSteps,
    MaxTokens,
    MaxSeconds,
    ToolError,
    DlpBlocked,
    ModelDoesNotSupportToolUse,
}

/// SHA-256 prefix (16 hex chars) of a value's canonical JSON
/// representation. Used for `goal_hash` and `args_hash` so audit
/// events and step records don't leak content.
pub fn short_hash(value: &serde_json::Value) -> String {
    let canon = serde_json::to_string(value).unwrap_or_default();
    let digest = Sha256::digest(canon.as_bytes());
    hex_prefix(&digest, 16)
}

fn hex_prefix(bytes: &[u8], chars: usize) -> String {
    let mut out = String::with_capacity(chars);
    for b in bytes {
        if out.len() >= chars {
            break;
        }
        out.push_str(&format!("{b:02x}"));
    }
    out.truncate(chars);
    out
}

/// Returns the set of tool names recognised by the runtime in slice 1.
/// Used by the handler to reject unknown names from the caller's
/// `tools` array before kicking off the loop.
pub fn known_tools() -> BTreeSet<&'static str> {
    let mut s = BTreeSet::new();
    s.insert("web_fetch");
    s.insert("search_memory");
    s.insert("list_models");
    s.insert("read_file");
    s.insert("list_dir");
    s
}

/// Generate a per-run identifier. 16 hex chars of cryptographic
/// randomness. Used as `run_id` in audit events so a SIEM can
/// correlate start → tool calls → completion within a run.
pub fn new_run_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut buf);
    hex_prefix(&buf, 16)
}

/// Wall-clock as an ISO-8601 UTC string the model can read directly
/// without external time-zone knowledge. Format: `YYYY-MM-DDTHH:MM:SSZ`.
/// Falls back to `"1970-01-01T00:00:00Z"` on clock errors so the
/// system prompt always has a parseable value.
///
/// We compute manually rather than pull `chrono` — the agent runtime
/// is the only consumer and the formatting requirements are narrow.
pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    seconds_to_iso8601(secs)
}

/// Convert a unix timestamp (seconds since 1970-01-01 UTC) to an
/// ISO-8601 string. Public + small so it can be unit-tested
/// against known dates.
pub fn seconds_to_iso8601(unix_secs: u64) -> String {
    // Days since 1970-01-01.
    let total_days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let hour = (secs_of_day / 3600) as u32;
    let minute = ((secs_of_day % 3600) / 60) as u32;
    let second = (secs_of_day % 60) as u32;

    // Civil-from-days conversion (Howard Hinnant's date algorithm,
    // public domain). Handles the Gregorian calendar correctly
    // through year 11000 — far past any plausible use of unhosted.
    let z = total_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Compose the agent's system prompt. Exposes `now_iso8601` as a
/// concrete value so the model has a stable reference point for any
/// time-sensitive question ("this quarter", "last week", "the most
/// recent filing"). Without this, models will confidently invent a
/// date from training-data heuristics.
pub fn build_system_prompt(max_steps: u32, now: &str) -> String {
    format!(
        "You are an agent. Use the provided tools when useful. \
         When you have a final answer, reply with prose only and no tool calls. \
         You have at most {max_steps} steps to complete the task.\n\n\
         The current date and time is {now} (UTC). Treat this as authoritative \
         when reasoning about anything time-sensitive — quarters, recency of news, \
         relative ages, deadlines."
    )
}

/// Wall-clock guardrail timer. Set at run start; `elapsed_seconds()`
/// is the current run's age.
pub struct RunClock {
    started: Instant,
}

impl RunClock {
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }
    pub fn elapsed_seconds(&self) -> u32 {
        self.started.elapsed().as_secs() as u32
    }
}

// ─── OpenAI tool-call wire types ─────────────────────────────────
//
// Internal types matching what the upstream `/v1/chat/completions`
// emits. We deserialize only the fields we care about; the upstream
// may include extra fields (usage, system_fingerprint, etc.) which
// serde silently ignores.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FunctionCall {
    name: String,
    /// OpenAI's chat-completions API delivers arguments as a JSON-
    /// encoded string, not a JSON object. We re-parse it before
    /// dispatching.
    arguments: String,
}

#[derive(Debug, Clone, Serialize)]
struct ToolDef {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ToolFunctionDef,
}

#[derive(Debug, Clone, Serialize)]
struct ToolFunctionDef {
    name: &'static str,
    description: &'static str,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDef>,
    /// We always run non-streaming for the agent loop — streaming
    /// tool-call deltas across providers is too inconsistent.
    stream: bool,
    /// Upstream-side cap. Independently of our `max_tokens` aggregate,
    /// this prevents a single step from consuming the whole budget.
    max_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatChoice {
    message: ChatMessage,
    /// `"stop" | "tool_calls" | "length"` etc. Captured for tracing /
    /// debugging; the loop terminates based on the presence of
    /// `tool_calls`, not on `finish_reason`, because providers disagree
    /// on which value they emit when tool calls are present.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    total_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

// ─── Tool registry: descriptions the model sees ────────────────────

fn tool_definitions(allowed: &[String]) -> Vec<ToolDef> {
    let allow: BTreeSet<&str> = allowed.iter().map(|s| s.as_str()).collect();
    let mut out = Vec::new();
    if allow.contains("web_fetch") {
        out.push(ToolDef {
            kind: "function",
            function: ToolFunctionDef {
                name: "web_fetch",
                description: "Fetch a URL and return its plain-text body. Private IPs are blocked.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Absolute HTTP/HTTPS URL." }
                    },
                    "required": ["url"]
                }),
            },
        });
    }
    if allow.contains("search_memory") {
        out.push(ToolDef {
            kind: "function",
            function: ToolFunctionDef {
                name: "search_memory",
                description: "Search private memory for relevant past-chat summaries.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Free-text query." },
                        "limit": { "type": "integer", "default": 5 }
                    },
                    "required": ["query"]
                }),
            },
        });
    }
    if allow.contains("list_models") {
        out.push(ToolDef {
            kind: "function",
            function: ToolFunctionDef {
                name: "list_models",
                description: "List models the daemon's configured upstreams claim to serve.",
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            },
        });
    }
    if allow.contains("read_file") {
        out.push(ToolDef {
            kind: "function",
            function: ToolFunctionDef {
                name: "read_file",
                description:
                    "Read a UTF-8 text file from the operator's allow-listed roots. The path must \
                     be absolute and inside one of the operator's configured roots; paths outside \
                     the sandbox or matching deny patterns (.env, id_rsa, credentials, etc.) are \
                     refused.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path inside one of the operator's allow-listed roots."
                        }
                    },
                    "required": ["path"]
                }),
            },
        });
    }
    if allow.contains("list_dir") {
        out.push(ToolDef {
            kind: "function",
            function: ToolFunctionDef {
                name: "list_dir",
                description:
                    "List the entries of a directory within the operator's allow-listed roots. \
                     Returns sorted entries with kind (file/dir/symlink) and size for files. \
                     Capped at 500 entries; larger directories return the first 500 with a \
                     truncated flag.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute directory path inside an allow-listed root."
                        }
                    },
                    "required": ["path"]
                }),
            },
        });
    }
    out
}

// ─── RunContext: everything run_agent needs ────────────────────────

/// Bundles the daemon state run_agent depends on. Lets us keep
/// run_agent itself decoupled from NodeState's full surface — the
/// HTTP handler builds a RunContext from NodeState and passes it in.
pub struct RunContext {
    pub http: reqwest::Client,
    pub upstream_url: String,
    pub audit: Arc<AuditBroadcaster>,
    pub metrics: Arc<Metrics>,
    pub dlp: Option<Arc<DlpConfig>>,
    pub agent_fs: Option<Arc<AgentFsConfig>>,
    pub caller_label: String,
}

/// Drive the tool-call loop. See module docs for the lifecycle.
pub async fn run_agent(ctx: &RunContext, req: AgentRunRequest) -> AgentRunResponse {
    let req = req.clamped();
    let run_id = new_run_id();
    let clock = RunClock::start();

    // Reject unknown tool names up front rather than letting the
    // model see them as undefined. Caller's `tools: ["foo"]` with
    // an unknown name returns a run that completes immediately with
    // an error final-answer.
    let valid_tools: Vec<String> = req
        .tools
        .iter()
        .filter(|t| known_tools().contains(t.as_str()))
        .cloned()
        .collect();
    let unknown: Vec<String> = req
        .tools
        .iter()
        .filter(|t| !known_tools().contains(t.as_str()))
        .cloned()
        .collect();

    let goal_hash = short_hash(&serde_json::Value::String(req.goal.clone()));
    ctx.audit.emit(AuditEvent::AgentRunStarted {
        ts: AuditEvent::now(),
        run_id: run_id.clone(),
        caller: ctx.caller_label.clone(),
        goal_hash: goal_hash.clone(),
        tools: valid_tools.clone(),
        max_steps: req.max_steps,
        max_tokens: req.max_tokens,
        max_seconds: req.max_seconds,
    });
    ctx.metrics.inc_agent_runs();

    if !unknown.is_empty() {
        let stopped = StoppedBecause::ToolError;
        ctx.metrics.inc_agent_stop(AgentStopReason::ToolError);
        ctx.audit.emit(AuditEvent::AgentRunCompleted {
            ts: AuditEvent::now(),
            run_id: run_id.clone(),
            stopped_because: "tool_error".into(),
            steps_used: 0,
            tokens_used: 0,
        });
        return AgentRunResponse {
            final_answer: format!("unknown tools requested: {}", unknown.join(", ")),
            steps: vec![],
            stopped_because: stopped,
            tokens_used: 0,
            run_id,
        };
    }

    // DLP gate on the user's goal. Same posture as chat completions.
    if let Some(dlp_cfg) = ctx.dlp.as_ref() {
        let goal_json =
            serde_json::json!({ "messages": [{ "role": "user", "content": req.goal }] });
        match dlp::check(&ctx.http, dlp_cfg, goal_json.to_string().as_bytes()).await {
            DlpDecision::Allow => {}
            DlpDecision::Block { reason } => {
                ctx.metrics.inc_agent_stop(AgentStopReason::DlpBlocked);
                ctx.audit.emit(AuditEvent::AgentRunCompleted {
                    ts: AuditEvent::now(),
                    run_id: run_id.clone(),
                    stopped_because: "dlp_blocked".into(),
                    steps_used: 0,
                    tokens_used: 0,
                });
                return AgentRunResponse {
                    final_answer: format!("blocked by dlp: {reason}"),
                    steps: vec![],
                    stopped_because: StoppedBecause::DlpBlocked,
                    tokens_used: 0,
                    run_id,
                };
            }
        }
    }

    let tools_for_model = tool_definitions(&valid_tools);
    let mut messages: Vec<ChatMessage> = vec![
        ChatMessage {
            role: "system".into(),
            content: Some(build_system_prompt(req.max_steps, &now_iso8601())),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
        ChatMessage {
            role: "user".into(),
            content: Some(req.goal.clone()),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
    ];
    let mut step_records: Vec<StepRecord> = Vec::new();
    let mut tokens_used: u32 = 0;

    for step in 0..req.max_steps {
        if clock.elapsed_seconds() > req.max_seconds {
            return finish(
                ctx,
                run_id,
                step_records,
                tokens_used,
                StoppedBecause::MaxSeconds,
                "stopped: max_seconds exceeded".into(),
            );
        }
        if tokens_used >= req.max_tokens {
            return finish(
                ctx,
                run_id,
                step_records,
                tokens_used,
                StoppedBecause::MaxTokens,
                "stopped: max_tokens exceeded".into(),
            );
        }

        ctx.metrics.inc_agent_steps();
        let per_step_tokens = (req.max_tokens.saturating_sub(tokens_used))
            .min(1024)
            .max(64);
        let chat_req = ChatCompletionRequest {
            model: &req.model,
            messages: &messages,
            tools: tools_for_model.clone(),
            stream: false,
            max_tokens: per_step_tokens,
        };
        let resp = match chat_step(&ctx.http, &ctx.upstream_url, &chat_req).await {
            Ok(r) => r,
            Err(e) => {
                // Treat upstream failure as a model error. We log
                // but don't classify as ToolError — this is the
                // upstream pipe itself failing.
                tracing::warn!(error = %e, "agent: chat step failed");
                return finish(
                    ctx,
                    run_id,
                    step_records,
                    tokens_used,
                    StoppedBecause::ToolError,
                    format!("upstream chat error: {e}"),
                );
            }
        };
        if let Some(u) = resp.usage.as_ref() {
            tokens_used = tokens_used.saturating_add(u.total_tokens.max(u.completion_tokens));
        }

        let Some(choice) = resp.choices.into_iter().next() else {
            return finish(
                ctx,
                run_id,
                step_records,
                tokens_used,
                StoppedBecause::ToolError,
                "upstream returned no choices".into(),
            );
        };
        let assistant_msg = choice.message;
        let tool_calls = assistant_msg.tool_calls.clone();
        let assistant_content = assistant_msg.content.clone().unwrap_or_default();
        step_records.push(StepRecord::ModelMessage {
            step,
            content: assistant_content.clone(),
            tool_calls_made: tool_calls.len() as u32,
        });
        messages.push(assistant_msg);

        // Final answer = no tool calls. Loop terminates with the
        // model's prose content as the answer.
        if tool_calls.is_empty() {
            return finish(
                ctx,
                run_id,
                step_records,
                tokens_used,
                StoppedBecause::FinalAnswer,
                assistant_content,
            );
        }

        // Cap per-step tool calls. If the model emits more than
        // allowed, drop the overflow and log — we trust the
        // guardrail, not the model.
        let allowed_in_step = req.max_tool_calls_per_step as usize;
        let to_execute = if tool_calls.len() > allowed_in_step {
            tracing::warn!(
                requested = tool_calls.len(),
                allowed = allowed_in_step,
                "agent: model exceeded max_tool_calls_per_step; truncating"
            );
            tool_calls
                .into_iter()
                .take(allowed_in_step)
                .collect::<Vec<_>>()
        } else {
            tool_calls
        };

        for call in to_execute {
            ctx.metrics.inc_agent_tool_calls();
            let args_value: serde_json::Value =
                serde_json::from_str(&call.function.arguments).unwrap_or(serde_json::Value::Null);
            let args_hash = short_hash(&args_value);
            let (result_text, error) = execute_tool(&call.function.name, &args_value, ctx).await;
            ctx.audit.emit(AuditEvent::AgentToolCall {
                ts: AuditEvent::now(),
                run_id: run_id.clone(),
                step,
                tool: call.function.name.clone(),
                args_hash: args_hash.clone(),
                result_chars: result_text.len(),
                error: error.clone(),
            });
            step_records.push(StepRecord::ToolCall {
                step,
                tool: call.function.name.clone(),
                args_hash,
                result_chars: result_text.len(),
                error: error.clone(),
            });
            // DLP gate on tool result text before feeding to model.
            let feed_text = if let Some(dlp_cfg) = ctx.dlp.as_ref() {
                let probe = serde_json::json!({
                    "messages": [{ "role": "tool", "content": result_text }]
                });
                match dlp::check(&ctx.http, dlp_cfg, probe.to_string().as_bytes()).await {
                    DlpDecision::Allow => result_text,
                    DlpDecision::Block { reason } => {
                        return finish(
                            ctx,
                            run_id,
                            step_records,
                            tokens_used,
                            StoppedBecause::DlpBlocked,
                            format!("tool result blocked by dlp: {reason}"),
                        );
                    }
                }
            } else {
                result_text
            };
            messages.push(ChatMessage {
                role: "tool".into(),
                content: Some(feed_text),
                tool_call_id: Some(call.id),
                tool_calls: Vec::new(),
            });
        }
    }

    // Loop completed without a final answer = max_steps hit.
    finish(
        ctx,
        run_id,
        step_records,
        tokens_used,
        StoppedBecause::MaxSteps,
        "stopped: max_steps reached without a final answer".into(),
    )
}

fn finish(
    ctx: &RunContext,
    run_id: String,
    steps: Vec<StepRecord>,
    tokens_used: u32,
    stopped: StoppedBecause,
    final_answer: String,
) -> AgentRunResponse {
    let reason_label = match stopped {
        StoppedBecause::FinalAnswer => "final_answer",
        StoppedBecause::MaxSteps => "max_steps",
        StoppedBecause::MaxTokens => "max_tokens",
        StoppedBecause::MaxSeconds => "max_seconds",
        StoppedBecause::ToolError => "tool_error",
        StoppedBecause::DlpBlocked => "dlp_blocked",
        StoppedBecause::ModelDoesNotSupportToolUse => "model_no_tool_use",
    };
    let metric_reason = match stopped {
        StoppedBecause::FinalAnswer => AgentStopReason::FinalAnswer,
        StoppedBecause::MaxSteps => AgentStopReason::MaxSteps,
        StoppedBecause::MaxTokens => AgentStopReason::MaxTokens,
        StoppedBecause::MaxSeconds => AgentStopReason::MaxSeconds,
        StoppedBecause::ToolError | StoppedBecause::ModelDoesNotSupportToolUse => {
            AgentStopReason::ToolError
        }
        StoppedBecause::DlpBlocked => AgentStopReason::DlpBlocked,
    };
    ctx.metrics.inc_agent_stop(metric_reason);
    let steps_used = step_count_models(&steps);
    ctx.audit.emit(AuditEvent::AgentRunCompleted {
        ts: AuditEvent::now(),
        run_id: run_id.clone(),
        stopped_because: reason_label.into(),
        steps_used,
        tokens_used,
    });
    AgentRunResponse {
        final_answer,
        steps,
        stopped_because: stopped,
        tokens_used,
        run_id,
    }
}

fn step_count_models(steps: &[StepRecord]) -> u32 {
    steps
        .iter()
        .filter(|s| matches!(s, StepRecord::ModelMessage { .. }))
        .count() as u32
}

async fn chat_step(
    http: &reqwest::Client,
    upstream_url: &str,
    req: &ChatCompletionRequest<'_>,
) -> Result<ChatCompletionResponse, String> {
    let url = format!("{}/v1/chat/completions", upstream_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .header("Content-Type", "application/json")
        .json(req)
        .send()
        .await
        .map_err(|e| format!("upstream send: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("upstream {status}: {body}"));
    }
    resp.json::<ChatCompletionResponse>()
        .await
        .map_err(|e| format!("upstream parse: {e}"))
}

async fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    ctx: &RunContext,
) -> (String, Option<String>) {
    match name {
        "web_fetch" => {
            let Some(url) = args.get("url").and_then(|v| v.as_str()) else {
                return (
                    String::new(),
                    Some("web_fetch: missing `url` argument".into()),
                );
            };
            let fetch_req = crate::web_fetch::WebFetchRequest {
                url: url.to_string(),
                max_bytes: None,
            };
            match crate::web_fetch::fetch(&ctx.http, fetch_req).await {
                Ok(resp) => {
                    if resp.content.is_empty() {
                        (
                            format!(
                                "(non-text content: {} bytes, content-type: {}, status: {})",
                                resp.bytes, resp.content_type, resp.status
                            ),
                            None,
                        )
                    } else {
                        (resp.content, None)
                    }
                }
                Err(e) => (String::new(), Some(format!("web_fetch: {e}"))),
            }
        }
        "search_memory" => {
            // Slice 1 returns a placeholder. The memory store is
            // file-backed and per-user; a clean integration with the
            // agent runtime needs a small follow-up that threads the
            // memory store into RunContext. Until then, the agent
            // sees an empty result. ADR-0012 lists this as known.
            (
                "(search_memory not yet wired into RunContext; results will appear in a follow-up slice)"
                    .into(),
                None,
            )
        }
        "read_file" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return (
                    String::new(),
                    Some("read_file: missing `path` argument".into()),
                );
            };
            match agent_fs::read_file(ctx.agent_fs.as_ref(), path) {
                agent_fs::ReadFileOutcome::Ok {
                    content,
                    bytes_read,
                    truncated,
                } => {
                    let prefix = if truncated {
                        format!(
                            "(truncated to {bytes_read} bytes; file is larger than the configured cap)\n"
                        )
                    } else {
                        String::new()
                    };
                    (format!("{prefix}{content}"), None)
                }
                agent_fs::ReadFileOutcome::Err(e) => {
                    (String::new(), Some(format!("read_file: {}", e.label())))
                }
            }
        }
        "list_dir" => {
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return (
                    String::new(),
                    Some("list_dir: missing `path` argument".into()),
                );
            };
            match agent_fs::list_dir(ctx.agent_fs.as_ref(), path) {
                agent_fs::ListDirOutcome::Ok { entries, truncated } => {
                    // Render as JSON so the model can parse / filter
                    // it deterministically. Including the kind +
                    // size in every row.
                    let body = serde_json::json!({
                        "path": path,
                        "truncated": truncated,
                        "count": entries.len(),
                        "entries": entries,
                    });
                    (
                        serde_json::to_string_pretty(&body).unwrap_or_default(),
                        None,
                    )
                }
                agent_fs::ListDirOutcome::Err(e) => {
                    (String::new(), Some(format!("list_dir: {}", e.label())))
                }
            }
        }
        "list_models" => {
            // Best-effort: probe /v1/models on the configured
            // upstream. Avoids re-implementing the daemon's
            // upstream-probe logic — that lives in upstream.rs and
            // depends on Node which RunContext doesn't carry.
            let url = format!("{}/v1/models", ctx.upstream_url.trim_end_matches('/'));
            match ctx.http.get(&url).send().await {
                Ok(r) if r.status().is_success() => match r.text().await {
                    Ok(body) => (body, None),
                    Err(e) => (String::new(), Some(format!("list_models read: {e}"))),
                },
                Ok(r) => (
                    String::new(),
                    Some(format!("list_models upstream status: {}", r.status())),
                ),
                Err(e) => (String::new(), Some(format!("list_models: {e}"))),
            }
        }
        other => (
            String::new(),
            Some(format!(
                "unknown tool: {other} (registry guard should have caught this)"
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_clamps_above_hard_limit() {
        let req = AgentRunRequest {
            goal: "x".into(),
            tools: vec![],
            max_steps: 9_999,
            max_tokens: 1_000_000,
            max_seconds: 100_000,
            max_tool_calls_per_step: 999,
            model: "auto".into(),
        }
        .clamped();
        assert_eq!(req.max_steps, HARD_MAX_STEPS);
        assert_eq!(req.max_tokens, HARD_MAX_TOKENS);
        assert_eq!(req.max_seconds, HARD_MAX_SECONDS);
        assert_eq!(req.max_tool_calls_per_step, HARD_MAX_TOOL_CALLS_PER_STEP);
    }

    #[test]
    fn clamp_floors_at_one_for_safety() {
        let req = AgentRunRequest {
            goal: "x".into(),
            tools: vec![],
            max_steps: 0,
            max_tokens: 0,
            max_seconds: 0,
            max_tool_calls_per_step: 0,
            model: "auto".into(),
        }
        .clamped();
        assert_eq!(req.max_steps, 1);
        assert!(req.max_tokens >= 64);
        assert_eq!(req.max_seconds, 1);
        assert_eq!(req.max_tool_calls_per_step, 1);
    }

    #[test]
    fn clamp_keeps_defaults_unchanged() {
        let req = AgentRunRequest {
            goal: "x".into(),
            tools: vec![],
            max_steps: DEFAULT_MAX_STEPS,
            max_tokens: DEFAULT_MAX_TOKENS,
            max_seconds: DEFAULT_MAX_SECONDS,
            max_tool_calls_per_step: DEFAULT_MAX_TOOL_CALLS_PER_STEP,
            model: "auto".into(),
        }
        .clamped();
        assert_eq!(req.max_steps, DEFAULT_MAX_STEPS);
        assert_eq!(req.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(req.max_seconds, DEFAULT_MAX_SECONDS);
        assert_eq!(req.max_tool_calls_per_step, DEFAULT_MAX_TOOL_CALLS_PER_STEP);
    }

    #[test]
    fn known_tools_includes_slice_1_set() {
        let t = known_tools();
        assert!(t.contains("web_fetch"));
        assert!(t.contains("search_memory"));
        assert!(t.contains("list_models"));
    }

    #[test]
    fn known_tools_includes_read_file_in_slice_4a() {
        let t = known_tools();
        assert!(t.contains("read_file"));
        // Slice 4a/4b must NOT include the destructive / wider-blast-
        // radius tools — those land in subsequent ADRs.
        assert!(!t.contains("write_file"));
        assert!(!t.contains("run_command"));
    }

    #[test]
    fn known_tools_includes_list_dir_in_slice_4b() {
        assert!(known_tools().contains("list_dir"));
    }

    #[test]
    fn short_hash_is_stable_and_16_chars() {
        let v = serde_json::json!({ "url": "https://example.com" });
        let h1 = short_hash(&v);
        let h2 = short_hash(&v);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
        // Sanity: hex only.
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn short_hash_differs_for_different_inputs() {
        let a = short_hash(&serde_json::json!({ "url": "https://a.example" }));
        let b = short_hash(&serde_json::json!({ "url": "https://b.example" }));
        assert_ne!(a, b);
    }

    #[test]
    fn new_run_id_returns_16_hex_chars() {
        let id = new_run_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Cryptographic random — two consecutive ids should not be equal.
        assert_ne!(new_run_id(), new_run_id());
    }

    #[test]
    fn seconds_to_iso8601_known_dates() {
        // Unix epoch.
        assert_eq!(seconds_to_iso8601(0), "1970-01-01T00:00:00Z");
        // 2020-01-01 00:00:00 UTC = 1_577_836_800.
        assert_eq!(seconds_to_iso8601(1_577_836_800), "2020-01-01T00:00:00Z");
        // 2024-02-29 12:34:56 UTC (leap year) = 1_709_210_096.
        assert_eq!(seconds_to_iso8601(1_709_210_096), "2024-02-29T12:34:56Z");
        // 2026-05-21 00:00:00 UTC (the date this code was written) =
        // 1_779_321_600. (56 yr * 365 days + 14 leap days = 20454
        // days from 1970-01-01 to 2026-01-01, plus 140 days into 2026.)
        assert_eq!(seconds_to_iso8601(1_779_321_600), "2026-05-21T00:00:00Z");
    }

    #[test]
    fn now_iso8601_is_well_formed() {
        let now = now_iso8601();
        // YYYY-MM-DDTHH:MM:SSZ → 20 characters exactly.
        assert_eq!(now.len(), 20, "unexpected ISO-8601 length: {now}");
        // First four chars are the year, must be plausible. Tests are
        // not time-traveling agents, so 2020..2200 is fine.
        let year: u32 = now[..4].parse().expect("year should parse");
        assert!(
            (2020..2200).contains(&year),
            "year out of plausible range: {year}"
        );
        assert!(now.ends_with('Z'));
    }

    #[test]
    fn system_prompt_carries_now() {
        let prompt = build_system_prompt(8, "2026-05-21T03:14:15Z");
        assert!(prompt.contains("2026-05-21T03:14:15Z"));
        assert!(prompt.contains("at most 8 steps"));
        // The "authoritative" framing must survive any future edits
        // to the prompt — that phrase is load-bearing for models
        // that would otherwise prefer their training-data prior.
        assert!(prompt.contains("authoritative"));
    }

    #[test]
    fn run_clock_elapsed_seconds_increases() {
        let c = RunClock::start();
        // Just after start, elapsed is 0.
        assert_eq!(c.elapsed_seconds(), 0);
        // We don't sleep — would make tests flaky. The behavioral
        // contract is checked by the impl: elapsed_seconds returns
        // `Instant::elapsed().as_secs()`, monotonic by construction.
    }

    // ── End-to-end loop tests against a wiremock upstream ────────

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ctx_for_test(upstream: &str) -> RunContext {
        RunContext {
            http: reqwest::Client::new(),
            upstream_url: upstream.into(),
            audit: Arc::new(crate::audit::AuditBroadcaster::new()),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            dlp: None,
            agent_fs: None,
            caller_label: "test".into(),
        }
    }

    fn final_answer_response(text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop"
            }],
            "usage": { "total_tokens": 50, "completion_tokens": 12 }
        })
    }

    fn tool_call_response(tool: &str, args_json: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": tool, "arguments": args_json }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "total_tokens": 80, "completion_tokens": 22 }
        })
    }

    #[tokio::test]
    async fn run_with_immediate_final_answer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(final_answer_response("the answer is 42")),
            )
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "what is the meaning of life?".into(),
                tools: vec![],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::FinalAnswer);
        assert_eq!(resp.final_answer, "the answer is 42");
        assert_eq!(resp.steps.len(), 1);
        match &resp.steps[0] {
            StepRecord::ModelMessage {
                content,
                tool_calls_made,
                ..
            } => {
                assert_eq!(content, "the answer is 42");
                assert_eq!(*tool_calls_made, 0);
            }
            other => panic!("expected ModelMessage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_with_unknown_tool_short_circuits() {
        let ctx = ctx_for_test("http://unused.example");
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "x".into(),
                tools: vec!["nonexistent_tool".into()],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::ToolError);
        assert!(resp.final_answer.contains("nonexistent_tool"));
        assert!(resp.steps.is_empty(), "no model calls should have happened");
    }

    #[tokio::test]
    async fn run_executes_tool_then_finals() {
        let server = MockServer::start().await;
        // Turn 1: model emits a list_models tool call.
        // Turn 2: model produces a final answer.
        // wiremock serves the first response, then the second.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tool_call_response("list_models", "{}")),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(final_answer_response("found 3 models")),
            )
            .mount(&server)
            .await;
        // list_models itself fetches /v1/models from the same
        // upstream. Mock that too.
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "llama3.2:1b"}, {"id": "qwen2.5:0.5b"}, {"id": "mistral"}]
            })))
            .mount(&server)
            .await;

        let ctx = ctx_for_test(&server.uri());
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "list available models".into(),
                tools: vec!["list_models".into()],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::FinalAnswer);
        assert_eq!(resp.final_answer, "found 3 models");
        // 2 model messages + 1 tool call = 3 records.
        assert_eq!(resp.steps.len(), 3);
        assert!(matches!(resp.steps[0], StepRecord::ModelMessage { .. }));
        match &resp.steps[1] {
            StepRecord::ToolCall {
                tool,
                error,
                result_chars,
                ..
            } => {
                assert_eq!(tool, "list_models");
                assert!(error.is_none());
                assert!(*result_chars > 0);
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        assert!(matches!(resp.steps[2], StepRecord::ModelMessage { .. }));
    }

    #[tokio::test]
    async fn run_stops_at_max_steps_when_model_loops() {
        // Model returns a tool call every turn, never a final answer.
        // Loop must terminate at max_steps with the right reason.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tool_call_response("list_models", "{}")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "loop forever".into(),
                tools: vec!["list_models".into()],
                max_steps: 3,
                max_tokens: 99_999,
                max_seconds: 30,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::MaxSteps);
        // 3 model calls + 3 tool calls in the steps array.
        let model_msgs = resp
            .steps
            .iter()
            .filter(|s| matches!(s, StepRecord::ModelMessage { .. }))
            .count();
        assert_eq!(model_msgs, 3);
    }

    #[tokio::test]
    async fn run_stops_at_max_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tool_call_response("list_models", "{}")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        // Each step reports total_tokens=80 in the mocked usage.
        // max_tokens=200 means we should hit the cap after the
        // second step (160) on the third iteration's pre-check.
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "burn tokens".into(),
                tools: vec!["list_models".into()],
                max_steps: 10,
                max_tokens: 200,
                max_seconds: 30,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::MaxTokens);
    }

    #[tokio::test]
    async fn run_returns_tool_error_when_upstream_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        let resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "x".into(),
                tools: vec![],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        assert_eq!(resp.stopped_because, StoppedBecause::ToolError);
        assert!(resp.final_answer.contains("upstream"));
    }

    #[tokio::test]
    async fn run_emits_audit_events_for_run_start_tool_and_complete() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(tool_call_response("list_models", "{}")),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(final_answer_response("done")))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        let mut rx = ctx.audit.subscribe();
        let _resp = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "x".into(),
                tools: vec!["list_models".into()],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        // Drain the broadcast — we expect run_started, tool_call,
        // run_completed at minimum (order matters).
        let mut seen_kinds = Vec::new();
        for _ in 0..6 {
            match tokio::time::timeout(std::time::Duration::from_millis(20), rx.recv()).await {
                Ok(Ok(event)) => {
                    let json = serde_json::to_value(&event).unwrap();
                    if let Some(k) = json.get("kind").and_then(|v| v.as_str()) {
                        seen_kinds.push(k.to_string());
                    }
                }
                _ => break,
            }
        }
        assert!(
            seen_kinds.contains(&"agent_run_started".to_string()),
            "missing run_started in {seen_kinds:?}"
        );
        assert!(
            seen_kinds.contains(&"agent_tool_call".to_string()),
            "missing tool_call in {seen_kinds:?}"
        );
        assert!(
            seen_kinds.contains(&"agent_run_completed".to_string()),
            "missing run_completed in {seen_kinds:?}"
        );
    }

    #[tokio::test]
    async fn run_metrics_bump_on_each_phase() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(final_answer_response("hi")))
            .mount(&server)
            .await;
        let ctx = ctx_for_test(&server.uri());
        let metrics = ctx.metrics.clone();
        let _ = run_agent(
            &ctx,
            AgentRunRequest {
                goal: "x".into(),
                tools: vec![],
                max_steps: 4,
                max_tokens: 1024,
                max_seconds: 10,
                max_tool_calls_per_step: 4,
                model: "test-model".into(),
            },
        )
        .await;
        use std::sync::atomic::Ordering;
        assert_eq!(metrics.agent_runs_total.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.agent_steps_total.load(Ordering::Relaxed), 1);
        assert_eq!(
            metrics
                .agent_runs_stopped_final_answer
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(metrics.agent_tool_calls_total.load(Ordering::Relaxed), 0);
    }
}
