//! Cross-platform config paths.
//!
//! Until v0.0.5, three modules (peer / identity / auth) each rolled
//! their own `~/.config/unhosted/<file>` resolution by reading the
//! `HOME` env var directly. That works on macOS + Linux + BSD because
//! a shell always sets `HOME`. It does **not** work on Windows: there
//! is no `HOME`, only `USERPROFILE`. The result was `unhosted serve`
//! aborting at startup on every Windows machine with
//! `Error: HOME env var not set` — surfaced by the v0.0.5 CI smoke
//! test on `windows-latest`.
//!
//! This module owns the resolution in one place.
//!
//! Precedence:
//!   1. `XDG_CONFIG_HOME`    — explicit override, any platform
//!   2. `HOME`               — macOS / Linux / BSD / WSL
//!   3. `USERPROFILE`        — Windows
//!   4. `HOMEDRIVE+HOMEPATH` — older Windows fallback

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `~/.config/unhosted/<file>` on Unix, `%USERPROFILE%\.config\unhosted\<file>`
/// on Windows. Honors `XDG_CONFIG_HOME` everywhere. The returned path
/// is just a `PathBuf` — callers handle the file-existence check and
/// any IO.
pub fn config_file(file: &str) -> Result<PathBuf> {
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        home_dir()?.join(".config")
    };
    Ok(base.join("unhosted").join(file))
}

/// Best-effort home-directory lookup. Tries the common env vars in
/// order. Returns a descriptive error if every option is empty —
/// previously we said "HOME env var not set", which was both
/// confusing on Windows and not actionable.
pub fn home_dir() -> Result<PathBuf> {
    if let Ok(v) = std::env::var("HOME") {
        if !v.is_empty() {
            return Ok(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("USERPROFILE") {
        if !v.is_empty() {
            return Ok(PathBuf::from(v));
        }
    }
    // Older Windows: HOMEDRIVE + HOMEPATH (e.g. `C:` + `\Users\Foo`)
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        if !drive.is_empty() && !path.is_empty() {
            return Ok(PathBuf::from(format!("{drive}{path}")));
        }
    }
    Err(anyhow::anyhow!(
        "could not determine the user home directory — \
         tried HOME, USERPROFILE, HOMEDRIVE+HOMEPATH"
    ))
    .context("resolving config directory")
}
