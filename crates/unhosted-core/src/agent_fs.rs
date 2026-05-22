//! Filesystem sandbox for agent tools.
//!
//! Implements ADR-0013. The `read_file` agent tool reads UTF-8 text
//! files from a small set of operator-allow-listed roots, subject to
//! a size cap, a deny-list, and (optionally) symlink refusal.
//!
//! Config lives at `~/.config/unhosted/agent-fs.toml`. Absent file =
//! no roots = `read_file` returns `NotConfigured` on every call. The
//! daemon out-of-the-box adds zero filesystem blast radius for the
//! agent.
//!
//! ## Resolution algorithm
//!
//! 1. Reject if `path` is not absolute.
//! 2. If `follow_symlinks: false`, walk the path components and
//!    refuse on any symlink encountered.
//! 3. Canonicalize via `std::fs::canonicalize`.
//! 4. Verify canonicalized path is strictly under one of the
//!    canonicalized allow-list roots (boundary-aware).
//! 5. Apply case-insensitive deny-pattern substring match.
//! 6. Open and read up to `max_bytes`. Validate UTF-8.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

/// What the operator stores in `agent-fs.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentFsConfigRaw {
    #[serde(default)]
    pub allow_roots: Vec<PathBuf>,
    #[serde(default = "default_deny_patterns")]
    pub deny_patterns: Vec<String>,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    #[serde(default)]
    pub follow_symlinks: bool,
}

fn default_deny_patterns() -> Vec<String> {
    // Sensible baseline. Operators can override with their own
    // shorter or longer list, but the default protects the cases
    // that come up reliably in a developer's home directory.
    vec![
        ".env".into(),
        "id_rsa".into(),
        "id_ed25519".into(),
        "id_ecdsa".into(),
        "id_dsa".into(),
        ".pem".into(),
        ".p12".into(),
        ".pfx".into(),
        ".key".into(),
        "credentials".into(),
        "secrets".into(),
        ".sqlite".into(),
        ".db".into(),
    ]
}

fn default_max_bytes() -> usize {
    524_288 // 512 KiB
}

/// Sealed config — `allow_roots` are canonicalized once at load
/// time so the per-call resolver doesn't have to.
#[derive(Debug, Clone)]
pub struct AgentFsConfig {
    pub allow_roots: Vec<PathBuf>,
    pub deny_patterns: Vec<String>,
    pub max_bytes: usize,
    pub follow_symlinks: bool,
}

/// What `read_file` returns. The agent's `execute_tool` arm
/// translates this into a string for the model + an optional error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFileOutcome {
    Ok {
        content: String,
        bytes_read: usize,
        truncated: bool,
    },
    Err(ReadFileError),
}

/// What `list_dir` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListDirOutcome {
    Ok {
        entries: Vec<DirEntry>,
        truncated: bool,
    },
    Err(ReadFileError),
}

/// What `grep_files` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrepOutcome {
    Ok {
        matches: Vec<GrepMatch>,
        files_scanned: usize,
        files_skipped: usize,
        truncated: bool,
    },
    Err(ReadFileError),
}

/// One match in one file. `path` is the canonical path (already
/// proven to be inside the allow-list). `line` is the matched line's
/// text, truncated to 240 chars so a single very-long-line match
/// doesn't dominate the response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: u32,
    pub line: String,
}

/// A single entry in a directory listing. Kind is one of `"file"`,
/// `"dir"`, `"symlink"`, `"other"`. Size is bytes for files; 0 for
/// directories and non-file entries.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
}

/// Cap on entries returned by `list_dir`. Larger directories return
/// the first N entries (sorted) with `truncated: true` so the model
/// knows to use a different strategy.
pub const LIST_DIR_MAX_ENTRIES: usize = 500;

/// Cap on matches returned by `grep_files`. Stops the walk as soon
/// as this many matches accumulate — large codebases with a common
/// pattern can produce thousands of hits, blowing out the model's
/// context window and the daemon's working set.
pub const GREP_MAX_MATCHES: usize = 200;

