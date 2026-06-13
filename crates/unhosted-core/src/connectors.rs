//! Connector consent ledger + token vault (local-user only).
//!
//! This module persists two local files under `~/.config/unhosted/`:
//! - `connector-consent.json` — explicit consent + connection state.
//! - `connector-vault.json`   — OAuth tokens (access/refresh), never
//!   returned by API handlers.
//!
//! Defaults are deny-by-default: every connector is disabled +
//! disconnected until the local user opts in.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const CONSENT_FILE: &str = "connector-consent.json";
const VAULT_FILE: &str = "connector-vault.json";

pub const CONNECTORS: &[&str] = &["google", "notion", "slack"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConsent {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub has_token: bool,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConsentLedger {
    #[serde(default)]
    pub connectors: BTreeMap<String, ConnectorConsent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectorToken {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConnectorTokenVault {
    #[serde(default)]
    tokens: BTreeMap<String, ConnectorToken>,
}

pub fn is_known_connector(name: &str) -> bool {
    CONNECTORS.contains(&name)
}

pub fn load_consent() -> ConnectorConsentLedger {
    let Ok(path) = consent_path() else {
        return default_ledger();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return default_ledger();
    };
    let mut ledger = serde_json::from_slice::<ConnectorConsentLedger>(&bytes).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %path.display(), "connectors: consent parse failed");
        default_ledger()
    });
    normalize_ledger(&mut ledger);
    let vault = load_vault();
    for (name, c) in &mut ledger.connectors {
        c.has_token = vault.tokens.contains_key(name);
    }
    ledger
}

pub fn set_enabled(connector: &str, enabled: bool) -> Result<ConnectorConsentLedger> {
    let mut ledger = load_consent();
    let now = now_secs();
    let entry = ledger
        .connectors
        .entry(connector.to_string())
        .or_insert_with(|| ConnectorConsent {
            enabled: false,
            connected: false,
            has_token: false,
            scopes: Vec::new(),
            account: None,
            updated_at: now,
        });

    entry.enabled = enabled;
    entry.updated_at = now;

    if !enabled {
        entry.connected = false;
        entry.has_token = false;
        entry.scopes.clear();
        entry.account = None;
        remove_token(connector)?;
    }

    save_consent(&ledger)?;
    Ok(ledger)
}

#[derive(Debug, Clone)]
pub struct ConnectInput {
    pub account: Option<String>,
    pub scopes: Vec<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
}

pub fn connect(connector: &str, input: ConnectInput) -> Result<ConnectorConsentLedger> {
    let mut ledger = load_consent();
    let now = now_secs();
    let entry = ledger
        .connectors
        .entry(connector.to_string())
        .or_insert_with(|| ConnectorConsent {
            enabled: false,
            connected: false,
            has_token: false,
            scopes: Vec::new(),
            account: None,
            updated_at: now,
        });

    entry.enabled = true;
    entry.connected = true;
    entry.updated_at = now;
    if !input.scopes.is_empty() {
        let mut uniq = BTreeSet::new();
        for s in input.scopes {
            let t = s.trim();
            if !t.is_empty() {
                uniq.insert(t.to_string());
            }
        }
        entry.scopes = uniq.into_iter().collect();
    }
    if input.account.is_some() {
        entry.account = input.account;
    }

    if let Some(access) = input.access_token {
        store_token(
            connector,
            ConnectorToken {
                access_token: access,
                refresh_token: input.refresh_token,
                expires_at: input.expires_at,
                updated_at: now,
            },
        )?;
        entry.has_token = true;
    } else {
        // Still connected (consent granted), token optional for now.
        entry.has_token = load_vault().tokens.contains_key(connector);
    }

    save_consent(&ledger)?;
    Ok(ledger)
}

pub fn disconnect(connector: &str) -> Result<ConnectorConsentLedger> {
    let mut ledger = load_consent();
    let now = now_secs();
    let entry = ledger
        .connectors
        .entry(connector.to_string())
        .or_insert_with(|| ConnectorConsent {
            enabled: false,
            connected: false,
            has_token: false,
            scopes: Vec::new(),
            account: None,
            updated_at: now,
        });

    entry.connected = false;
    entry.has_token = false;
    entry.scopes.clear();
    entry.account = None;
    entry.updated_at = now;
    remove_token(connector)?;

    save_consent(&ledger)?;
    Ok(ledger)
}

fn default_ledger() -> ConnectorConsentLedger {
    let mut connectors = BTreeMap::new();
    let now = now_secs();
    for name in CONNECTORS {
        connectors.insert(
            (*name).to_string(),
            ConnectorConsent {
                enabled: false,
                connected: false,
                has_token: false,
                scopes: Vec::new(),
                account: None,
                updated_at: now,
            },
        );
    }
    ConnectorConsentLedger { connectors }
}

fn normalize_ledger(ledger: &mut ConnectorConsentLedger) {
    let now = now_secs();
    for name in CONNECTORS {
        ledger
            .connectors
            .entry((*name).to_string())
            .or_insert_with(|| ConnectorConsent {
                enabled: false,
                connected: false,
                has_token: false,
                scopes: Vec::new(),
                account: None,
                updated_at: now,
            });
    }
}

fn consent_path() -> Result<PathBuf> {
    crate::paths::config_file(CONSENT_FILE)
}

fn vault_path() -> Result<PathBuf> {
    crate::paths::config_file(VAULT_FILE)
}

fn save_consent(ledger: &ConnectorConsentLedger) -> Result<()> {
    let path = consent_path().context("resolving consent path")?;
    atomic_write_json(&path, ledger).context("saving connector consent")
}

fn load_vault() -> ConnectorTokenVault {
    let Ok(path) = vault_path() else {
        return ConnectorTokenVault::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return ConnectorTokenVault::default();
    };
    serde_json::from_slice::<ConnectorTokenVault>(&bytes).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %path.display(), "connectors: token vault parse failed");
        ConnectorTokenVault::default()
    })
}

fn save_vault(vault: &ConnectorTokenVault) -> Result<()> {
    let path = vault_path().context("resolving vault path")?;
    atomic_write_json(&path, vault).context("saving connector vault")
}

fn store_token(connector: &str, token: ConnectorToken) -> Result<()> {
    let mut vault = load_vault();
    vault.tokens.insert(connector.to_string(), token);
    save_vault(&vault)
}

fn remove_token(connector: &str) -> Result<()> {
    let mut vault = load_vault();
    vault.tokens.remove(connector);
    save_vault(&vault)
}

fn atomic_write_json<T: Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).context("serialize json")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
