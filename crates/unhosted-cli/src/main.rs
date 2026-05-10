use std::io::Write;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use unhosted_core::{serve, Node, DEFAULT_LLAMA_SERVER_URL, DEFAULT_NODE_ADDR};

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
