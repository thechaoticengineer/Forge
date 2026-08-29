use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use orchestrator_core::ipc::default_socket_path;

#[derive(Debug, Parser)]
#[command(about = "Run the local software build orchestration engine")]
struct Arguments {
    /// Override the Unix-domain socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    orchestrator_engine::serve(socket_path).await
}
