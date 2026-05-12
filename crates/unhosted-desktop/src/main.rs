//! Desktop shell for Unhosted — opens a native window pointed at the local
//! daemon's web UI.
//!
//! v0.0.7 swapped the bare `tao` + `wry` pair for **Tauri 2**. Same underlying
//! WebView (WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux) — the
//! Tauri wrap buys us the updater plugin, native bundling (`tauri bundle`
//! produces signed `.dmg` / `.msi` / `.AppImage`), and a place to bolt on the
//! Phase 2 polish (system tray, deep links, native notifications) without
//! rolling our own infrastructure.
//!
//! The desktop binary still bundles **zero** HTML/JS of its own. The window
//! loads `http://127.0.0.1:7777`, which is the daemon — so every UI change
//! ships through a daemon release, no separate desktop release needed.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use anyhow::Result;
use tauri_plugin_updater::UpdaterExt;

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7777";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("unhosted_desktop=info")),
        )
        .with_target(false)
        .init();

    let node_url =
        std::env::var("UNHOSTED_NODE_URL").unwrap_or_else(|_| DEFAULT_NODE_URL.to_string());

    if !daemon_reachable(&node_url) {
        // Better UX than aborting silently: the embedded dist/index.html
        // gives the user a clear "start the daemon" page. But warn loudly
        // so terminal users see what's wrong.
        eprintln!();
        eprintln!("unhosted daemon is not reachable at {node_url}.");
        eprintln!();
        eprintln!("start it in another terminal:");
        eprintln!("    unhosted serve");
        eprintln!();
        eprintln!("opening anyway — the window will retry on refresh.");
        eprintln!();
    }

    tracing::info!(node_url = %node_url, "opening tauri desktop shell");

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Kick off a background updater check on startup. Failures
            // are silent — the user can also trigger this manually
            // from the UI later (Phase 2).
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = check_for_update(handle).await {
                    tracing::warn!(error = %e, "updater check failed");
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Best-effort updater check. The updater plugin reads the endpoints +
/// pubkey from `tauri.conf.json`. If a newer signed release exists, the
/// `dialog: true` config flag pops the native "update available" prompt.
async fn check_for_update(app: tauri::AppHandle) -> Result<()> {
    let updater = app.updater()?;
    match updater.check().await {
        Ok(Some(update)) => {
            tracing::info!(
                version = %update.version,
                date = ?update.date,
                "update available"
            );
            // With `dialog: true` in tauri.conf.json the plugin shows
            // its own prompt + downloads + relaunches. No further code
            // needed here.
        }
        Ok(None) => tracing::info!("desktop shell is up to date"),
        Err(e) => tracing::warn!(error = %e, "updater check failed"),
    }
    Ok(())
}

fn daemon_reachable(url: &str) -> bool {
    let health = format!("{}/health", url.trim_end_matches('/'));
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(&health)
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
