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
    /// Create a durable draft run for a local Git repository.
    Draft {
        /// Path to a local Git repository.
        #[arg(long)]
        repository: PathBuf,
        /// Engineering goal to preserve with the draft.
        #[arg(long)]
        goal: String,
        /// Print the resulting snapshot as JSON.
        #[arg(long)]
        json: bool,
    },
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
        Command::Draft {
            repository,
            goal,
            json,
        } => create_draft(&socket_path, repository, goal, json).await,
        Command::Ping => ping(&socket_path).await,
        Command::Status { json } => status(&socket_path, json).await,
    }
}

async fn create_draft(
    socket_path: &PathBuf,
    repository: PathBuf,
    goal: String,
    json: bool,
) -> Result<()> {
    let repository = std::fs::canonicalize(&repository)
        .with_context(|| format!("cannot resolve repository at {}", repository.display()))?;
    let repository = repository
        .to_str()
        .context("repository path is not valid UTF-8")?
        .to_owned();
    let request_id = request_id();
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request: ClientRequest::CreateDraftRun { repository, goal },
    };

    let stream = connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    send_request(&mut write_half, &request).await?;
    let mut reader = BufReader::new(read_half);

    loop {
        match read_message(&mut reader).await? {
            ServerMessage::Snapshot {
                request_id: Some(response_id),
                snapshot,
                ..
            } if response_id == request_id => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&snapshot)
                            .context("cannot encode snapshot")?
                    );
                } else {
                    print_snapshot(&snapshot);
                }
                return Ok(());
            }
            ServerMessage::Error {
                request_id: Some(response_id),
                code,
                message,
                ..
            } if response_id == request_id => bail!("engine rejected draft ({code}): {message}"),
            _ => {}
        }
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
    let request_id = request_id();
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request: ClientRequest::Ping,
    };
    send_request(&mut write_half, &request).await?;

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

async fn send_request<W>(writer: &mut W, request: &ClientMessage) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut encoded = serde_json::to_vec(request).context("cannot encode request")?;
    encoded.push(b'\n');
    writer
        .write_all(&encoded)
        .await
        .context("cannot send request")
}

async fn read_message<R>(reader: &mut R) -> Result<ServerMessage>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    let read = timeout(Duration::from_secs(10), reader.read_line(&mut line))
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
            println!("Run status: {}", run.run_status.as_str());
            println!("Revision: {}", run.base_revision);
            println!(
                "Branch: {}",
                run.branch.as_deref().unwrap_or("detached HEAD")
            );
            println!(
                "Working tree: {}",
                if run.worktree_dirty { "dirty" } else { "clean" }
            );
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

fn request_id() -> String {
    format!("cli-{}", process::id())
}
