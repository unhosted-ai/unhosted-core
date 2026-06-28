use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use unhosted_core::{
    default_node_name, serve, Node, Peer, PeerRegistry, DEFAULT_LLAMA_SERVER_URL, DEFAULT_NODE_ADDR,
};

/// Known models keyed by short name. Bartowski's quants on Hugging Face —
/// the de facto standard quantizations. Q4_K_M is the size/quality sweet spot.
/// (name, download URL, approximate file size in bytes)
const MODELS: &[(&str, &str, u64)] = &[
    (
        "llama3.2:1b",
        "https://huggingface.co/bartowski/Llama-3.2-1B-Instruct-GGUF/resolve/main/Llama-3.2-1B-Instruct-Q4_K_M.gguf",
        770_000_000,
    ),
    (
        "llama3.2:3b",
        "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        2_020_000_000,
    ),
    (
        "llama3.1:8b",
        "https://huggingface.co/bartowski/Meta-Llama-3.1-8B-Instruct-GGUF/resolve/main/Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        4_920_000_000,
    ),
    (
        "qwen2.5:0.5b",
        "https://huggingface.co/bartowski/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/Qwen2.5-0.5B-Instruct-Q4_K_M.gguf",
        400_000_000,
    ),
    (
        "qwen2.5-coder:7b",
        "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf",
        4_700_000_000,
    ),
];

#[derive(Parser, Debug)]
#[command(name = "unhosted", version, about = "AI that lives where you do.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start a local unhosted node. Proxies to llama-server upstream.
    Serve {
        /// Address the node listens on.
        #[arg(long, default_value = DEFAULT_NODE_ADDR)]
        addr: SocketAddr,
        /// Upstream llama-server URL. Overrides UNHOSTED_LLAMA_SERVER_URL.
        #[arg(long)]
        upstream: Option<String>,
        /// Relay URL (ws:// or wss://). Daemon registers with the relay
        /// for trusted-peer reachability across the internet. Overrides
        /// UNHOSTED_RELAY env var.
        #[arg(long)]
        relay: Option<String>,
        /// Eagerly start the Cloudflare tunnel at daemon boot, so the
        /// public URL is live by the time the user clicks "open to
        /// internet" in the UI (0s perceived latency vs ~4s). Off by
        /// default — exposing the daemon publicly is opt-in. Overrides
        /// UNHOSTED_EAGER_TUNNEL env var.
        #[arg(long)]
        eager_tunnel: bool,
    },
    /// Run a prompt against a local node and stream tokens to stdout.
    Run {
        /// The prompt to send.
        prompt: String,
        /// Node URL to send the request to.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
        /// Maximum tokens to generate.
        #[arg(long, default_value_t = 256)]
        max_tokens: u32,
    },
    /// Manage peer nodes (v0.0.2 multi-node cluster).
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
    /// Download a model to the local cache so llama-server can serve it.
    Pull {
        /// Short name (llama3.2:1b, llama3.1:8b, qwen2.5:0.5b, …) or full URL.
        model: String,
    },
    /// List known models and what's already cached on this machine.
    Models,
    /// Show which cached models this node can seed to peers over the
    /// swarm protocol (ADR-0014), each keyed by its content digest.
    /// Hashes every local GGUF — slow on a large library, by design.
    SeedStatus,
    /// Print this node's stable Ed25519 identity (pubkey).
    Identity,
    /// Trusted-peer pairing (v0.1.0). Two-step out-of-band flow.
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Attempt a UDP hole-punch with a trusted peer through the relay.
    /// Both daemons must run this within ~10s of each other; the relay
    /// matches the requests and tells each side where to dial. Reports
    /// whether direct UDP traffic actually arrived.
    Punch {
        /// Peer name (as shown in `peer list`).
        peer: String,
        /// Address of the local daemon.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
        /// Coordination timeout in seconds.
        #[arg(long, default_value_t = 8)]
        timeout: u64,
    },
    /// Round-trip ping over the QUIC peer transport. Confirms the
    /// encrypted path between two paired daemons works end-to-end.
    QuicPing {
        /// Peer name (as shown in `peer list`).
        peer: String,
        /// Address of the local daemon.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
    },
    /// Probe the local environment for a model runtime — llama.cpp's
    /// `llama-server`, Ollama, or LM Studio. Prints what's reachable
    /// on the default localhost ports and, if nothing is, an
    /// OS-specific install hint.
    Doctor,
    /// VRAM-pooling (ADR 0009) — distribute one model's layers
    /// across multiple LAN peers via llama.cpp's RPC mode, so a
    /// 70B-class model is usable across hardware that no single
    /// machine could fit. v0.1.0 ships orchestration; this slice
    /// ships *detection* — every subcommand except `detect`
    /// reports "not yet implemented" but prints the local
    /// capability so users can verify their build is ready.
    #[command(name = "vram-pool")]
    VramPool {
        #[command(subcommand)]
        action: VramPoolAction,
    },
    /// Distillation recipe — train a small specialist by SFT'ing
    /// a tiny open base on synthetic data from a teacher model.
    /// Thin wrapper around the Python scripts in `models/distill/`;
    /// see that directory's README for the full recipe + expected
    /// hardware budget.
    Distill {
        #[command(subcommand)]
        action: DistillAction,
    },
    /// MCP plugin wiring — point an MCP-aware host (Claude Desktop,
    /// Cursor, Zed) at the local daemon via the
    /// `@unhosted-ai/mcp-server` shim. `print` shows the config
    /// snippet on stdout; `install` writes it to the host's config
    /// file. Code for the shim lives in the `unhosted-plugins` repo.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Upgrade unhosted to the latest published release. Re-runs the
    /// official install script (the same one the README points at)
    /// which downloads the signed tarball for the current platform
    /// from GitHub releases and replaces the on-disk binaries.
    /// Desktop users get a native auto-update prompt instead —
    /// this is for CLI installs.
    Upgrade {
        /// Skip the published-version check and run the install
        /// script unconditionally. Useful when reinstalling the
        /// same version to fix a botched install.
        #[arg(long)]
        force: bool,
    },
    /// Agent runtime (ADR-0012). Drive the model in a tool-call loop
    /// against the configured upstream. The model uses the tools the
    /// caller allow-lists and either reaches a final answer or hits
    /// a guardrail (max_steps / max_tokens / max_seconds).
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Run an agent against a user-supplied goal. Prints the step
    /// trace to stdout as the run progresses, then the final answer.
    Run {
        /// The goal the agent should achieve. Wrap in quotes.
        goal: String,
        /// Comma-separated allow-list of tools the model may call.
        /// Slice-1 registry: `web_fetch`, `search_memory`,
        /// `list_models`. The daemon never injects a tool the caller
        /// didn't list.
        #[arg(long, value_delimiter = ',', default_value = "web_fetch")]
        tools: Vec<String>,
        /// Maximum tool-call loop iterations. Clamped to 32 by the
        /// daemon's hard limit.
        #[arg(long, default_value_t = 8)]
        max_steps: u32,
        /// Cumulative token budget across all steps. Clamped to
        /// 32 768 by the daemon's hard limit.
        #[arg(long, default_value_t = 4096)]
        max_tokens: u32,
        /// Wall-clock budget in seconds. Clamped to 600 by the
        /// daemon's hard limit.
        #[arg(long, default_value_t = 60)]
        max_seconds: u32,
        /// Daemon URL.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
        /// Model id. `"auto"` (default) routes through the daemon's
        /// configured upstream selection.
        #[arg(long, default_value = "auto")]
        model: String,
        /// Print the raw JSON response instead of the pretty trace.
        /// Useful for piping into `jq` or for scripts.
        #[arg(long)]
        json: bool,
    },
}

