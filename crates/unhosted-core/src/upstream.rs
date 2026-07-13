//! Upstream-backend detection.
//!
//! v0.0.1 shipped against llama.cpp's `llama-server`. Users in the wild
//! also run **Ollama** and **LM Studio** — both speak OpenAI-compatible
//! HTTP, both work as drop-in upstreams. Rather than failing silently
//! when llama-server isn't on :8080, this module probes the three common
//! backends on their default localhost ports and reports which (if any)
//! is reachable. Callers use this for:
//!
//! 1. The startup banner in `serve()` — print a helpful install hint
//!    when the configured upstream is down and a different backend is
//!    actually running (or nothing is).
//! 2. The `unhosted doctor` CLI command — explicit "what's wrong with
//!    my setup" probe.

use std::time::Duration;

/// Default localhost URL llama.cpp's `llama-server` binds to.
pub const LLAMA_SERVER_DEFAULT_URL: &str = "http://127.0.0.1:8080";
/// Default localhost URL Ollama serves on.
pub const OLLAMA_DEFAULT_URL: &str = "http://127.0.0.1:11434";
/// Default localhost URL LM Studio's local server binds to.
pub const LM_STUDIO_DEFAULT_URL: &str = "http://127.0.0.1:1234";

const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    LlamaServer,
    Ollama,
    LmStudio,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::LlamaServer => "llama-server",
            Backend::Ollama => "ollama",
            Backend::LmStudio => "lm studio",
        }
    }

    /// The URL the daemon should set as `llama_server_url` to proxy to
    /// this backend. All three speak OpenAI-compatible `/v1/...`.
    pub fn upstream_url(self) -> &'static str {
        match self {
            Backend::LlamaServer => LLAMA_SERVER_DEFAULT_URL,
            Backend::Ollama => OLLAMA_DEFAULT_URL,
            Backend::LmStudio => LM_STUDIO_DEFAULT_URL,
        }
    }

    /// Endpoint used to verify the backend is alive. Each one returns
    /// 200 on a healthy install with no models required.
    fn probe_path(self) -> &'static str {
        match self {
            // llama-server exposes /health → "ok"; /v1/models also works
            // but requires a loaded model. /health does not.
            Backend::LlamaServer => "/health",
            // Ollama's list-models endpoint is always available.
            Backend::Ollama => "/api/tags",
            // LM Studio exposes the OpenAI /v1/models route.
            Backend::LmStudio => "/v1/models",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub backend: Backend,
    pub url: String,
    pub reachable: bool,
}

#[derive(Debug, Clone)]
pub struct BackendReport {
    pub results: Vec<ProbeResult>,
}

impl BackendReport {
    /// First backend that responded, in priority order
    /// (llama-server → ollama → lm studio).
    pub fn first_reachable(&self) -> Option<&ProbeResult> {
        self.results.iter().find(|r| r.reachable)
    }

    pub fn any_reachable(&self) -> bool {
        self.results.iter().any(|r| r.reachable)
    }
}

/// Probe all known local backends in parallel. Times out fast — this
/// runs at daemon startup, so we don't want to block boot if something
/// is hung.
pub async fn probe_all() -> BackendReport {
    let backends = [Backend::LlamaServer, Backend::Ollama, Backend::LmStudio];
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .expect("reqwest client builds");

    let futures = backends.iter().map(|&backend| {
        let client = client.clone();
        async move {
            let url = format!("{}{}", backend.upstream_url(), backend.probe_path());
            let reachable = match client.get(&url).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            };
            ProbeResult {
                backend,
                url: backend.upstream_url().to_string(),
                reachable,
            }
        }
    });

    let results = futures::future::join_all(futures).await;
    BackendReport { results }
}

/// Probe an arbitrary URL — used to verify the *configured* upstream
/// (whatever `UNHOSTED_LLAMA_SERVER_URL` points at) actually responds.
/// We try `/v1/models` (OpenAI-compat, works for all three backends) and
/// fall back to `/health` (llama-server specific).
pub async fn probe_configured(upstream: &str) -> bool {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let base = upstream.trim_end_matches('/');
    for path in ["/v1/models", "/health", "/api/tags"] {
        let url = format!("{base}{path}");
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
    }
    false
}

/// A live upstream that the daemon can route a chat completion to:
/// the base URL it's running on, plus a chat-capable model id we
/// discovered from its `/v1/models`. The model is `None` when the
/// backend either has no models loaded or only exposes embedding
/// models; in that case the daemon falls back to the next backend
/// instead of trying to talk to an empty one.
#[derive(Debug, Clone)]
pub struct LiveUpstream {
    pub url: String,
    pub model: Option<String>,
}

