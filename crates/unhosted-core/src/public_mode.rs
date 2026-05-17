//! Public-mode policy persistence + HTTP surface.
//!
//! ADR-0010 ("transactional public mode") splits payments work into a
//! separate repo (`unhosted-payments`) and ships the rail-agnostic
//! primitives — `PeerPaymentPolicy`, `PaymentRail`, `KycTier`,
//! `PayerContext` — in the `unhosted-payments-core` crate. This module
//! is the daemon's seam: it owns the on-disk policy file, exposes a
//! read endpoint (anyone on the loopback can see what this peer
//! accepts) and a write endpoint (loopback-only — only the local user
//! can change what their machine offers).
//!
//! Quoting is **not** here yet. Slice 2 (this file) only stands up the
//! policy surface so a UI can show "this is what your node currently
//! advertises". Slice 3 adds the quote endpoint that actually consults
//! the policy.
//!
//! On-disk shape (`public-mode-policy.json`):
//! ```json
//! {
//!   "accepted_rails": ["lightning"],
//!   "min_kyc": "none",
//!   "blocked_countries": ["KP"]
//! }
//! ```
//! A missing file reads as the "closed" policy (accept nothing) — the
//! safe default for a peer that has not opted in.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub use unhosted_payments_core::{
    Country, KycTier, PayerContext, PaymentRail, PeerPaymentPolicy, PolicyError,
};

const POLICY_FILE: &str = "public-mode-policy.json";

fn policy_path() -> Result<PathBuf> {
    crate::paths::config_file(POLICY_FILE)
}

/// Read the persisted policy. Returns `PeerPaymentPolicy::closed()`
/// when the file is missing — i.e. the user has not opted in. Errors
/// only when the file exists but cannot be parsed, which is a real
/// problem the user should see (it would otherwise silently downgrade
/// to "closed" and the user would wonder why their peer rejects
/// everything).
pub fn load() -> Result<PeerPaymentPolicy> {
    let path = policy_path()?;
    if !path.exists() {
        return Ok(PeerPaymentPolicy::closed());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice::<PeerPaymentPolicy>(&bytes)
        .with_context(|| format!("parse {}", path.display()))
}

/// Atomically replace the persisted policy. Writes to a `.tmp`
/// sibling then renames — survives a power loss mid-write without
/// leaving a half-written JSON file the next `load()` would choke on.
pub fn save(policy: &PeerPaymentPolicy) -> Result<()> {
    let path = policy_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(policy).context("serialize policy")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
