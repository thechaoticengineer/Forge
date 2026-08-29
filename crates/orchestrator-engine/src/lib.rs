use std::{
    fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use orchestrator_core::{
    protocol::{ClientMessage, ClientRequest, PROTOCOL_VERSION, ServerMessage},
    state::EngineSnapshot,
};
use orchestrator_git::inspect_repository;
use orchestrator_store::{DraftRunInput, StatePaths, StorageWorker, StoredSnapshot};
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
    let state_paths = StatePaths::discover().context("cannot determine state paths")?;
    serve_with_state(socket_path, state_paths).await
}

/// Serves local IPC clients with an explicit persistent state directory.
///
/// # Errors
///
/// Returns an error when storage cannot initialize, the runtime directory or
/// socket cannot be prepared, another engine owns the socket, or the listener
/// fails.
pub async fn serve_with_state(socket_path: PathBuf, state_paths: StatePaths) -> Result<()> {
    let storage = Arc::new(
        StorageWorker::start(state_paths).context("cannot initialize durable state storage")?,
    );
    let stored_snapshot = storage
        .current_snapshot()
        .await
        .context("cannot restore durable engine state")?;
    prepare_socket(&socket_path).await?;

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("cannot bind engine socket at {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot protect engine socket at {}", socket_path.display()))?;

    let (state_sender, _state_receiver) = watch::channel(engine_snapshot(stored_snapshot));
    println!("engine listening at {}", socket_path.display());

    let result = run_listener(&listener, state_sender, storage).await;
    drop(listener);
    remove_owned_socket(&socket_path)?;
    result
}

async fn run_listener(
    listener: &UnixListener,
    state_sender: watch::Sender<EngineSnapshot>,
    storage: Arc<StorageWorker>,
) -> Result<()> {
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("cannot listen for shutdown signal")?;
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("cannot accept IPC client")?;
                let client_state = state_sender.clone();
                let client_storage = Arc::clone(&storage);
                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, client_state, client_storage).await {
                        eprintln!("IPC client disconnected after error: {error:#}");
                    }
                });
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    state_sender: watch::Sender<EngineSnapshot>,
    storage: Arc<StorageWorker>,
) -> Result<()> {
    let mut state_receiver = state_sender.subscribe();
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
                handle_request(&line, &state_sender, &storage, &mut write_half).await?;
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
    state_sender: &watch::Sender<EngineSnapshot>,
    storage: &StorageWorker,
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
        ClientRequest::CreateDraftRun { repository, goal } => {
            match create_draft_run(repository, goal, storage).await {
                Ok(stored_snapshot) => {
                    publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                    ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
                }
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::GetSnapshot => {
            ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
        }
        ClientRequest::Ping => ServerMessage::Pong {
            version: PROTOCOL_VERSION,
            request_id: request.request_id,
        },
    };

    send_message(write_half, &response).await
}

#[derive(Debug)]
struct RequestFailure {
    code: &'static str,
    message: String,
}

async fn create_draft_run(
    repository: String,
    goal: String,
    storage: &StorageWorker,
) -> Result<StoredSnapshot, RequestFailure> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(RequestFailure {
            code: "invalid_goal",
            message: "goal must not be empty".to_owned(),
        });
    }
    if goal.chars().count() > 20_000 {
        return Err(RequestFailure {
            code: "invalid_goal",
            message: "goal must not exceed 20,000 characters".to_owned(),
        });
    }

    let repository_path = PathBuf::from(repository);
    if !repository_path.is_absolute() {
        return Err(RequestFailure {
            code: "invalid_repository",
            message: "repository path must be absolute".to_owned(),
        });
    }

    let inspected = tokio::task::spawn_blocking(move || inspect_repository(&repository_path))
        .await
        .map_err(|error| RequestFailure {
            code: "repository_inspection_failed",
            message: format!("repository inspection task failed: {error}"),
        })?
        .map_err(|error| RequestFailure {
            code: "invalid_repository",
            message: error.to_string(),
        })?;

    let repository_path = path_to_string(&inspected.root)?;
    let git_common_dir = path_to_string(&inspected.git_common_dir)?;
    storage
        .create_draft_run(DraftRunInput {
            repository_path,
            git_common_dir,
            goal: goal.to_owned(),
            base_revision: inspected.head_revision,
            branch: inspected.branch,
            worktree_dirty: inspected.dirty,
        })
        .await
        .map_err(|error| RequestFailure {
            code: "storage_failed",
            message: format!("cannot persist draft run: {error}"),
        })
}

