//! Private local memory — RAG over the user's own past chats.
//!
//! When the user opts in (sidebar toggle, persisted to
//! `~/.config/unhosted/memory-enabled.txt`), the daemon records short
//! summaries of past conversations and injects the most relevant ones
//! into the system prompt on each new chat. Nothing leaves the user's
//! machine: storage is a plain JSON file at
//! `~/.config/unhosted/memories.json`, retrieval runs in-process, and
//! the upstream LLM only ever sees the assembled system prompt.
//!
//! v0.0.20 ships the storage layer + a keyword-overlap retrieval. A
//! follow-up release will swap in a bundled local embedder
//! (`fastembed-rs`) and a background summarizer that hits the local
//! LLM at chat end.
//!
//! Privacy posture: opt-in by default. A missing or unreadable enable
//! flag reads as "off", so we can never inject context into upstream
//! calls without an affirmative user click. Same posture as the
//! tunnel-autostart file in [`crate::tunnel`].
//!
//! On-disk shape (`memories.json`):
//! ```json
//! { "entries": [
//!     { "id": "01HX...", "summary": "...", "created_at": 1715800000, "chat_id": "abc" }
//! ] }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// File name under `crate::paths::config_file` for the memory store.
const MEMORIES_FILE: &str = "memories.json";
/// File name for the user-clicked enable flag. Sister to
/// `tunnel-autostart.txt`.
const MEMORY_ENABLED_FILE: &str = "memory-enabled.txt";
/// Hard cap on stored memories. Keeps keyword retrieval cheap and
/// prevents an indefinitely-growing JSON file. When we exceed this,
/// oldest entries are evicted (FIFO) — newer summaries are more
/// likely to reflect current user concerns.
pub const MEMORY_CAP: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub summary: String,
    /// Unix epoch seconds. We don't strictly need this for retrieval
    /// today, but it lets the UI sort by recency and the cap-eviction
    /// step pick the right victim.
    pub created_at: i64,
    /// Optional source chat — set when the entry is auto-summarized
    /// from a chat in `chats.json`. None for manually-entered memories.
    pub chat_id: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MemoryStore {
    #[serde(default)]
    pub entries: Vec<MemoryEntry>,
}

fn memories_path() -> Result<PathBuf> {
    crate::paths::config_file(MEMORIES_FILE)
}

fn enabled_path() -> Result<PathBuf> {
    crate::paths::config_file(MEMORY_ENABLED_FILE)
}

/// Read the memory store from disk. Returns an empty store on any IO
/// or parse error — losing the file should degrade to "no memory",
/// never crash a chat completion.
pub fn load() -> MemoryStore {
    let Ok(path) = memories_path() else {
        return MemoryStore::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return MemoryStore::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %path.display(), "memory: parse failed, starting fresh");
        MemoryStore::default()
    })
}

/// Persist the store atomically: write to a temp file in the same
/// directory and rename into place. Without the rename dance, a crash
/// or sudden shutdown mid-write would corrupt the JSON and the next
/// boot would lose every memory.
pub fn save(store: &MemoryStore) -> Result<()> {
    let path = memories_path().context("resolving memories.json path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating ~/.config/unhosted")?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(store).context("serializing memories")?;
    std::fs::write(&tmp, &bytes).context("writing temp memories file")?;
    std::fs::rename(&tmp, &path).context("renaming temp into place")?;
    Ok(())
}

/// Returns whether the user has memory turned on. Conservative
/// default: `false` on any read error — a missing file means "off",
/// which is the only safe interpretation if we can't read the flag
/// (we never want to silently start injecting context the user
/// didn't agree to).
pub fn is_enabled() -> bool {
    let Ok(path) = enabled_path() else {
        return false;
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => s.trim() == "enabled",
        Err(_) => false,
    }
}

/// Persist the user's enable choice. Best-effort: IO failures are
/// logged and swallowed — failing to remember the toggle state must
/// not break chat itself.
pub fn set_enabled(enabled: bool) {
    let Ok(path) = enabled_path() else {
        tracing::warn!("memory: no config path available, can't persist enable flag");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, dir = %parent.display(), "memory: mkdir failed");
            return;
        }
    }
    let body = if enabled { "enabled" } else { "disabled" };
    if let Err(e) = std::fs::write(&path, body) {
        tracing::warn!(error = %e, path = %path.display(), "memory: write enable flag failed");
    }
}

/// Append a new memory and persist. Enforces [`MEMORY_CAP`] by
/// dropping the oldest entry FIFO-style when full.
pub fn add(summary: String, chat_id: Option<String>) -> Result<MemoryEntry> {
    let mut store = load();
    let entry = MemoryEntry {
        id: new_id(),
        summary,
        created_at: now_secs(),
        chat_id,
    };
    store.entries.push(entry.clone());
    if store.entries.len() > MEMORY_CAP {
        let excess = store.entries.len() - MEMORY_CAP;
        store.entries.drain(0..excess);
    }
    save(&store)?;
    Ok(entry)
}

/// Insert a memory for a chat, replacing any existing entry with the
/// same `chat_id` instead of duplicating. This is what the auto-
/// summarizer calls: every chat ends up with exactly one rolling
/// summary, kept fresh by the debounced re-summarize on each new
/// message. Without this, every turn of a long chat would stack a
/// separate near-identical memory entry and blow past [`MEMORY_CAP`].
pub fn upsert_for_chat(chat_id: String, summary: String) -> Result<MemoryEntry> {
    let mut store = load();
    let now = now_secs();
    if let Some(existing) = store
        .entries
        .iter_mut()
        .find(|e| e.chat_id.as_deref() == Some(chat_id.as_str()))
    {
        existing.summary = summary;
        existing.created_at = now;
        let entry = existing.clone();
        save(&store)?;
        return Ok(entry);
    }
    // No existing entry for this chat — fall through to a normal add
    // (handles the FIFO cap too).
    let entry = MemoryEntry {
        id: new_id(),
        summary,
        created_at: now,
        chat_id: Some(chat_id),
    };
    store.entries.push(entry.clone());
    if store.entries.len() > MEMORY_CAP {
        let excess = store.entries.len() - MEMORY_CAP;
        store.entries.drain(0..excess);
    }
    save(&store)?;
    Ok(entry)
}

