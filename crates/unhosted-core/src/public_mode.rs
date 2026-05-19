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
use std::collections::BTreeSet;
use std::path::PathBuf;

pub use unhosted_payments_core::{
    Country, KycTier, PayerContext, PaymentRail, PeerPaymentPolicy, PolicyError,
};

const POLICY_FILE: &str = "public-mode-policy.json";

/// Recommended sanctions-default block-list. These are the
/// jurisdictions under comprehensive U.S. OFAC sanctions at the time
/// of writing (see COMPLIANCE.md). It is **the operator's** ongoing
/// duty to update this list as sanctions change — but the daemon
/// auto-merges this set into any policy that arrives from PUT, so a
/// caller can't accidentally ship a policy that's open to a comp-
/// sanctioned jurisdiction. They can still PUT a policy with these
/// countries omitted; the merge re-adds them. Removing one requires
/// editing this list and rebuilding — a deliberate friction.
const SANCTIONS_DEFAULT_BLOCKED: &[&str] = &["KP", "IR", "SY", "CU"];

fn sanctions_defaults() -> BTreeSet<Country> {
    SANCTIONS_DEFAULT_BLOCKED
        .iter()
        .map(|c| Country::new(c).expect("hard-coded ISO codes are valid"))
        .collect()
}

/// Merge the recommended sanctions block-list into `policy` in place.
/// Caller's own additions are preserved. Returns whether anything was
/// added so the caller can surface a notice ("we added KP to your
/// block-list to keep the daemon safe").
pub fn enforce_sanctions_defaults(policy: &mut PeerPaymentPolicy) -> bool {
    let before = policy.blocked_countries.len();
    for c in sanctions_defaults() {
        policy.blocked_countries.insert(c);
    }
    policy.blocked_countries.len() > before
}

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
///
/// The sanctions-default block-list is merged into `policy` before
/// writing. A caller PUTting a policy with `blocked_countries: []`
/// will get back a policy with the default sanctions list applied
/// — there is no way through this code path to disable the defaults
/// short of editing `SANCTIONS_DEFAULT_BLOCKED` and rebuilding the
/// daemon. This is deliberate. See COMPLIANCE.md.
pub fn save(policy: &mut PeerPaymentPolicy) -> Result<()> {
    enforce_sanctions_defaults(policy);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanctions_defaults_include_comp_ofac_jurisdictions() {
        let s = sanctions_defaults();
        for code in ["KP", "IR", "SY", "CU"] {
            assert!(
                s.contains(&Country::new(code).unwrap()),
                "sanctions defaults must include {code}"
            );
        }
    }

    #[test]
    fn enforce_adds_missing_codes() {
        let mut p = PeerPaymentPolicy::closed();
        let changed = enforce_sanctions_defaults(&mut p);
        assert!(changed);
        for code in ["KP", "IR", "SY", "CU"] {
            assert!(p.blocked_countries.contains(&Country::new(code).unwrap()));
        }
    }

    #[test]
    fn enforce_preserves_caller_additions() {
        let mut p = PeerPaymentPolicy::closed();
        p.blocked_countries.insert(Country::new("BY").unwrap());
        enforce_sanctions_defaults(&mut p);
        assert!(p.blocked_countries.contains(&Country::new("BY").unwrap()));
        assert!(p.blocked_countries.contains(&Country::new("KP").unwrap()));
    }

    #[test]
    fn enforce_is_idempotent() {
        let mut p = PeerPaymentPolicy::closed();
        let first = enforce_sanctions_defaults(&mut p);
        let second = enforce_sanctions_defaults(&mut p);
        assert!(first);
        assert!(!second);
    }
}
