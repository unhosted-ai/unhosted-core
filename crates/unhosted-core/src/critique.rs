//! Pre-output self-correction gate.
//!
//! Before the agent finalizes a prose answer, it runs that answer
//! through a deterministic set of quality gates — a "critique ledger"
//! — and records any findings. This mirrors the self-correction loop
//! from the cognitive-twin-agent behavior spec: catch obvious tone,
//! architecture, UX, and risk problems *before* the output reaches
//! the user, without a second model round-trip.
//!
//! The check is intentionally deterministic (no model call): it is a
//! cheap, auditable guardrail, not a second opinion. A failing gate
//! does not rewrite the answer — it surfaces a structured
//! [`CritiqueReport`] the caller can attach to the run, log to the
//! audit trail, or use to decide whether to retry.
//!
//! ### Gates
//!
//! - **Tone** — flags inflated marketing language and excessive
//!   hedging that the behavior spec forbids ("technical, concise,
//!   direct; no inflated marketing language").
//! - **Architecture** — flags answers that reach for cloud coupling
//!   without acknowledging a local-first alternative, contradicting
//!   the local-first principle.
//! - **UX** — flags answers that describe a destructive or
//!   irreversible action without mentioning reversibility or a
//!   failure/undo path.
//! - **Risk** — flags answers that propose a destructive command
//!   without a confirmation step.
//!
//! Each gate is a pure function over the candidate text. Findings are
//! advisory and ordered by gate.

use serde::Serialize;

/// Which gate produced a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    Tone,
    Architecture,
    Ux,
    Risk,
}

/// A single advisory finding from one gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub gate: Gate,
    /// Short, human-readable explanation of what tripped the gate.
    pub message: String,
}

/// The outcome of running every gate over a candidate answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CritiqueReport {
    pub findings: Vec<Finding>,
}

impl CritiqueReport {
    /// True when no gate raised a finding.
    pub fn passed(&self) -> bool {
        self.findings.is_empty()
    }

    /// True when any finding came from the risk gate — the caller may
    /// want to treat this more seriously (e.g. require confirmation).
    pub fn has_risk_finding(&self) -> bool {
        self.findings.iter().any(|f| f.gate == Gate::Risk)
    }
}

/// Marketing / inflated-language markers the tone gate flags. Kept
/// lowercase; matching is case-insensitive on word boundaries.
const MARKETING_MARKERS: &[&str] = &[
    "world-class",
    "cutting-edge",
    "best-in-class",
    "revolutionary",
    "game-changer",
    "game changer",
    "synergy",
    "seamlessly",
    "unparalleled",
    "next-generation",
    "leverage our",
    "supercharge",
];

/// Markers that suggest a destructive or irreversible action.
const DESTRUCTIVE_MARKERS: &[&str] = &[
    "rm -rf",
    "drop table",
    "delete from",
    "force push",
    "git push --force",
    "git reset --hard",
    "truncate",
    "format the disk",
    "wipe",
];

/// Words that signal reversibility / undo / safety were considered.
const REVERSIBILITY_MARKERS: &[&str] = &[
    "revers", // reversible / reversibility
    "undo",
    "backup",
    "restore",
    "rollback",
    "roll back",
    "recover",
    "dry run",
    "dry-run",
];

/// Words that signal a confirmation step was included.
const CONFIRMATION_MARKERS: &[&str] = &[
    "confirm",
    "are you sure",
    "before proceeding",
    "with your approval",
    "ask first",
    "double-check",
    "double check",
];

/// Markers that suggest cloud coupling.
const CLOUD_MARKERS: &[&str] = &[
    "aws ",
    "amazon s3",
    "s3 bucket",
    "google cloud",
    "gcp ",
    "azure ",
    "in the cloud",
    "cloud-hosted",
    "managed cloud",
    "third-party api",
];