/// Where the daemon is reachable from the host app. Defaults to
/// loopback at the standard port; override for tunnels.
const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:7777";

#[derive(Subcommand, Debug)]
enum McpAction {
    /// Print the MCP server config snippet for the named host. Does
    /// not modify any files — pipe / paste it yourself.
    Print {
        /// claude-desktop, cursor, or zed
        #[arg(value_enum)]
        host: McpHost,
        /// Daemon URL the MCP shim should call. Defaults to loopback.
        #[arg(long, default_value = DEFAULT_DAEMON_URL)]
        daemon_url: String,
        /// Optional bearer token. Only needed for non-loopback daemons
        /// (e.g. behind a Cloudflare tunnel).
        #[arg(long)]
        bearer: Option<String>,
    },
    /// Install the MCP server config into the named host's config
    /// file. Idempotent: re-running with the same args is a no-op.
    /// Backs up the existing config to `<file>.unhosted.bak` on
    /// first write so a misconfigured host can be recovered.
    Install {
        #[arg(value_enum)]
        host: McpHost,
        #[arg(long, default_value = DEFAULT_DAEMON_URL)]
        daemon_url: String,
        #[arg(long)]
        bearer: Option<String>,
        /// Skip the host-config write and just print the file path
        /// that would be written to. Useful for dry-run / debugging.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
enum McpHost {
    ClaudeDesktop,
    Cursor,
    Zed,
}

#[derive(Subcommand, Debug)]
enum DistillAction {
    /// Generate synthetic (prompt, response) pairs from a directory
    /// of documents using any OpenAI-compatible teacher endpoint.
    /// Defaults to a local unhosted daemon at 127.0.0.1:7777.
    /// All args after `--` are passed through to gen_data.py.
    Data {
        /// Trailing args forwarded verbatim. Use `--` to separate
        /// (e.g. `unhosted distill data -- --docs ./notes --out data/train.jsonl`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// SFT a LoRA adapter on a JSONL of training pairs. Defaults
    /// target TinyLlama-1.1B-Chat. All args after `--` forward to
    /// train.py.
    Train {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compare a fine-tuned adapter against a baseline on a held-out
    /// JSONL test set. Both models reached via OpenAI-compatible
    /// endpoints. All args after `--` forward to eval.py.
    Eval {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Publish a trained adapter to the Hugging Face Hub. Fills in
    /// the model-card template from the flags you pass and uploads
    /// the adapter directory. Requires HF_TOKEN. All args after `--`
    /// forward to push_to_hub.py.
    Push {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// End-to-end "pick a model and teach it": run the whole recipe in
    /// one shot (data -> train -> eval) via pipeline.py. This is the
    /// high-level path; `data`/`train`/`eval` above are the stages it
    /// wraps. Generate from a teacher (`-- --docs ./notes
    /// --teacher claude-opus-4-8`) or reuse a dataset (`-- --data
    /// pairs.jsonl`); set the student with `--base-model`. All args
    /// after `--` forward to pipeline.py.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum VramPoolAction {
    /// Probe this machine for the binaries + build flags VRAM-pooling
    /// requires (rpc-server on PATH, --rpc flag on llama-server).
    /// Prints a status line plus an install hint pinned to the
    /// actual gap detected. Safe to run with no daemon running.
    Detect,
    /// Advise whether a model fits in this machine's memory or wants a
    /// VRAM pool. Estimates the model's memory need (from a cached GGUF,
    /// a known model's size, or --size-gb), compares it to total RAM
    /// minus a headroom margin, and prints a recommendation plus a rough
    /// peer count if pooling is needed. A heuristic, not a guarantee —
    /// quantization, context length, and KV-cache all move the real
    /// number. Safe to run with no daemon running.
    Fit {
        /// Model name/short-id (e.g. llama3.1:8b), a path to a local
        /// .gguf, or a full GGUF URL. Used to estimate memory need.
        #[arg(long)]
        model: Option<String>,
        /// Override the model size estimate, in gigabytes. Use when the
        /// model isn't cached/known and you only know its rough size.
        #[arg(long)]
        size_gb: Option<f64>,
        /// Memory to leave free for the OS and other apps, in gigabytes.
        #[arg(long, default_value_t = 4.0)]
        headroom_gb: f64,
    },
    /// (v0.1.0+ — not yet implemented in this slice.) Start a
    /// layer-split inference cluster across this machine and the
    /// specified peers.
    Start {
        /// Model name/short-id or full GGUF URL.
        #[arg(long)]
        model: Option<String>,
        /// LAN peers to enlist as layer hosts. Repeat the flag for
        /// each peer (`--peers thunder --peers homelab`) or use a
        /// comma-separated list (`--peers thunder,homelab`). By
        /// default uses paired peers from `~/.config/unhosted/peers.toml`.
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        peers: Vec<String>,
    },
    /// (v0.1.0+ — not yet implemented.) Stop the cluster.
    Stop,
    /// (v0.1.0+ — not yet implemented.) Show cluster topology and
    /// per-peer VRAM utilization. For now reports local capability.
    Status,
}

#[derive(Subcommand, Debug)]
enum PairAction {
    /// Generate a pairing offer. Share the printed URI with the other
    /// device's owner; the token expires in 5 minutes.
    Offer {
        /// Address of the local daemon to request the offer from.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
    },
    /// Accept a pairing offer received from another node. Adds them as a
    /// trusted peer (and they add you).
    Accept {
        /// The full `unhosted://pair?addr=...&token=...` URI from the other side.
        offer: String,
        /// Local daemon to register the new trusted peer with.
        #[arg(long, default_value_t = format!("http://{}", DEFAULT_NODE_ADDR))]
        node: String,
    },
}

#[derive(Subcommand, Debug)]
enum PeerAction {
    /// List configured peers.
    List,
    /// Add a peer by name and address. Replaces an existing entry with the same name.
    Add {
        /// Human-readable name for this peer.
        name: String,
        /// Peer address (e.g. 192.168.1.42:7777).
        addr: SocketAddr,
        /// Lower priorities are preferred. Default 10.
        #[arg(long, default_value_t = 10)]
        priority: u8,
    },
    /// Remove a peer by name.
    Remove {
        /// The name passed to `peer add`.
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve {
            addr,
            upstream,
            relay,
            eager_tunnel,
        } => {
            let llama_server_url = upstream
                .or_else(|| std::env::var("UNHOSTED_LLAMA_SERVER_URL").ok())
                .unwrap_or_else(|| DEFAULT_LLAMA_SERVER_URL.to_string());
            let registry = PeerRegistry::load().context("loading peer registry")?;
            if !registry.peers.is_empty() {
                tracing::info!(
                    count = registry.peers.len(),
                    "loaded peers from registry — request routing is round-robin local + peers"
                );
            }
            let relay_url = relay.or_else(|| std::env::var("UNHOSTED_RELAY").ok());
            // Eager-tunnel intent OR's three independent sources:
            //   1. `--eager-tunnel` CLI flag (explicit per-invocation).
            //   2. `UNHOSTED_EAGER_TUNNEL` env var (operator policy).
            //   3. Persisted user-click state at
            //      `~/.config/unhosted/tunnel-autostart.txt`, written by
            //      the tunnel UI's start/stop handlers. This is what
            //      makes "I enabled the tunnel once" survive a daemon
            //      restart for desktop users without them rediscovering
            //      the env var.
            let eager_tunnel = eager_tunnel
                || std::env::var("UNHOSTED_EAGER_TUNNEL")
                    .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                    .unwrap_or(false)
                || unhosted_core::tunnel::load_autostart();
            let node = Node {
                addr,
                llama_server_url,
                peers: registry.peers,
                name: default_node_name(),
                relay_url,
                eager_tunnel,
            };
            serve(node).await?;
        }
        Command::Run {
            prompt,
            node,
            max_tokens,
        } => {
            run_prompt(&node, &prompt, max_tokens).await?;
        }
        Command::Peer { action } => {
            handle_peer(action)?;
        }
        Command::Pull { model } => {
            pull_model(&model).await?;
        }
        Command::Models => {
            list_models()?;
        }
        Command::SeedStatus => {
            seed_status().await?;
        }
        Command::Identity => {
            let id = unhosted_core::Identity::load_or_create()?;
            println!("pubkey: {}", id.public_b64());
            println!(
                "path:   {}",
                unhosted_core::identity::config_path()?.display()
            );
        }
        Command::Pair { action } => {
            handle_pair(action).await?;
        }
        Command::Punch {
            peer,
            node,
            timeout,
        } => {
            handle_punch(&node, &peer, timeout).await?;
        }
        Command::QuicPing { peer, node } => {
            handle_quic_ping(&node, &peer).await?;
        }
        Command::Doctor => {
            run_doctor().await?;
        }
        Command::VramPool { action } => {
            run_vram_pool(action).await?;
        }
        Command::Distill { action } => {
            run_distill(action)?;
        }
        Command::Mcp { action } => {
            run_mcp(action)?;
        }
        Command::Upgrade { force } => {
            run_upgrade(force).await?;
        }
        Command::Agent { action } => {
            run_agent_action(action).await?;
        }
    }

    Ok(())
}

// ─── mcp plugin wiring ────────────────────────────────────────────────
// Generates the JSON snippet that points an MCP-aware host at the
// `@unhosted-ai/mcp-server` shim, with env vars pointing at the local
// daemon. Hosts use slightly different config shapes:
//
//   Claude Desktop:  ~/Library/Application Support/Claude/claude_desktop_config.json
//                    { "mcpServers": { "unhosted": { command, args, env } } }
//   Cursor:          ~/.cursor/mcp.json   (per-user, since Cursor 0.42)
//                    { "mcpServers": { ... } }   (same shape as Claude Desktop)
//   Zed:             ~/.config/zed/settings.json
//                    { "context_servers": { "unhosted": { command, args, env } } }
//
// We don't ship the MCP server itself — it's in unhosted-plugins. The
// `npx -y @unhosted-ai/mcp-server` invocation fetches the latest
// published version on demand. Until that package is on npm (pending
// user creds), the snippet still works for anyone who has the repo
// checked out — they can replace the command/args with
// `node /path/to/unhosted-plugins/mcp-server/dist/index.js`.

fn run_mcp(action: McpAction) -> Result<()> {
    match action {
        McpAction::Print {
            host,
            daemon_url,
            bearer,
        } => {
            let snippet = build_mcp_config(host, &daemon_url, bearer.as_deref());
            println!("{}", serde_json::to_string_pretty(&snippet)?);
        }
        McpAction::Install {
            host,
            daemon_url,
            bearer,
            dry_run,
        } => {
            let path = mcp_host_config_path(host)?;
            if dry_run {
                println!("would write to: {}", path.display());
                return Ok(());
            }
            install_mcp_config(host, &daemon_url, bearer.as_deref(), &path)?;
            println!("wrote: {}", path.display());
            println!();
            println!(
                "restart {} for changes to take effect.",
                mcp_host_name(host)
            );
        }
    }
    Ok(())
}

fn mcp_host_name(host: McpHost) -> &'static str {
    match host {
        McpHost::ClaudeDesktop => "Claude Desktop",
        McpHost::Cursor => "Cursor",
        McpHost::Zed => "Zed",
    }
}

/// Build the `unhosted` MCP server stanza. Shape varies per host, so
/// the caller's `host` decides which top-level key wraps it.
fn build_mcp_config(host: McpHost, daemon_url: &str, bearer: Option<&str>) -> serde_json::Value {
    let mut env = serde_json::Map::new();
    env.insert(
        "UNHOSTED_DAEMON_URL".to_string(),
        serde_json::Value::String(daemon_url.to_string()),
    );
    if let Some(b) = bearer {
        env.insert(
            "UNHOSTED_BEARER".to_string(),
            serde_json::Value::String(b.to_string()),
        );
    }
    let stanza = serde_json::json!({
        "command": "npx",
        "args": ["-y", "@unhosted-ai/mcp-server"],
        "env": serde_json::Value::Object(env),
    });
    // Top-level key: Claude Desktop + Cursor both use `mcpServers`;
    // Zed uses `context_servers`. Cross-host consistency would be
    // nice; reality is what it is.
    let outer_key = match host {
        McpHost::ClaudeDesktop | McpHost::Cursor => "mcpServers",
        McpHost::Zed => "context_servers",
    };
    serde_json::json!({
        outer_key: { "unhosted": stanza }
    })
}

fn mcp_host_config_path(host: McpHost) -> Result<PathBuf> {
    let home = unhosted_core::paths::home_dir()?;
    Ok(match host {
        McpHost::ClaudeDesktop => home
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json"),
        McpHost::Cursor => home.join(".cursor").join("mcp.json"),
        McpHost::Zed => home.join(".config").join("zed").join("settings.json"),
    })
}

/// Merge the unhosted MCP stanza into the host's existing config (or
/// create the config if missing). Other servers / settings in the
/// file are preserved. Backs up the original to
/// `<file>.unhosted.bak` once on first write.
fn install_mcp_config(
    host: McpHost,
    daemon_url: &str,
    bearer: Option<&str>,
    path: &std::path::Path,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let outer_key = match host {
        McpHost::ClaudeDesktop | McpHost::Cursor => "mcpServers",
        McpHost::Zed => "context_servers",
    };

    let mut root: serde_json::Value = if path.exists() {
        let bak = path.with_extension("json.unhosted.bak");
        if !bak.exists() {
            std::fs::copy(path, &bak)
                .with_context(|| format!("backing up {} -> {}", path.display(), bak.display()))?;
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if text.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        }
    } else {
        serde_json::json!({})
    };

    if !root.is_object() {
        anyhow::bail!(
            "{} is not a JSON object — refusing to overwrite",
            path.display()
        );
    }
    let obj = root.as_object_mut().expect("checked above");
    let stanza = build_mcp_config(host, daemon_url, bearer);
    let stanza_obj = stanza
        .as_object()
        .expect("build_mcp_config always returns an object")
        .clone();
    // Get/insert the top-level key as an object.
    let bucket = obj
        .entry(outer_key.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !bucket.is_object() {
        anyhow::bail!(
            "{} has a non-object '{}' field — refusing to overwrite",
            path.display(),
            outer_key
        );
    }
    let bucket_obj = bucket.as_object_mut().expect("checked above");
    // Merge: we own only the "unhosted" sub-key.
    if let Some(serde_json::Value::Object(inner)) = stanza_obj.get(outer_key) {
        if let Some(unhosted_entry) = inner.get("unhosted") {
            bucket_obj.insert("unhosted".to_string(), unhosted_entry.clone());
        }
    }

    let tmp = path.with_extension("json.unhosted.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// ─── distillation recipe ──────────────────────────────────────────────
// Shells out to the Python scripts in `models/distill/`. Keeping these
// Python rather than reimplementing in Rust on purpose: the ML
// ecosystem (transformers, trl, peft, bitsandbytes) only exists in
// Python, and rewriting the SFT loop in Rust would be a months-long
// project that delivers nothing the user couldn't already do with the
// same scripts.

fn run_distill(action: DistillAction) -> Result<()> {
    let (script, extra) = match action {
        DistillAction::Data { args } => ("gen_data.py", args),
        DistillAction::Train { args } => ("train.py", args),
        DistillAction::Eval { args } => ("eval.py", args),
        DistillAction::Push { args } => ("push_to_hub.py", args),
        DistillAction::Run { args } => ("pipeline.py", args),
    };
    let script_path = locate_distill_script(script)?;
    let python = python_executable();
    let status = std::process::Command::new(&python)
        .arg(&script_path)
        .args(&extra)
        .status()
        .with_context(|| {
            format!(
                "failed to exec `{} {}` — is Python ≥ 3.10 installed and on PATH?",
                python,
                script_path.display()
            )
        })?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn python_executable() -> String {
    // Respect $UNHOSTED_PYTHON, then $PYTHON, then default to python3.
    // venv users will set $UNHOSTED_PYTHON to their venv's interpreter.
    if let Ok(v) = std::env::var("UNHOSTED_PYTHON") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Ok(v) = std::env::var("PYTHON") {
        if !v.is_empty() {
            return v;
        }
    }
    "python3".to_string()
}

/// Find a distillation script across a few sensible locations:
///   1. $UNHOSTED_DISTILL_DIR/<script>   — explicit override
///   2. ./models/distill/<script>        — running from the repo
///   3. <exe-dir>/../share/unhosted/distill/<script> — installed layout
fn locate_distill_script(name: &str) -> Result<PathBuf> {
    let candidates: Vec<PathBuf> = std::iter::empty::<PathBuf>()
        .chain(
            std::env::var("UNHOSTED_DISTILL_DIR")
                .ok()
                .map(|d| PathBuf::from(d).join(name)),
        )
        .chain(std::iter::once(PathBuf::from("models/distill").join(name)))
        .chain(
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_owned()))
                .map(|d| d.join("../share/unhosted/distill").join(name)),
        )
        .collect();
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "could not find {name}. Looked in: {:?}.\n\
         Set $UNHOSTED_DISTILL_DIR to the directory containing it, \
         or run from the repo root.",
        candidates
    )
}

/// Build `PeerCandidate` entries from the peer registry for the
/// names the user listed in `--peers`. Marks every resolved peer as
/// `rpc_capable: true` optimistically — the planner refuses with
/// `UnknownPeer` if a name doesn't exist in the registry, but it
/// trusts the user's assertion that the named peers have an
/// RPC-capable llama.cpp. The spawn supervisor (next slice) will
/// probe each peer's `/v1/status` before actually starting, which
/// is where we'll catch a peer that the user *thought* had RPC but
/// doesn't. For now, the plan output's `rpc-server cmd` block tells
/// the user exactly what each peer would need to run, so a
/// pre-flight check is at least possible by reading the printed
/// plan.
fn resolve_peer_candidates(requested: &[String]) -> Vec<unhosted_core::vram_pool::PeerCandidate> {
    use unhosted_core::vram_pool::PeerCandidate;
    let Ok(registry) = PeerRegistry::load() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(requested.len());
    for name in requested {
        if let Some(p) = registry.peers.iter().find(|p| &p.name == name) {
            out.push(PeerCandidate {
                name: p.name.clone(),
                addr: p.addr,
                rpc_capable: true,
            });
        }
        // Names not in the registry are left out — the planner will
        // surface `UnknownPeer(name)` and the CLI exits 1.
    }
    out
}

async fn run_vram_pool(action: VramPoolAction) -> Result<()> {
    use unhosted_core::vram_pool;

    let cap = vram_pool::probe();

    let print_capability = |c: &vram_pool::RpcCapability| {
        println!("VRAM-pooling capability on this machine:");
        println!(
            "  llama-server        : {}",
            c.llama_server_path
                .as_deref()
                .unwrap_or("(not found on PATH)")
        );
        println!(
            "  llama-server --rpc  : {}",
            if c.llama_server_has_rpc_flag {
                "yes"
            } else {
                "no — build lacks -DGGML_RPC=ON"
            }
        );
        println!(
            "  rpc-server          : {}",
            c.rpc_server_path
                .as_deref()
                .unwrap_or("(not found on PATH)")
        );
        println!(
            "  ready for pool      : {}",
            if c.ready() { "YES" } else { "no" }
        );
        println!();
        println!("hint:");
        for line in c.install_hint().split('\n') {
            println!("  {}", line.trim());
        }
    };

    match action {
        VramPoolAction::Detect => {
            print_capability(&cap);
        }
        VramPoolAction::Fit {
            model,
            size_gb,
            headroom_gb,
        } => {
            vram_pool_fit(model.as_deref(), size_gb, headroom_gb)?;
        }
        VramPoolAction::Start { model, peers } => {
            // 1. Build the plan locally so a bad request fails before
            //    any HTTP round-trip.
            let candidates = resolve_peer_candidates(&peers);
            let local_capable = cap.ready();
            let plan = match vram_pool::plan(local_capable, &candidates, &peers, model) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("vram-pool: cannot build a plan: {e}");
                    eprintln!();
                    print_capability(&cap);
                    std::process::exit(1);
                }
            };
            // 2. Show the user what we're about to do (same preview
            //    block as before), then POST to the daemon to actually
            //    spawn the children.
            println!("VRAM-pool plan:");
            println!();
            println!("  orchestrator       : {}", plan.orchestrator);
            println!("  model              : {}", plan.model);
            println!("  layer hosts        :");
            for h in &plan.layer_hosts {
                println!("    - {:<12} @ {}", h.name, h.addr);
            }
            println!();
            println!("posting to local daemon at http://{DEFAULT_NODE_ADDR}/v1/vram-pool/start …");
            let body = serde_json::json!({ "plan": plan });
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("http://{DEFAULT_NODE_ADDR}/v1/vram-pool/start"))
                .json(&body)
                .send()
                .await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    let s: serde_json::Value = r.json().await.unwrap_or_default();
                    println!("daemon accepted. current state:");
                    println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
                }
                Ok(r) => {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    eprintln!("daemon rejected (HTTP {status}):");
                    eprintln!("{body}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!(
                        "could not reach the local daemon at http://{DEFAULT_NODE_ADDR}: {e}"
                    );
                    eprintln!("is `unhosted serve` running, or is the .app open?");
                    std::process::exit(1);
                }
            }
        }
        VramPoolAction::Stop => {
            let client = reqwest::Client::new();
            match client
                .post(format!("http://{DEFAULT_NODE_ADDR}/v1/vram-pool/stop"))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    println!("vram-pool stopped.");
                }
                Ok(r) => {
                    eprintln!("daemon returned HTTP {}", r.status());
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("could not reach the local daemon: {e}");
                    std::process::exit(1);
                }
            }
        }
        VramPoolAction::Status => {
            print_capability(&cap);
            println!();
            let client = reqwest::Client::new();
            match client
                .get(format!("http://{DEFAULT_NODE_ADDR}/v1/vram-pool"))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let s: serde_json::Value = r.json().await.unwrap_or_default();
                    println!("pool state:");
                    println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
                }
                Ok(_) | Err(_) => {
                    println!(
                        "(orchestration status unavailable — daemon not reachable at http://{DEFAULT_NODE_ADDR})"
                    );
                }
            }
            return Ok(());
        }
    }
    Ok(())
}

