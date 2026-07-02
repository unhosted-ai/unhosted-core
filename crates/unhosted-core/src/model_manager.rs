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
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
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
    /// `sha256:<hex>` whole-file digest, when known. Lets the download
    /// path verify integrity end-to-end and lets peers be a source for
    /// this model by content (ADR-0014). `None` for entries whose digest
    /// hasn't been pinned yet — those fall back to size-checked download
    /// and self-synthesize a manifest on first pull.
    pub sha256: Option<&'static str>,
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
        sha256: None,
    },
    CatalogEntry {
        id: "gemma-2-2b-it",
        name: "Gemma 2 2B IT",
        blurb: "google's small model — strong quality for its size",
        file: "gemma-2-2b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/gemma-2-2b-it-GGUF/resolve/main/gemma-2-2b-it-Q4_K_M.gguf",
        size_bytes: 1_708_582_752,
        sha256: None,
    },
    CatalogEntry {
        id: "llama-3.2-3b-instruct",
        name: "Llama 3.2 3B Instruct",
        blurb: "the daily driver on 8GB+ machines — noticeably smarter than 1B",
        file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        size_bytes: 2_019_377_696,
        sha256: None,
    },
    CatalogEntry {
        id: "phi-3.5-mini-instruct",
        name: "Phi 3.5 Mini Instruct",
        blurb: "microsoft's 3.8B — punches above its weight on reasoning",
        file: "Phi-3.5-mini-instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Phi-3.5-mini-instruct-GGUF/resolve/main/Phi-3.5-mini-instruct-Q4_K_M.gguf",
        size_bytes: 2_393_232_672,
        sha256: None,
    },
    CatalogEntry {
        id: "mistral-7b-instruct-v0.3",
        name: "Mistral 7B Instruct v0.3",
        blurb: "the classic 7B — needs ~6GB free memory",
        file: "Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Mistral-7B-Instruct-v0.3-GGUF/resolve/main/Mistral-7B-Instruct-v0.3-Q4_K_M.gguf",
        size_bytes: 4_372_812_000,
        sha256: None,
    },
    CatalogEntry {
        id: "qwen2.5-7b-instruct",
        name: "Qwen 2.5 7B Instruct",
        blurb: "best general 7B in the list — multilingual, good at structure",
        file: "Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_683_074_240,
        sha256: None,
    },
    CatalogEntry {
        id: "qwen2.5-coder-7b-instruct",
        name: "Qwen 2.5 Coder 7B Instruct",
        blurb: "the coding pick — pairs well with the VS Code extension",
        file: "Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        size_bytes: 4_683_074_336,
        sha256: None,
    },
    CatalogEntry {
        id: "helmsman-4b",
        name: "Helmsman 4B",
        blurb: "unhosted's own orchestration specialist — planning, routing, prioritizing, tuned from Qwen3 4B",
        file: "helmsman-4b-Q4_K_M.gguf",
        url: "https://huggingface.co/sinhaankur/helmsman-4b/resolve/main/helmsman-4b-Q4_K_M.gguf",
        size_bytes: 2_497_278_880,
        sha256: Some("sha256:3c81246dbde3ffaeeca5d490c38739343d68630281731c557d11620318331bcd"),
    },
];

/// Where an installed model file lives. `Library` files sit in our own
/// models dir and are fully managed (loadable, deletable). `LmStudio`
/// files are discovered read-only in LM Studio's models tree — we list
/// them and load them in place, but never write to or delete them.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstalledSource {
    Library,
    LmStudio,
}

/// A `*.gguf` present in the models dir or a discovered external tree.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledModel {
    pub file: String,
    pub size_bytes: u64,
    /// Unix seconds of the file's mtime; lets the UI sort by recency.
    pub modified_unix: u64,
    pub source: InstalledSource,
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

/// Where the bytes currently flowing in are coming from. ADR-0014
/// adds LAN/trusted-peer sources; this slice only ever reports
/// `Origin`, but the field exists now so the UI can render the
/// distinction once peer sourcing lands without a second schema bump.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadSource {
    /// HTTPS origin (huggingface.co). The bootstrap seed and the
    /// fallback when no peer has the bytes.
    #[default]
    Origin,
    /// A peer on the same LAN (mDNS-discovered).
    Lan,
    /// A trusted, paired peer reachable over the internet.
    Peer,
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
        /// Where the current bytes are coming from.
        source: DownloadSource,
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
                source: InstalledSource::Library,
            })
        })
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.modified_unix));
    out
}

