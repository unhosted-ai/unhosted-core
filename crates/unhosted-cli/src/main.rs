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
    }

    Ok(())
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

            // Need our own identity + name + addr to send to the offerer.
            let me = unhosted_core::Identity::load_or_create()?;
            let my_name = unhosted_core::default_node_name();
            // Parse local node's listen address from --node so we know what
            // address to give the offerer for return-routing.
            let my_addr = derive_listen_addr(&node)?;

            // 1. Tell the OFFERER to accept us — they validate the token and
            //    store us as a trusted peer.
            let accept_url = format!("http://{}/v1/pair/accept", parsed.addr);
            let body = serde_json::json!({
                "token": parsed.token,
                "peer_name": my_name,
                "peer_pubkey": me.public_b64(),
                "peer_addr": my_addr.to_string(),
            });

            let client = reqwest::Client::new();
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

            // 2. Now register them as a trusted peer locally too, via our
            //    own daemon's existing POST /v1/peers endpoint.
            let local_url = format!("{}/v1/peers", node.trim_end_matches('/'));
            let resp = client
                .post(&local_url)
                .json(&serde_json::json!({
                    "name": their_name,
                    "addr": their_addr.to_string(),
                    "priority": 5,
                }))
                .send()
                .await
                .with_context(|| format!("registering trusted peer at {local_url}"))?;
            if !resp.status().is_success() {
                anyhow::bail!("local daemon rejected peer add: HTTP {}", resp.status());
            }

            // 3. Decorate the local registry with the pubkey so requests
            //    will eventually carry signed-auth headers. (The daemon's
            //    /v1/peers endpoint doesn't accept pubkey yet, so we patch
            //    the on-disk file directly.)
            let mut reg = PeerRegistry::load()?;
            if let Some(p) = reg.peers.iter_mut().find(|p| p.name == their_name) {
                p.pubkey = Some(their_pubkey.to_string());
                reg.save()?;
            }

            println!();
            println!("paired with {their_name} ({their_addr})");
            println!("their pubkey: {their_pubkey}");
            println!("ours:         {}", me.public_b64());
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
