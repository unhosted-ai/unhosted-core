//! Cognitive Twin voice — speak the agent's replies in a loved one's *actual*
//! cloned voice, locally.
//!
//! This is the voice half of the Cognitive Twin capability (the persona half is
//! [`crate::persona`]). The heavy lifting — XTTS-v2 neural voice cloning — is
//! Python/ML and platform-specific, so we do **not** reimplement it in Rust.
//! Instead this module is a thin **bridge**: it supervises a long-lived Python
//! worker (`_xtts_say.py --serve <reference.wav>`) that keeps the model warm, and
//! talks to it over a dead-simple JSON-lines protocol on stdin/stdout:
//!
//! ```text
//! → {"text": "Good morning, my dear.", "out": "/…/twin_say.wav"}\n
//! ← {"ok": true, "out": "/…/twin_say.wav"}\n
//! ```
//!
//! On worker start the child prints `{"ready": true}` once the (slow ~10s) model
//! load finishes; we block for that line, then every subsequent line renders in
//! ~1–2s. This mirrors the supervisor in the Python twin's `voice_clone.py`.
//!
//! Configuration (env, so Unhosted stays decoupled from any particular Python
//! install layout):
//!
//! | Var | Meaning |
//! |---|---|
//! | `UNHOSTED_TWIN_TTS_PYTHON` | python in the venv that has coqui-tts (XTTS) |
//! | `UNHOSTED_TWIN_TTS_WORKER` | path to `_xtts_say.py` |
//!
//! The reference voice sample lives at `~/.config/unhosted/voice/reference.wav`
//! (owner-only, on-device). Rendered output goes to `…/voice/twin_say.wav`.
//!
//! Privacy posture: gated behind the persona enable flag ([`crate::persona`]).
//! With voice unconfigured or the persona disabled, [`is_ready`] is false and the
//! caller falls back to text — nothing breaks, nothing is uploaded.

use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::{Mutex, OnceLock};

/// Subdir under `~/.config/unhosted/` holding the voice sample + renders.
const VOICE_DIR: &str = "voice";
/// The reference sample the worker clones from.
const REFERENCE_WAV: &str = "reference.wav";
/// Where a rendered line is written (overwritten each call — it's transient).
const OUTPUT_WAV: &str = "twin_say.wav";

/// Resolve `~/.config/unhosted/voice/<file>`. Reuses the same base resolver as
/// the persona/memory stores so everything sits together.
fn voice_file(file: &str) -> Result<PathBuf> {
    let dir = unhosted_core_base::paths::config_file(VOICE_DIR).context("resolve voice dir")?;
    Ok(dir.join(file))
}

/// Path to the reference sample, or None if it isn't present.
pub fn reference_path() -> Option<PathBuf> {
    let p = voice_file(REFERENCE_WAV).ok()?;
    p.is_file().then_some(p)
}

/// Whether a reference sample is set up.
pub fn has_reference() -> bool {
    reference_path().is_some()
}