fn path_to_string(path: &Path) -> Result<String, RequestFailure> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| RequestFailure {
            code: "invalid_repository",
            message: format!("repository path is not valid UTF-8: {}", path.display()),
        })
}

fn engine_snapshot(stored: StoredSnapshot) -> EngineSnapshot {
    EngineSnapshot {
        sequence: stored.sequence,
        status: orchestrator_core::state::EngineStatus::Idle,
        active_run: stored.active_run,
        requires_attention: false,
    }
}

fn publish_newer_snapshot(state_sender: &watch::Sender<EngineSnapshot>, candidate: EngineSnapshot) {
    state_sender.send_if_modified(|current| {
        if candidate.sequence <= current.sequence {
            return false;
        }
        *current = candidate;
        true
    });
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
    let created = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                bail!("runtime path is not a directory: {}", path.display());
            }
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                bail!(
                    "runtime directory must be private (mode 0700 or stricter): {} has mode {mode:04o}",
                    path.display()
                );
            }
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .with_context(|| format!("cannot create runtime directory {}", path.display()))?;
            true
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect runtime directory {}", path.display()));
        }
    };

    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot protect runtime directory {}", path.display()))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    use tempfile::TempDir;

    #[test]
    fn refuses_to_change_a_permissive_runtime_directory() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let runtime_directory = temporary.path().join("runtime");
        fs::create_dir(&runtime_directory).expect("runtime directory should exist");
        fs::set_permissions(&runtime_directory, fs::Permissions::from_mode(0o755))
            .expect("test permissions should be set");

        let error = ensure_private_directory(&runtime_directory)
            .expect_err("permissive runtime directory should fail");
        assert!(error.to_string().contains("must be private"));
        assert_eq!(
            fs::metadata(runtime_directory)
                .expect("metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[tokio::test]
    async fn creates_a_durable_draft_from_repository_state() {
        let repository = initialized_repository();
        fs::write(repository.path().join("untracked.txt"), "change")
            .expect("untracked file should be created");
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");

        let snapshot = create_draft_run(
            repository.path().display().to_string(),
            "  Add repository support  ".to_owned(),
            &storage,
        )
        .await
        .expect("draft should be created");
        let run = snapshot.active_run.expect("draft should be active");

        assert_eq!(snapshot.sequence, 1);
        assert_eq!(run.goal, "Add repository support");
        assert_eq!(run.repository, repository.path().display().to_string());
        assert_eq!(run.run_status, orchestrator_core::state::RunStatus::Draft);
        assert!(run.worktree_dirty);
    }

    #[tokio::test]
    async fn rejects_invalid_draft_input_before_persisting() {
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");

        let empty_goal = create_draft_run("/tmp".to_owned(), "  ".to_owned(), &storage)
            .await
            .expect_err("empty goal should fail");
        assert_eq!(empty_goal.code, "invalid_goal");

        let relative_path =
            create_draft_run("relative".to_owned(), "Valid goal".to_owned(), &storage)
                .await
                .expect_err("relative path should fail");
        assert_eq!(relative_path.code, "invalid_repository");
        assert_eq!(
            storage
                .current_snapshot()
                .await
                .expect("snapshot should load"),
            StoredSnapshot::default()
        );
    }

    fn initialized_repository() -> TempDir {
        let directory = TempDir::new().expect("repository directory should exist");
        git(directory.path(), &["init", "--quiet"]);
        fs::write(directory.path().join("README.md"), "test")
            .expect("tracked file should be created");
        git(directory.path(), &["add", "README.md"]);
        git(
            directory.path(),
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: initialize",
            ],
        );
        directory
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .env("LC_ALL", "C")
            .output()
            .expect("Git should start");
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