/// Advise whether a model fits in local memory or wants a VRAM pool.
///
/// This is deliberately a heuristic. The real footprint depends on
/// quantization, context length, and KV-cache growth — none of which we
/// can know from a file size alone. We estimate the *weights* footprint
/// from the model size and add a modest runtime overhead factor, then
/// compare to total RAM minus headroom. The output is guidance, framed
/// as such, not a guarantee.
fn vram_pool_fit(model: Option<&str>, size_gb: Option<f64>, headroom_gb: f64) -> Result<()> {
    const BYTES_PER_GB: f64 = 1_073_741_824.0;
    // Weights + a runtime overhead allowance (activations, KV-cache at a
    // modest context). 1.3x is a rough, slightly-conservative middle.
    const RUNTIME_OVERHEAD: f64 = 1.3;

    // 1. Determine the model's on-disk size in GB.
    let (model_gb, source) = if let Some(g) = size_gb {
        (g, "from --size-gb".to_string())
    } else if let Some(m) = model {
        estimate_model_size_gb(m)?
    } else {
        anyhow::bail!(
            "vram-pool fit needs a model: pass --model <name|path|url> or --size-gb <n>."
        );
    };

    let need_gb = model_gb * RUNTIME_OVERHEAD;

    // 2. Read total system RAM.
    let total_gb = match total_ram_bytes() {
        Some(b) => b as f64 / BYTES_PER_GB,
        None => {
            anyhow::bail!("could not read this machine's total memory on this platform.");
        }
    };
    let usable_gb = (total_gb - headroom_gb).max(0.0);

    // 3. Report.
    println!("vram-pool fit");
    println!("  model size       : {model_gb:.1} GB ({source})");
    println!("  est. memory need : {need_gb:.1} GB (weights x{RUNTIME_OVERHEAD} runtime overhead)");
    println!("  this machine RAM : {total_gb:.1} GB total, {usable_gb:.1} GB usable (after {headroom_gb:.0} GB headroom)");
    println!();

    if need_gb <= usable_gb {
        println!("  ✅ fits locally — no pool needed.");
        println!("     run it directly (e.g. `unhosted pull` + serve, or load it in LM Studio).");
    } else {
        // How many *additional* equally-sized peers would cover the gap.
        // Each peer contributes roughly `usable_gb` (assume similar machines;
        // a real planner would use per-peer probed memory — not built yet).
        let deficit = need_gb - usable_gb;
        let extra_peers = if usable_gb > 0.0 {
            (deficit / usable_gb).ceil() as u64
        } else {
            // No usable local memory at all — can't anchor an estimate.
            0
        };
        println!("  ⚠️  does NOT fit locally — needs a VRAM pool.");
        if extra_peers > 0 {
            println!(
                "     roughly {extra_peers} more peer(s) of similar memory would cover the {deficit:.1} GB gap."
            );
            println!("     (heuristic — assumes peers ~as capable as this machine; per-peer");
            println!("      memory probing is not implemented yet, so treat as a ballpark.)");
        } else {
            println!("     this machine has no usable memory headroom to anchor an estimate;");
            println!("     enlist peers and try `unhosted vram-pool start --model <m> --peers <…>`.");
        }
        println!();
        println!("     next: `unhosted vram-pool detect` to confirm RPC capability, then");
        println!("           `unhosted vram-pool start --model <m> --peers <names>`.");
    }
    Ok(())
}