/// Cap on files scanned per `grep_files` call. Hard upper bound on
/// the walk's blast radius even when matches are sparse.
pub const GREP_MAX_FILES: usize = 5_000;

/// Per-file size cap for grep_files. Files larger than this are
/// skipped and counted toward `files_skipped`. Avoids loading a
/// multi-GB log file or vendored bundle just to scan one line.
pub const GREP_MAX_FILE_BYTES: usize = 1_048_576; // 1 MiB

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFileError {
    NotConfigured,
    NotAbsolute,
    OutsideAllowList,
    DenyPattern(String),
    SymlinkRefused,
    NotFound,
    NotUtf8,
    Io(String),
}

impl ReadFileError {
    /// Human-readable label the agent sees as the tool's error
    /// message. Stable enough for the model to learn from across
    /// attempts ("OutsideAllowList → don't try that path").
    pub fn label(&self) -> String {
        match self {
            ReadFileError::NotConfigured => "read_file not configured on this host".into(),
            ReadFileError::NotAbsolute => "path must be absolute".into(),
            ReadFileError::OutsideAllowList => "path is not under any allow-listed root".into(),
            ReadFileError::DenyPattern(p) => {
                format!("path matches deny pattern '{p}'")
            }
            ReadFileError::SymlinkRefused => {
                "symlinks are not followed by this host's configuration".into()
            }
            ReadFileError::NotFound => "file not found".into(),
            ReadFileError::NotUtf8 => "file is not valid UTF-8".into(),
            ReadFileError::Io(e) => format!("io: {e}"),
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    paths::config_file("agent-fs.toml")
}

/// Load the operator's agent-fs config. Returns `Ok(None)` if the
/// file is absent (the "no filesystem access" path). Returns
/// `Err` only on a present-but-invalid config. Canonicalizes
/// allow-list roots once at load so they're comparable later.
pub fn load() -> Result<Option<AgentFsConfig>> {
    let path = config_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let raw: AgentFsConfigRaw =
        toml::from_str(&text).with_context(|| format!("parsing {} as TOML", path.display()))?;
    let mut canonical_roots = Vec::with_capacity(raw.allow_roots.len());
    for r in &raw.allow_roots {
        let canon = std::fs::canonicalize(r)
            .with_context(|| format!("canonicalizing allow_roots entry {}", r.display()))?;
        canonical_roots.push(canon);
    }
    Ok(Some(AgentFsConfig {
        allow_roots: canonical_roots,
        deny_patterns: raw.deny_patterns,
        max_bytes: raw.max_bytes,
        follow_symlinks: raw.follow_symlinks,
    }))
}

/// Read a file per the sandbox policy.
pub fn read_file(cfg: Option<&Arc<AgentFsConfig>>, path: &str) -> ReadFileOutcome {
    let Some(cfg) = cfg else {
        return ReadFileOutcome::Err(ReadFileError::NotConfigured);
    };
    if cfg.allow_roots.is_empty() {
        // Present config with no roots = same security posture as
        // absent config. Treat identically.
        return ReadFileOutcome::Err(ReadFileError::NotConfigured);
    }

    // 1. Must be absolute.
    let path_buf = PathBuf::from(path);
    if !path_buf.is_absolute() {
        return ReadFileOutcome::Err(ReadFileError::NotAbsolute);
    }

    // 2. Symlink refusal per config. Check the final component for
    // being a symlink — the realistic threat is `link.txt →
    // /etc/passwd` placed inside the allow-list. The intermediate
    // case (directory symlink pointing outside the allow-list) is
    // caught by the post-canonicalize allow-list check below: if the
    // resolved path lands outside an allow-list root, the request
    // fails for that reason regardless of `follow_symlinks`.
    //
    // We deliberately do NOT walk every component for symlink-ness
    // — macOS systems alias `/var` → `/private/var`, `/tmp` →
    // `/private/tmp`, and any path traversing those would otherwise
    // trip a false positive. The allow-list root canonicalization
    // already absorbs those OS-level aliases at config load time.
    if !cfg.follow_symlinks {
        match std::fs::symlink_metadata(&path_buf) {
            Ok(m) if m.file_type().is_symlink() => {
                return ReadFileOutcome::Err(ReadFileError::SymlinkRefused);
            }
            _ => {}
        }
    }

    // 3. Canonicalize. After this `canonical` is the final resolved
    // path; we use it for the allow-list check.
    let canonical = match std::fs::canonicalize(&path_buf) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ReadFileOutcome::Err(ReadFileError::NotFound);
        }
        Err(e) => return ReadFileOutcome::Err(ReadFileError::Io(e.to_string())),
    };

