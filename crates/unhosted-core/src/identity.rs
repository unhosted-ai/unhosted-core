//! Per-node Ed25519 identity. The keypair is generated on first start and
//! persisted to `~/.config/unhosted/identity.toml`. It survives restarts,
//! model swaps, and IP changes — it's the stable name of *this* daemon.
//!
//! Used by trusted-mode pairing (v0.1.0+) to authenticate peers without
//! a central PKI.

use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD_NO_PAD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

/// On-disk shape. We store the secret as a 32-byte base64 string; the
/// public key is derivable but cached for human inspection.
#[derive(Serialize, Deserialize)]
struct IdentityFile {
    secret_b64: String,
    public_b64: String,
}

/// In-memory identity. Cheap to clone (Arc-backed key material).
#[derive(Clone)]
pub struct Identity {
    signing: std::sync::Arc<SigningKey>,
}

impl Identity {
    /// Load the identity from disk, generating + persisting a new keypair if
    /// none exists yet.
    pub fn load_or_create() -> Result<Self> {
        Self::load_or_create_at(&config_path()?)
    }

    /// Same as [`Identity::load_or_create`] but uses an explicit path
    /// instead of reading `XDG_CONFIG_HOME`. Useful for tests that need
    /// isolation from process-global env vars.
    pub fn load_or_create_at(path: &std::path::Path) -> Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let stored: IdentityFile =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            let secret_bytes = B64
                .decode(stored.secret_b64.as_bytes())
                .context("decoding identity secret")?;
            let secret_array: [u8; 32] = secret_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("identity secret is not 32 bytes"))?;
            let signing = SigningKey::from_bytes(&secret_array);
            return Ok(Self {
                signing: std::sync::Arc::new(signing),
            });
        }

        // generate + persist
        let signing = SigningKey::generate(&mut OsRng);
        let public = signing.verifying_key();
        let file = IdentityFile {
            secret_b64: B64.encode(signing.to_bytes()),
            public_b64: B64.encode(public.to_bytes()),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, toml::to_string_pretty(&file)?)
            .with_context(|| format!("writing {}", path.display()))?;
        // Tighten permissions on the secret file (owner-only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }

        Ok(Self {
            signing: std::sync::Arc::new(signing),
        })
    }

    pub fn public_b64(&self) -> String {
        B64.encode(self.signing.verifying_key().to_bytes())
    }

    /// Raw 32-byte Ed25519 secret. Used by the transport module to
    /// derive a self-signed TLS cert; the bytes never leave the process.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// Borrow the inner SigningKey. Used by code that needs to feed
    /// the key into an ed25519-dalek-compatible helper (e.g.
    /// `unhosted_payments_core::sign_receipt`) without going through
    /// our own `sign()` wrapper.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    pub fn sign(&self, message: &[u8]) -> String {
        let sig: Signature = self.signing.sign(message);
        B64.encode(sig.to_bytes())
    }

    /// Build an `X-Unhosted-Auth` header value over the given request body.
    /// Format: `<pubkey>:<unix_ts>:<sig>`. Signature is over `<ts>\n<body>`.
    pub fn sign_request(&self, body: &[u8]) -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut to_sign = Vec::with_capacity(20 + body.len());
        to_sign.extend_from_slice(ts.to_string().as_bytes());
        to_sign.push(b'\n');
        to_sign.extend_from_slice(body);
        let sig = self.sign(&to_sign);
        format!("{}:{}:{}", self.public_b64(), ts, sig)
    }

    /// Verify an `X-Unhosted-Auth` header. Returns the sender's pubkey on
    /// success, or `None` if the header is malformed, the timestamp is
    /// too far skewed (>5min), or the signature doesn't check out.
    pub fn verify_request(header: &str, body: &[u8]) -> Option<String> {
        let mut parts = header.splitn(3, ':');
        let pubkey = parts.next()?;
        let ts_str = parts.next()?;
        let sig = parts.next()?;

        let ts: u64 = ts_str.parse().ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let skew = now.abs_diff(ts);
        if skew > 300 {
            return None; // 5min replay window
        }

        let mut to_verify = Vec::with_capacity(20 + body.len());
        to_verify.extend_from_slice(ts_str.as_bytes());
        to_verify.push(b'\n');
        to_verify.extend_from_slice(body);

        if Self::verify(pubkey, &to_verify, sig) {
            Some(pubkey.to_string())
        } else {
            None
        }
    }

    /// Verify a base64-encoded signature against a pubkey + message.
    pub fn verify(pubkey_b64: &str, message: &[u8], sig_b64: &str) -> bool {
        let Ok(pk_bytes) = B64.decode(pubkey_b64.as_bytes()) else {
            return false;
        };
        let pk_array: [u8; 32] = match pk_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let Ok(verifying) = VerifyingKey::from_bytes(&pk_array) else {
            return false;
        };
        let Ok(sig_bytes) = B64.decode(sig_b64.as_bytes()) else {
            return false;
        };
        let sig_array: [u8; 64] = match sig_bytes.try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let signature = Signature::from_bytes(&sig_array);
        verifying.verify(message, &signature).is_ok()
    }
}

pub fn config_path() -> Result<PathBuf> {
    crate::paths::config_file("identity.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        // Use an explicit, per-test path. No XDG_CONFIG_HOME, no shared state.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("unhosted-id-{}-{stamp}", std::process::id()));
        let path = tmp.join("identity.toml");

        let id = Identity::load_or_create_at(&path).unwrap();
        let msg = b"hello world";
        let sig = id.sign(msg);

        assert!(Identity::verify(&id.public_b64(), msg, &sig));
        assert!(!Identity::verify(&id.public_b64(), b"different", &sig));

        // load_or_create twice returns the same key (persistence works).
        let id2 = Identity::load_or_create_at(&path).unwrap();
        assert_eq!(id.public_b64(), id2.public_b64());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