/// Estimate a model's on-disk size in GB from a name, path, or URL.
/// Order: a local .gguf path -> a cached file matching the name ->
/// a known entry in MODELS. Returns (size_gb, human-readable source).
fn estimate_model_size_gb(model: &str) -> Result<(f64, String)> {
    const BYTES_PER_GB: f64 = 1_073_741_824.0;

    // A direct path to a .gguf on disk.
    let p = std::path::Path::new(model);
    if p.is_file() {
        let bytes = std::fs::metadata(p)
            .with_context(|| format!("reading {model}"))?
            .len();
        return Ok((bytes as f64 / BYTES_PER_GB, format!("from file {model}")));
    }

    // A known model in the built-in table.
    if let Some((_, _, size)) = MODELS.iter().find(|(n, _, _)| *n == model) {
        if *size > 0 {
            return Ok((*size as f64 / BYTES_PER_GB, format!("known model {model}")));
        }
    }

    // A cached GGUF whose filename contains the requested name.
    if let Ok(cache) = model_cache_dir() {
        if cache.exists() {
            if let Ok(entries) = std::fs::read_dir(&cache) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".gguf")
                        && name.to_lowercase().contains(&model.to_lowercase())
                    {
                        if let Ok(meta) = e.metadata() {
                            return Ok((
                                meta.len() as f64 / BYTES_PER_GB,
                                format!("cached file {name}"),
                            ));
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!(
        "couldn't determine the size of '{model}'. Pass --size-gb <n>, a path to a \
         local .gguf, or a known model name (`unhosted models`)."
    )
}

/// Total physical RAM in bytes, read from the OS without extra crates.
/// macOS: `sysctl -n hw.memsize`. Linux: `/proc/meminfo` MemTotal.
/// Returns None on unsupported platforms or read failure.
fn total_ram_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        return String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok();
    }
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                // Format: "MemTotal:       32788516 kB"
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb * 1024);
            }
        }
        return None;
    }
    #[allow(unreachable_code)]
    None
}

