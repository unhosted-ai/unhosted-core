//! Desktop shell for Unhosted — opens a native window pointed at the local
//! daemon's web UI. This is the v0.2.0 alpha: just a webview wrapping the
//! existing `unhosted serve` HTTP UI. Full Tauri (auto-updater, native menus,
//! tray icon, bundling, code signing) follows once this skeleton is solid.
//!
//! Built on `tao` (cross-platform window/event-loop) + `wry` (cross-platform
//! webview) — the same lower-level stack Tauri itself sits on. macOS uses
//! WKWebView, Linux uses WebKitGTK, Windows uses Edge WebView2.

use anyhow::{Context, Result};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

const DEFAULT_NODE_URL: &str = "http://127.0.0.1:7777";

fn main() -> Result<()> {
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
        eprintln!();
        eprintln!("unhosted daemon is not reachable at {node_url}.");
        eprintln!();
        eprintln!("start it in another terminal:");
        eprintln!("    unhosted serve");
        eprintln!();
        eprintln!("or set UNHOSTED_NODE_URL=<other-host:port> to point this app at a remote node.");
        eprintln!();
        std::process::exit(1);
    }

    tracing::info!(node_url = %node_url, "opening desktop shell");

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("unhosted")
        .with_inner_size(tao::dpi::LogicalSize::new(960.0, 720.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(480.0, 480.0))
        .build(&event_loop)
        .context("creating window")?;

    let _webview = WebViewBuilder::new()
        .with_url(&node_url)
        .build(&window)
        .context("creating webview")?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
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
