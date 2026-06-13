//! Local model management — download, load, switch, delete GGUF models.
//!
//! Until now the daemon assumed the user runs their own runtime
//! (llama-server / Ollama / LM Studio) and just proxied to it. That's
//! fine for people who already live in a terminal; everyone else
//! expects the LM Studio flow: pick a model from a list, click
//! download, click load, start chatting. This module is that flow,
//! built on the `llama-server` binary the vram-pool feature already
//! knows how to find.
//!
//! Three responsibilities, one manager:
//!
//! - **Library**: scan `~/.cache/unhosted/models` for `*.gguf` files
//!   (the same directory `scripts/start-llama.sh` auto-picks from, so
//!   CLI users and UI users share a library).
//! - **Downloads**: stream a curated-catalog or user-supplied
//!   HuggingFace URL to disk with byte-level progress. One download
//!   at a time; partial files land at `<name>.gguf.part` and rename
//!   on completion so a crashed download never poses as a real model.
//! - **Runtime**: spawn `llama-server -m <model>` as a supervised
//!   child on the daemon's configured upstream port, wait for its
//!   `/health` to go green, and kill/replace it on switch. The
//!   existing per-request upstream probe (`upstream::select_live`)
//!   then routes chat turns to it with zero extra wiring.
//!
//! Safety:
//! - Model file names from the API are restricted to bare
//!   `*.gguf` names — no separators, no traversal — and only ever
//!   joined under the models dir.
//! - Download URLs are restricted to HTTPS on huggingface.co (the
//!   catalog only points there; custom URLs get the same check) so
//!   the daemon can't be turned into a generic fetch-to-disk proxy.
//! - The child is `kill_on_drop`, so daemon shutdown takes the
//!   runtime down with it — same policy as the tunnel and vram-pool
//!   children.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// How long we give a freshly spawned llama-server to answer
/// `/health`. Large models on spinning disks are slow to mmap; 180s
/// is generous without leaving a hung child in `Starting` forever.
const HEALTH_WAIT_BUDGET: Duration = Duration::from_secs(180);
/// Poll interval against the child's `/health` while starting.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A model the user can one-click download. Sizes are the exact
/// Content-Length the hub reported when the entry was added —
/// they drive the progress bar denominator before the first byte
/// arrives and the "do I have disk for this" display.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    /// Stable id the API accepts (`POST /v1/models/download {"id": …}`).
    pub id: &'static str,
    /// Display name for the UI.
    pub name: &'static str,
    /// One-line "why pick this one".
    pub blurb: &'static str,
    /// File name it lands under in the models dir.
    pub file: &'static str,
    /// Direct download URL (HTTPS, huggingface.co).
    pub url: &'static str,
    /// Exact size in bytes.
    pub size_bytes: u64,
}

/// Curated starter set. All Q4_K_M GGUFs from bartowski's repos —
/// the same quant family the docs recommend. Ordered smallest-first
/// so the top of the list is the fastest path to a working chat.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "llama-3.2-1b-instruct",
        name: "Llama 3.2 1B Instruct",
        blurb: "the starter model — fast on anything, fine for chat and quick questions",
        file: "Llama-3.2-1B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Llama-3.2-1B-Instruct-GGUF/resolve/main/Llama-3.2-1B-Instruct-Q4_K_M.gguf",
        size_bytes: 807_694_464,
    },
    CatalogEntry {
        id: "gemma-2-2b-it",
        name: "Gemma 2 2B IT",
        blurb: "google's small model — strong quality for its size",
        file: "gemma-2-2b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/gemma-2-2b-it-GGUF/resolve/main/gemma-2-2b-it-Q4_K_M.gguf",
        size_bytes: 1_708_582_752,
    },
    CatalogEntry {
        id: "llama-3.2-3b-instruct",
        name: "Llama 3.2 3B Instruct",
        blurb: "the daily driver on 8GB+ machines — noticeably smarter than 1B",
        file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        size_bytes: 2_019_377_696,
    },
    CatalogEntry {
        id: "phi-3.5-mini-instruct",
        name: "Phi 3.5 Mini Instruct",
        blurb: "microsoft's 3.8B — punches above its weight on reasoning",
        file: "Phi-3.5-mini-instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Phi-3.5-mini-instruct-GGUF/resolve/main/Phi-3.5-mini-instruct-Q4_K_M.gguf",
        size_bytes: 2_393_232_672,
    },
    CatalogEntry {
        id: "mistral-7b-instruct-v0.3",
        name: "Mistral 7B Instruct v0.3",
        blurb: "the classic 7B — needs ~6GB free memory",
        file: "Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Mistral-7B-Instruct-v0.3-GGUF/resolve/main/Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        size_bytes: 4_372_812_000,
    },
    CatalogEntry {
        id: "qwen2.5-7b-instruct",
        name: "Qwen 2.5 7B Instruct",
        blurb: "best general 7B in the list — multilingual, good at structure",
        file: "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_683_074_240,
    },
    CatalogEntry {
        id: "qwen2.5-coder-7b-instruct",
        name: "Qwen 2.5 Coder 7B Instruct",
        blurb: "the coding pick — pairs well with the VS Code extension",
        file: "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_683_074_336,
    },
];

