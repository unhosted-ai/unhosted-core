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
//! v0.0.14 fixes the long-running "blank window on first launch" bug. The
//! prior fix tried to put the wait inside the WebView itself (an embedded
//! probe page that polled `/health` and `location.replace`d to the daemon
//! once it came up). That fix was unreliable: WKWebView throttled the JS
//! `setTimeout` chain when the window was backgrounded, and on macOS Tauri 2
//! the cross-origin navigation from `tauri://localhost/index.html` to
//! `http://127.0.0.1:7777` sometimes resolved to a blank page. We now do
//! the wait Rust-side here, before Tauri ever opens the WebView: poll
//! `/health` on a tight loop, auto-spawn `unhosted serve` if no daemon is
//! listening yet, and only call `Tauri::run` once the daemon answers.
//! Result: the WebView loads the real UI on first paint, every time.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use anyhow::Result;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7777";

/// How long to wait for the daemon to answer before giving up and opening
/// the WebView anyway. 60s is generous — a cold-start daemon answers in
/// well under 2s on every platform we ship, and a stuck daemon is almost
/// certainly stuck for some upstream reason (port collision, panic on
/// boot) that 60s won't unstick.
const DAEMON_WAIT_BUDGET: Duration = Duration::from_secs(60);

/// Poll interval while waiting for the daemon. Short enough that boot
/// feels instant once `unhosted serve` is responding, slow enough that
/// we don't hammer the kernel scheduler on cold boot.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckStatus {
    current_version: &'static str,
    available: bool,
    latest_version: Option<String>,
    status: &'static str,
}

#[tauri::command]
async fn check_for_app_update(app: tauri::AppHandle) -> Result<UpdateCheckStatus, String> {
    check_for_update(app).await.map_err(|e| e.to_string())
}

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
        // No daemon listening yet. Try to spawn one — the .app bundle
        // can't reasonably expect the user to have run `unhosted serve`
        // in another terminal before double-clicking the dock icon, so
        // we make the .app self-contained when we can.
        let spawned = try_spawn_daemon();
        if spawned {
            tracing::info!("spawned `unhosted serve` from desktop shell");
        } else {
            eprintln!();
            eprintln!("unhosted daemon is not reachable at {node_url},");
            eprintln!("and we couldn't find the `unhosted` binary on $PATH to");
            eprintln!("auto-start it. Install it with:");
            eprintln!();
            eprintln!(
                "    curl -fsSL https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.sh | sh"
            );
            eprintln!();
            eprintln!("or start it manually in another terminal:");
            eprintln!("    unhosted serve");
            eprintln!();
        }

        // Wait for the daemon to answer — either the one we just
        // spawned, or one the user starts in another terminal during
        // this window.
        wait_for_daemon(&node_url, DAEMON_WAIT_BUDGET);
    }

    tracing::info!(node_url = %node_url, "opening tauri desktop shell");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![check_for_app_update])
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second instance tried to launch — focus the existing window instead.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.unminimize();
            }
        }))
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
async fn check_for_update(app: tauri::AppHandle) -> Result<UpdateCheckStatus> {
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
            Ok(UpdateCheckStatus {
                current_version: env!("CARGO_PKG_VERSION"),
                available: true,
                latest_version: Some(update.version.to_string()),
                status: "available",
            })
        }
        Ok(None) => {
            tracing::info!("desktop shell is up to date");
            Ok(UpdateCheckStatus {
                current_version: env!("CARGO_PKG_VERSION"),
                available: false,
                latest_version: None,
                status: "up_to_date",
            })
        }
        Err(e) => {
            tracing::warn!(error = %e, "updater check failed");
            Err(e.into())
        }
    }
}

fn daemon_reachable(url: &str) -> bool {
    let health = format!("{}/health", url.trim_end_matches('/'));
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(800))
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

/// Block until `/health` answers, or we hit `budget`. Used on cold start
/// so that the WebView never opens against a refused-connection daemon
/// — Tauri 2's WKWebView renders an unrecoverable blank page in that
/// case and won't retry on its own.
fn wait_for_daemon(url: &str, budget: Duration) {
    let started = Instant::now();
    let mut announced_wait = false;
    while started.elapsed() < budget {
        if daemon_reachable(url) {
            tracing::info!(
                waited_ms = started.elapsed().as_millis() as u64,
                "daemon is reachable"
            );
            return;
        }
        if !announced_wait && started.elapsed() > Duration::from_millis(500) {
            tracing::info!("waiting for daemon to come up at {url}");
            announced_wait = true;
        }
        std::thread::sleep(DAEMON_POLL_INTERVAL);
    }
    tracing::warn!(
        budget_ms = budget.as_millis() as u64,
        "daemon never answered — opening WebView anyway"
    );
}

/// Look for the `unhosted` binary near the user's $PATH and spawn it as
/// a detached background process running `serve`. Returns whether a
/// spawn was attempted.
///
/// The first candidate is the daemon bundled NEXT TO this executable —
/// inside the .app's Contents/MacOS on macOS, the install dir on
/// Windows, the AppImage payload on Linux. That's what makes a
/// DMG-only install work on a machine that never ran install.sh.
/// After that we search a small whitelist of well-known install
/// locations so a fresh `install.sh` user gets a working .app without
/// needing any terminal state — the desktop binary is what they
/// double-clicked, so PATH inheritance from a login shell is
/// unreliable on macOS (the .app is launched from
/// Finder/Spotlight/LaunchPad with a minimal env).
fn try_spawn_daemon() -> bool {
    use std::process::{Command, Stdio};

    let exe_name = if cfg!(windows) {
        "unhosted.exe"
    } else {
        "unhosted"
    };
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe_name)))
        .map(|p| p.to_string_lossy().to_string());

    let home = std::env::var("HOME").ok();
    let candidates: Vec<String> = {
        let mut v = Vec::new();
        if let Some(b) = bundled {
            v.push(b);
        }
        v.extend([
            "/usr/local/bin/unhosted".to_string(),
            "/opt/homebrew/bin/unhosted".to_string(),
            "/usr/bin/unhosted".to_string(),
        ]);
        if let Some(h) = home.as_deref() {
            v.push(format!("{h}/.local/bin/unhosted"));
            v.push(format!("{h}/.cargo/bin/unhosted"));
        }
        v
    };

    let resolved = candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());

    let Some(bin) = resolved else {
        tracing::warn!("no `unhosted` binary found in standard install locations");
        return false;
    };

    tracing::info!(binary = %bin, "spawning `{bin} serve`");
    let result = Command::new(&bin)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match result {
        Ok(_child) => {
            // We intentionally don't keep the child handle: if the
            // .app quits, the daemon should keep running so anything
            // pointing at 127.0.0.1:7777 (the phone PWA, a cron job)
            // still works. `unhosted serve` already handles its own
            // shutdown via SIGTERM / Ctrl-C.
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn `unhosted`");
            false
        }
    }
}