/// Markers that show a local-first alternative was acknowledged.
const LOCAL_FIRST_MARKERS: &[&str] = &[
    "local-first",
    "local first",
    "locally",
    "on-device",
    "on device",
    "self-host",
    "self host",
    "offline",
    "no cloud",
];

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Run every gate over a candidate final answer and collect findings.
///
/// `text` is the prose the agent is about to return. The function is
/// pure and allocation-light; it lowercases once and scans.
pub fn critique(text: &str) -> CritiqueReport {
    let lower = text.to_lowercase();
    let mut findings = Vec::new();

    // ── Tone gate ────────────────────────────────────────────────
    for marker in MARKETING_MARKERS {
        if lower.contains(marker) {
            findings.push(Finding {
                gate: Gate::Tone,
                message: format!(
                    "Inflated/marketing phrasing detected ({marker:?}); \
                     prefer concise, technical language."
                ),
            });
            break;
        }
    }

    // ── Architecture gate ────────────────────────────────────────
    if contains_any(&lower, CLOUD_MARKERS) && !contains_any(&lower, LOCAL_FIRST_MARKERS) {
        findings.push(Finding {
            gate: Gate::Architecture,
            message: "Cloud coupling proposed without evaluating a \
                      local-first alternative."
                .to_string(),
        });
    }

    // ── UX + Risk gates (both keyed off destructive markers) ─────
    if contains_any(&lower, DESTRUCTIVE_MARKERS) {
        if !contains_any(&lower, REVERSIBILITY_MARKERS) {
            findings.push(Finding {
                gate: Gate::Ux,
                message: "Describes a destructive/irreversible action \
                          without mentioning a reversibility or undo path."
                    .to_string(),
            });
        }
        if !contains_any(&lower, CONFIRMATION_MARKERS) {
            findings.push(Finding {
                gate: Gate::Risk,
                message: "Proposes a destructive command without a \
                          confirmation step."
                    .to_string(),
            });
        }
    }

    CritiqueReport { findings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_answer_passes_all_gates() {
        let report = critique(
            "Run the migration locally first, verify the output, then \
             apply it. You can roll back with the saved backup if needed.",
        );
        assert!(
            report.passed(),
            "unexpected findings: {:?}",
            report.findings
        );
        assert!(!report.has_risk_finding());
    }

    #[test]
    fn tone_gate_flags_marketing_language() {
        let report = critique("This is a world-class, cutting-edge solution.");
        assert!(report.findings.iter().any(|f| f.gate == Gate::Tone));
    }

    #[test]
    fn architecture_gate_flags_unqualified_cloud() {
        let report = critique("Store the data in an S3 bucket on AWS for durability.");
        assert!(report.findings.iter().any(|f| f.gate == Gate::Architecture));
    }

    #[test]
    fn architecture_gate_allows_cloud_with_local_alternative() {
        let report = critique(
            "You could use an S3 bucket, but the local-first option is to \
             keep encrypted blobs on-device.",
        );
        assert!(
            !report.findings.iter().any(|f| f.gate == Gate::Architecture),
            "should not flag when a local-first alternative is named"
        );
    }

    #[test]
    fn risk_and_ux_gates_flag_unconfirmed_destructive_action() {
        let report = critique("Just run rm -rf ./build to clear it.");
        assert!(report.findings.iter().any(|f| f.gate == Gate::Risk));
        assert!(report.findings.iter().any(|f| f.gate == Gate::Ux));
        assert!(report.has_risk_finding());
    }

    #[test]
    fn destructive_action_with_safeguards_passes() {
        let report = critique(
            "Before proceeding, confirm the path. This rm -rf targets only \
             the build dir and you can restore it from the backup.",
        );
        assert!(
            !report.has_risk_finding(),
            "confirmation present should clear the risk gate"
        );
        assert!(
            !report.findings.iter().any(|f| f.gate == Gate::Ux),
            "reversibility present should clear the UX gate"
        );
    }

    #[test]
    fn report_serializes_to_snake_case_json() {
        let report = critique("rm -rf /tmp/x");
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"risk\""));
        assert!(json.contains("\"ux\""));
    }
}
