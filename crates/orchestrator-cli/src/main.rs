use std::{path::PathBuf, process};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use orchestrator_core::{
    ipc::default_socket_path,
    protocol::{ClientMessage, ClientRequest, PROTOCOL_VERSION, ServerMessage},
    state::EngineSnapshot,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::{Duration, timeout},
};

#[derive(Debug, Parser)]
#[command(about = "Inspect and control the local software build orchestrator")]
struct Arguments {
    /// Override the Unix-domain socket path.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check whether the engine is responsive.
    Ping,
    /// Show the current authoritative engine snapshot.
    Status {
        /// Print the snapshot as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    match arguments.command {
        Command::Ping => ping(&socket_path).await,
        Command::Status { json } => status(&socket_path, json).await,
    }
}

async fn connect(socket_path: &PathBuf) -> Result<UnixStream> {
    UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("cannot connect to engine at {}", socket_path.display()))
}

async fn status(socket_path: &PathBuf, json: bool) -> Result<()> {
    let stream = connect(socket_path).await?;
    let mut reader = BufReader::new(stream);
    let message = read_message(&mut reader).await?;
    let ServerMessage::Snapshot { snapshot, .. } = message else {
        bail!("engine did not send a state snapshot after connection");
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).context("cannot encode snapshot")?
        );
    } else {
        print_snapshot(&snapshot);
    }
    Ok(())
}

async fn ping(socket_path: &PathBuf) -> Result<()> {
    let stream = connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    let request_id = format!("cli-{}", process::id());
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request: ClientRequest::Ping,
    };
    let mut encoded = serde_json::to_vec(&request).context("cannot encode ping request")?;
    encoded.push(b'\n');
    write_half
        .write_all(&encoded)
        .await
        .context("cannot send ping request")?;

    let mut reader = BufReader::new(read_half);
    loop {
        match read_message(&mut reader).await? {
            ServerMessage::Pong {
                request_id: response_id,
                ..
            } if response_id == request_id => {
                println!("ok");
                return Ok(());
            }
            ServerMessage::Error { message, .. } => bail!("engine rejected ping: {message}"),
            _ => {}
        }
    }
}

async fn read_message<R>(reader: &mut R) -> Result<ServerMessage>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    let read = timeout(Duration::from_secs(3), reader.read_line(&mut line))
        .await
        .context("engine response timed out")?
        .context("cannot read engine response")?;
    if read == 0 {
        bail!("engine closed the connection without responding");
    }
    serde_json::from_str(&line).context("engine returned an invalid response")
}

fn print_snapshot(snapshot: &EngineSnapshot) {
    println!("Engine: {}", snapshot.status.as_str());
    match &snapshot.active_run {
        Some(run) => {
            println!("Run: {}", run.id);
            println!("Repository: {}", run.repository);
            println!("Goal: {}", run.goal);
        }
        None => println!("Active run: none"),
    }
    println!(
        "Attention: {}",
        if snapshot.requires_attention {
            "required"
        } else {
            "not required"
        }
    );
}