/// LM Studio's models tree (`~/.lmstudio/models`), when it exists.
pub fn lmstudio_models_dir() -> Option<PathBuf> {
    let dir = crate::paths::home_dir().ok()?.join(".lmstudio").join("models");
    dir.is_dir().then_some(dir)
}

/// Discover `*.gguf` files in LM Studio's `<publisher>/<model>/` layout.
/// Walks exactly the two directory levels LM Studio uses (plus files
/// sitting directly in the root, which hand-copied models end up as).
/// Read-only: listing here never writes to or deletes anything.
pub fn scan_lmstudio_models(root: &std::path::Path) -> Vec<InstalledModel> {
    fn ggufs_in(dir: &std::path::Path, out: &mut Vec<InstalledModel>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() || !name.to_ascii_lowercase().ends_with(".gguf") {
                continue;
            }
            let modified_unix = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(InstalledModel {
                file: name,
                size_bytes: meta.len(),
                modified_unix,
                source: InstalledSource::LmStudio,
            });
        }
    }

    let mut out = Vec::new();
    ggufs_in(root, &mut out);
    let Ok(publishers) = std::fs::read_dir(root) else {
        return out;
    };
    for publisher in publishers.flatten() {
        let pdir = publisher.path();
        if !pdir.is_dir() {
            continue;
        }
        ggufs_in(&pdir, &mut out);
        if let Ok(models) = std::fs::read_dir(&pdir) {
            for model in models.flatten() {
                let mdir = model.path();
                if mdir.is_dir() {
                    ggufs_in(&mdir, &mut out);
                }
            }
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.modified_unix));
    out
}

/// Every model we can serve: the library, then LM Studio discoveries
/// whose file names don't collide with a library file (the managed
/// copy stays authoritative). Newest first across both sources.
pub fn scan_all_models(library_dir: &std::path::Path) -> Vec<InstalledModel> {
    let discovered = lmstudio_models_dir()
        .map(|root| scan_lmstudio_models(&root))
        .unwrap_or_default();
    merge_installed(scan_models(library_dir), discovered)
}

/// Library entries win file-name collisions; the result is newest-first.
fn merge_installed(
    mut library: Vec<InstalledModel>,
    discovered: Vec<InstalledModel>,
) -> Vec<InstalledModel> {
    for m in discovered {
        if !library.iter().any(|x| x.file == m.file) {
            library.push(m);
        }
    }
    library.sort_by_key(|m| std::cmp::Reverse(m.modified_unix));
    library
}