/// Pick a live upstream for a request, *right now*. Tries the
/// configured URL first; if it doesn't respond, or responds but has
/// no usable chat model, falls back to the other known local
/// backends in priority order. Returns `None` when nothing is
/// reachable with a chat model — callers should surface a
/// structured "upstream offline" error to the user, not a bare 502.
///
/// This is called per-request, not just at startup, so the daemon
/// stays responsive when the user starts llama-server / ollama /
/// lm studio mid-session, or loads a model after the fact. The
/// probes are cheap (~750 ms each) and run sequentially to bias
/// toward the configured one when it's healthy.
pub async fn select_live(configured: &str) -> Option<LiveUpstream> {
    let configured_base = configured.trim_end_matches('/').to_string();

    // Try the configured upstream first. We treat it as usable if
    // EITHER it has a discoverable chat model on /v1/models OR it
    // exposes /health (llama-server, which accepts any model name
    // and just serves whatever's loaded).
    if let Some(live) = try_backend(&configured_base).await {
        return Some(live);
    }

    // Fall back in priority order. Skip whatever matches the
    // configured base so we don't double-probe.
    for backend in [Backend::Ollama, Backend::LmStudio, Backend::LlamaServer] {
        let candidate = backend.upstream_url();
        if candidate == configured_base {
            continue;
        }
        if let Some(live) = try_backend(candidate).await {
            return Some(live);
        }
    }
    None
}

/// Probe one backend: confirm it's alive, then try to discover a
/// chat-capable model id from its `/v1/models`. Returns `None` if
/// the backend is unreachable AND we can't fall back to llama-style
/// behavior (i.e., it's not the bare /health-only case).
async fn try_backend(base: &str) -> Option<LiveUpstream> {
    let base = base.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;

    // Discover a chat model. `/v1/models` is OpenAI-compat across
    // all three backends; we pick the first id that doesn't look
    // like an embedding model.
    if let Ok(resp) = client.get(format!("{base}/v1/models")).send().await {
        if resp.status().is_success() {
            let model = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                v.get("data").and_then(|d| d.as_array()).and_then(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|id| id.as_str()))
                        .find(|id| !is_embedding_model(id))
                        .map(|s| s.to_string())
                })
            });
            return Some(LiveUpstream { url: base, model });
        }
    }

    // /v1/models failed — try /health (llama-server). A healthy
    // llama-server accepts any chat completion without a model
    // field, so we return None for the model.
    if let Ok(resp) = client.get(format!("{base}/health")).send().await {
        if resp.status().is_success() {
            return Some(LiveUpstream {
                url: base,
                model: None,
            });
        }
    }

    None
}

fn is_embedding_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("embed") || lower.contains("reranker")
}

/// Structured "no upstream is reachable" body. Returned with HTTP 503
/// from request handlers so the web UI can render a friendly message
/// (and the install hint) instead of "node returned 502 bad gateway".
pub fn offline_error_json(configured: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "type": "upstream_offline",
            "message": "no local model runtime is responding",
            "configured": configured,
            "checked": [
                LLAMA_SERVER_DEFAULT_URL,
                OLLAMA_DEFAULT_URL,
                LM_STUDIO_DEFAULT_URL,
            ],
            "hint": "start one of: llama-server, ollama, or lm studio. run `unhosted doctor` for an install walkthrough.",
        }
    })
}

