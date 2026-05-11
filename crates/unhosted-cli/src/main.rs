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
        Command::Serve { addr, upstream } => {
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
            let node = Node {
                addr,
                llama_server_url,
                peers: registry.peers,
                name: default_node_name(),
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
    }

    Ok(())
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