async fn run_doctor() -> Result<()> {
    use unhosted_core::upstream;

    let configured = std::env::var("UNHOSTED_LLAMA_SERVER_URL")
        .unwrap_or_else(|_| upstream::LLAMA_SERVER_DEFAULT_URL.to_string());

    println!("unhosted doctor");
    println!();
    println!("  configured upstream: {configured}");
    let configured_ok = upstream::probe_configured(&configured).await;
    println!(
        "    {}",
        if configured_ok {
            "ok — responds on /v1/models or /health"
        } else {
            "absent — nothing responded"
        }
    );
    println!();

    println!("  scanning known backends on localhost defaults:");
    let report = upstream::probe_all().await;
    for r in &report.results {
        let mark = if r.reachable { "ok    " } else { "absent" };
        println!("    [{}] {:<13} {}", mark, r.backend.name(), r.url);
    }
    println!();

    if configured_ok {
        println!("looks good — your daemon will proxy to {configured}.");
        return Ok(());
    }

    if let Some(found) = report.first_reachable() {
        println!("a local backend is running on a different port.");
        println!("set the upstream env var, then re-run `unhosted serve`:");
        println!();
        println!("  UNHOSTED_LLAMA_SERVER_URL={} unhosted serve", found.url);
        return Ok(());
    }

    println!("{}", upstream::install_hints());
    Ok(())
}

async fn handle_quic_ping(node: &str, peer: &str) -> Result<()> {
    let url = format!("{}/v1/quic/ping", node.trim_end_matches('/'));
    let body = serde_json::json!({ "peer": peer });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let resp = client.post(&url).json(&body).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("quic-ping request failed ({status}): {text}");
    }
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let target = parsed
        .get("target_addr")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let rtt = parsed
        .get("rtt_ms")
        .and_then(|v| v.as_u64())
        .map(|v| format!("{v} ms"))
        .unwrap_or_else(|| "-".into());
    let error = parsed.get("error").and_then(|v| v.as_str());

    println!("ok:         {ok}");
    println!("target:     {target}");
    println!("rtt:        {rtt}");
    if let Some(err) = error {
        println!("error:      {err}");
    }
    Ok(())
}

async fn handle_punch(node: &str, peer: &str, timeout_secs: u64) -> Result<()> {
    let url = format!("{}/v1/punch", node.trim_end_matches('/'));
    let body = serde_json::json!({
        "peer": peer,
        "timeout_secs": timeout_secs,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs + 5))
        .build()?;
    let resp = client.post(&url).json(&body).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("punch request failed ({status}): {text}");
    }
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let coordinated = parsed
        .get("coordinated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let bidirectional = parsed
        .get("bidirectional")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let peer_addr = parsed
        .get("peer_addr")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let local_port = parsed
        .get("local_port")
        .and_then(|v| v.as_u64())
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".into());
    let error = parsed.get("error").and_then(|v| v.as_str());

    println!("coordinated:   {coordinated}");
    println!("bidirectional: {bidirectional}");
    println!("peer_addr:     {peer_addr}");
    println!("local_port:    {local_port}");
    if let Some(err) = error {
        println!("error:         {err}");
    }
    if coordinated && !bidirectional {
        println!();
        println!("relay matched both sides but no UDP traffic arrived —");
        println!("at least one NAT looks symmetric. Relay fallback still works.");
    }
    Ok(())
}

