//! Server-side chat history.
//!
//! Before v0.0.7 each browser kept its own chat list in `localStorage`.
//! That meant the desktop UI on `localhost:7777` and the phone PWA on
//! `<lan-ip>:7777` (different origins → separate storage) showed
//! different histories even though both talked to the same daemon.
//!
//! This module owns the canonical store. Single-tenant for now: every
//! paired client of this daemon sees the same list. When multi-user
//! lands (ADR 0006 public mode), chats will be keyed by `owner_pubkey`.
//! Until then YAGNI.
//!
//! Persistence: a single JSON file at `~/.config/unhosted/chats.json`.
//! Writes go through `write_atomic` (tmp + rename) so a crash mid-write
//! can't corrupt the file. Fits the project's existing flat-file
//! pattern (peers.toml, identity.toml, api-token.txt).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

const CHATS_FILE: &str = "chats.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<MessageStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageStats {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "servedBy")]
    pub served_by: Option<String>,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "tokPerSec")]
    pub tok_per_sec: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: u64,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChatsFile {
    #[serde(default)]
    pub chats: Vec<Chat>,
}

/// Sort key — most-recently-active first. We expose this on `Chat`
/// rather than reaching into the messages array everywhere.
fn chat_sort_key(c: &Chat) -> u64 {
    c.updated_at.max(c.created_at)
}

#[derive(Clone)]
pub struct ChatStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    file: ChatsFile,
}

impl ChatStore {
    /// Load (or create) the chats file. Missing → empty store; malformed →
    /// log and start empty rather than blowing up daemon startup.
    pub fn load_or_create() -> Result<Self> {
        let path = paths::config_file(CHATS_FILE).context("resolving chats path")?;
        let file = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(s) => match serde_json::from_str::<ChatsFile>(&s) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), "chats file malformed — starting empty");
                        ChatsFile::default()
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "could not read chats file");
                    ChatsFile::default()
                }
            }
        } else {
            ChatsFile::default()
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner { path, file })),
        })
    }

    /// Returns every chat, newest-activity first. With the per-account
    /// cap of 50 chats this is a tiny payload — single round trip keeps
    /// the web UI bootstrap simple.
    pub fn list(&self) -> Vec<Chat> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<Chat> = inner.file.chats.clone();
        out.sort_by(|a, b| chat_sort_key(b).cmp(&chat_sort_key(a)));
        out
    }

    pub fn get(&self, id: &str) -> Option<Chat> {
        let inner = self.inner.lock().unwrap();
        inner.file.chats.iter().find(|c| c.id == id).cloned()
    }

    /// Insert-or-replace by id. Returns the stored chat after normalization
    /// (timestamps filled in if the client omitted them).
    pub fn upsert(&self, mut chat: Chat) -> Result<Chat> {
        let now = now_ms();
        if chat.created_at == 0 {
            chat.created_at = now;
        }
        chat.updated_at = now;
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = inner.file.chats.iter_mut().find(|c| c.id == chat.id) {
            *existing = chat.clone();
        } else {
            inner.file.chats.push(chat.clone());
        }
        write_atomic(&inner.path, &inner.file)?;
        Ok(chat)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.file.chats.len();
        inner.file.chats.retain(|c| c.id != id);
        let removed = inner.file.chats.len() < before;
        if removed {
            write_atomic(&inner.path, &inner.file)?;
        }
        Ok(removed)
    }

    pub fn clear(&self) -> Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        let n = inner.file.chats.len();
        inner.file.chats.clear();
        write_atomic(&inner.path, &inner.file)?;
        Ok(n)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write JSON to `path` atomically: serialize to `<path>.tmp`, then
/// rename. On the same filesystem rename is atomic on POSIX and Windows
/// (since NTFS journaling); a crash mid-write leaves the old file intact.
fn write_atomic(path: &PathBuf, file: &ChatsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating chats dir {}", parent.display())
        })?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(file).context("serializing chats")?;
    std::fs::write(&tmp, &body)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}