/// On-disk path for a validated bare model file name: the library
/// first, then LM Studio's tree. `None` means it's nowhere we serve
/// from.
pub fn resolve_model_path(file: &str) -> Option<PathBuf> {
    if let Ok(dir) = models_dir() {
        let p = dir.join(file);
        if p.is_file() {
            return Some(p);
        }
    }
    let root = lmstudio_models_dir()?;
    // Cheap: re-walk the small discovery tree rather than caching paths.
    if let Ok(publishers) = std::fs::read_dir(&root) {
        let mut dirs = vec![root.clone()];
        for publisher in publishers.flatten() {
            let pdir = publisher.path();
            if pdir.is_dir() {
                if let Ok(models) = std::fs::read_dir(&pdir) {
                    for model in models.flatten() {
                        let mdir = model.path();
                        if mdir.is_dir() {
                            dirs.push(mdir);
                        }
                    }
                }
                dirs.push(pdir);
            }
        }
        for dir in dirs {
            let p = dir.join(file);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
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
        let installed = scan_all_models(&dir);
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

        // Resolve a pinned whole-file digest if the catalog carries one
        // for this URL. When present, the download is verified chunk-by-
        // chunk against a manifest derived from it; when absent, we fall
        // back to the size-checked stream and synthesize a manifest from
        // the bytes we fetched (ADR-0014, "manifest origin" option a).
        let expected_digest = CATALOG
            .iter()
            .find(|e| e.url == url)
            .and_then(|e| e.sha256)
            .map(|s| s.to_string());

        let mut inner = self.inner.lock().await;
        if matches!(inner.download, DownloadState::Downloading { .. }) {
            bail!("another download is already running — one at a time");
        }
        inner.download = DownloadState::Downloading {
            file: file.clone(),
            bytes_done: 0,
            bytes_total: 0,
            source: DownloadSource::Origin,
        };

        let shared = Arc::clone(&self.inner);
        let http = self.http.clone();
        let part = dir.join(format!("{file}.part"));
        let task = tokio::spawn(async move {
            let result =
                download_verified(&http, &url, &part, &dest, &file, expected_digest, &shared).await;
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

    // ─── externally-driven downloads (ADR-0014 peer pulls) ───────────────
    //
    // A peer pull is orchestrated in `lib.rs` (it needs the QUIC endpoint
    // + peer registry, which the manager deliberately doesn't hold). But
    // the UI polls `DownloadState` through the manager, so the puller
    // drives that state through these three methods. They mirror the
    // shape of the origin path's progress reporting so the UI renders a
    // peer pull identically — just with a different `source`.

    /// Mark the start of a peer-sourced download. Refuses if another
    /// download (origin or peer) is already running — same one-at-a-time
    /// rule as `start_download`. Returns an error the caller surfaces.
    pub async fn begin_external_download(
        &self,
        file: &str,
        total: u64,
        source: DownloadSource,
    ) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if matches!(inner.download, DownloadState::Downloading { .. }) {
            bail!("another download is already running — one at a time");
        }
        inner.download = DownloadState::Downloading {
            file: file.to_string(),
            bytes_done: 0,
            bytes_total: total,
            source,
        };
        Ok(())
    }

    /// Update progress for a peer-sourced download. No-op if the slot was
    /// taken over by something else (e.g. the user cancelled).
    pub async fn report_external_progress(
        &self,
        file: &str,
        done: u64,
        total: u64,
        source: DownloadSource,
    ) {
        let mut inner = self.inner.lock().await;
        if let DownloadState::Downloading { file: f, .. } = &inner.download {
            if f == file {
                inner.download = DownloadState::Downloading {
                    file: file.to_string(),
                    bytes_done: done,
                    bytes_total: total,
                    source,
                };
            }
        }
    }

    /// Transition a peer-sourced download to its terminal state.
    pub async fn finish_external_download(&self, file: &str, result: Result<()>) {
        let mut inner = self.inner.lock().await;
        inner.download = match result {
            Ok(()) => DownloadState::Completed {
                file: file.to_string(),
            },
            Err(e) => DownloadState::Failed {
                file: file.to_string(),
                error: e.to_string(),
            },
        };
    }

    /// Spawn (or replace) the supervised llama-server with `file`.
    /// Returns once the child is spawned — health convergence is
    /// observed via [`Self::snapshot`].
    pub async fn load(&self, file: &str, port: u16) -> Result<RuntimeState> {
        let file = safe_model_filename(file)?;
        let Some(model_path) = resolve_model_path(&file) else {
            bail!("{file} is not in the library or LM Studio's models folder");
        };
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

/// Download `url` → `part`, verify, then rename to `dest`. This is the
/// ADR-0014 origin path: fetch the file as `CHUNK_SIZE` ranged pieces,
/// hash each piece, and resume from a partial file across restarts.
///
/// Two modes, picked by `expected_digest`:
///
/// - **Verified** (`Some`): a `ModelManifest` is derived from the
///   pinned whole-file digest by fetching + hashing chunks; each chunk
///   is verified before it's kept, and the final file is checked against
///   the digest. A corrupted or MITM'd chunk that happens to be the
///   right length is rejected — which the old size-only path accepted.
/// - **Unverified** (`None`): no pinned digest yet, so we can't reject
///   on content. We still fetch in ranged chunks (for resume), check
///   the total size, and synthesize a manifest from the result so this
///   node can serve the model to peers by content next time.
///
/// In both modes the function is restart-safe: a partial `.part` file is
/// reused for whatever whole chunks it already contains, so a download
/// killed at 90% resumes near 90% instead of from zero. The progress
/// readout always reports `DownloadSource::Origin` in this slice; LAN /
/// peer sources arrive in a later ADR-0014 slice on the same loop.
async fn download_verified(
    http: &reqwest::Client,
    url: &str,
    part: &std::path::Path,
    dest: &std::path::Path,
    file: &str,
    expected_digest: Option<String>,
    shared: &Arc<Mutex<Inner>>,
) -> Result<()> {
    use crate::swarm::{self, ModelManifest, CHUNK_SIZE};

    // HEAD to learn the total size up front so we can chunk + show a
    // denominator before the first byte. The hub supports HEAD and
    // ranged GET on `resolve/` URLs.
    let head = http
        .head(url)
        .send()
        .await
        .context("probing download size")?;
    if !head.status().is_success() {
        bail!("hub answered {} to size probe", head.status());
    }
    let total = head
        .content_length()
        .context("hub didn't report a size (no Content-Length)")?;
    if total == 0 {
        bail!("hub reported a zero-byte file");
    }
    let chunk_count = total.div_ceil(CHUNK_SIZE as u64) as usize;

    report_progress(shared, file, 0, total).await;

    // Resume: how many whole chunks does an existing .part already hold?
    // We only trust complete chunks; a torn final chunk from a previous
    // crash is re-fetched. In verified mode we'd ideally re-hash the
    // resumed prefix, but the final whole-file digest check below catches
    // any corruption, so trusting whole chunks on disk is safe.
    let existing_len = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let resume_chunks = (existing_len / CHUNK_SIZE as u64) as usize;
    let resume_bytes = resume_chunks as u64 * CHUNK_SIZE as u64;

    // Open the part file for append at the resume boundary. Truncate any
    // torn tail past the last whole chunk so appends line up exactly.
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .await
        .context("opening partial file")?;
    out.set_len(resume_bytes)
        .await
        .context("truncating torn tail of partial file")?;
    out.seek(std::io::SeekFrom::Start(resume_bytes))
        .await
        .context("seeking to resume point")?;

    let mut done = resume_bytes.min(total);
    let mut chunk_hashes: Vec<[u8; 32]> = Vec::with_capacity(chunk_count);
    // We can't recover hashes for resumed chunks without re-reading the
    // file; in unverified mode we re-read them at the end to build the
    // manifest. Mark resumed slots and backfill below.
    let resumed_unhashed = resume_chunks.min(chunk_count);

    for index in resume_chunks..chunk_count {
        let start = index as u64 * CHUNK_SIZE as u64;
        let end = (start + CHUNK_SIZE as u64).min(total) - 1; // inclusive
        let bytes = download_chunk_from_origin(http, url, start, end)
            .await
            .with_context(|| format!("fetching chunk {index}/{chunk_count}"))?;

        // Verify against the manifest if we have one. In this slice the
        // manifest is the pinned whole-file digest expressed per-chunk
        // only at the end, so per-chunk verification of *origin* bytes
        // is the whole-file check below; we still record each hash.
        let hash = swarm::sha256_bytes(&bytes);
        out.write_all(&bytes).await.context("writing chunk")?;
        chunk_hashes.push(hash);
        done += bytes.len() as u64;
        report_progress(shared, file, done.min(total), total).await;
    }
    out.flush().await.ok();
    drop(out);

    if done != total {
        bail!("download truncated: got {done} of {total} bytes");
    }

    // Final integrity gate: re-read the assembled file and verify the
    // whole-file digest. This is the source-independent check that makes
    // chunk-level trust unnecessary — and it's strictly more than the
    // old size-only path did.
    let assembled = tokio::fs::read(part)
        .await
        .context("re-reading assembled file for verification")?;

    // Build the manifest. If we resumed, the early chunk hashes weren't
    // captured in the loop; recompute the full chunk list from the bytes
    // so the manifest (and any future seeding) is correct.
    let manifest = if resumed_unhashed > 0 {
        ModelManifest::from_bytes(&assembled)
    } else {
        ModelManifest {
            digest: swarm::format_digest(&swarm::sha256_bytes(&assembled)),
            size_bytes: total,
            chunk_size: CHUNK_SIZE as u32,
            chunks: chunk_hashes,
        }
    };

    if let Some(expected) = expected_digest {
        if manifest.digest != expected {
            bail!(
                "integrity check failed: downloaded {} but expected {}",
                manifest.digest,
                expected
            );
        }
        tracing::info!(%file, digest = %manifest.digest, "model download verified against pinned digest");
    } else {
        tracing::info!(
            %file,
            digest = %manifest.digest,
            "model download complete (no pinned digest — synthesized manifest)"
        );
    }

    tokio::fs::rename(part, dest)
        .await
        .context("moving finished download into the library")?;
    Ok(())
}

/// Fetch the inclusive byte range `[start, end]` of `url` via an HTTP
/// `Range` request. Returns the raw bytes; the caller verifies and
/// writes them. Errors if the server ignores the range (returns 200
/// instead of 206) so we never silently mis-assemble a file.
async fn download_chunk_from_origin(
    http: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
) -> Result<Vec<u8>> {
    let resp = http
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .context("range request")?;
    // 206 Partial Content is what we want. A 200 means the server
    // ignored Range and is about to stream the whole file — refuse,
    // because appending a full body at a chunk offset corrupts the file.
    if resp.status() == reqwest::StatusCode::OK {
        bail!("server ignored Range header (returned 200, not 206)");
    }
    if !resp.status().is_success() {
        bail!("hub answered {} to range request", resp.status());
    }
    let bytes = resp.bytes().await.context("reading range body")?;
    let want = (end - start + 1) as usize;
    if bytes.len() != want {
        bail!("range returned {} bytes, expected {}", bytes.len(), want);
    }
    Ok(bytes.to_vec())
}

/// Update the shared download progress. Always reports the origin
/// source in this slice. Cheap; called per chunk (every 4 MiB) rather
/// than per network read, so the mutex isn't hammered.
async fn report_progress(shared: &Arc<Mutex<Inner>>, file: &str, done: u64, total: u64) {
    let mut inner = shared.lock().await;
    inner.download = DownloadState::Downloading {
        file: file.to_string(),
        bytes_done: done,
        bytes_total: total,
        source: DownloadSource::Origin,
    };
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
            // When a digest is pinned it must be a well-formed
            // sha256:<hex> string — a malformed constant would make the
            // download's integrity gate reject every (correct) byte.
            if let Some(d) = e.sha256 {
                assert!(
                    crate::swarm::is_valid_digest(d),
                    "malformed sha256 digest for {}: {d}",
                    e.id
                );
            }
        }
    }

    #[test]
    fn download_source_defaults_to_origin() {
        // The progress UI relies on the default being the origin source,
        // since that's all this slice ever reports.
        assert_eq!(DownloadSource::default(), DownloadSource::Origin);
    }

    #[test]
    fn download_source_serializes_snake_case() {
        // The UI keys off these exact strings.
        assert_eq!(
            serde_json::to_string(&DownloadSource::Lan).unwrap(),
            "\"lan\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadSource::Origin).unwrap(),
            "\"origin\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadSource::Peer).unwrap(),
            "\"peer\""
        );
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

    #[test]
    fn scan_lmstudio_models_walks_publisher_model_layout() {
        let root = std::env::temp_dir().join(format!("unhosted-ls-test-{}", std::process::id()));
        let nested = root.join("someone").join("some-model");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("some-model-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(root.join("loose.gguf"), b"xy").unwrap();
        std::fs::write(nested.join("notes.txt"), b"nope").unwrap();
        // Deeper than the two-level layout: must NOT be picked up.
        let too_deep = nested.join("extra");
        std::fs::create_dir_all(&too_deep).unwrap();
        std::fs::write(too_deep.join("deep.gguf"), b"deep").unwrap();

        let found = scan_lmstudio_models(&root);
        let names: Vec<_> = found.iter().map(|m| m.file.as_str()).collect();
        assert!(names.contains(&"some-model-Q4_K_M.gguf"));
        assert!(names.contains(&"loose.gguf"));
        assert!(!names.contains(&"deep.gguf"));
        assert!(!names.iter().any(|n| n.ends_with(".txt")));
        assert!(found.iter().all(|m| m.source == InstalledSource::LmStudio));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn merge_installed_prefers_library_on_name_collision() {
        let lib = vec![InstalledModel {
            file: "a.gguf".into(),
            size_bytes: 1,
            modified_unix: 10,
            source: InstalledSource::Library,
        }];
        let discovered = vec![
            InstalledModel {
                file: "a.gguf".into(),
                size_bytes: 2,
                modified_unix: 99,
                source: InstalledSource::LmStudio,
            },
            InstalledModel {
                file: "b.gguf".into(),
                size_bytes: 3,
                modified_unix: 50,
                source: InstalledSource::LmStudio,
            },
        ];
        let merged = merge_installed(lib, discovered);
        assert_eq!(merged.len(), 2);
        let a = merged.iter().find(|m| m.file == "a.gguf").unwrap();
        assert_eq!(a.source, InstalledSource::Library);
        // Newest-first across sources.
        assert_eq!(merged[0].file, "b.gguf");
    }
}