async fn handle_pair(action: PairAction) -> Result<()> {
    match action {
        PairAction::Offer { node } => {
            let url = format!("{}/v1/pair/offer", node.trim_end_matches('/'));
            let client = reqwest::Client::new();
            let resp: serde_json::Value = client
                .post(&url)
                .send()
                .await
                .with_context(|| format!("requesting offer from {url}"))?
                .json()
                .await
                .context("parsing offer response")?;

            let offer = resp.get("offer").and_then(|v| v.as_str()).unwrap_or("");
            let ttl = resp
                .get("expires_in_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            println!();
            println!("share this with the other node's owner:");
            println!();
            println!("    {offer}");
            println!();
            println!("they should run on their device:");
            println!("    unhosted pair accept \"{offer}\"");
            println!();
            println!("expires in {ttl} seconds.");
        }
        PairAction::Accept { offer, node } => {
            let parsed =
                parse_offer(&offer).with_context(|| format!("parsing offer URI: {offer}"))?;

            // Ask the target daemon what its identity is — over HTTP,
            // not by loading our process's own identity.toml. The CLI's
            // XDG_CONFIG_HOME may not match the daemon's (loopback
            // multi-daemon test setups, deployments where the CLI runs
            // as a different user than the daemon, etc.). The
            // /v1/identity endpoint is the source of truth.
            let client = reqwest::Client::new();
            let identity_url = format!("{}/v1/identity", node.trim_end_matches('/'));
            let identity: serde_json::Value = client
                .get(&identity_url)
                .send()
                .await
                .with_context(|| format!("reading local daemon identity at {identity_url}"))?
                .error_for_status()
                .with_context(|| format!("daemon at {identity_url} returned an error"))?
                .json()
                .await?;
            let my_pubkey = identity
                .get("pubkey")
                .and_then(|v| v.as_str())
                .context("local daemon /v1/identity missing pubkey")?
                .to_string();
            let my_name = identity
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(unhosted_core::default_node_name);
            // Parse local node's listen address from --node so we know what
            // address to give the offerer for return-routing.
            let my_addr = derive_listen_addr(&node)?;

            // 1. Tell the OFFERER to accept us — they validate the token and
            //    store us as a trusted peer with our real pubkey.
            let accept_url = format!("http://{}/v1/pair/accept", parsed.addr);
            let body = serde_json::json!({
                "token": parsed.token,
                "peer_name": my_name,
                "peer_pubkey": my_pubkey,
                "peer_addr": my_addr.to_string(),
            });

            let resp = client
                .post(&accept_url)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("contacting offerer at {accept_url}"))?;

            if !resp.status().is_success() {
                anyhow::bail!(
                    "offerer rejected pairing: HTTP {} — token expired or invalid?",
                    resp.status()
                );
            }
            let confirmation: serde_json::Value = resp.json().await?;
            let their_name = confirmation
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            let their_pubkey = confirmation
                .get("pubkey")
                .and_then(|v| v.as_str())
                .context("offerer response missing pubkey")?;
            let their_addr = confirmation
                .get("addr")
                .and_then(|v| v.as_str())
                .context("offerer response missing addr")?
                .parse::<SocketAddr>()
                .context("offerer response addr not parseable")?;

            // 2. Register the offerer as a trusted peer in OUR daemon's
            //    registry — including their pubkey in the same call, so
            //    we don't have to patch peers.toml on disk afterwards
            //    (which broke when the CLI's XDG_CONFIG_HOME differed
            //    from the daemon's).
            let local_url = format!("{}/v1/peers", node.trim_end_matches('/'));
            let resp = client
                .post(&local_url)
                .json(&serde_json::json!({
                    "name": their_name,
                    "addr": their_addr.to_string(),
                    "priority": 5,
                    "pubkey": their_pubkey,
                }))
                .send()
                .await
                .with_context(|| format!("registering trusted peer at {local_url}"))?;
            if !resp.status().is_success() {
                anyhow::bail!("local daemon rejected peer add: HTTP {}", resp.status());
            }

            println!();
            println!("paired with {their_name} ({their_addr})");
            println!("their pubkey: {their_pubkey}");
            println!("ours:         {my_pubkey}");
            println!();
            println!("both sides now treat each other as trusted peers.");
        }
    }
    Ok(())
}

struct ParsedOffer {
    addr: SocketAddr,
    token: String,
}

fn parse_offer(s: &str) -> Result<ParsedOffer> {
    let s = s.trim();
    let rest = s
        .strip_prefix("unhosted://pair?")
        .or_else(|| s.strip_prefix("unhosted://pair/"))
        .context("offer must start with 'unhosted://pair?'")?;

    let mut addr: Option<String> = None;
    let mut token: Option<String> = None;
    for kv in rest.split('&') {
        let mut it = kv.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some("addr"), Some(v)) => addr = Some(v.to_string()),
            (Some("token"), Some(v)) => token = Some(v.to_string()),
            _ => {}
        }
    }
    let addr = addr.context("offer missing addr= parameter")?;
    let token = token.context("offer missing token= parameter")?;
    Ok(ParsedOffer {
        addr: addr
            .parse()
            .context("offer addr is not a valid host:port")?,
        token,
    })
}

/// Derive the daemon's *listen* address from a `http://host:port` URL the
/// CLI uses to talk to it. The pairing payload sends this to the other
/// node so they can route back to us.
fn derive_listen_addr(node_url: &str) -> Result<SocketAddr> {
    let trimmed = node_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    trimmed
        .parse::<SocketAddr>()
        .with_context(|| format!("could not parse listen addr from {node_url}"))
}

// ----- model management -----------------------------------------------------

fn model_cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME env var not set")?;
    Ok(PathBuf::from(home)
        .join(".cache")
        .join("unhosted")
        .join("models"))
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", size, UNITS[unit])
}

fn list_models() -> Result<()> {
    let cache = model_cache_dir()?;
    let cached_files: Vec<String> = if cache.exists() {
        std::fs::read_dir(&cache)?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".gguf"))
            .collect()
    } else {
        vec![]
    };

    println!("known models (pull with `unhosted pull <name>`):");
    println!();
    println!("  {:<20} {:>8}  CACHED", "NAME", "SIZE");
    for (name, url, size) in MODELS {
        let filename = url.rsplit('/').next().unwrap_or("");
        let cached = if cached_files.iter().any(|f| f == filename) {
            "yes"
        } else {
            "no"
        };
        println!("  {:<20} {:>8}  {}", name, human_size(*size), cached);
    }

    println!();
    println!("cache dir: {}", cache.display());
    Ok(())
}

async fn seed_status() -> Result<()> {
    // Prefer the running daemon: it can report live seeding *activity*
    // (chunks served, last-served time, peer pulls) that a cold cache
    // scan can't. Fall back to a local hash-scan when the daemon's down
    // — then we can only show what's *capable* of being seeded.
    if let Some(()) = seed_status_via_daemon().await {
        return Ok(());
    }

    let cache = model_cache_dir()?;
    if !cache.exists() {
        println!("no models cached yet — nothing to seed.");
        println!("cache dir: {}", cache.display());
        return Ok(());
    }
    eprintln!("(daemon not reachable — showing local library only; start the daemon to see live seeding activity)");
    eprintln!("hashing cached models (this can take a moment on a large library)…");
    let seedable = unhosted_core::swarm::seedable_models_in(&cache);
    print_seedable(&seedable, None);
    println!("cache dir: {}", cache.display());
    Ok(())
}

