use std::{
    collections::HashMap,
    fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use orchestrator_agents::{PlannerRunner, build_planning_prompt, validate_proposal};
use orchestrator_core::{
    protocol::{ClientMessage, ClientRequest, MoveDirection, PROTOCOL_VERSION, ServerMessage},
    state::{
        ActiveRunSummary, AgentKind, EngineSnapshot, EngineStatus, PlanProposal, PlanStatus,
        PlanSummary, ProposedTask, RunStatus,
    },
};
use orchestrator_git::inspect_repository;
use orchestrator_store::{
    DraftRunInput, PlanAttemptFailure, PlanAttemptInput, PlanAttemptSuccess, PlanRevisionInput,
    StatePaths, StorageWorker, StoredSnapshot,
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
    serve_with_state_and_planner(socket_path, state_paths, PlannerRunner::default()).await
}

/// Serves local IPC clients with explicit state paths and planner commands.
///
/// # Errors
///
/// Returns an error when storage, the socket, or the listener cannot be initialized.
pub async fn serve_with_state_and_planner(
    socket_path: PathBuf,
    state_paths: StatePaths,
    planner: PlannerRunner,
) -> Result<()> {
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

    let result = run_listener(&listener, state_sender, storage, Arc::new(planner)).await;
    drop(listener);
    remove_owned_socket(&socket_path)?;
    result
}

async fn run_listener(
    listener: &UnixListener,
    state_sender: watch::Sender<EngineSnapshot>,
    storage: Arc<StorageWorker>,
    planner: Arc<PlannerRunner>,
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
                let client_planner = Arc::clone(&planner);
                tokio::spawn(async move {
                    if let Err(error) = handle_client(
                        stream,
                        client_state,
                        client_storage,
                        client_planner,
                    ).await {
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
    planner: Arc<PlannerRunner>,
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
                handle_request(
                    &line,
                    &state_sender,
                    &storage,
                    &planner,
                    &mut write_half,
                ).await?;
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

#[allow(clippy::too_many_lines)]
async fn handle_request(
    line: &str,
    state_sender: &watch::Sender<EngineSnapshot>,
    storage: &StorageWorker,
    planner: &PlannerRunner,
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
        ClientRequest::GeneratePlan { run_id, agent } => {
            match generate_plan(run_id, agent, storage, planner, state_sender).await {
                Ok(stored_snapshot) => {
                    publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                    ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
                }
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::UpdatePlanTask {
            run_id,
            plan_id,
            task_id,
            title,
            description,
            acceptance_criteria,
        } => match update_plan_task(
            run_id,
            plan_id,
            task_id,
            title,
            description,
            acceptance_criteria,
            storage,
        )
        .await
        {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::MovePlanTask {
            run_id,
            plan_id,
            task_id,
            direction,
        } => match move_plan_task(run_id, plan_id, task_id, direction, storage).await {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::ApprovePlan { run_id, plan_id } => {
            match decide_plan(run_id, plan_id, true, None, storage).await {
                Ok(stored_snapshot) => {
                    publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                    ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
                }
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::RejectPlan {
            run_id,
            plan_id,
            reason,
        } => match decide_plan(run_id, plan_id, false, reason, storage).await {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
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

impl RequestFailure {
    fn storage(context: &str, error: impl std::fmt::Display) -> Self {
        Self {
            code: "storage_failed",
            message: format!("{context}: {error}"),
        }
    }
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

async fn generate_plan(
    run_id: String,
    agent: AgentKind,
    storage: &StorageWorker,
    planner: &PlannerRunner,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    let run = load_active_run(storage, &run_id).await?;
    if !matches!(run.run_status, RunStatus::Draft | RunStatus::Failed) {
        return Err(RequestFailure {
            code: "run_not_plannable",
            message: "the active run is not ready to generate a plan".to_owned(),
        });
    }
    let prompt = build_planning_prompt(&run);
    let started = storage
        .begin_plan_attempt(PlanAttemptInput {
            run_id,
            agent,
            prompt: prompt.clone(),
        })
        .await
        .map_err(|error| RequestFailure::storage("cannot start planning", error))?;
    publish_newer_snapshot(state_sender, engine_snapshot(started.snapshot));

    match planner
        .generate(agent, Path::new(&run.repository), &prompt)
        .await
    {
        Ok(output) => storage
            .complete_plan_attempt(PlanAttemptSuccess {
                attempt_id: started.attempt_id,
                proposal: output.proposal,
                final_output: output.final_output,
                diagnostic_output: output.diagnostic_output,
                exit_code: output.exit_code,
            })
            .await
            .map_err(|error| RequestFailure::storage("cannot persist proposed plan", error)),
        Err(failure) => {
            let message = failure.message.clone();
            let failed = storage
                .fail_plan_attempt(PlanAttemptFailure {
                    attempt_id: started.attempt_id,
                    final_output: failure.final_output,
                    diagnostic_output: failure.diagnostic_output,
                    exit_code: failure.exit_code,
                    error_message: failure.message,
                })
                .await
                .map_err(|error| {
                    RequestFailure::storage("cannot persist planner failure", error)
                })?;
            publish_newer_snapshot(state_sender, engine_snapshot(failed));
            Err(RequestFailure {
                code: "planner_failed",
                message,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn update_plan_task(
    run_id: String,
    plan_id: String,
    task_id: String,
    title: String,
    description: String,
    acceptance_criteria: Vec<String>,
    storage: &StorageWorker,
) -> Result<StoredSnapshot, RequestFailure> {
    let plan = load_current_proposal(storage, &run_id, &plan_id).await?;
    let mut proposal = plan_to_proposal(&plan);
    let task_index = plan
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| RequestFailure {
            code: "task_not_found",
            message: "the selected task is not part of the current plan".to_owned(),
        })?;
    proposal.tasks[task_index].title = title;
    proposal.tasks[task_index].description = description;
    proposal.tasks[task_index].acceptance_criteria = acceptance_criteria;
    validate_proposal(&mut proposal).map_err(|message| RequestFailure {
        code: "invalid_plan",
        message,
    })?;
    storage
        .revise_plan(PlanRevisionInput {
            run_id,
            based_on_plan_id: plan_id,
            proposal,
        })
        .await
        .map_err(|error| RequestFailure::storage("cannot persist plan revision", error))
}

async fn move_plan_task(
    run_id: String,
    plan_id: String,
    task_id: String,
    direction: MoveDirection,
    storage: &StorageWorker,
) -> Result<StoredSnapshot, RequestFailure> {
    let plan = load_current_proposal(storage, &run_id, &plan_id).await?;
    let current_index = plan
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| RequestFailure {
            code: "task_not_found",
            message: "the selected task is not part of the current plan".to_owned(),
        })?;
    let target_index = match direction {
        MoveDirection::Up => current_index.checked_sub(1),
        MoveDirection::Down => current_index
            .checked_add(1)
            .filter(|index| *index < plan.tasks.len()),
    }
    .ok_or_else(|| RequestFailure {
        code: "invalid_move",
        message: "the selected task cannot move farther in that direction".to_owned(),
    })?;

    let old_position_to_id: HashMap<u32, String> = plan
        .tasks
        .iter()
        .map(|task| (task.position, task.id.clone()))
        .collect();
    let mut reordered = plan.tasks.clone();
    reordered.swap(current_index, target_index);
    let new_position_by_id: HashMap<String, u32> = reordered
        .iter()
        .enumerate()
        .map(|(index, task)| {
            (
                task.id.clone(),
                u32::try_from(index + 1).unwrap_or(u32::MAX),
            )
        })
        .collect();

    let mut proposal = PlanProposal {
        summary: plan.summary.clone(),
        tasks: reordered
            .iter()
            .map(|task| ProposedTask {
                title: task.title.clone(),
                description: task.description.clone(),
                acceptance_criteria: task.acceptance_criteria.clone(),
                depends_on: task
                    .depends_on
                    .iter()
                    .filter_map(|position| old_position_to_id.get(position))
                    .filter_map(|id| new_position_by_id.get(id))
                    .copied()
                    .collect(),
            })
            .collect(),
    };
    validate_proposal(&mut proposal).map_err(|message| RequestFailure {
        code: "invalid_move",
        message: format!("task move would violate plan dependencies: {message}"),
    })?;
    storage
        .revise_plan(PlanRevisionInput {
            run_id,
            based_on_plan_id: plan_id,
            proposal,
        })
        .await
        .map_err(|error| RequestFailure::storage("cannot persist reordered plan", error))
}

async fn decide_plan(
    run_id: String,
    plan_id: String,
    approved: bool,
    reason: Option<String>,
    storage: &StorageWorker,
) -> Result<StoredSnapshot, RequestFailure> {
    load_current_proposal(storage, &run_id, &plan_id).await?;
    let reason = reason
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if reason
        .as_ref()
        .is_some_and(|value| value.chars().count() > 4_000)
    {
        return Err(RequestFailure {
            code: "invalid_reason",
            message: "rejection reason must not exceed 4,000 characters".to_owned(),
        });
    }
    let result = if approved {
        storage.approve_plan(run_id, plan_id).await
    } else {
        storage.reject_plan(run_id, plan_id, reason).await
    };
    result.map_err(|error| RequestFailure::storage("cannot persist plan decision", error))
}

async fn load_active_run(
    storage: &StorageWorker,
    run_id: &str,
) -> Result<ActiveRunSummary, RequestFailure> {
    let snapshot = storage
        .current_snapshot()
        .await
        .map_err(|error| RequestFailure::storage("cannot load active run", error))?;
    snapshot
        .active_run
        .filter(|run| run.id == run_id)
        .ok_or_else(|| RequestFailure {
            code: "run_not_found",
            message: "the selected run is not the active run".to_owned(),
        })
}

async fn load_current_proposal(
    storage: &StorageWorker,
    run_id: &str,
    plan_id: &str,
) -> Result<PlanSummary, RequestFailure> {
    let run = load_active_run(storage, run_id).await?;
    run.plan
        .filter(|plan| plan.id == plan_id && plan.status == PlanStatus::Proposed)
        .ok_or_else(|| RequestFailure {
            code: "plan_not_current",
            message: "the selected plan is not the current proposal".to_owned(),
        })
}

fn plan_to_proposal(plan: &PlanSummary) -> PlanProposal {
    PlanProposal {
        summary: plan.summary.clone(),
        tasks: plan
            .tasks
            .iter()
            .map(|task| ProposedTask {
                title: task.title.clone(),
                description: task.description.clone(),
                acceptance_criteria: task.acceptance_criteria.clone(),
                depends_on: task.depends_on.clone(),
            })
            .collect(),
    }
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
    let (status, requires_attention) =
        stored
            .active_run
            .as_ref()
            .map_or((EngineStatus::Idle, false), |run| match run.run_status {
                RunStatus::Planning | RunStatus::Running => (EngineStatus::Running, false),
                RunStatus::WaitingForUser => {
                    if run
                        .plan
                        .as_ref()
                        .is_some_and(|plan| plan.status == PlanStatus::Approved)
                    {
                        (EngineStatus::Idle, false)
                    } else {
                        (EngineStatus::WaitingForUser, true)
                    }
                }
                RunStatus::Blocked => (EngineStatus::Blocked, true),
                RunStatus::Failed => (EngineStatus::Failed, true),
                RunStatus::Completed => (EngineStatus::Completed, false),
                RunStatus::Draft | RunStatus::Rejected | RunStatus::Cancelled => {
                    (EngineStatus::Idle, false)
                }
            });
    EngineSnapshot {
        sequence: stored.sequence,
        status,
        active_run: stored.active_run,
        requires_attention,
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn generates_revises_and_approves_a_plan() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let draft = create_draft_run(
            repository.path().display().to_string(),
            "Add planning".to_owned(),
            &storage,
        )
        .await
        .expect("draft should be created");
        let run_id = draft
            .active_run
            .as_ref()
            .expect("run should exist")
            .id
            .clone();
        let (state_sender, _receiver) = watch::channel(engine_snapshot(draft));

        let fake_codex = state.path().join("codex");
        let plan_json = serde_json::json!({
            "summary": "Implement planning safely",
            "tasks": [
                {
                    "title": "Inspect",
                    "description": "Inspect current behavior.",
                    "acceptance_criteria": ["Behavior is understood."],
                    "depends_on": []
                },
                {
                    "title": "Implement",
                    "description": "Add the focused behavior.",
                    "acceptance_criteria": ["Tests pass."],
                    "depends_on": [1]
                },
                {
                    "title": "Document",
                    "description": "Document the implemented behavior.",
                    "acceptance_criteria": ["Documentation is accurate."],
                    "depends_on": [1]
                }
            ]
        })
        .to_string();
        fs::write(
            &fake_codex,
            format!("#!/bin/sh\ninput=$(cat)\nprintf '%s' '{plan_json}'\n"),
        )
        .expect("fake Codex should be written");
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
            .expect("fake Codex should be executable");
        let planner = PlannerRunner::new(orchestrator_agents::AgentCommands {
            codex: fake_codex,
            claude: state.path().join("unused-claude"),
        });

        let proposed = generate_plan(
            run_id.clone(),
            AgentKind::Codex,
            &storage,
            &planner,
            &state_sender,
        )
        .await
        .expect("planner should propose a plan");
        let plan = proposed
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("plan should be visible")
            .clone();
        assert_eq!(plan.status, PlanStatus::Proposed);
        assert_eq!(plan.tasks.len(), 3);
        assert_eq!(state_sender.borrow().status, EngineStatus::Running);

        let revised = update_plan_task(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[1].id.clone(),
            "Implement the focused change".to_owned(),
            plan.tasks[1].description.clone(),
            plan.tasks[1].acceptance_criteria.clone(),
            &storage,
        )
        .await
        .expect("task edit should create a revision");
        let revised_plan = revised
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("revised plan should exist")
            .clone();
        assert_eq!(revised_plan.revision, 2);
        assert_eq!(revised_plan.tasks[1].title, "Implement the focused change");

        let reordered = move_plan_task(
            run_id.clone(),
            revised_plan.id.clone(),
            revised_plan.tasks[2].id.clone(),
            MoveDirection::Up,
            &storage,
        )
        .await
        .expect("independent tasks should reorder");
        let reordered_plan = reordered
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("reordered plan should exist")
            .clone();
        assert_eq!(reordered_plan.revision, 3);
        assert_eq!(reordered_plan.tasks[1].title, "Document");

        let invalid_move = move_plan_task(
            run_id.clone(),
            reordered_plan.id.clone(),
            reordered_plan.tasks[0].id.clone(),
            MoveDirection::Down,
            &storage,
        )
        .await
        .expect_err("dependency-breaking move should fail");
        assert_eq!(invalid_move.code, "invalid_move");

        let approved = decide_plan(run_id, reordered_plan.id, true, None, &storage)
            .await
            .expect("plan should be approved");
        assert_eq!(
            approved
                .active_run
                .and_then(|run| run.plan)
                .map(|plan| plan.status),
            Some(PlanStatus::Approved)
        );
    }

    #[tokio::test]
    async fn persists_planner_process_failures() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let draft = create_draft_run(
            repository.path().display().to_string(),
            "Plan safely".to_owned(),
            &storage,
        )
        .await
        .expect("draft should be created");
        let run_id = draft
            .active_run
            .as_ref()
            .expect("run should exist")
            .id
            .clone();
        let (state_sender, _receiver) = watch::channel(engine_snapshot(draft));
        let failing = state.path().join("codex");
        fs::write(&failing, "#!/bin/sh\nprintf 'auth failed' >&2\nexit 2\n")
            .expect("fake Codex should be written");
        fs::set_permissions(&failing, fs::Permissions::from_mode(0o700))
            .expect("fake Codex should be executable");
        let planner = PlannerRunner::new(orchestrator_agents::AgentCommands {
            codex: failing,
            claude: state.path().join("unused-claude"),
        });

        let failure = generate_plan(run_id, AgentKind::Codex, &storage, &planner, &state_sender)
            .await
            .expect_err("planner failure should be reported");
        assert_eq!(failure.code, "planner_failed");
        let snapshot = storage
            .current_snapshot()
            .await
            .expect("failed snapshot should load");
        assert_eq!(
            snapshot.active_run.as_ref().map(|run| run.run_status),
            Some(RunStatus::Failed)
        );
        assert_eq!(engine_snapshot(snapshot).status, EngineStatus::Failed);
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