/// A `*.gguf` present in the models dir.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledModel {
    pub file: String,
    pub size_bytes: u64,
    /// Unix seconds of the file's mtime; lets the UI sort by recency.
    pub modified_unix: u64,
}

/// State of the supervised llama-server child.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RuntimeState {
    /// No child; chat routes to whatever external upstream answers.
    Idle,
    /// Child spawned, waiting for `/health`.
    Starting { file: String, port: u16 },
    /// Child healthy and serving.
    Running { file: String, port: u16 },
    /// Child died or never came healthy. Sticky until the next
    /// load/unload so the UI can show the reason.
    Failed { error: String },
}

/// State of the (single) download slot.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DownloadState {
    Idle,
    Downloading {
        file: String,
        bytes_done: u64,
        /// 0 when the server didn't send Content-Length.
        bytes_total: u64,
    },
    /// Sticky until the next download starts.
    Failed {
        file: String,
        error: String,
    },
    /// Sticky until the next download starts.
    Completed {
        file: String,
    },
}

struct Inner {
    runtime: RuntimeState,
    child: Option<Child>,
    download: DownloadState,
    download_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct ModelManager {
    inner: Arc<Mutex<Inner>>,
    /// Dedicated client: no global timeout (multi-GB downloads),
    /// but a bounded connect timeout so a dead network fails fast.
    http: reqwest::Client,
}

/// Everything the UI needs in one poll.
#[derive(Serialize)]
pub struct Snapshot {
    pub models_dir: String,
    pub llama_server_found: bool,
    pub llama_server_path: Option<String>,
    pub runtime: RuntimeState,
    pub download: DownloadState,
    pub installed: Vec<InstalledModel>,
    pub catalog: Vec<CatalogSnapshotEntry>,
}

/// Catalog entry + "already on disk" flag resolved per snapshot.
#[derive(Serialize)]
pub struct CatalogSnapshotEntry {
    #[serde(flatten)]
    pub entry: CatalogEntry,
    pub installed: bool,
}

/// `~/.cache/unhosted/models` — shared with `scripts/start-llama.sh`.
pub fn models_dir() -> Result<PathBuf> {
    Ok(crate::paths::home_dir()?
        .join(".cache")
        .join("unhosted")
        .join("models"))
}

/// Accept only a bare `*.gguf` file name: no separators, no
/// traversal, no hidden files, sane length. Everything from the API
/// goes through here before touching the filesystem.
pub fn safe_model_filename(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 255 {
        bail!("model file name must be 1–255 characters");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("model file name must be a bare file name, not a path");
    }
    if name.starts_with('.') {
        bail!("model file name must not start with '.'");
    }
    if !name.to_ascii_lowercase().ends_with(".gguf") {
        bail!("model file name must end with .gguf");
    }
    Ok(name.to_string())
}

/// Downloads are restricted to HTTPS on huggingface.co so this
/// endpoint can't be used as a generic fetch-anything-to-disk proxy.
pub fn validate_download_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("invalid URL")?;
    if parsed.scheme() != "https" {
        bail!("download URL must be https");
    }
    let host = parsed.host_str().unwrap_or_default();
    if host != "huggingface.co" && !host.ends_with(".huggingface.co") {
        bail!("downloads are limited to huggingface.co URLs");
    }
    if !parsed.path().to_ascii_lowercase().ends_with(".gguf") {
        bail!("download URL must point at a .gguf file");
    }
    Ok(())
}

/// Port the managed llama-server should bind: the port of the
/// daemon's configured upstream when it's a loopback URL (so the
/// existing proxy path picks the child up unchanged), else the
/// stock 8080.
pub fn runtime_port(upstream_url: &str) -> u16 {
    if let Ok(u) = reqwest::Url::parse(upstream_url) {
        let host_is_local = matches!(u.host_str(), Some("127.0.0.1") | Some("localhost"));
        if host_is_local {
            if let Some(p) = u.port_or_known_default() {
                return p;
            }
        }
    }
    8080
}

