use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use orchestrator_core::ipc::default_socket_path;
use orchestrator_store::StatePaths;

#[derive(Debug, Parser)]
#[command(about = "Run the local software build orchestration engine")]
struct Arguments {
    /// Override the Unix-domain socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Override the application state directory.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    match arguments.state_dir {
        Some(state_directory) => {
            if !state_directory.is_absolute() {
                bail!("state directory must be absolute");
            }
            orchestrator_engine::serve_with_state(socket_path, StatePaths::new(state_directory))
                .await
        }
        None => orchestrator_engine::serve(socket_path).await,
    }
}
