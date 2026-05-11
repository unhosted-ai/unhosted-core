use std::io::Write;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use unhosted_core::{serve, Node, Peer, PeerRegistry, DEFAULT_LLAMA_SERVER_URL, DEFAULT_NODE_ADDR};

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
            let node = Node {
                addr,
                llama_server_url,
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
    }

    Ok(())
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
            println!(
                "note: v0.0.2 routing is not yet wired — peers are registered but inference still runs locally."
            );
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