/// Query the daemon's `/v1/models/seedable`. Returns `Some(())` when the
/// daemon answered (we printed the full view), `None` when it's
/// unreachable so the caller falls back to the local scan.
async fn seed_status_via_daemon() -> Option<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .ok()?;
    let resp = client
        .get(format!("http://{DEFAULT_NODE_ADDR}/v1/models/seedable"))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;

    let models: Vec<unhosted_core::swarm::SeedableModel> =
        serde_json::from_value(v.get("models")?.clone()).ok()?;
    print_seedable(&models, v.get("activity"));
    Some(())
}

/// Print the seedable-models list and, when present, the live activity
/// counters that answer "is it actually seeding right now?".
fn print_seedable(
    seedable: &[unhosted_core::swarm::SeedableModel],
    activity: Option<&serde_json::Value>,
) {
    if seedable.is_empty() {
        println!("no seedable models on this node yet.");
    } else {
        println!(
            "{} model{} seedable on this node:",
            seedable.len(),
            if seedable.len() == 1 { "" } else { "s" }
        );
        println!();
        for m in seedable {
            // Short digest prefix — enough to eyeball-match against a peer.
            let short = m.digest.get(..18).unwrap_or(&m.digest);
            println!(
                "  {:<40} {:<20} {:>9}",
                m.file,
                format!("{short}…"),
                human_size(m.size_bytes)
            );
        }
    }
    println!();

    if let Some(a) = activity {
        let chunks = a.get("chunks_served").and_then(|x| x.as_u64()).unwrap_or(0);
        let bytes = a.get("bytes_served").and_then(|x| x.as_u64()).unwrap_or(0);
        let pulls = a.get("peer_pulls").and_then(|x| x.as_u64()).unwrap_or(0);
        let last = a
            .get("last_served_unix")
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        println!("seeding activity (since daemon start):");
        println!("  chunks served to peers : {chunks}");
        println!("  bytes served           : {}", human_size(bytes));
        println!("  models pulled from peers: {pulls}");
        if last == 0 {
            println!("  last served            : never — no peer has pulled from this node yet");
        } else {
            println!("  last served            : {}", fmt_unix_ago(last));
        }
        println!();
    }
    println!("peers on your network can pull these without re-downloading from the origin.");
}

/// Render a unix timestamp as a short "Ns/m/h ago" relative string.
fn fmt_unix_ago(unix: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - unix).max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

async fn pull_model(spec: &str) -> Result<()> {
    let (url, approx_size) = if spec.starts_with("http://") || spec.starts_with("https://") {
        (spec.to_string(), 0u64)
    } else {
        MODELS
            .iter()
            .find(|(n, _, _)| *n == spec)
            .map(|(_, u, s)| (u.to_string(), *s))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown model '{}'. try one of:\n  {}\nor pass a full https://… URL.",
                    spec,
                    MODELS
                        .iter()
                        .map(|(n, _, _)| *n)
                        .collect::<Vec<_>>()
                        .join("\n  ")
                )
            })?
    };

    let cache = model_cache_dir()?;
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;

    let filename = url
        .rsplit('/')
        .next()
        .filter(|f| !f.is_empty())
        .context("could not derive filename from URL")?;
    let dest = cache.join(filename);
    let tmp = cache.join(format!("{filename}.tmp"));

    if dest.exists() {
        println!("already cached: {}", dest.display());
        print_run_hint(&dest);
        return Ok(());
    }

    println!("pulling {}", spec);
    if approx_size > 0 {
        println!("  ~{} from {}", human_size(approx_size), url);
    } else {
        println!("  from {}", url);
    }
    println!("  to   {}", dest.display());
    println!();

    let resp = reqwest::get(&url).await.context("HTTP request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", resp.status());
    }
    let total = resp.content_length().unwrap_or(approx_size);

    let mut file = tokio::fs::File::create(&tmp)
        .await
        .with_context(|| format!("creating {}", tmp.display()))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_print = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading download stream")?;
        file.write_all(&chunk).await.context("writing to disk")?;
        downloaded += chunk.len() as u64;

        // Print at most every ~10 MB or every 1% of total.
        let step = total
            .checked_div(100)
            .map(|s| s.max(1_000_000))
            .unwrap_or(10_000_000);
        if downloaded - last_print >= step {
            last_print = downloaded;
            match downloaded
                .checked_mul(100)
                .and_then(|v| v.checked_div(total))
            {
                Some(pct) => eprint!(
                    "\r  {pct:>3}%  {} / {}            ",
                    human_size(downloaded),
                    human_size(total)
                ),
                None => eprint!("\r  {} downloaded            ", human_size(downloaded)),
            }
            let _ = std::io::stderr().flush();
        }
    }
    file.flush().await?;
    drop(file);
    eprintln!();

    tokio::fs::rename(&tmp, &dest)
        .await
        .with_context(|| format!("renaming {} → {}", tmp.display(), dest.display()))?;

    println!();
    println!("done. cached at {}", dest.display());
    print_run_hint(&dest);
    Ok(())
}

fn print_run_hint(path: &std::path::Path) {
    println!();
    println!("next:");
    println!("  start llama-server with this model:");
    println!(
        "    llama-server -m {} --port 8080 -c 2048 -ngl 99",
        path.display()
    );
    println!("  then in another terminal:");
    println!("    unhosted serve");
}

fn handle_peer(action: PeerAction) -> Result<()> {
    let mut registry = PeerRegistry::load().context("loading peer registry")?;
    match action {
        PeerAction::List => {
            if registry.peers.is_empty() {
                println!("no peers configured. add one with `unhosted peer add <name> <addr>`.");
                return Ok(());
            }
            println!("{:<16} {:<24} {:<10} MODELS", "NAME", "ADDRESS", "PRIORITY");
            for peer in registry.by_priority() {
                let models = if peer.models.is_empty() {
                    "(any)".to_string()
                } else {
                    peer.models.join(", ")
                };
                println!(
                    "{:<16} {:<24} {:<10} {}",
                    peer.name, peer.addr, peer.priority, models
                );
            }
        }
        PeerAction::Add {
            name,
            addr,
            priority,
        } => {
            registry.add(Peer {
                name: name.clone(),
                addr,
                priority,
                models: vec![],
                pubkey: None,
            })?;
            println!("peer added: {name} @ {addr} (priority {priority})");
            println!("config: {}", PeerRegistry::config_path()?.display());
            println!("restart `unhosted serve` to include this peer in the routing rotation.");
        }
        PeerAction::Remove { name } => {
            if registry.remove(&name)? {
                println!("peer removed: {name}");
            } else {
                anyhow::bail!("no peer named '{name}' is configured");
            }
        }
    }
    Ok(())
}

