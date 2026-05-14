//! Embedded web UI — the daemon serves a chat interface at `/` so users
//! don't need to install anything beyond `unhosted` itself.
//!
//! In release builds, the contents of `crates/unhosted-core/web/` are baked
//! into the binary via `rust-embed`. In debug builds, files are loaded from
//! disk so changes take effect on refresh without recompiling. Tauri (when
//! it lands in v0.2.0) wraps this same UI.

use axum::{
    extract::Path,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/web/"]
struct WebAssets;

/// Handler for `GET /`. Serves the SPA entry point.
pub async fn serve_index() -> Response {
    serve_path("index.html")
}

/// Wildcard fallback handler for any path not claimed by an API route.
/// Returns the matching asset, or `index.html` for unknown paths so a
/// future client-side router can take over without 404s.
pub async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return serve_path("index.html");
    }
    if WebAssets::get(path).is_some() {
        serve_path(path)
    } else {
        // Fallback: hand unknown paths to the SPA so it can render its
        // own 404 (and so future client-side routes work out of the box).
        let mut resp = serve_path("index.html");
        *resp.status_mut() = StatusCode::NOT_FOUND;
        resp
    }
}

/// Handler used by `Router::route("/{*path}", ...)` when you prefer the
/// `Path<String>` extractor over the raw `Uri`.
#[allow(dead_code)]
pub async fn serve_static_path(Path(path): Path<String>) -> Response {
    if WebAssets::get(&path).is_some() {
        serve_path(&path)
    } else {
        let mut resp = serve_path("index.html");
        *resp.status_mut() = StatusCode::NOT_FOUND;
        resp
    }
}

fn serve_path(path: &str) -> Response {
    match WebAssets::get(path) {
        Some(asset) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            // HTML / JS / CSS / manifest change between releases (and on
            // every `cargo build` during dev). WKWebView's default cache
            // is happy to serve `no-cache` content from disk while it
            // revalidates in the background, which surfaced as a "I
            // shipped a UI fix but the user's WebView is still running
            // yesterday's JS" bug across multiple sessions. `no-store,
            // max-age=0` forces a fresh fetch on every page load so the
            // running app always reflects the daemon's currently-served
            // assets.
            //
            // Images / fonts can still cache normally — they don't
            // change between dev iterations and re-fetching them on
            // every load is wasteful.
            let cache_control = match path.rsplit('.').next() {
                Some("html") | Some("js") | Some("css") | Some("json") => {
                    "no-store, max-age=0, must-revalidate"
                }
                _ => "no-cache",
            };
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, cache_control)
                .body(axum::body::Body::from(asset.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