/// Plain-English install hints, picked to match the user's OS. Printed
/// when no backend is reachable. We keep it short and direct — the
/// brand voice rules say no marketing, no padding.
pub fn install_hints() -> &'static str {
    if cfg!(target_os = "macos") {
        "\
no local model runtime is reachable. install one:

  • llama.cpp  — brew install llama.cpp           (recommended; matches the docs)
  • ollama     — brew install ollama && ollama serve
  • lm studio  — https://lmstudio.ai             (gui, easiest)

then point unhosted at it (only needed for non-default ports):
  UNHOSTED_LLAMA_SERVER_URL=http://127.0.0.1:8080 unhosted serve

after llama.cpp is installed:
  unhosted pull llama3.2:3b
  llama-server -m ~/.cache/unhosted/models/llama3.2-3b.gguf --port 8080 -c 4096 -ngl 99
"
    } else if cfg!(target_os = "linux") {
        "\
no local model runtime is reachable. install one:

  • llama.cpp  — https://github.com/ggerganov/llama.cpp  (build from source, or use a release tarball)
  • ollama     — curl -fsSL https://ollama.com/install.sh | sh && ollama serve
  • lm studio  — https://lmstudio.ai                     (appimage)

then point unhosted at it (only needed for non-default ports):
  UNHOSTED_LLAMA_SERVER_URL=http://127.0.0.1:8080 unhosted serve
"
    } else if cfg!(target_os = "windows") {
        "\
no local model runtime is reachable. install one:

  • lm studio  — https://lmstudio.ai             (recommended on windows; gui)
  • ollama     — https://ollama.com/download/windows
  • llama.cpp  — https://github.com/ggerganov/llama.cpp/releases (precompiled .exe)

then point unhosted at it (only needed for non-default ports):
  set UNHOSTED_LLAMA_SERVER_URL=http://127.0.0.1:8080
"
    } else {
        "\
no local model runtime is reachable. install llama.cpp, ollama, or lm studio,
then re-run `unhosted serve`. see https://github.com/unhosted-ai/unhosted-core
for setup details.
"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_url_and_probe_path_are_consistent() {
        // Each backend's upstream_url must be the localhost default it maps
        // to, and its probe path must be the always-available endpoint.
        assert_eq!(
            Backend::LlamaServer.upstream_url(),
            LLAMA_SERVER_DEFAULT_URL
        );
        assert_eq!(Backend::Ollama.upstream_url(), OLLAMA_DEFAULT_URL);
        assert_eq!(Backend::LmStudio.upstream_url(), LM_STUDIO_DEFAULT_URL);

        assert_eq!(Backend::LlamaServer.probe_path(), "/health");
        assert_eq!(Backend::Ollama.probe_path(), "/api/tags");
        assert_eq!(Backend::LmStudio.probe_path(), "/v1/models");
    }

    #[test]
    fn backend_names_are_stable() {
        // Names surface in the doctor CLI + startup banner; keep them fixed.
        assert_eq!(Backend::LlamaServer.name(), "llama-server");
        assert_eq!(Backend::Ollama.name(), "ollama");
        assert_eq!(Backend::LmStudio.name(), "lm studio");
    }

    #[test]
    fn embedding_models_are_classified_out() {
        // These must be skipped when picking a chat model.
        assert!(is_embedding_model("nomic-embed-text"));
        assert!(is_embedding_model("text-embedding-3-large"));
        assert!(is_embedding_model("bge-reranker-base"));
        assert!(is_embedding_model("BAAI/bge-EMBED")); // case-insensitive
                                                       // These are chat models and must NOT be filtered.
        assert!(!is_embedding_model("llama-3.2-3b-instruct"));
        assert!(!is_embedding_model("qwen2.5-coder-7b"));
        assert!(!is_embedding_model("mistral-7b"));
    }

    fn report(pairs: &[(Backend, bool)]) -> BackendReport {
        BackendReport {
            results: pairs
                .iter()
                .map(|&(backend, reachable)| ProbeResult {
                    backend,
                    url: backend.upstream_url().to_string(),
                    reachable,
                })
                .collect(),
        }
    }

    #[test]
    fn first_reachable_returns_none_when_all_down() {
        let r = report(&[
            (Backend::LlamaServer, false),
            (Backend::Ollama, false),
            (Backend::LmStudio, false),
        ]);
        assert!(r.first_reachable().is_none());
        assert!(!r.any_reachable());
    }

    #[test]
    fn first_reachable_respects_priority_order() {
        // Ollama + LmStudio up, llama-server down: first_reachable is the
        // first *reachable* in the list order (priority), i.e. Ollama.
        let r = report(&[
            (Backend::LlamaServer, false),
            (Backend::Ollama, true),
            (Backend::LmStudio, true),
        ]);
        assert!(r.any_reachable());
        assert_eq!(r.first_reachable().unwrap().backend, Backend::Ollama);
    }

    #[test]
    fn offline_error_json_has_stable_shape() {
        // The web UI + API clients key off this structure; lock it.
        let v = offline_error_json("http://127.0.0.1:9999");
        assert_eq!(v["error"]["type"], "upstream_offline");
        assert_eq!(v["error"]["configured"], "http://127.0.0.1:9999");
        let checked = v["error"]["checked"].as_array().unwrap();
        assert_eq!(checked.len(), 3);
        assert!(checked.iter().any(|u| u == LLAMA_SERVER_DEFAULT_URL));
        assert!(v["error"]["message"].is_string());
        assert!(v["error"]["hint"].is_string());
    }

    #[test]
    fn install_hints_are_non_empty_for_this_platform() {
        // Whatever OS the tests run on, we should get a real hint block.
        let h = install_hints();
        assert!(h.contains("no local model runtime is reachable"));
        assert!(!h.trim().is_empty());
    }
}