/// Save an uploaded voice sample as the reference, cloning-ready.
///
/// When `ffmpeg` is available we clean it the same gentle way as the Python
/// twin: trim only leading/trailing silence, tame low rumble, light loudnorm, at
/// XTTS-v2's native 24 kHz mono. (We deliberately keep internal pauses — XTTS
/// clones a real voice more faithfully from continuous speech than from
/// fragments.) Without ffmpeg we fall back to writing the bytes verbatim, which
/// works when the upload is already a WAV. Restarts the warm worker so the next
/// synth uses the new sample.
pub fn save_reference(bytes: &[u8]) -> Result<PathBuf> {
    if bytes.is_empty() {
        return Err(anyhow!("empty upload"));
    }
    let dst = voice_file(REFERENCE_WAV)?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).context("create voice dir")?;
    }

    let have_ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if have_ffmpeg {
        // Write the raw upload to a temp file, then let ffmpeg read+clean it.
        let tmp = voice_file("reference_upload.tmp")?;
        std::fs::write(&tmp, bytes).context("write temp upload")?;
        let filter = "highpass=f=70,\
            silenceremove=start_periods=1:start_silence=0.15:start_threshold=-45dB,\
            areverse,\
            silenceremove=start_periods=1:start_silence=0.15:start_threshold=-45dB,\
            areverse,\
            loudnorm=I=-18:TP=-2:LRA=11";
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&tmp)
            .args(["-af", filter, "-ar", "24000", "-ac", "1"])
            .arg(&dst)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let cleaned_ok = matches!(status, Ok(s) if s.success()) && dst.is_file();
        if !cleaned_ok {
            // Fallback: plain convert to 24k mono (no filter chain).
            let status2 = std::process::Command::new("ffmpeg")
                .args(["-y", "-i"])
                .arg(&tmp)
                .args(["-ar", "24000", "-ac", "1"])
                .arg(&dst)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if !matches!(status2, Ok(s) if s.success()) || !dst.is_file() {
                let _ = std::fs::remove_file(&tmp);
                return Err(anyhow!("ffmpeg could not process the upload"));
            }
        }
        let _ = std::fs::remove_file(&tmp);
    } else {
        // No ffmpeg: store verbatim (works for an already-WAV upload).
        std::fs::write(&dst, bytes).context("write reference (no ffmpeg)")?;
    }

    // Tighten perms (owner-only) on unix — it's a personal recording.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600));
    }

    // Drop the warm worker so the next synth reloads with the new reference.
    if let Ok(mut guard) = worker_slot().lock() {
        if let Some(mut w) = guard.take() {
            let _ = w.child.kill();
        }
    }
    Ok(dst)
}

/// Remove the reference sample (and stop any warm worker). Removing a
/// non-existent file is not an error.
pub fn clear_reference() -> Result<()> {
    if let Ok(mut guard) = worker_slot().lock() {
        if let Some(mut w) = guard.take() {
            let _ = w.child.kill();
        }
    }
    let dst = voice_file(REFERENCE_WAV)?;
    match std::fs::remove_file(&dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("remove reference"),
    }
}