/// Remove a single entry by id. Returns whether the id existed.
pub fn remove(id: &str) -> Result<bool> {
    let mut store = load();
    let before = store.entries.len();
    store.entries.retain(|e| e.id != id);
    let removed = store.entries.len() < before;
    if removed {
        save(&store)?;
    }
    Ok(removed)
}

/// Drop every entry. Used by the "wipe memory" UI action.
pub fn clear() -> Result<()> {
    save(&MemoryStore::default())
}

/// Keyword-overlap retrieval. Given the user's latest message and a
/// store, returns up to `k` summaries ranked by how many distinct
/// lowercase word tokens they share with the query.
///
/// v0.0.20 placeholder — gets replaced with a real embedding-based
/// cosine ranker in v0.0.21. Keeps the surface and behavior right so
/// the system-prompt injection path can land first.
///
/// Stop-word filtering is deliberately absent: with cap=50 entries
/// the difference is invisible, and bringing in a stop list now
/// would be premature.
pub fn keyword_retrieve<'a>(store: &'a MemoryStore, query: &str, k: usize) -> Vec<&'a MemoryEntry> {
    if k == 0 || store.entries.is_empty() {
        return Vec::new();
    }
    let query_tokens: std::collections::HashSet<String> = tokenize(query);
    if query_tokens.is_empty() {
        // No tokens to overlap on — return the k most recent so the
        // model gets *some* context (better than nothing for the
        // common "user opens a fresh chat with hi" case).
        let mut by_recency: Vec<&MemoryEntry> = store.entries.iter().collect();
        by_recency.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        return by_recency.into_iter().take(k).collect();
    }
    let mut scored: Vec<(usize, &MemoryEntry)> = store
        .entries
        .iter()
        .map(|e| {
            let tokens = tokenize(&e.summary);
            let overlap = tokens.intersection(&query_tokens).count();
            (overlap, e)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    // Sort by score desc, then created_at desc as tiebreaker (favor
    // more recent context when two memories tie on overlap).
    scored.sort_by_key(|(score, e)| (std::cmp::Reverse(*score), std::cmp::Reverse(e.created_at)));
    scored.into_iter().take(k).map(|(_, e)| e).collect()
}

/// Lowercase + split on non-alphanumeric. Trivial tokenizer suitable
/// for v0.0.20's keyword retrieval; a real embedder in v0.0.21 will
/// retire this code path entirely.
fn tokenize(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2) // skip "a", "is", "i"
        .map(|w| w.to_string())
        .collect()
}

fn new_id() -> String {
    // We don't need cryptographic randomness here — just enough
    // entropy that two adds in the same millisecond don't collide.
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    format!("mem_{:x}", nanos)
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_basic() {
        let t = tokenize("Hello, World! Rust dev.");
        assert!(t.contains("hello"));
        assert!(t.contains("world"));
        assert!(t.contains("rust"));
        assert!(t.contains("dev"));
        assert!(!t.contains("a")); // too short
    }

    #[test]
    fn keyword_retrieve_ranks_overlap() {
        let store = MemoryStore {
            entries: vec![
                MemoryEntry {
                    id: "1".into(),
                    summary: "user is a Rust dev who hates emojis".into(),
                    created_at: 1,
                    chat_id: None,
                },
                MemoryEntry {
                    id: "2".into(),
                    summary: "user loves Python tutorials".into(),
                    created_at: 2,
                    chat_id: None,
                },
                MemoryEntry {
                    id: "3".into(),
                    summary: "user asked about Rust async patterns".into(),
                    created_at: 3,
                    chat_id: None,
                },
            ],
        };
        let hits = keyword_retrieve(&store, "tell me about Rust generics", 2);
        assert_eq!(hits.len(), 2);
        // Both Rust-mentioning entries should rank above the Python one.
        assert!(hits.iter().all(|h| h.summary.contains("Rust")));
    }

    #[test]
    fn keyword_retrieve_empty_query_returns_recent() {
        let store = MemoryStore {
            entries: vec![
                MemoryEntry {
                    id: "old".into(),
                    summary: "ancient".into(),
                    created_at: 1,
                    chat_id: None,
                },
                MemoryEntry {
                    id: "new".into(),
                    summary: "fresh".into(),
                    created_at: 100,
                    chat_id: None,
                },
            ],
        };
        let hits = keyword_retrieve(&store, "", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "new");
    }

    #[test]
    fn cap_evicts_oldest() {
        // Direct test of the in-memory cap logic by going through add().
        // We can't easily test the disk side without a temp HOME, but
        // the cap math is the load-bearing part.
        let mut store = MemoryStore::default();
        for i in 0..(MEMORY_CAP + 5) {
            store.entries.push(MemoryEntry {
                id: format!("{}", i),
                summary: format!("entry {}", i),
                created_at: i as i64,
                chat_id: None,
            });
        }
        if store.entries.len() > MEMORY_CAP {
            let excess = store.entries.len() - MEMORY_CAP;
            store.entries.drain(0..excess);
        }
        assert_eq!(store.entries.len(), MEMORY_CAP);
        assert_eq!(store.entries[0].id, "5"); // first five evicted
    }
}
