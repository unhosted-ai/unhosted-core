//! Prometheus-format metrics.
//!
//! Companion to the [`audit`] feed: where audit emits *events*
//! (chat_completion_started, peer_paired, …), metrics expose
//! *aggregates* (counters, gauges) suitable for a time-series
//! database. Prometheus scrapers poll `GET /metrics` and store the
//! returned values; the operator builds dashboards / alert rules
//! on top.
//!
//! Deliberately no `prometheus`-crate dependency. The text format
//! is small and stable; we emit it by hand from atomic counters.
//! Keeps the binary lean and avoids pulling protobuf / lazy_static
//! transitively.
//!
//! Wire format (Prometheus text 0.0.4):
//!
//! ```text
//! # HELP unhosted_build_info Build info constant (always 1).
//! # TYPE unhosted_build_info gauge
//! unhosted_build_info{version="0.0.66"} 1
//!
//! # HELP unhosted_chat_completions_total Total chat completion requests.
//! # TYPE unhosted_chat_completions_total counter
//! unhosted_chat_completions_total 42
//! ```
//!
//! Authentication: the `/metrics` endpoint is auth-gated (same as
//! `/v1/audit`). Anonymous LAN callers must not be able to enumerate
//! peers, chat counts, or tunnel state.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Instant;

/// Aggregate runtime metrics. Counters and gauges only — no
/// histograms in this slice; we'll add chat_completion_duration as
/// a separate follow-up when we wire timing through the handler.
pub struct Metrics {
    /// Total chat completion requests handled by this daemon since
    /// startup. Monotonic counter. Increments before the upstream
    /// call, so a stuck request still bumps it.
    pub chat_completions_total: AtomicU64,
    /// Current in-flight chat completions. Increments at handler
    /// entry, decrements at exit (success or error). A non-zero
    /// resting value typically indicates a stuck upstream.
    pub chat_completions_active: AtomicI64,
    /// Peer-pair operations performed (additions only). Decoupled
    /// from `peers_paired`, which is a gauge of the current count.
    pub peer_pairs_total: AtomicU64,
    /// Peer-unpair operations performed.
    pub peer_unpairs_total: AtomicU64,
    /// Current count of paired peers. Read from the registry on
    /// every scrape rather than tracked here, so a peer removed by
    /// editing the registry file out-of-band is still reflected.
    /// See [`Metrics::set_peers_paired`].
    pub peers_paired: AtomicI64,
    /// Public-mode policy changes (PUT /v1/public-mode/policy).
    pub policy_changes_total: AtomicU64,
    /// Tunnel state transitions. Broken out by destination state
    /// in the emit handler — this is the cardinality-safe rollup.
    pub tunnel_state_changes_total: AtomicU64,
    /// 0 = off, 1 = starting, 2 = live, 3 = failed. Reflects the
    /// last observed transition.
    pub tunnel_state: AtomicI64,
    /// Auth-rejected requests (signed-peer sig invalid, bearer
    /// wrong, replay caught). Useful as a security-monitoring
    /// signal — a sudden spike is worth alerting on.
    pub auth_rejections_total: AtomicU64,
    /// Agent runs started (ADR-0012). Increments at the top of
    /// `run_agent` before any model call.
    pub agent_runs_total: AtomicU64,
    /// Total steps executed across all agent runs. One step =
    /// one round-trip to the model. Stops at max_steps per run.
    pub agent_steps_total: AtomicU64,
    /// Total tool invocations across all agent runs.
    pub agent_tool_calls_total: AtomicU64,
    /// Citations emitted by the agent via the `cite` tool.
    /// Operators tracking research-quality runs can alert when this
    /// is consistently zero on prompts that should have been
    /// source-backed.
    pub agent_citations_total: AtomicU64,
    /// Agent runs that terminated via the model's final answer.
    pub agent_runs_stopped_final_answer: AtomicU64,
    /// Agent runs that hit max_steps before producing a final answer.
    pub agent_runs_stopped_max_steps: AtomicU64,
    /// Agent runs that hit max_tokens before producing a final answer.
    pub agent_runs_stopped_max_tokens: AtomicU64,
    /// Agent runs that hit max_seconds before producing a final answer.
    pub agent_runs_stopped_max_seconds: AtomicU64,
    /// Agent runs that aborted because a tool returned an error.
    pub agent_runs_stopped_tool_error: AtomicU64,
    /// Agent runs that aborted because the DLP hook blocked content.
    pub agent_runs_stopped_dlp_blocked: AtomicU64,
    /// Process start time, used to compute uptime on scrape.
    started_at: Instant,
    /// Static build version (Cargo's CARGO_PKG_VERSION). Recorded
    /// once so the build_info gauge can carry it as a label.
    version: &'static str,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            chat_completions_total: AtomicU64::new(0),
            chat_completions_active: AtomicI64::new(0),
            peer_pairs_total: AtomicU64::new(0),
            peer_unpairs_total: AtomicU64::new(0),
            peers_paired: AtomicI64::new(0),
            policy_changes_total: AtomicU64::new(0),
            tunnel_state_changes_total: AtomicU64::new(0),
            tunnel_state: AtomicI64::new(0),
            auth_rejections_total: AtomicU64::new(0),
            agent_runs_total: AtomicU64::new(0),
            agent_steps_total: AtomicU64::new(0),
            agent_tool_calls_total: AtomicU64::new(0),
            agent_citations_total: AtomicU64::new(0),
            agent_runs_stopped_final_answer: AtomicU64::new(0),
            agent_runs_stopped_max_steps: AtomicU64::new(0),
            agent_runs_stopped_max_tokens: AtomicU64::new(0),
            agent_runs_stopped_max_seconds: AtomicU64::new(0),
            agent_runs_stopped_tool_error: AtomicU64::new(0),
            agent_runs_stopped_dlp_blocked: AtomicU64::new(0),
            started_at: Instant::now(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    pub fn inc_chat_total(&self) {
        self.chat_completions_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_chat_active(&self) {
        self.chat_completions_active.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_chat_active(&self) {
        self.chat_completions_active.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn inc_peer_pairs(&self) {
        self.peer_pairs_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_peer_unpairs(&self) {
        self.peer_unpairs_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn set_peers_paired(&self, n: i64) {
        self.peers_paired.store(n, Ordering::Relaxed);
    }
    pub fn inc_policy_changes(&self) {
        self.policy_changes_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_tunnel_state(&self, new_state: TunnelStateCode) {
        self.tunnel_state_changes_total
            .fetch_add(1, Ordering::Relaxed);
        self.tunnel_state.store(new_state as i64, Ordering::Relaxed);
    }
    pub fn inc_auth_rejections(&self) {
        self.auth_rejections_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_agent_runs(&self) {
        self.agent_runs_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_agent_steps(&self) {
        self.agent_steps_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_agent_tool_calls(&self) {
        self.agent_tool_calls_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_agent_citations(&self) {
        self.agent_citations_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_agent_stop(&self, reason: AgentStopReason) {
        match reason {
            AgentStopReason::FinalAnswer => {
                self.agent_runs_stopped_final_answer
                    .fetch_add(1, Ordering::Relaxed);
            }
            AgentStopReason::MaxSteps => {
                self.agent_runs_stopped_max_steps
                    .fetch_add(1, Ordering::Relaxed);
            }
            AgentStopReason::MaxTokens => {
                self.agent_runs_stopped_max_tokens
                    .fetch_add(1, Ordering::Relaxed);
            }
            AgentStopReason::MaxSeconds => {
                self.agent_runs_stopped_max_seconds
                    .fetch_add(1, Ordering::Relaxed);
            }
            AgentStopReason::ToolError => {
                self.agent_runs_stopped_tool_error
                    .fetch_add(1, Ordering::Relaxed);
            }
            AgentStopReason::DlpBlocked => {
                self.agent_runs_stopped_dlp_blocked
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Render the current values as Prometheus text format. Called
    /// per scrape; cost is one allocation + a handful of atomic
    /// reads. Each metric carries the canonical `# HELP` + `# TYPE`
    /// headers Prometheus expects.
    pub fn to_prometheus_text(&self) -> String {
        let mut out = String::with_capacity(2048);

        out.push_str("# HELP unhosted_build_info Build info constant (always 1).\n");
        out.push_str("# TYPE unhosted_build_info gauge\n");
        out.push_str(&format!(
            "unhosted_build_info{{version=\"{}\"}} 1\n",
            self.version
        ));

        let uptime = self.started_at.elapsed().as_secs();
        out.push_str("# HELP unhosted_uptime_seconds Seconds since this daemon started.\n");
        out.push_str("# TYPE unhosted_uptime_seconds counter\n");
        out.push_str(&format!("unhosted_uptime_seconds {uptime}\n"));

        out.push_str(
            "# HELP unhosted_chat_completions_total Total chat completion requests handled.\n",
        );
        out.push_str("# TYPE unhosted_chat_completions_total counter\n");
        out.push_str(&format!(
            "unhosted_chat_completions_total {}\n",
            self.chat_completions_total.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP unhosted_chat_completions_active Currently in-flight chat completions.\n",
        );
        out.push_str("# TYPE unhosted_chat_completions_active gauge\n");
        out.push_str(&format!(
            "unhosted_chat_completions_active {}\n",
            self.chat_completions_active.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_peer_pairs_total Peer-pair operations performed.\n");
        out.push_str("# TYPE unhosted_peer_pairs_total counter\n");
        out.push_str(&format!(
            "unhosted_peer_pairs_total {}\n",
            self.peer_pairs_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_peer_unpairs_total Peer-unpair operations performed.\n");
        out.push_str("# TYPE unhosted_peer_unpairs_total counter\n");
        out.push_str(&format!(
            "unhosted_peer_unpairs_total {}\n",
            self.peer_unpairs_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_peers_paired Currently paired peers.\n");
        out.push_str("# TYPE unhosted_peers_paired gauge\n");
        out.push_str(&format!(
            "unhosted_peers_paired {}\n",
            self.peers_paired.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_policy_changes_total Public-mode policy mutations.\n");
        out.push_str("# TYPE unhosted_policy_changes_total counter\n");
        out.push_str(&format!(
            "unhosted_policy_changes_total {}\n",
            self.policy_changes_total.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP unhosted_tunnel_state_changes_total Cloudflare-tunnel state transitions.\n",
        );
        out.push_str("# TYPE unhosted_tunnel_state_changes_total counter\n");
        out.push_str(&format!(
            "unhosted_tunnel_state_changes_total {}\n",
            self.tunnel_state_changes_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_tunnel_state Current tunnel state: 0 off, 1 starting, 2 live, 3 failed.\n");
        out.push_str("# TYPE unhosted_tunnel_state gauge\n");
        out.push_str(&format!(
            "unhosted_tunnel_state {}\n",
            self.tunnel_state.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_auth_rejections_total Requests rejected by auth (bad sig, bearer, replay).\n");
        out.push_str("# TYPE unhosted_auth_rejections_total counter\n");
        out.push_str(&format!(
            "unhosted_auth_rejections_total {}\n",
            self.auth_rejections_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_agent_runs_total Total agent runs started.\n");
        out.push_str("# TYPE unhosted_agent_runs_total counter\n");
        out.push_str(&format!(
            "unhosted_agent_runs_total {}\n",
            self.agent_runs_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_agent_steps_total Total agent steps across all runs.\n");
        out.push_str("# TYPE unhosted_agent_steps_total counter\n");
        out.push_str(&format!(
            "unhosted_agent_steps_total {}\n",
            self.agent_steps_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_agent_tool_calls_total Total tool invocations across all agent runs.\n");
        out.push_str("# TYPE unhosted_agent_tool_calls_total counter\n");
        out.push_str(&format!(
            "unhosted_agent_tool_calls_total {}\n",
            self.agent_tool_calls_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_agent_citations_total Citations emitted by the agent via the cite tool.\n");
        out.push_str("# TYPE unhosted_agent_citations_total counter\n");
        out.push_str(&format!(
            "unhosted_agent_citations_total {}\n",
            self.agent_citations_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP unhosted_agent_runs_stopped_by Agent runs by stop reason.\n");
        out.push_str("# TYPE unhosted_agent_runs_stopped_by counter\n");
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"final_answer\"}} {}\n",
            self.agent_runs_stopped_final_answer.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"max_steps\"}} {}\n",
            self.agent_runs_stopped_max_steps.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"max_tokens\"}} {}\n",
            self.agent_runs_stopped_max_tokens.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"max_seconds\"}} {}\n",
            self.agent_runs_stopped_max_seconds.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"tool_error\"}} {}\n",
            self.agent_runs_stopped_tool_error.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "unhosted_agent_runs_stopped_by{{reason=\"dlp_blocked\"}} {}\n",
            self.agent_runs_stopped_dlp_blocked.load(Ordering::Relaxed)
        ));

        out
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable integer codes for tunnel state, used as the value of the
/// `unhosted_tunnel_state` gauge. Documented inline in the gauge's
/// `# HELP` so a dashboard author doesn't have to dig.
#[derive(Debug, Clone, Copy)]
pub enum TunnelStateCode {
    Off = 0,
    Starting = 1,
    Live = 2,
    Failed = 3,
}

/// Why an agent run ended. Mirrors `agent::StoppedBecause` but lives
/// here so `metrics.rs` doesn't depend on `agent.rs`.
#[derive(Debug, Clone, Copy)]
pub enum AgentStopReason {
    FinalAnswer,
    MaxSteps,
    MaxTokens,
    MaxSeconds,
    ToolError,
    DlpBlocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment() {
        let m = Metrics::new();
        m.inc_chat_total();
        m.inc_chat_total();
        m.inc_chat_total();
        assert_eq!(m.chat_completions_total.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn active_can_go_up_and_down() {
        let m = Metrics::new();
        m.inc_chat_active();
        m.inc_chat_active();
        m.dec_chat_active();
        assert_eq!(m.chat_completions_active.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn tunnel_state_record_updates_both_counter_and_gauge() {
        let m = Metrics::new();
        m.record_tunnel_state(TunnelStateCode::Starting);
        m.record_tunnel_state(TunnelStateCode::Live);
        assert_eq!(m.tunnel_state_changes_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.tunnel_state.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn prometheus_text_has_help_type_value_per_metric() {
        let m = Metrics::new();
        m.inc_chat_total();
        m.inc_peer_pairs();
        m.record_tunnel_state(TunnelStateCode::Live);
        let text = m.to_prometheus_text();
        // Every metric has all three lines.
        assert!(text.contains("# HELP unhosted_build_info"));
        assert!(text.contains("# TYPE unhosted_build_info gauge"));
        assert!(text.contains("unhosted_build_info{version=\""));
        assert!(text.contains("unhosted_chat_completions_total 1"));
        assert!(text.contains("unhosted_peer_pairs_total 1"));
        assert!(text.contains("unhosted_tunnel_state 2"));
    }

    #[test]
    fn build_info_carries_cargo_pkg_version() {
        let m = Metrics::new();
        let text = m.to_prometheus_text();
        let expected = format!(
            "unhosted_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );
        assert!(
            text.contains(&expected),
            "missing build_info line: {expected}\n{text}"
        );
    }
}