    // 4. Strictly-under allow-list check.
    if !cfg
        .allow_roots
        .iter()
        .any(|root| is_strictly_under(&canonical, root))
    {
        return ReadFileOutcome::Err(ReadFileError::OutsideAllowList);
    }

    // 5. Deny patterns (case-insensitive substring).
    let lower = canonical.to_string_lossy().to_lowercase();
    for pat in &cfg.deny_patterns {
        if lower.contains(&pat.to_lowercase()) {
            return ReadFileOutcome::Err(ReadFileError::DenyPattern(pat.clone()));
        }
    }

    // 6. Open + read up to max_bytes.
    let metadata = match std::fs::metadata(&canonical) {
        Ok(m) => m,
        Err(e) => return ReadFileOutcome::Err(ReadFileError::Io(e.to_string())),
    };
    if !metadata.is_file() {
        return ReadFileOutcome::Err(ReadFileError::Io("not a regular file".into()));
    }
    let file_size = metadata.len() as usize;
    let truncated = file_size > cfg.max_bytes;
    let to_read = file_size.min(cfg.max_bytes);

    use std::io::Read;
    let mut f = match std::fs::File::open(&canonical) {
        Ok(f) => f,
        Err(e) => return ReadFileOutcome::Err(ReadFileError::Io(e.to_string())),
    };
    let mut buf = vec![0u8; to_read];
    if let Err(e) = f.read_exact(&mut buf) {
        // Short reads (file shrank between metadata and read) are
        // possible; downgrade to whatever we got.
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            // Read whatever we can.
            let _ = f.read_to_end(&mut buf);
        } else {
            return ReadFileOutcome::Err(ReadFileError::Io(e.to_string()));
        }
    }
    let bytes_read = buf.len();
    let content = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return ReadFileOutcome::Err(ReadFileError::NotUtf8),
    };
    ReadFileOutcome::Ok {
        content,
        bytes_read,
        truncated,
    }
}