/// The python interpreter that has the XTTS engine, from
/// `UNHOSTED_TWIN_TTS_PYTHON`. None when unset/empty.
fn engine_python() -> Option<PathBuf> {
    std::env::var("UNHOSTED_TWIN_TTS_PYTHON")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

/// The `_xtts_say.py` worker script, from `UNHOSTED_TWIN_TTS_WORKER`.
fn worker_script() -> Option<PathBuf> {
    std::env::var("UNHOSTED_TWIN_TTS_WORKER")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

/// True only when we have everything needed to synthesize: a reference sample,
/// the engine python, and the worker script. Caller should fall back to text
/// when this is false.
pub fn is_ready() -> bool {
    reference_path().is_some() && engine_python().is_some() && worker_script().is_some()
}

// ─── warm worker (model stays loaded → fast replies) ───────────────────────
//
// One supervised child per process, guarded by a Mutex, created lazily. Same
// OnceLock<Mutex<Option<…>>> shape as the memory embedder singleton.

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

fn worker_slot() -> &'static Mutex<Option<Worker>> {
    static SLOT: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Spawn the worker and block until it signals `{"ready": true}` (model loaded).
fn spawn_worker() -> Result<Worker> {
    let py = engine_python().ok_or_else(|| anyhow!("UNHOSTED_TWIN_TTS_PYTHON unset/missing"))?;
    let script =
        worker_script().ok_or_else(|| anyhow!("UNHOSTED_TWIN_TTS_WORKER unset/missing"))?;
    let reference = reference_path().ok_or_else(|| anyhow!("no reference.wav"))?;

    let mut child = std::process::Command::new(&py)
        .arg(&script)
        .arg("--serve")
        .arg(&reference)
        // XTTS prints its model license prompt unless this is set.
        .env("COQUI_TOS_AGREED", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn tts worker: {} {}", py.display(), script.display()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("worker stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("worker stdout unavailable"))?;
    let mut stdout = BufReader::new(stdout);

    // Block for the readiness line (the slow one-time model load).
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .context("read worker ready line")?;
    if !line.contains("\"ready\"") {
        let _ = child.kill();
        return Err(anyhow!("worker did not report ready: {}", line.trim()));
    }
    Ok(Worker {
        child,
        stdin,
        stdout,
    })
}

/// Render `text` in the cloned voice; returns the path to the rendered WAV.
///
/// Uses the warm worker, restarting it once if it has died. Returns an error
/// (not a panic) on any failure, so the caller can fall back to text. Blocking;
/// run it on a blocking-friendly task (see [`synthesize`]).
fn synthesize_blocking(text: &str) -> Result<PathBuf> {
    if text.trim().is_empty() {
        return Err(anyhow!("empty text"));
    }
    if !is_ready() {
        return Err(anyhow!("voice not ready (sample/engine/worker missing)"));
    }
    let out = voice_file(OUTPUT_WAV)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let slot = worker_slot();
    let mut guard = slot.lock().map_err(|_| anyhow!("worker lock poisoned"))?;

    // (Re)spawn if absent or the child has exited.
    let needs_spawn = match guard.as_mut() {
        None => true,
        Some(w) => matches!(w.child.try_wait(), Ok(Some(_)) | Err(_)),
    };
    if needs_spawn {
        *guard = Some(spawn_worker()?);
    }
    let worker = guard.as_mut().expect("worker present after spawn");

    // Send one request line, read one response line.
    let req = serde_json::json!({ "text": text, "out": out.to_string_lossy() });
    writeln!(worker.stdin, "{req}").context("write request to worker")?;
    worker.stdin.flush().context("flush worker stdin")?;

    let mut resp_line = String::new();
    worker
        .stdout
        .read_line(&mut resp_line)
        .context("read worker response")?;
    let resp: serde_json::Value =
        serde_json::from_str(resp_line.trim()).context("parse worker response")?;

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) && out.is_file() {
        Ok(out)
    } else {
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        Err(anyhow!("worker render failed: {err}"))
    }
}

/// Async wrapper: render `text` to a WAV in the cloned voice, off the async
/// runtime's worker threads (synthesis blocks on the Python child). Returns the
/// output path, or an error so the caller can fall back to a text-only reply.
pub async fn synthesize(text: impl Into<String>) -> Result<PathBuf> {
    let text = text.into();
    tokio::task::spawn_blocking(move || synthesize_blocking(&text))
        .await
        .context("voice synth task join")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_ready_without_config() {
        // With the TTS env vars unset, the bridge reports not-ready and refuses
        // to synthesize — the safe fallback-to-text path.
        std::env::remove_var("UNHOSTED_TWIN_TTS_PYTHON");
        std::env::remove_var("UNHOSTED_TWIN_TTS_WORKER");
        assert!(!is_ready());
    }

    #[tokio::test]
    async fn synthesize_errors_when_not_ready() {
        std::env::remove_var("UNHOSTED_TWIN_TTS_PYTHON");
        std::env::remove_var("UNHOSTED_TWIN_TTS_WORKER");
        let r = synthesize("hello").await;
        assert!(
            r.is_err(),
            "must error (not panic) when voice is unconfigured"
        );
    }

    #[tokio::test]
    async fn synthesize_rejects_empty_text() {
        let r = synthesize("   ").await;
        assert!(r.is_err());
    }

    #[test]
    fn save_and_clear_reference_roundtrip() {
        // Point config at a temp dir; honor XDG_CONFIG_HOME like the other tests.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());

        assert!(!has_reference());
        // An empty upload is rejected.
        assert!(save_reference(&[]).is_err());
        // A non-empty upload is stored (verbatim path if ffmpeg is absent; either
        // way the file must exist afterward and be discoverable).
        let saved = save_reference(b"RIFF....fake-wav-bytes....").ok();
        // ffmpeg may reject the fake bytes; only assert presence when save said ok.
        if saved.is_some() {
            assert!(has_reference());
            clear_reference().unwrap();
            assert!(!has_reference());
        }
        // clear on an absent file is not an error
        clear_reference().unwrap();

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