/// List `*.gguf` files in the models dir, newest first. A missing
/// dir is an empty library, not an error.
pub fn scan_models(dir: &std::path::Path) -> Vec<InstalledModel> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<InstalledModel> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".gguf") {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified_unix = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some(InstalledModel {
                file: name,
                size_bytes: meta.len(),
                modified_unix,
            })
        })
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.modified_unix));
    out
}

impl ModelManager {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client builds");
        Self {
            inner: Arc::new(Mutex::new(Inner {
                runtime: RuntimeState::Idle,
                child: None,
                download: DownloadState::Idle,
                download_task: None,
            })),
            http,
        }
    }

    /// One-poll status for the UI. Also reaps a dead child: if the
    /// supervised llama-server exited behind our back (OOM-killed,
    /// crashed), the snapshot transitions Running → Failed instead
    /// of advertising a healthy runtime that isn't there.
    pub async fn snapshot(&self) -> Snapshot {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                inner.child = None;
                inner.runtime = RuntimeState::Failed {
                    error: format!("llama-server exited ({status})"),
                };
            }
        }
        let dir = models_dir().unwrap_or_else(|_| PathBuf::from("."));
        let installed = scan_models(&dir);
        let llama_bin = crate::vram_pool::find_llama_server();
        let catalog = CATALOG
            .iter()
            .map(|e| CatalogSnapshotEntry {
                entry: e.clone(),
                installed: installed.iter().any(|m| m.file == e.file),
            })
            .collect();
        Snapshot {
            models_dir: dir.to_string_lossy().to_string(),
            llama_server_found: llama_bin.is_some(),
            llama_server_path: llama_bin.map(|p| p.to_string_lossy().to_string()),
            runtime: inner.runtime.clone(),
            download: inner.download.clone(),
            installed,
            catalog,
        }
    }

    /// Begin downloading `url` into the models dir as `file`.
    /// Returns immediately; progress is read via [`Self::snapshot`].
    pub async fn start_download(&self, url: String, file: String) -> Result<()> {
        validate_download_url(&url)?;
        let file = safe_model_filename(&file)?;
        let dir = models_dir()?;
        std::fs::create_dir_all(&dir).context("creating models dir")?;
        let dest = dir.join(&file);
        if dest.exists() {
            bail!("{file} is already in the library");
        }

        let mut inner = self.inner.lock().await;
        if matches!(inner.download, DownloadState::Downloading { .. }) {
            bail!("another download is already running — one at a time");
        }
        inner.download = DownloadState::Downloading {
            file: file.clone(),
            bytes_done: 0,
            bytes_total: 0,
        };

        let shared = Arc::clone(&self.inner);
        let http = self.http.clone();
        let part = dir.join(format!("{file}.part"));
        let task = tokio::spawn(async move {
            let result = download_to(&http, &url, &part, &dest, &file, &shared).await;
            let mut inner = shared.lock().await;
            match result {
                Ok(()) => {
                    tracing::info!(%file, "model download finished");
                    inner.download = DownloadState::Completed { file };
                }
                Err(e) => {
                    tracing::warn!(%file, error = %e, "model download failed");
                    let _ = std::fs::remove_file(&part);
                    inner.download = DownloadState::Failed {
                        file,
                        error: e.to_string(),
                    };
                }
            }
        });
        inner.download_task = Some(task);
        Ok(())
    }

    /// Abort the in-flight download and clean up its partial file.
    pub async fn cancel_download(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let DownloadState::Downloading { file, .. } = inner.download.clone() else {
            bail!("no download in progress");
        };
        if let Some(task) = inner.download_task.take() {
            task.abort();
        }
        if let Ok(dir) = models_dir() {
            let _ = std::fs::remove_file(dir.join(format!("{file}.part")));
        }
        inner.download = DownloadState::Idle;
        Ok(())
    }

    /// Spawn (or replace) the supervised llama-server with `file`.
    /// Returns once the child is spawned — health convergence is
    /// observed via [`Self::snapshot`].
    pub async fn load(&self, file: &str, port: u16) -> Result<RuntimeState> {
        let file = safe_model_filename(file)?;
        let dir = models_dir()?;
        let model_path = dir.join(&file);
        if !model_path.is_file() {
            bail!("{file} is not in the library");
        }
        let Some(bin) = crate::vram_pool::find_llama_server() else {
            bail!(
                "llama-server not found — install llama.cpp \
                 (macOS: `brew install llama.cpp`)"
            );
        };

        let mut inner = self.inner.lock().await;

        // Switching: take down our own child first.
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill().await;
            // Give the kernel a beat to release the port before the
            // replacement tries to bind it.
            tokio::time::sleep(Duration::from_millis(300)).await;
        } else if port_in_use(port).await {
            // Something we don't own already listens there (user's own
            // llama-server, Ollama on a custom port…). Refusing beats
            // racing it for the bind and losing confusingly.
            bail!(
                "port {port} is already in use by another server — \
                 stop it first, or point UNHOSTED_LLAMA_SERVER_URL elsewhere"
            );
        }

        tracing::info!(bin = %bin.display(), model = %file, port, "model runtime: spawning llama-server");
        let child = Command::new(&bin)
            .arg("-m")
            .arg(&model_path)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("spawning llama-server")?;

        inner.child = Some(child);
        inner.runtime = RuntimeState::Starting {
            file: file.clone(),
            port,
        };
        drop(inner);

        // Supervisor: poll /health until green, the child dies, or
        // the budget runs out. Transitions the shared state.
        let shared = Arc::clone(&self.inner);
        let http = self.http.clone();
        let returned_state = RuntimeState::Starting {
            file: file.clone(),
            port,
        };
        tokio::spawn(async move {
            let health = format!("http://127.0.0.1:{port}/health");
            let deadline = tokio::time::Instant::now() + HEALTH_WAIT_BUDGET;
            loop {
                tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
                let mut inner = shared.lock().await;
                // A newer load()/unload() replaced us — stand down.
                let still_ours = matches!(
                    &inner.runtime,
                    RuntimeState::Starting { file: f, port: p } if *f == file && *p == port
                );
                if !still_ours {
                    return;
                }
                if let Some(child) = inner.child.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        inner.child = None;
                        inner.runtime = RuntimeState::Failed {
                            error: format!(
                                "llama-server exited during startup ({status}) — \
                                 likely not enough memory for {file}"
                            ),
                        };
                        return;
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    if let Some(mut child) = inner.child.take() {
                        let _ = child.kill().await;
                    }
                    inner.runtime = RuntimeState::Failed {
                        error: format!(
                            "llama-server didn't become healthy within {HEALTH_WAIT_BUDGET:?}"
                        ),
                    };
                    return;
                }
                drop(inner);
                let healthy = match http
                    .get(&health)
                    .timeout(Duration::from_millis(900))
                    .send()
                    .await
                {
                    Ok(r) => r.status().is_success(),
                    Err(_) => false,
                };
                if healthy {
                    let mut inner = shared.lock().await;
                    let still_ours = matches!(
                        &inner.runtime,
                        RuntimeState::Starting { file: f, port: p } if *f == file && *p == port
                    );
                    if still_ours {
                        tracing::info!(model = %file, port, "model runtime: healthy");
                        inner.runtime = RuntimeState::Running { file, port };
                    }
                    return;
                }
            }
        });

        Ok(returned_state)
    }

    /// Kill the supervised child (if any) and go back to Idle.
    pub async fn unload(&self) -> Result<RuntimeState> {
        let mut inner = self.inner.lock().await;
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill().await;
        }
        inner.runtime = RuntimeState::Idle;
        Ok(inner.runtime.clone())
    }

    /// Delete a library file. Refuses while that file backs the
    /// running (or starting) child.
    pub async fn delete(&self, file: &str) -> Result<()> {
        let file = safe_model_filename(file)?;
        let inner = self.inner.lock().await;
        let active = match &inner.runtime {
            RuntimeState::Starting { file: f, .. } | RuntimeState::Running { file: f, .. } => {
                Some(f.as_str())
            }
            _ => None,
        };
        if active == Some(file.as_str()) {
            bail!("{file} is loaded — unload it before deleting");
        }
        drop(inner);
        let path = models_dir()?.join(&file);
        if !path.is_file() {
            bail!("{file} is not in the library");
        }
        std::fs::remove_file(&path).with_context(|| format!("deleting {file}"))?;
        Ok(())
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// True when something already accepts TCP connections on the port.
async fn port_in_use(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(400),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Stream `url` → `part`, rename to `dest` on success, updating the
/// shared download progress as chunks land.
async fn download_to(
    http: &reqwest::Client,
    url: &str,
    part: &std::path::Path,
    dest: &std::path::Path,
    file: &str,
    shared: &Arc<Mutex<Inner>>,
) -> Result<()> {
    let resp = http.get(url).send().await.context("starting download")?;
    if !resp.status().is_success() {
        bail!("hub answered {}", resp.status());
    }
    let total = resp.content_length().unwrap_or(0);
    {
        let mut inner = shared.lock().await;
        inner.download = DownloadState::Downloading {
            file: file.to_string(),
            bytes_done: 0,
            bytes_total: total,
        };
    }

    let mut out = tokio::fs::File::create(part)
        .await
        .context("creating partial file")?;
    let mut stream = resp.bytes_stream();
    let mut done: u64 = 0;
    let mut last_reported: u64 = 0;
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading download stream")?;
        out.write_all(&chunk).await.context("writing model file")?;
        done += chunk.len() as u64;
        // Update shared progress at most every 2 MB so the mutex
        // isn't hammered on fast links.
        if done - last_reported >= 2 * 1024 * 1024 {
            last_reported = done;
            let mut inner = shared.lock().await;
            inner.download = DownloadState::Downloading {
                file: file.to_string(),
                bytes_done: done,
                bytes_total: total,
            };
        }
    }
    out.flush().await.ok();
    drop(out);
    if total > 0 && done != total {
        bail!("download truncated: got {done} of {total} bytes");
    }
    tokio::fs::rename(part, dest)
        .await
        .context("moving finished download into the library")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_accepts_normal_gguf() {
        assert_eq!(
            safe_model_filename("Llama-3.2-1B-Instruct-Q4_K_M.gguf").unwrap(),
            "Llama-3.2-1B-Instruct-Q4_K_M.gguf"
        );
    }

    #[test]
    fn safe_filename_rejects_traversal_and_paths() {
        assert!(safe_model_filename("../etc/passwd.gguf").is_err());
        assert!(safe_model_filename("/abs/path.gguf").is_err());
        assert!(safe_model_filename("a\\b.gguf").is_err());
        assert!(safe_model_filename("..gguf").is_err());
    }

    #[test]
    fn safe_filename_rejects_non_gguf_and_hidden() {
        assert!(safe_model_filename("model.bin").is_err());
        assert!(safe_model_filename(".hidden.gguf").is_err());
        assert!(safe_model_filename("").is_err());
    }

    #[test]
    fn download_url_must_be_https_huggingface_gguf() {
        assert!(validate_download_url(
            "https://huggingface.co/bartowski/x-GGUF/resolve/main/x-Q4_K_M.gguf"
        )
        .is_ok());
        assert!(validate_download_url("http://huggingface.co/x.gguf").is_err());
        assert!(validate_download_url("https://example.com/x.gguf").is_err());
        assert!(validate_download_url("https://huggingface.co/x.bin").is_err());
    }

    #[test]
    fn catalog_entries_are_consistent() {
        let mut ids = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(ids.insert(e.id), "duplicate catalog id {}", e.id);
            assert!(validate_download_url(e.url).is_ok(), "bad url for {}", e.id);
            assert!(safe_model_filename(e.file).is_ok(), "bad file for {}", e.id);
            assert!(e.size_bytes > 100_000_000, "implausible size for {}", e.id);
        }
    }

    #[test]
    fn runtime_port_follows_loopback_upstream() {
        assert_eq!(runtime_port("http://127.0.0.1:8080"), 8080);
        assert_eq!(runtime_port("http://127.0.0.1:9090"), 9090);
        assert_eq!(runtime_port("http://localhost:8081"), 8081);
        // Non-loopback upstream → stock port, never a remote bind.
        assert_eq!(runtime_port("http://192.168.1.20:8080"), 8080);
        assert_eq!(runtime_port("not a url"), 8080);
    }

    #[test]
    fn scan_models_lists_only_gguf_files() {
        let dir = std::env::temp_dir().join(format!("unhosted-mm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.gguf"), b"x").unwrap();
        std::fs::write(dir.join("b.GGUF"), b"xy").unwrap();
        std::fs::write(dir.join("c.txt"), b"nope").unwrap();
        std::fs::write(dir.join("d.gguf.part"), b"partial").unwrap();
        let found = scan_models(&dir);
        let names: Vec<_> = found.iter().map(|m| m.file.as_str()).collect();
        assert!(names.contains(&"a.gguf"));
        assert!(names.contains(&"b.GGUF"));
        assert!(!names.iter().any(|n| n.ends_with(".txt")));
        assert!(!names.iter().any(|n| n.ends_with(".part")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_models_missing_dir_is_empty() {
        let dir = std::path::Path::new("/nonexistent/unhosted-test-dir");
        assert!(scan_models(dir).is_empty());
    }
}