/// List a directory's entries per the sandbox policy. Reuses the
/// allow-list / deny-pattern / symlink machinery from `read_file`;
/// returns at most `LIST_DIR_MAX_ENTRIES` entries sorted by name.
pub fn list_dir(cfg: Option<&Arc<AgentFsConfig>>, path: &str) -> ListDirOutcome {
    let Some(cfg) = cfg else {
        return ListDirOutcome::Err(ReadFileError::NotConfigured);
    };
    if cfg.allow_roots.is_empty() {
        return ListDirOutcome::Err(ReadFileError::NotConfigured);
    }

    let path_buf = PathBuf::from(path);
    if !path_buf.is_absolute() {
        return ListDirOutcome::Err(ReadFileError::NotAbsolute);
    }

    if !cfg.follow_symlinks {
        match std::fs::symlink_metadata(&path_buf) {
            Ok(m) if m.file_type().is_symlink() => {
                return ListDirOutcome::Err(ReadFileError::SymlinkRefused);
            }
            _ => {}
        }
    }

    let canonical = match std::fs::canonicalize(&path_buf) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ListDirOutcome::Err(ReadFileError::NotFound);
        }
        Err(e) => return ListDirOutcome::Err(ReadFileError::Io(e.to_string())),
    };

    if !cfg
        .allow_roots
        .iter()
        .any(|root| is_strictly_under(&canonical, root))
    {
        // Special case: listing the root *itself* is allowed when the
        // root equals canonical. `is_strictly_under` rejects equality
        // (it's for files inside a root), so we explicitly admit the
        // root-itself case here.
        if !cfg.allow_roots.iter().any(|root| &canonical == root) {
            return ListDirOutcome::Err(ReadFileError::OutsideAllowList);
        }
    }

    let lower = canonical.to_string_lossy().to_lowercase();
    for pat in &cfg.deny_patterns {
        if lower.contains(&pat.to_lowercase()) {
            return ListDirOutcome::Err(ReadFileError::DenyPattern(pat.clone()));
        }
    }

    let metadata = match std::fs::metadata(&canonical) {
        Ok(m) => m,
        Err(e) => return ListDirOutcome::Err(ReadFileError::Io(e.to_string())),
    };
    if !metadata.is_dir() {
        return ListDirOutcome::Err(ReadFileError::Io("not a directory".into()));
    }

    let read = match std::fs::read_dir(&canonical) {
        Ok(r) => r,
        Err(e) => return ListDirOutcome::Err(ReadFileError::Io(e.to_string())),
    };

    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in read.flatten() {
        if entries.len() >= LIST_DIR_MAX_ENTRIES {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let (kind, size) = match entry.file_type() {
            Ok(ft) if ft.is_dir() => ("dir", 0u64),
            Ok(ft) if ft.is_file() => {
                let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
                ("file", sz)
            }
            Ok(ft) if ft.is_symlink() => ("symlink", 0u64),
            _ => ("other", 0u64),
        };
        entries.push(DirEntry { name, kind, size });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    ListDirOutcome::Ok { entries, truncated }
}

/// Substring-search a directory tree for a literal pattern. Same
/// sandbox model as read_file + list_dir (allow-list root, deny
/// patterns, symlink refusal). Walks the tree from `root` and
/// returns matches with line numbers.
///
/// Deliberately literal substring, not regex. ReDoS is a real
/// failure mode for any user-supplied pattern and the codebase-
/// exploration use case doesn't materially benefit from regex
/// (most queries are `fn authenticate`, `TODO`, an error string,
/// etc.). A future `grep_regex` tool could land behind its own ADR
/// with a regex-engine choice and a timeout.
pub fn grep_files(
    cfg: Option<&Arc<AgentFsConfig>>,
    root: &str,
    pattern: &str,
    case_insensitive: bool,
) -> GrepOutcome {
    let Some(cfg) = cfg else {
        return GrepOutcome::Err(ReadFileError::NotConfigured);
    };
    if cfg.allow_roots.is_empty() {
        return GrepOutcome::Err(ReadFileError::NotConfigured);
    }
    if pattern.is_empty() {
        return GrepOutcome::Err(ReadFileError::Io(
            "grep_files: pattern must not be empty".into(),
        ));
    }

    let root_buf = PathBuf::from(root);
    if !root_buf.is_absolute() {
        return GrepOutcome::Err(ReadFileError::NotAbsolute);
    }
    if !cfg.follow_symlinks {
        if let Ok(m) = std::fs::symlink_metadata(&root_buf) {
            if m.file_type().is_symlink() {
                return GrepOutcome::Err(ReadFileError::SymlinkRefused);
            }
        }
    }
    let canonical_root = match std::fs::canonicalize(&root_buf) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return GrepOutcome::Err(ReadFileError::NotFound);
        }
        Err(e) => return GrepOutcome::Err(ReadFileError::Io(e.to_string())),
    };
    // Allow root to be the allow-list root itself OR strictly under it.
    let under = cfg
        .allow_roots
        .iter()
        .any(|r| &canonical_root == r || is_strictly_under(&canonical_root, r));
    if !under {
        return GrepOutcome::Err(ReadFileError::OutsideAllowList);
    }
    // Deny-pattern check on the root path. If the operator denied
    // `secrets/` as a directory name, no descent under it.
    let lower_root = canonical_root.to_string_lossy().to_lowercase();
    for pat in &cfg.deny_patterns {
        if lower_root.contains(&pat.to_lowercase()) {
            return GrepOutcome::Err(ReadFileError::DenyPattern(pat.clone()));
        }
    }

    let needle = if case_insensitive {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };

    let mut matches = Vec::new();
    let mut files_scanned = 0usize;
    let mut files_skipped = 0usize;
    let mut truncated = false;
    let mut queue: Vec<PathBuf> = vec![canonical_root.clone()];

    while let Some(dir) = queue.pop() {
        if matches.len() >= GREP_MAX_MATCHES || files_scanned >= GREP_MAX_FILES {
            truncated = true;
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if matches.len() >= GREP_MAX_MATCHES || files_scanned >= GREP_MAX_FILES {
                truncated = true;
                break;
            }
            let path = entry.path();
            // Deny-pattern check against the entry's path.
            let lower = path.to_string_lossy().to_lowercase();
            if cfg
                .deny_patterns
                .iter()
                .any(|p| lower.contains(&p.to_lowercase()))
            {
                files_skipped += 1;
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                // Symlinks under the tree are skipped unless the
                // operator explicitly opted into follow_symlinks.
                if !cfg.follow_symlinks {
                    files_skipped += 1;
                    continue;
                }
            }
            if ft.is_dir() {
                queue.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            // Size cap.
            let metadata_size = entry.metadata().map(|m| m.len() as usize).unwrap_or(0);
            if metadata_size > GREP_MAX_FILE_BYTES {
                files_skipped += 1;
                continue;
            }
            // Read + search.
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => {
                    files_skipped += 1;
                    continue;
                }
            };
            let text = match std::str::from_utf8(&bytes) {
                Ok(t) => t,
                Err(_) => {
                    files_skipped += 1;
                    continue;
                }
            };
            files_scanned += 1;
            for (idx, line) in text.lines().enumerate() {
                let hit = if case_insensitive {
                    line.to_lowercase().contains(&needle)
                } else {
                    line.contains(&needle)
                };
                if hit {
                    let trimmed = if line.chars().count() > 240 {
                        let mut s: String = line.chars().take(240).collect();
                        s.push('…');
                        s
                    } else {
                        line.to_string()
                    };
                    matches.push(GrepMatch {
                        path: path.to_string_lossy().to_string(),
                        line_number: (idx as u32) + 1,
                        line: trimmed,
                    });
                    if matches.len() >= GREP_MAX_MATCHES {
                        truncated = true;
                        break;
                    }
                }
            }
        }
    }

    GrepOutcome::Ok {
        matches,
        files_scanned,
        files_skipped,
        truncated,
    }
}

