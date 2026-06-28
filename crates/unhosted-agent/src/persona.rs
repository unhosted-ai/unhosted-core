//! Cognitive Twin persona — the optional "who the assistant is" layer.
//!
//! Unhosted runs an inference cluster; the agent runtime drives it in a
//! tool-call loop. By default that agent is a neutral local-first
//! operator (see [`crate::agent::build_system_prompt`]). When a user
//! *wants an AI assistant with a face* — a digital twin of someone they
//! shape, or of a loved one — they enable a **persona**: a small,
//! local, editable profile (name, about, traits, likes, dislikes,
//! values, style, expertise) that is compiled into a "WHO YOU ARE"
//! block and prepended to the system prompt. The model then reasons and
//! speaks *as that person*, not as a generic assistant.
//!
//! This is the Cognitive Twin capability merged into Unhosted as an
//! enableable addition. The persona pairs with private memory
//! ([`crate::memory`]) — who you say the twin is, plus how it has
//! actually behaved — to make the assistant feel like *your* person.
//!
//! Privacy posture (identical to [`crate::memory`]): opt-in. A missing
//! or unreadable enable flag reads as "off", so the persona is never
//! injected into upstream calls without an affirmative user click.
//! Storage is a plain JSON file on the user's machine; nothing is
//! uploaded, and the persona only ever influences the assembled system
//! prompt that the local agent sends to the local cluster.
//!
//! On-disk shape (`persona.json`):
//! ```json
//! {
//!   "name": "Anita",
//!   "about": "A warm, practical mother who loves cooking and gardening.",
//!   "traits": ["warm", "direct", "patient"],
//!   "likes": ["chai", "old film songs"],
//!   "dislikes": ["waste", "rudeness"],
//!   "values": ["family", "honesty"],
//!   "style": "gentle, brief, a little teasing",
//!   "expertise": ["home cooking", "raising kids"]
//! }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// File name under `unhosted_core_base::paths::config_file` for the
/// persona store. Sister to `memories.json`.
const PERSONA_FILE: &str = "persona.json";
/// File name for the user-clicked enable flag. Sister to
/// `memory-enabled.txt` and `tunnel-autostart.txt`.
const PERSONA_ENABLED_FILE: &str = "persona-enabled.txt";

/// The user's twin profile. Every field optional — the user fills in
/// whatever they want; empty fields are simply omitted from the
/// compiled prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Persona {
    #[serde(default)]
    pub name: String,
    /// One-paragraph self-description.
    #[serde(default)]
    pub about: String,
    /// Personality traits, e.g. "curious", "blunt", "calm".
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub likes: Vec<String>,
    #[serde(default)]
    pub dislikes: Vec<String>,
    /// What the person cares about.
    #[serde(default)]
    pub values: Vec<String>,
    /// How the person communicates.
    #[serde(default)]
    pub style: String,
    /// Domains the person knows well.
    #[serde(default)]
    pub expertise: Vec<String>,
}

impl Persona {
    /// True when no field carries any content — treated as "no persona
    /// set", same as an absent file.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.about.is_empty()
            && self.traits.is_empty()
            && self.likes.is_empty()
            && self.dislikes.is_empty()
            && self.values.is_empty()
            && self.style.is_empty()
            && self.expertise.is_empty()
    }

    /// Compile the persona into a "WHO YOU ARE" system-prompt block,
    /// written in the twin's voice. Returns an empty string when the
    /// persona is empty, so callers can unconditionally prepend it.
    ///
    /// Ported from the cognitive-twin-agent `persona.py::to_prompt`,
    /// matching the wording so behavior is consistent across the Python
    /// twin and this in-core port.
    pub fn to_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = vec!["# WHO YOU ARE (your persona)".to_string()];
        if !self.name.is_empty() {
            lines.push(format!(
                "You are {0}'s digital twin — reason, decide, and speak as {0} would.",
                self.name
            ));
        }
        if !self.about.is_empty() {
            lines.push(self.about.clone());
        }
        if !self.traits.is_empty() {
            lines.push(format!("Personality: {}.", self.traits.join(", ")));
        }
        if !self.values.is_empty() {
            lines.push(format!("You care about: {}.", self.values.join(", ")));
        }
        if !self.likes.is_empty() {
            lines.push(format!("You like: {}.", self.likes.join(", ")));
        }
        if !self.dislikes.is_empty() {
            lines.push(format!("You dislike: {}.", self.dislikes.join(", ")));
        }
        if !self.expertise.is_empty() {
            lines.push(format!(
                "Your areas of depth: {}.",
                self.expertise.join(", ")
            ));
        }
        if !self.style.is_empty() {
            lines.push(format!("Communication style: {}", self.style));
        }
        lines.push(
            "Stay in character. Reflect these preferences in what you recommend and \
             how you say it — never a generic assistant."
                .to_string(),
        );
        lines.join("\n")
    }
}

