use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use codewhale_app_server::{AppServerOptions, run};

#[derive(Debug, Parser)]
#[command(
    name = "deepseek-app-server",
    about = "Run the DeepSeek app-server transport"
)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8787)]
    port: u16,
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let listen: SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .with_context(|| format!("invalid listen address {}:{}", cli.host, cli.port))?;
    run(AppServerOptions {
        listen,
        config_path: cli.config,
    })
    .await
}