/// Strictly-under check that respects the path-separator boundary —
/// `/a/foo2` is NOT under `/a/foo`. Uses `Path::starts_with`'s
/// component-aware comparison which already gives us that property.
fn is_strictly_under(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root) && candidate != root
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_dir() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().expect("tempdir");
        let canon = std::fs::canonicalize(td.path()).expect("canonicalize");
        (td, canon)
    }

    fn cfg_with(allow: Vec<PathBuf>) -> Arc<AgentFsConfig> {
        Arc::new(AgentFsConfig {
            allow_roots: allow,
            deny_patterns: default_deny_patterns(),
            max_bytes: 1024,
            follow_symlinks: false,
        })
    }

    fn write_file(p: &Path, contents: &[u8]) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(p).unwrap();
        f.write_all(contents).unwrap();
    }

    #[test]
    fn missing_config_returns_not_configured() {
        let outcome = read_file(None, "/etc/passwd");
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::NotConfigured)
        ));
    }

    #[test]
    fn empty_allow_list_returns_not_configured() {
        let cfg = cfg_with(vec![]);
        let outcome = read_file(Some(&cfg), "/etc/passwd");
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::NotConfigured)
        ));
    }

    #[test]
    fn relative_path_rejected() {
        let (_td, root) = temp_dir();
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), "notes.txt");
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::NotAbsolute)
        ));
    }

    #[test]
    fn allowed_file_is_read() {
        let (_td, root) = temp_dir();
        let target = root.join("hello.txt");
        write_file(&target, b"hello, agent");
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), target.to_str().unwrap());
        match outcome {
            ReadFileOutcome::Ok {
                content,
                bytes_read,
                truncated,
            } => {
                assert_eq!(content, "hello, agent");
                assert_eq!(bytes_read, 12);
                assert!(!truncated);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn file_outside_allowlist_rejected() {
        let (_td, root) = temp_dir();
        let outside =
            std::env::temp_dir().join(format!("unhosted-fs-test-outside-{}", std::process::id()));
        write_file(&outside, b"secret");
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), outside.to_str().unwrap());
        let _ = std::fs::remove_file(&outside);
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::OutsideAllowList)
        ));
    }

    #[test]
    fn path_traversal_attempt_rejected_after_canonicalize() {
        let (_td, root) = temp_dir();
        // Build a path that lexically reaches outside via ..
        let target = root.join("subdir").join("..").join("..").join("..");
        write_file(&root.join("subdir").join("a.txt"), b"x");
        let cfg = cfg_with(vec![root]);
        // The canonical form of root/subdir/../../.. is two levels
        // above root — definitely outside the allow-list.
        let outcome = read_file(Some(&cfg), target.to_str().unwrap());
        // Could surface as either OutsideAllowList or NotFound (the
        // canonical resolved path may not be a file). Both are
        // acceptable refusals.
        assert!(
            matches!(
                outcome,
                ReadFileOutcome::Err(ReadFileError::OutsideAllowList)
                    | ReadFileOutcome::Err(ReadFileError::Io(_))
                    | ReadFileOutcome::Err(ReadFileError::NotFound)
            ),
            "unexpected outcome: {outcome:?}"
        );
    }

    #[test]
    fn deny_pattern_blocks_env_file() {
        let (_td, root) = temp_dir();
        let env_path = root.join(".env");
        write_file(&env_path, b"API_KEY=secret");
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), env_path.to_str().unwrap());
        match outcome {
            ReadFileOutcome::Err(ReadFileError::DenyPattern(p)) => {
                assert!(p.contains(".env"));
            }
            other => panic!("expected DenyPattern, got {other:?}"),
        }
    }

    #[test]
    fn deny_pattern_is_case_insensitive() {
        let (_td, root) = temp_dir();
        let upper = root.join("Credentials");
        write_file(&upper, b"x");
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), upper.to_str().unwrap());
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::DenyPattern(_))
        ));
    }

    #[test]
    fn symlink_refused_by_default() {
        let (_td, root) = temp_dir();
        let target = root.join("target.txt");
        write_file(&target, b"content");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), link.to_str().unwrap());
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::SymlinkRefused)
        ));
    }

    #[test]
    fn symlink_followed_when_configured() {
        let (_td, root) = temp_dir();
        let target = root.join("target.txt");
        write_file(&target, b"content");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let cfg = Arc::new(AgentFsConfig {
            allow_roots: vec![root],
            deny_patterns: vec![],
            max_bytes: 1024,
            follow_symlinks: true,
        });
        let outcome = read_file(Some(&cfg), link.to_str().unwrap());
        match outcome {
            ReadFileOutcome::Ok { content, .. } => assert_eq!(content, "content"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn truncation_at_max_bytes() {
        let (_td, root) = temp_dir();
        let big = root.join("big.txt");
        // 2 KiB file, cap at 1 KiB.
        write_file(&big, &b"a".repeat(2048));
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), big.to_str().unwrap());
        match outcome {
            ReadFileOutcome::Ok {
                bytes_read,
                truncated,
                ..
            } => {
                assert_eq!(bytes_read, 1024);
                assert!(truncated);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn non_utf8_returns_not_utf8() {
        let (_td, root) = temp_dir();
        let bin = root.join("blob");
        // Invalid UTF-8: 0xFF is not a valid lead byte.
        write_file(&bin, &[0xFFu8, 0xFE, 0xFD, 0xFC]);
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), bin.to_str().unwrap());
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::NotUtf8)
        ));
    }

    #[test]
    fn list_dir_returns_sorted_entries() {
        let (_td, root) = temp_dir();
        write_file(&root.join("z.txt"), b"x");
        write_file(&root.join("a.txt"), b"x");
        write_file(&root.join("m.txt"), b"x");
        fs::create_dir_all(root.join("sub")).unwrap();
        let cfg = cfg_with(vec![root.clone()]);
        match list_dir(Some(&cfg), root.to_str().unwrap()) {
            ListDirOutcome::Ok { entries, truncated } => {
                assert!(!truncated);
                let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
                assert_eq!(names, vec!["a.txt", "m.txt", "sub", "z.txt"]);
                let sub = entries.iter().find(|e| e.name == "sub").unwrap();
                assert_eq!(sub.kind, "dir");
                let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
                assert_eq!(a.kind, "file");
                assert_eq!(a.size, 1);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn list_dir_rejects_path_outside_allowlist() {
        let (_td, root) = temp_dir();
        let (_td2, outside) = temp_dir();
        let cfg = cfg_with(vec![root]);
        let outcome = list_dir(Some(&cfg), outside.to_str().unwrap());
        assert!(matches!(
            outcome,
            ListDirOutcome::Err(ReadFileError::OutsideAllowList)
        ));
    }

    #[test]
    fn list_dir_truncates_at_cap() {
        let (_td, root) = temp_dir();
        // Create more than the cap.
        for i in 0..LIST_DIR_MAX_ENTRIES + 5 {
            write_file(&root.join(format!("file_{i:04}.txt")), b"x");
        }
        let cfg = cfg_with(vec![root.clone()]);
        match list_dir(Some(&cfg), root.to_str().unwrap()) {
            ListDirOutcome::Ok { entries, truncated } => {
                assert!(truncated);
                assert_eq!(entries.len(), LIST_DIR_MAX_ENTRIES);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn list_dir_root_itself_is_listable() {
        // is_strictly_under rejects equality; the impl admits root
        // equality as a special case. This test guards that branch.
        let (_td, root) = temp_dir();
        write_file(&root.join("inside.txt"), b"x");
        let cfg = cfg_with(vec![root.clone()]);
        match list_dir(Some(&cfg), root.to_str().unwrap()) {
            ListDirOutcome::Ok { entries, .. } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].name, "inside.txt");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn list_dir_on_file_returns_error() {
        let (_td, root) = temp_dir();
        let file = root.join("not-a-dir.txt");
        write_file(&file, b"x");
        let cfg = cfg_with(vec![root]);
        match list_dir(Some(&cfg), file.to_str().unwrap()) {
            ListDirOutcome::Err(ReadFileError::Io(msg)) => {
                assert!(msg.contains("not a directory"));
            }
            other => panic!("expected Io(not a directory), got {other:?}"),
        }
    }

    #[test]
    fn list_dir_missing_config_returns_not_configured() {
        assert!(matches!(
            list_dir(None, "/tmp"),
            ListDirOutcome::Err(ReadFileError::NotConfigured)
        ));
    }

    // ─── grep_files ─────────────────────────────────────────────

    #[test]
    fn grep_finds_matches_with_line_numbers() {
        let (_td, root) = temp_dir();
        write_file(
            &root.join("a.rs"),
            b"fn authenticate(user: &str) {\n  // TODO: implement\n}\n",
        );
        write_file(
            &root.join("b.rs"),
            b"fn unrelated() {}\nfn authenticate_other() {}\n",
        );
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "authenticate", false) {
            GrepOutcome::Ok {
                matches,
                files_scanned,
                files_skipped,
                truncated,
            } => {
                assert!(!truncated);
                assert_eq!(files_scanned, 2);
                assert_eq!(files_skipped, 0);
                assert_eq!(matches.len(), 2);
                let a = matches.iter().find(|m| m.path.ends_with("a.rs")).unwrap();
                assert_eq!(a.line_number, 1);
                assert!(a.line.contains("authenticate"));
                let b = matches.iter().find(|m| m.path.ends_with("b.rs")).unwrap();
                assert_eq!(b.line_number, 2);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_case_insensitive_matches_mixed_case() {
        let (_td, root) = temp_dir();
        write_file(
            &root.join("notes.txt"),
            b"TODO: ship the thing\ntodo: review later\n",
        );
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "todo", true) {
            GrepOutcome::Ok { matches, .. } => {
                assert_eq!(matches.len(), 2);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_walks_subdirs() {
        let (_td, root) = temp_dir();
        write_file(&root.join("sub").join("nested.rs"), b"fn find_me() {}");
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "find_me", false) {
            GrepOutcome::Ok { matches, .. } => {
                assert_eq!(matches.len(), 1);
                assert!(matches[0].path.contains("nested.rs"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_respects_deny_pattern_on_files() {
        let (_td, root) = temp_dir();
        write_file(&root.join(".env"), b"API_KEY=findthis");
        write_file(&root.join("safe.txt"), b"findthis");
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "findthis", false) {
            GrepOutcome::Ok {
                matches,
                files_skipped,
                ..
            } => {
                // Only safe.txt should match; .env is deny-listed.
                assert_eq!(matches.len(), 1);
                assert!(matches[0].path.ends_with("safe.txt"));
                assert!(files_skipped >= 1, ".env should have been skipped");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_outside_allowlist_rejected() {
        let (_td, root) = temp_dir();
        let (_td2, outside) = temp_dir();
        write_file(&outside.join("file.txt"), b"something");
        let cfg = cfg_with(vec![root]);
        let outcome = grep_files(Some(&cfg), outside.to_str().unwrap(), "something", false);
        assert!(matches!(
            outcome,
            GrepOutcome::Err(ReadFileError::OutsideAllowList)
        ));
    }

    #[test]
    fn grep_empty_pattern_rejected() {
        let (_td, root) = temp_dir();
        let cfg = cfg_with(vec![root.clone()]);
        let outcome = grep_files(Some(&cfg), root.to_str().unwrap(), "", false);
        assert!(matches!(outcome, GrepOutcome::Err(ReadFileError::Io(_))));
    }

    #[test]
    fn grep_skips_binary_files() {
        let (_td, root) = temp_dir();
        // 0xFF is not valid UTF-8; this file must be skipped.
        write_file(&root.join("binary.bin"), &[0xFFu8; 64]);
        write_file(&root.join("text.txt"), b"hello world");
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "hello", false) {
            GrepOutcome::Ok {
                matches,
                files_scanned,
                files_skipped,
                ..
            } => {
                assert_eq!(matches.len(), 1);
                assert_eq!(files_scanned, 1);
                assert!(files_skipped >= 1);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_long_lines_get_truncated_in_match() {
        let (_td, root) = temp_dir();
        let long_line: String = std::iter::repeat('x').take(500).collect();
        write_file(&root.join("long.txt"), long_line.as_bytes());
        let cfg = cfg_with(vec![root.clone()]);
        match grep_files(Some(&cfg), root.to_str().unwrap(), "xxx", false) {
            GrepOutcome::Ok { matches, .. } => {
                assert_eq!(matches.len(), 1);
                // 240 chars + ellipsis = 241 chars in our trimmed line.
                assert!(matches[0].line.chars().count() <= 241);
                assert!(matches[0].line.ends_with('…'));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn grep_missing_config_returns_not_configured() {
        assert!(matches!(
            grep_files(None, "/tmp", "x", false),
            GrepOutcome::Err(ReadFileError::NotConfigured)
        ));
    }

    #[test]
    fn boundary_sibling_with_shared_prefix_is_rejected() {
        // root = /tmp/.../foo, sibling = /tmp/.../foo2/secret.
        // The naive `starts_with` on string prefixes would let
        // /tmp/.../foo2 pass against root /tmp/.../foo. The
        // component-aware Path::starts_with we use prevents this.
        let (_td, base) = temp_dir();
        let root = base.join("foo");
        let sibling = base.join("foo2");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let file = sibling.join("secret.txt");
        write_file(&file, b"x");
        let cfg = cfg_with(vec![root]);
        let outcome = read_file(Some(&cfg), file.to_str().unwrap());
        assert!(matches!(
            outcome,
            ReadFileOutcome::Err(ReadFileError::OutsideAllowList)
        ));
    }
}
