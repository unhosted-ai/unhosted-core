//! Operator configuration for the Lightning rail adapter.
//!
//! Reads `~/.config/unhosted/lightning.toml` and produces a
//! `LightningConfig` ready to hand to `LightningAdapter::new`. The
//! file is optional: a missing file is the explicit "operator has not
//! opted into Lightning yet" signal, and the daemon skips
//! registration in that case (no error, no log spam).
//!
//! Example file:
//!
//! ```toml
//! rest_url = "https://127.0.0.1:8080"
//! macaroon_hex = "0201036c6e6402..."
//! tls_skip_verify = true   # LND default is a self-signed cert
//! sats_per_unit = 1
//! invoice_ttl_seconds = 3600
//! ```
//!
//! The crate that owns `LightningConfig` is gated behind the
//! `rail-lightning` cargo feature, so this whole module is too.

#![cfg(feature = "rail-lightning")]

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use unhosted_payments_lightning::LightningConfig;

use crate::paths;

/// What the TOML file deserializes into. We deliberately keep this
/// parallel to `LightningConfig` rather than aliasing — the on-disk
/// shape is the operator-facing contract and shouldn't move when
/// `LightningConfig` is internally restructured.
#[derive(Debug, Deserialize)]
struct OnDisk {
    rest_url: String,
    macaroon_hex: String,
    #[serde(default = "default_skip")]
    tls_skip_verify: bool,
    sats_per_unit: u64,
    #[serde(default = "default_ttl")]
    invoice_ttl_seconds: u64,
}

fn default_skip() -> bool {
    // LND ships with a self-signed cert by default and most operators
    // never replace it with a public-CA cert. Defaulting to "skip
    // verify" matches what a working LND deployment actually needs;
    // operators who pin the cert can flip this to false explicitly.
    // A follow-up slice will add cert-pinning so the default can
    // tighten without breaking existing configs.
    true
}

fn default_ttl() -> u64 {
    3600
}

pub fn config_path() -> Result<PathBuf> {
    paths::config_file("lightning.toml")
}

/// Returns `Ok(Some(cfg))` if the file exists and parses, `Ok(None)`
/// if the file is absent (the "operator hasn't opted in" path), and
/// `Err` on a parse / IO error. The daemon treats the parse error as
/// fatal-for-Lightning but not fatal-for-the-daemon — it logs and
/// skips registration.
pub fn load() -> Result<Option<LightningConfig>> {
    let path = config_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };
    let on_disk: OnDisk = toml::from_str(&text)
        .with_context(|| format!("parsing {} as TOML", path.display()))?;
    Ok(Some(LightningConfig {
        rest_url: on_disk.rest_url,
        macaroon_hex: on_disk.macaroon_hex,
        tls_skip_verify: on_disk.tls_skip_verify,
        sats_per_unit: on_disk.sats_per_unit,
        invoice_ttl_seconds: on_disk.invoice_ttl_seconds,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<OnDisk> {
        Ok(toml::from_str(s)?)
    }

    #[test]
    fn minimal_config_parses_with_defaults() {
        let cfg = parse(
            r#"
            rest_url = "https://127.0.0.1:8080"
            macaroon_hex = "deadbeef"
            sats_per_unit = 1
            "#,
        )
        .unwrap();
        assert_eq!(cfg.rest_url, "https://127.0.0.1:8080");
        assert_eq!(cfg.macaroon_hex, "deadbeef");
        assert!(cfg.tls_skip_verify, "defaults to skip — LND is self-signed by default");
        assert_eq!(cfg.invoice_ttl_seconds, 3600);
    }

    #[test]
    fn explicit_skip_false_round_trips() {
        let cfg = parse(
            r#"
            rest_url = "https://lnd.example"
            macaroon_hex = "ab"
            sats_per_unit = 2
            tls_skip_verify = false
            invoice_ttl_seconds = 600
            "#,
        )
        .unwrap();
        assert!(!cfg.tls_skip_verify);
        assert_eq!(cfg.invoice_ttl_seconds, 600);
    }
}
