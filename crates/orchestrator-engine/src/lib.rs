use std::{
    fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use orchestrator_core::{
    protocol::{ClientMessage, ClientRequest, PROTOCOL_VERSION, ServerMessage},
    state::EngineSnapshot,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::watch,
};

/// Serves local IPC clients until the process receives an interrupt signal.
///
/// # Errors
///
/// Returns an error when the runtime directory or socket cannot be prepared,
/// when another engine already owns the socket, or when the listener fails.
pub async fn serve(socket_path: PathBuf) -> Result<()> {
    prepare_socket(&socket_path).await?;

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("cannot bind engine socket at {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot protect engine socket at {}", socket_path.display()))?;

    let (_state_sender, state_receiver) = watch::channel(EngineSnapshot::default());
    println!("engine listening at {}", socket_path.display());

    let result = run_listener(&listener, state_receiver).await;
    drop(listener);
    remove_owned_socket(&socket_path)?;
    result
}

async fn run_listener(
    listener: &UnixListener,
    state_receiver: watch::Receiver<EngineSnapshot>,
) -> Result<()> {
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("cannot listen for shutdown signal")?;
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("cannot accept IPC client")?;
                let client_state = state_receiver.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, client_state).await {
                        eprintln!("IPC client disconnected after error: {error:#}");
                    }
                });
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    mut state_receiver: watch::Receiver<EngineSnapshot>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let initial_snapshot = state_receiver.borrow_and_update().clone();

    send_message(
        &mut write_half,
        &ServerMessage::snapshot(initial_snapshot, None),
    )
    .await?;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.context("cannot read IPC request")? else {
                    return Ok(());
                };
                handle_request(&line, &state_receiver, &mut write_half).await?;
            }
            changed = state_receiver.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let snapshot = state_receiver.borrow_and_update().clone();
                send_message(
                    &mut write_half,
                    &ServerMessage::snapshot(snapshot, None),
                ).await?;
            }
        }
    }
}

async fn handle_request(
    line: &str,
    state_receiver: &watch::Receiver<EngineSnapshot>,
    write_half: &mut OwnedWriteHalf,
) -> Result<()> {
    let request: ClientMessage = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => {
            return send_message(
                write_half,
                &ServerMessage::error(None, "invalid_message", error.to_string()),
            )
            .await;
        }
    };

    if request.version != PROTOCOL_VERSION {
        return send_message(
            write_half,
            &ServerMessage::error(
                Some(request.request_id),
                "unsupported_version",
                format!(
                    "protocol version {} is not supported; expected {PROTOCOL_VERSION}",
                    request.version
                ),
            ),
        )
        .await;
    }

    let response = match request.request {
        ClientRequest::GetSnapshot => {
            ServerMessage::snapshot(state_receiver.borrow().clone(), Some(request.request_id))
        }
        ClientRequest::Ping => ServerMessage::Pong {
            version: PROTOCOL_VERSION,
            request_id: request.request_id,
        },
    };

    send_message(write_half, &response).await
}

async fn send_message(write_half: &mut OwnedWriteHalf, message: &ServerMessage) -> Result<()> {
    let mut encoded = serde_json::to_vec(message).context("cannot encode IPC response")?;
    encoded.push(b'\n');
    write_half
        .write_all(&encoded)
        .await
        .context("cannot write IPC response")
}

async fn prepare_socket(socket_path: &Path) -> Result<()> {
    let parent = socket_path
        .parent()
        .context("engine socket path has no parent directory")?;
    ensure_private_directory(parent)?;

    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", socket_path.display()));
        }
    };

    if !metadata.file_type().is_socket() {
        bail!(
            "refusing to replace non-socket path at {}",
            socket_path.display()
        );
    }

    if UnixStream::connect(socket_path).await.is_ok() {
        bail!(
            "an engine is already listening at {}",
            socket_path.display()
        );
    }

    fs::remove_file(socket_path)
        .with_context(|| format!("cannot remove stale socket at {}", socket_path.display()))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                bail!("runtime path is not a directory: {}", path.display());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .with_context(|| format!("cannot create runtime directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect runtime directory {}", path.display()));
        }
    }

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot protect runtime directory {}", path.display()))
}

fn remove_owned_socket(socket_path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", socket_path.display()));
        }
    };

    if metadata.file_type().is_socket() {
        fs::remove_file(socket_path)
            .with_context(|| format!("cannot remove engine socket at {}", socket_path.display()))?;
    }
    Ok(())
}