async fn run_prompt(node_url: &str, prompt: &str, max_tokens: u32) -> Result<()> {
    let url = format!("{}/v1/run", node_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "prompt": prompt,
            "max_tokens": max_tokens,
        }))
        .send()
        .await
        .with_context(|| format!("connecting to node at {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("node returned {}", resp.status());
    }

    let mut stream = resp.bytes_stream();
    let mut stdout = std::io::stdout();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading stream")?;
        stdout.write_all(&chunk)?;
        stdout.flush()?;
    }
    println!();
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{filter::EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("unhosted_core=info,unhosted_cli=info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

// ─── agent ────────────────────────────────────────────────────────
// Drive an agent run from the CLI. Posts to /v1/agents/run, then
// either pretty-prints the step trace or dumps the raw JSON. Same
// auth model as any other daemon request — loopback bypasses the
// bearer; off-loopback uses ~/.config/unhosted/local-token.txt
// (the standard CLI auth path the daemon already supports for
// the desktop shell).

async fn run_agent_action(action: AgentAction) -> Result<()> {
    match action {
        AgentAction::Run {
            goal,
            tools,
            max_steps,
            max_tokens,
            max_seconds,
            node,
            model,
            json,
        } => {
            run_agent_remote(
                goal,
                tools,
                max_steps,
                max_tokens,
                max_seconds,
                &node,
                &model,
                json,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_remote(
    goal: String,
    tools: Vec<String>,
    max_steps: u32,
    max_tokens: u32,
    max_seconds: u32,
    node: &str,
    model: &str,
    json: bool,
) -> Result<()> {
    let body = serde_json::json!({
        "goal": goal,
        "tools": tools,
        "max_steps": max_steps,
        "max_tokens": max_tokens,
        "max_seconds": max_seconds,
        "model": model,
    });
    let url = format!("{}/v1/agents/run", node.trim_end_matches('/'));

    // Echo what we're about to do before the request so a user
    // watching the terminal sees something even before the daemon
    // responds. Looks like the existing `unhosted run` echo line.
    eprintln!("agent run → {url}");
    eprintln!("  goal:    {}", trim_for_display(&goal, 120));
    eprintln!(
        "  tools:   {}",
        if tools.is_empty() {
            "(none)".to_string()
        } else {
            tools.join(", ")
        }
    );
    eprintln!(
        "  caps:    {max_steps} steps · {max_tokens} tokens · {max_seconds}s · model={model}"
    );
    eprintln!();

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("daemon returned {status}: {text}");
    }
    let raw: serde_json::Value = resp
        .json()
        .await
        .context("parsing /v1/agents/run response")?;
    if json {
        // Raw JSON mode: pipeable into jq.
        let pretty = serde_json::to_string_pretty(&raw)?;
        println!("{pretty}");
        return Ok(());
    }
    print_agent_run_trace(&raw);
    // Exit non-zero if the run didn't reach a final answer. Lets
    // shell scripts detect failure without parsing the JSON.
    let stopped = raw
        .get("stopped_because")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if stopped != "final_answer" {
        std::process::exit(2);
    }
    Ok(())
}

fn print_agent_run_trace(resp: &serde_json::Value) {
    let run_id = resp.get("run_id").and_then(|v| v.as_str()).unwrap_or("?");
    let stopped = resp
        .get("stopped_because")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let tokens = resp
        .get("tokens_used")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let steps = resp.get("steps").and_then(|v| v.as_array());

    if let Some(steps) = steps {
        for step in steps {
            let kind = step.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
            let step_n = step.get("step").and_then(|v| v.as_u64()).unwrap_or(0);
            match kind {
                "model_message" => {
                    let content = step.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let tcm = step
                        .get("tool_calls_made")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if content.is_empty() {
                        println!(
                            "  [step {step_n}] model: ({tcm} tool call{})",
                            if tcm == 1 { "" } else { "s" }
                        );
                    } else {
                        println!(
                            "  [step {step_n}] model: {}",
                            trim_for_display(content, 200)
                        );
                    }
                }
                "tool_call" => {
                    let tool = step.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                    let args_hash = step.get("args_hash").and_then(|v| v.as_str()).unwrap_or("");
                    let chars = step
                        .get("result_chars")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let err = step.get("error").and_then(|v| v.as_str());
                    match err {
                        Some(e) => println!(
                            "  [step {step_n}] tool {tool}  args#{args_hash}  ERROR  {}",
                            trim_for_display(e, 120)
                        ),
                        None => println!(
                            "  [step {step_n}] tool {tool}  args#{args_hash}  {chars} chars  ok"
                        ),
                    }
                }
                other => {
                    println!("  [step {step_n}] {other}");
                }
            }
        }
    }
    println!();
    println!("stopped: {stopped}   tokens: {tokens}   run: {run_id}");
    println!();
    let final_answer = resp
        .get("final_answer")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !final_answer.is_empty() {
        println!("{final_answer}");
    }
}

fn trim_for_display(s: &str, max: usize) -> String {
    let one_line = s.replace(['\n', '\r'], " ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let truncated: String = one_line.chars().take(max).collect();
        format!("{truncated}…")
    }
}

// ─── upgrade ──────────────────────────────────────────────────────────
// `unhosted upgrade` re-runs the official install script. Desktop
// users get an auto-update prompt via tauri-plugin-updater; this
// command is for CLI installs (curl install.sh | sh). It's a thin
// wrapper that:
//   1. (unless --force) calls `unhosted_core::update_check::check`
//      to see if a newer release exists. If we're already current,
//      print and bail without touching disk.
//   2. Detects platform — Windows ⇒ install.ps1, otherwise install.sh.
//   3. Pipes curl|sh (or iwr|iex on Windows) inheriting stdio so
//      the user sees the install script's own progress.
//
// We deliberately don't try to do an in-process self-rewrite —
// macOS code-signing makes that brittle (the freshly-replaced
// binary needs ad-hoc signing before it'll run), and the install
// script already handles every edge case the original install
// went through.

const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.sh";
const INSTALL_PS1_URL: &str =
    "https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.ps1";

async fn run_upgrade(force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("current version: {current}");

    if !force {
        let http = reqwest::Client::new();
        match unhosted_core::update_check::check(&http).await {
            Ok(Some(latest)) => {
                println!("latest published: v{latest} — upgrading");
            }
            Ok(None) => {
                println!("already on the latest published release.");
                println!("re-run with --force to reinstall the same version.");
                return Ok(());
            }
            Err(e) => {
                // Don't refuse to upgrade just because the version
                // check failed (offline, rate-limited, etc). Surface
                // the warning and let the user decide via --force,
                // but in the common case (intermittent network), the
                // install script itself will retry.
                eprintln!(
                    "warning: could not reach GitHub releases ({e}). \
                     re-run with --force to install anyway."
                );
                return Ok(());
            }
        }
    }

    if cfg!(target_os = "windows") {
        // PowerShell: irm <url> | iex. We invoke powershell directly
        // so the user doesn't have to be in a powershell prompt.
        let cmd = format!("irm {INSTALL_PS1_URL} | iex");
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &cmd])
            .status()
            .context("running powershell install.ps1")?;
        if !status.success() {
            anyhow::bail!("install.ps1 failed (exit {:?})", status.code());
        }
    } else {
        // curl -fsSL <url> | sh. Same one-liner as the README.
        //
        // We pass UNHOSTED_INSTALL_DIR through to the install script
        // pointing at the directory the CURRENT unhosted binary lives
        // in, so a user with `~/.local/bin/unhosted` keeps that layout
        // and doesn't get prompted for sudo to install over /usr/local.
        // Without this, install.sh's default of /usr/local/bin would
        // require a terminal sudo prompt that `unhosted upgrade`
        // can't satisfy.
        let install_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let cmd = format!("curl -fsSL {INSTALL_SH_URL} | sh");
        let mut child = std::process::Command::new("sh");
        child.args(["-c", &cmd]);
        if let Some(dir) = install_dir.as_ref() {
            // Only set the override if the dir is actually writable
            // by us. If we're at /usr/local/bin (root-owned), let
            // the script's existing sudo-prompt path run.
            let writable = std::fs::metadata(dir)
                .map(|m| !m.permissions().readonly())
                .unwrap_or(false);
            if writable {
                println!(
                    "install dir: {} (matching current binary location)",
                    dir.display()
                );
                child.env("UNHOSTED_INSTALL_DIR", dir);
            }
        }
        let status = child.status().context("running install.sh")?;
        if !status.success() {
            anyhow::bail!("install.sh failed (exit {:?})", status.code());
        }
    }

    println!();
    println!("upgrade complete. restart any running `unhosted serve` to pick up the new binary.");
    Ok(())
}