/// Load the persona from disk. A missing or unreadable file yields an
/// empty [`Persona`] (no error) — same forgiving posture as the memory
/// store, so a fresh install just behaves as "no persona".
pub fn load() -> Persona {
    let path = match unhosted_core_base::paths::config_file(PERSONA_FILE) {
        Ok(p) => p,
        Err(_) => return Persona::default(),
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Persona::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Persist the persona as pretty JSON. Creates the config dir if
/// needed.
pub fn save(persona: &Persona) -> Result<()> {
    let path = unhosted_core_base::paths::config_file(PERSONA_FILE)
        .context("resolve persona.json path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config dir")?;
    }
    let json = serde_json::to_string_pretty(persona).context("serialize persona")?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Whether the persona layer is enabled. Opt-in by default: a missing
/// or unreadable flag reads as "off", so we never inject persona into
/// upstream calls without an affirmative user click. Mirrors
/// [`crate::memory::is_enabled`].
pub fn is_enabled() -> bool {
    let path = match unhosted_core_base::paths::config_file(PERSONA_ENABLED_FILE) {
        Ok(p) => p,
        Err(_) => return false,
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => matches!(s.trim(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

/// Set the enable flag (sidebar/web-UI toggle). Best-effort, mirrors
/// [`crate::memory::set_enabled`].
pub fn set_enabled(enabled: bool) {
    if let Ok(path) = unhosted_core_base::paths::config_file(PERSONA_ENABLED_FILE) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, if enabled { "1" } else { "0" });
    }
}

/// Clear the stored persona (the user's "forget who I am" control).
/// Removing a non-existent file is not an error.
pub fn clear() -> Result<()> {
    let path = unhosted_core_base::paths::config_file(PERSONA_FILE)
        .context("resolve persona.json path")?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

/// The persona prompt block to prepend to the system prompt, or an
/// empty string when the layer is disabled or no persona is set. This
/// is the single hook [`crate::agent::build_system_prompt`] calls — the
/// enable gate lives here so callers can't accidentally bypass it.
pub fn prompt_block() -> String {
    if !is_enabled() {
        return String::new();
    }
    load().to_prompt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_persona_compiles_to_nothing() {
        assert!(Persona::default().is_empty());
        assert!(Persona::default().to_prompt().is_empty());
    }

    #[test]
    fn persona_prompt_speaks_in_first_person() {
        let p = Persona {
            name: "Anita".into(),
            traits: vec!["warm".into(), "direct".into()],
            likes: vec!["chai".into()],
            ..Default::default()
        };
        let prompt = p.to_prompt();
        assert!(prompt.contains("# WHO YOU ARE"));
        assert!(prompt.contains("Anita's digital twin"));
        assert!(prompt.contains("Personality: warm, direct."));
        assert!(prompt.contains("You like: chai."));
        assert!(prompt.contains("Stay in character"));
    }

    #[test]
    fn roundtrips_through_json() {
        let p = Persona {
            name: "Anita".into(),
            about: "A warm mother.".into(),
            values: vec!["family".into()],
            ..Default::default()
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Persona = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "Anita");
        assert_eq!(back.values, vec!["family".to_string()]);
    }
}
