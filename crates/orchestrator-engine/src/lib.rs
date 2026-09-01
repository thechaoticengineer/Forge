use std::{
    collections::{HashMap, HashSet},
    env, fs,
    os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use orchestrator_agents::{
    ImplementerActivity, ImplementerControl, ImplementerControlRequest, ImplementerRunner,
    ImplementerStopHandle, ImplementerStopRequestResult, PlannerRunner, ReviewerRunner,
    build_implementation_continuation_prompt, build_implementation_prompt, build_planning_prompt,
    build_review_prompt, implementer_stop_channel, validate_proposal,
};
use orchestrator_core::{
    protocol::{
        ClientMessage, ClientRequest, GithubRepository, LocalRepository, MoveDirection,
        PROTOCOL_VERSION, RepositoryCatalog, ServerMessage,
    },
    state::{
        ActiveRunSummary, AgentKind, EngineSnapshot, EngineStatus, ImplementationContinuationKind,
        ImplementationStatus, ImplementationStopReason, PlanProposal, PlanStatus, PlanSummary,
        PlanTaskSummary, ProposedTask, ReviewIndependence, ReviewPolicy, ReviewStatus, RunStatus,
        TaskCommitStatus, TaskWorktreeStatus, TaskWorktreeSummary, VerificationCommandResult,
        VerificationStatus,
    },
};
use orchestrator_git::{
    GitError, TaskWorktreeRequest, TaskWorktreeState, create_task_commit,
    discover_repositories_until, inspect_repository, prune_missing_worktrees,
    review_change_evidence, task_branch_name, task_worktree_path, task_worktree_state,
};
use orchestrator_store::{
    DraftRunInput, ImplementationActivityInput, ImplementationAttemptCancellation,
    ImplementationAttemptFailure, ImplementationAttemptInput, ImplementationAttemptSuccess,
    ImplementationContinuationReservation, PlanAttemptFailure, PlanAttemptInput,
    PlanAttemptSuccess, PlanRevisionInput, ReviewAttemptFailure, ReviewAttemptInput,
    ReviewAttemptSuccess, StatePaths, StorageWorker, StoredSnapshot, TaskCommitInput,
    TaskCommitSettlement, TaskWorktreeReservation, VerificationAttemptInput,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    process::Command as AsyncCommand,
    sync::{Mutex, mpsc, oneshot, watch},
    time::timeout,
};

const GITHUB_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const GITHUB_CLONE_TIMEOUT: Duration = Duration::from_mins(15);
const LOCAL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const LOCAL_DISCOVERY_WORK_BUDGET: Duration = Duration::from_secs(25);
const IMPLEMENTATION_ACTIVITY_CHANNEL_CAPACITY: usize = 64;
const MAX_IMPLEMENTATION_ACTIVITY_MESSAGE_CHARS: usize = 4_000;

#[derive(Default)]
struct ImplementationControls {
    active: Mutex<HashMap<String, ActiveImplementationControl>>,
}

struct ActiveImplementationControl {
    run_id: String,
    cancellation: ImplementerStopHandle,
    controls: mpsc::Sender<ImplementerControlRequest>,
}

impl ImplementationControls {
    async fn register(
        &self,
        run_id: String,
        attempt_id: String,
        cancellation: ImplementerStopHandle,
        controls: mpsc::Sender<ImplementerControlRequest>,
    ) {
        self.active.lock().await.insert(
            attempt_id,
            ActiveImplementationControl {
                run_id,
                cancellation,
                controls,
            },
        );
    }

    async fn remove(&self, attempt_id: &str) {
        self.active.lock().await.remove(attempt_id);
    }

    async fn stop(
        &self,
        run_id: &str,
        attempt_id: &str,
        reason: ImplementationStopReason,
    ) -> Result<(), RequestFailure> {
        let active = self.active.lock().await;
        let control = active.get(attempt_id).ok_or_else(|| RequestFailure {
            code: "implementation_not_running",
            message: "the implementation attempt is no longer running".to_owned(),
        })?;
        if control.run_id != run_id {
            return Err(RequestFailure {
                code: "implementation_not_running",
                message: "the implementation attempt does not belong to this run".to_owned(),
            });
        }
        match control.cancellation.request(reason) {
            ImplementerStopRequestResult::Accepted => Ok(()),
            ImplementerStopRequestResult::AlreadyRequested => Err(RequestFailure {
                code: "implementation_stop_pending",
                message: "another stop request already owns this implementation transition"
                    .to_owned(),
            }),
            ImplementerStopRequestResult::Closed => Err(RequestFailure {
                code: "implementation_not_running",
                message: "the implementation supervisor is no longer available".to_owned(),
            }),
        }
    }

    async fn control(
        &self,
        run_id: &str,
        attempt_id: &str,
        requested: ImplementerControl,
    ) -> Result<(), RequestFailure> {
        let controls = {
            let active = self.active.lock().await;
            let control = active.get(attempt_id).ok_or_else(|| RequestFailure {
                code: "implementation_not_running",
                message: "the implementation attempt is no longer running".to_owned(),
            })?;
            if control.run_id != run_id {
                return Err(RequestFailure {
                    code: "implementation_not_running",
                    message: "the implementation attempt does not belong to this run".to_owned(),
                });
            }
            control.controls.clone()
        };
        let (acknowledgement, response) = oneshot::channel();
        controls
            .send(ImplementerControlRequest {
                control: requested,
                acknowledgement,
            })
            .await
            .map_err(|_| RequestFailure {
                code: "implementation_not_running",
                message: "the implementation supervisor is no longer available".to_owned(),
            })?;
        timeout(Duration::from_secs(5), response)
            .await
            .map_err(|_| RequestFailure {
                code: "implementation_control_timeout",
                message: "the implementation supervisor did not acknowledge the control".to_owned(),
            })?
            .map_err(|_| RequestFailure {
                code: "implementation_not_running",
                message: "the implementation supervisor stopped before acknowledging the control"
                    .to_owned(),
            })?
            .map_err(|message| RequestFailure {
                code: "implementation_control_failed",
                message,
            })
    }
}

#[derive(Clone, Debug)]
pub struct RepositorySettings {
    pub project_roots: Vec<PathBuf>,
    pub gh_bin: PathBuf,
}

impl RepositorySettings {
    /// Uses `~/Projects` as the initial catalog root.
    ///
    /// # Errors
    ///
    /// Returns an error when `HOME` is missing or is not an absolute path.
    pub fn discover() -> Result<Self> {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .context("HOME is unavailable for the default projects root")?;
        Ok(Self {
            project_roots: vec![home.join("Projects")],
            gh_bin: PathBuf::from("gh"),
        })
    }
}

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
    let repositories = RepositorySettings::discover()?;
    serve_with_settings(socket_path, state_paths, planner, repositories).await
}

/// Serves local IPC clients with explicit state, agent, and repository settings.
///
/// # Errors
///
/// Returns an error when storage, the socket, or the listener cannot be initialized.
pub async fn serve_with_settings(
    socket_path: PathBuf,
    state_paths: StatePaths,
    planner: PlannerRunner,
    repositories: RepositorySettings,
) -> Result<()> {
    let implementer = ImplementerRunner::new(planner.commands().clone());
    let reviewer = ReviewerRunner::new(planner.commands().clone());
    let storage = Arc::new(
        StorageWorker::start(state_paths).context("cannot initialize durable state storage")?,
    );
    let stored_snapshot = storage
        .current_snapshot()
        .await
        .context("cannot restore durable engine state")?;
    let stored_snapshot = reconcile_task_worktrees(&storage, stored_snapshot).await;
    prepare_socket(&socket_path).await?;

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("cannot bind engine socket at {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot protect engine socket at {}", socket_path.display()))?;

    let (state_sender, _state_receiver) = watch::channel(engine_snapshot(stored_snapshot));
    let implementation_controls = Arc::new(ImplementationControls::default());
    println!("engine listening at {}", socket_path.display());

    let result = run_listener(
        &listener,
        state_sender,
        storage,
        Arc::new(planner),
        Arc::new(implementer),
        Arc::new(reviewer),
        implementation_controls,
        Arc::new(repositories),
    )
    .await;
    drop(listener);
    remove_owned_socket(&socket_path)?;
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_listener(
    listener: &UnixListener,
    state_sender: watch::Sender<EngineSnapshot>,
    storage: Arc<StorageWorker>,
    planner: Arc<PlannerRunner>,
    implementer: Arc<ImplementerRunner>,
    reviewer: Arc<ReviewerRunner>,
    implementation_controls: Arc<ImplementationControls>,
    repositories: Arc<RepositorySettings>,
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
                let client_implementer = Arc::clone(&implementer);
                let client_reviewer = Arc::clone(&reviewer);
                let client_implementation_controls = Arc::clone(&implementation_controls);
                let client_repositories = Arc::clone(&repositories);
                tokio::spawn(async move {
                    if let Err(error) = handle_client(
                        stream,
                        client_state,
                        client_storage,
                        client_planner,
                        client_implementer,
                        client_reviewer,
                        client_implementation_controls,
                        client_repositories,
                    ).await {
                        eprintln!("IPC client disconnected after error: {error:#}");
                    }
                });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_client(
    stream: UnixStream,
    state_sender: watch::Sender<EngineSnapshot>,
    storage: Arc<StorageWorker>,
    planner: Arc<PlannerRunner>,
    implementer: Arc<ImplementerRunner>,
    reviewer: Arc<ReviewerRunner>,
    implementation_controls: Arc<ImplementationControls>,
    repositories: Arc<RepositorySettings>,
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
                    &implementer,
                    &reviewer,
                    &implementation_controls,
                    &repositories,
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

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_request(
    line: &str,
    state_sender: &watch::Sender<EngineSnapshot>,
    storage: &StorageWorker,
    planner: &PlannerRunner,
    implementer: &ImplementerRunner,
    reviewer: &ReviewerRunner,
    implementation_controls: &ImplementationControls,
    repositories: &RepositorySettings,
    write_half: &mut OwnedWriteHalf,
) -> Result<()> {
    let envelope: serde_json::Value = match serde_json::from_str(line) {
        Ok(envelope) => envelope,
        Err(error) => {
            return send_message(
                write_half,
                &ServerMessage::error(None, "invalid_message", error.to_string()),
            )
            .await;
        }
    };
    let envelope_request_id = envelope
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let request: ClientMessage = match serde_json::from_value(envelope) {
        Ok(request) => request,
        Err(error) => {
            return send_message(
                write_half,
                &ServerMessage::error(envelope_request_id, "invalid_message", error.to_string()),
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
        ClientRequest::ListRepositories => {
            let catalog = list_repositories(repositories).await;
            ServerMessage::RepositoryCatalog {
                version: PROTOCOL_VERSION,
                request_id: request.request_id,
                catalog,
            }
        }
        ClientRequest::CloneRepository { name_with_owner } => {
            match clone_github_repository(&name_with_owner, repositories).await {
                Ok(path) => match path_to_string(&path) {
                    Ok(path) => ServerMessage::RepositoryCloned {
                        version: PROTOCOL_VERSION,
                        request_id: request.request_id,
                        name_with_owner,
                        path,
                    },
                    Err(error) => {
                        ServerMessage::error(Some(request.request_id), error.code, error.message)
                    }
                },
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::CompleteRepositoryPath { path } => {
            match complete_repository_path(path).await {
                Ok(completion) => ServerMessage::PathCompletion {
                    version: PROTOCOL_VERSION,
                    request_id: request.request_id,
                    replacement: completion.replacement,
                    candidates: completion.candidates,
                },
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
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
        ClientRequest::CreateTaskWorktree {
            run_id,
            plan_id,
            task_id,
        } => match prepare_task_worktree(run_id, plan_id, task_id, storage, state_sender).await {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::RunTaskImplementation {
            run_id,
            plan_id,
            task_id,
            worktree_id,
            agent,
        } => match run_task_implementation(
            run_id,
            plan_id,
            task_id,
            worktree_id,
            agent,
            storage,
            implementer,
            implementation_controls,
            state_sender,
        )
        .await
        {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::CancelTaskImplementation { run_id, attempt_id } => {
            match implementation_controls
                .stop(&run_id, &attempt_id, ImplementationStopReason::Cancelled)
                .await
            {
                Ok(()) => match wait_for_implementation_settlement(
                    state_sender,
                    &attempt_id,
                    Duration::from_secs(10),
                )
                .await
                {
                    Ok(snapshot) => ServerMessage::snapshot(snapshot, Some(request.request_id)),
                    Err(error) => {
                        ServerMessage::error(Some(request.request_id), error.code, error.message)
                    }
                },
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::PauseTaskImplementation { run_id, attempt_id } => {
            match set_implementation_paused(
                &run_id,
                &attempt_id,
                true,
                storage,
                implementation_controls,
                state_sender,
            )
            .await
            {
                Ok(snapshot) => ServerMessage::snapshot(snapshot, Some(request.request_id)),
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::ResumeTaskImplementation { run_id, attempt_id } => {
            match set_implementation_paused(
                &run_id,
                &attempt_id,
                false,
                storage,
                implementation_controls,
                state_sender,
            )
            .await
            {
                Ok(snapshot) => ServerMessage::snapshot(snapshot, Some(request.request_id)),
                Err(error) => {
                    ServerMessage::error(Some(request.request_id), error.code, error.message)
                }
            }
        }
        ClientRequest::ContinueTaskImplementation {
            run_id,
            attempt_id,
            kind,
            instruction,
        } => match continue_task_implementation(
            run_id,
            attempt_id,
            kind,
            instruction,
            storage,
            implementer,
            implementation_controls,
            state_sender,
        )
        .await
        {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::RunTaskReview {
            run_id,
            plan_id,
            task_id,
            worktree_id,
            implementation_attempt_id,
            policy,
        } => match run_task_review(
            run_id,
            plan_id,
            task_id,
            worktree_id,
            implementation_attempt_id,
            policy,
            storage,
            reviewer,
            state_sender,
            None,
        )
        .await
        {
            Ok(stored_snapshot) => {
                publish_newer_snapshot(state_sender, engine_snapshot(stored_snapshot));
                ServerMessage::snapshot(state_sender.borrow().clone(), Some(request.request_id))
            }
            Err(error) => ServerMessage::error(Some(request.request_id), error.code, error.message),
        },
        ClientRequest::FinishTask {
            run_id,
            plan_id,
            task_id,
            worktree_id,
            implementation_attempt_id,
            policy,
            max_corrections,
            create_commit: should_commit,
        } => match finish_task(
            run_id,
            plan_id,
            task_id,
            worktree_id,
            implementation_attempt_id,
            policy,
            max_corrections,
            should_commit,
            storage,
            implementer,
            reviewer,
            implementation_controls,
            state_sender,
        )
        .await
        {
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

async fn wait_for_implementation_settlement(
    state_sender: &watch::Sender<EngineSnapshot>,
    attempt_id: &str,
    duration: Duration,
) -> Result<EngineSnapshot, RequestFailure> {
    let mut receiver = state_sender.subscribe();
    timeout(duration, async {
        loop {
            let snapshot = receiver.borrow_and_update().clone();
            let running = snapshot
                .active_run
                .as_ref()
                .into_iter()
                .flat_map(|run| &run.implementation_attempts)
                .any(|attempt| {
                    attempt.id == attempt_id
                        && attempt.status == orchestrator_core::state::ImplementationStatus::Running
                });
            if !running {
                return Ok(snapshot);
            }
            receiver.changed().await.map_err(|_| RequestFailure {
                code: "implementation_not_running",
                message: "the implementation supervisor stopped before cancellation settled"
                    .to_owned(),
            })?;
        }
    })
    .await
    .map_err(|_| RequestFailure {
        code: "cancellation_timeout",
        message: "the implementation did not stop within the cancellation timeout".to_owned(),
    })?
}

async fn set_implementation_paused(
    run_id: &str,
    attempt_id: &str,
    paused: bool,
    storage: &StorageWorker,
    implementation_controls: &ImplementationControls,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<EngineSnapshot, RequestFailure> {
    let requested = if paused {
        ImplementerControl::Pause
    } else {
        ImplementerControl::Resume
    };
    implementation_controls
        .control(run_id, attempt_id, requested)
        .await?;
    match storage
        .set_implementation_paused(attempt_id.to_owned(), paused)
        .await
    {
        Ok(stored) => {
            let snapshot = engine_snapshot(stored);
            publish_newer_snapshot(state_sender, snapshot.clone());
            Ok(snapshot)
        }
        Err(error) => {
            let rollback = if paused {
                ImplementerControl::Resume
            } else {
                ImplementerControl::Pause
            };
            let _ = implementation_controls
                .control(run_id, attempt_id, rollback)
                .await;
            Err(RequestFailure::storage(
                "cannot persist implementation control",
                error,
            ))
        }
    }
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

async fn list_repositories(settings: &RepositorySettings) -> RepositoryCatalog {
    let roots = settings.project_roots.clone();
    let local_task = async move {
        timeout(
            LOCAL_DISCOVERY_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                discover_repositories_until(&roots, Instant::now() + LOCAL_DISCOVERY_WORK_BUDGET)
            }),
        )
        .await
    };
    let github_task = list_github_repositories(settings);
    let (local_result, github_result) = tokio::join!(local_task, github_task);

    let (discovered, local_error) = match local_result {
        Ok(Ok(report)) => {
            let warning = match (report.truncated, report.skipped_entries) {
                (false, 0) => None,
                (truncated, skipped) => Some(format!(
                    "local discovery is incomplete{}{}",
                    if truncated {
                        ": directory scan limit reached"
                    } else {
                        ""
                    },
                    if skipped > 0 {
                        format!("; {skipped} entries could not be inspected")
                    } else {
                        String::new()
                    }
                )),
            };
            (report.repositories, warning)
        }
        Ok(Err(error)) => (
            Vec::new(),
            Some(format!("local repository discovery task failed: {error}")),
        ),
        Err(_) => (
            Vec::new(),
            Some("local repository discovery timed out after 30 seconds".to_owned()),
        ),
    };
    let local_names = discovered
        .iter()
        .filter_map(|repository| repository.github_name_with_owner.as_ref())
        .map(|name| name.to_lowercase())
        .collect::<HashSet<_>>();
    let local = discovered
        .into_iter()
        .filter_map(|repository| {
            let path = repository.state.root.to_str()?.to_owned();
            let name = repository
                .state
                .root
                .file_name()
                .and_then(|name| name.to_str())?
                .to_owned();
            Some(LocalRepository {
                name,
                path,
                name_with_owner: repository.github_name_with_owner,
                branch: repository.state.branch,
                dirty: repository.state.dirty,
            })
        })
        .collect();
    let (github, github_error) = match github_result {
        Ok(repositories) => (
            repositories
                .into_iter()
                .filter(|repository| {
                    !local_names.contains(&repository.name_with_owner.to_lowercase())
                })
                .collect(),
            None,
        ),
        Err(error) => (Vec::new(), Some(error.message)),
    };

    RepositoryCatalog {
        project_roots: settings
            .project_roots
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect(),
        local,
        local_error,
        github,
        github_error,
    }
}

async fn list_github_repositories(
    settings: &RepositorySettings,
) -> Result<Vec<GithubRepository>, RequestFailure> {
    let mut command = AsyncCommand::new(&settings.gh_bin);
    command
        .args([
            "api",
            "--method",
            "GET",
            "--paginate",
            "--slurp",
            "-f",
            "per_page=100",
            "-f",
            "affiliation=owner,collaborator,organization_member",
            "-f",
            "sort=pushed",
            "-f",
            "direction=desc",
            "/user/repos",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = run_bounded_command(command, GITHUB_LIST_TIMEOUT, 8 * 1024 * 1024, 1024 * 1024)
        .await
        .map_err(|error| RequestFailure {
            code: "github_unavailable",
            message: format!("GitHub repository discovery failed: {error}"),
        })?;
    if !output.status.success() {
        return Err(RequestFailure {
            code: "github_unavailable",
            message: format!(
                "GitHub CLI could not list repositories: {}",
                bounded_diagnostic(&output.stderr)
            ),
        });
    }
    if output.stdout_truncated {
        return Err(RequestFailure {
            code: "github_response_too_large",
            message: "GitHub returned more than 8 MiB of repository metadata".to_owned(),
        });
    }
    parse_github_repositories(&output.stdout)
}

fn parse_github_repositories(output: &[u8]) -> Result<Vec<GithubRepository>, RequestFailure> {
    let pages: Vec<Vec<serde_json::Value>> =
        serde_json::from_slice(output).map_err(|error| RequestFailure {
            code: "invalid_github_response",
            message: format!("GitHub returned invalid repository metadata: {error}"),
        })?;
    let mut repositories = Vec::new();
    let mut seen = HashSet::new();
    for repository in pages.into_iter().flatten() {
        let Some(name_with_owner) = repository.get("full_name").and_then(|value| value.as_str())
        else {
            continue;
        };
        if !valid_github_name_with_owner(name_with_owner)
            || !seen.insert(name_with_owner.to_lowercase())
        {
            continue;
        }
        let Some(name) = name_with_owner.split_once('/').map(|(_, name)| name) else {
            continue;
        };
        repositories.push(GithubRepository {
            name: name.to_owned(),
            name_with_owner: name_with_owner.to_owned(),
            url: repository
                .get("html_url")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            archived: repository
                .get("archived")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            fork: repository
                .get("fork")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            pushed_at: repository
                .get("pushed_at")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
        });
    }
    Ok(repositories)
}

async fn clone_github_repository(
    name_with_owner: &str,
    settings: &RepositorySettings,
) -> Result<PathBuf, RequestFailure> {
    if !valid_github_name_with_owner(name_with_owner) {
        return Err(RequestFailure {
            code: "invalid_github_repository",
            message: "GitHub repository must use the owner/name form".to_owned(),
        });
    }

    let existing = find_local_github_repository(name_with_owner, settings).await?;
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let Some(project_root) = settings.project_roots.first().cloned() else {
        return Err(RequestFailure {
            code: "projects_root_unavailable",
            message: "no projects root is configured for cloning".to_owned(),
        });
    };
    let (owner, name) = name_with_owner
        .split_once('/')
        .expect("validated GitHub repository should contain a slash");
    let owner = owner.to_owned();
    let name = name.to_owned();
    let destination =
        tokio::task::spawn_blocking(move || clone_destination(&project_root, &owner, &name))
            .await
            .map_err(|error| RequestFailure {
                code: "clone_destination_failed",
                message: format!("clone destination task failed: {error}"),
            })??;
    let cleanup_root = settings
        .project_roots
        .first()
        .expect("clone requires a configured projects root")
        .clone();

    let mut command = AsyncCommand::new(&settings.gh_bin);
    command
        .args(["repo", "clone", name_with_owner])
        .arg(&destination)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output =
        match run_bounded_command(command, GITHUB_CLONE_TIMEOUT, 1024 * 1024, 1024 * 1024).await {
            Ok(output) => output,
            Err(error) => {
                return Err(failed_clone(
                    format!("GitHub CLI clone failed: {error}"),
                    destination,
                    cleanup_root,
                )
                .await);
            }
        };
    if !output.status.success() {
        return Err(failed_clone(
            format!(
                "GitHub CLI could not clone {name_with_owner}: {}",
                bounded_diagnostic(&output.stderr)
            ),
            destination,
            cleanup_root,
        )
        .await);
    }

    let inspection_destination = destination.clone();
    let inspection_root = cleanup_root.clone();
    let inspection = tokio::task::spawn_blocking(move || {
        inspect_cloned_repository(&inspection_destination, &inspection_root)
    })
    .await;
    match inspection {
        Ok(Ok(repository)) => Ok(repository),
        Ok(Err(error)) => Err(failed_clone(error, destination, cleanup_root).await),
        Err(error) => Err(failed_clone(
            format!("cloned repository inspection task failed: {error}"),
            destination,
            cleanup_root,
        )
        .await),
    }
}

async fn find_local_github_repository(
    name_with_owner: &str,
    settings: &RepositorySettings,
) -> Result<Option<PathBuf>, RequestFailure> {
    let roots = settings.project_roots.clone();
    let requested = name_with_owner.to_owned();
    timeout(
        LOCAL_DISCOVERY_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let report =
                discover_repositories_until(&roots, Instant::now() + LOCAL_DISCOVERY_WORK_BUDGET);
            if report.truncated {
                return Err("local repository discovery reached its safety limit".to_owned());
            }
            Ok(report.repositories.into_iter().find_map(|repository| {
                repository
                    .github_name_with_owner
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&requested))
                    .then_some(repository.state.root)
            }))
        }),
    )
    .await
    .map_err(|_| RequestFailure {
        code: "repository_discovery_timeout",
        message: "local repository discovery timed out before cloning".to_owned(),
    })?
    .map_err(|error| RequestFailure {
        code: "repository_discovery_failed",
        message: format!("local repository discovery failed: {error}"),
    })?
    .map_err(|message| RequestFailure {
        code: "repository_discovery_failed",
        message,
    })
}

async fn failed_clone(
    message: String,
    destination: PathBuf,
    project_root: PathBuf,
) -> RequestFailure {
    let cleanup =
        tokio::task::spawn_blocking(move || remove_failed_clone(&destination, &project_root)).await;
    let message = match cleanup {
        Ok(Ok(())) => message,
        Ok(Err(error)) => format!("{message}; failed clone cleanup requires attention: {error}"),
        Err(error) => format!("{message}; failed clone cleanup task failed: {error}"),
    };
    RequestFailure {
        code: "clone_failed",
        message,
    }
}

fn remove_failed_clone(destination: &Path, project_root: &Path) -> Result<(), String> {
    let project_root = fs::canonicalize(project_root)
        .map_err(|error| format!("cannot resolve projects root: {error}"))?;
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", destination.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "reserved destination became a symlink: {}",
            destination.display()
        ));
    }
    let canonical = fs::canonicalize(destination)
        .map_err(|error| format!("cannot resolve {}: {error}", destination.display()))?;
    if canonical == project_root || !canonical.starts_with(&project_root) {
        return Err(format!(
            "refusing to clean a path outside the projects root: {}",
            canonical.display()
        ));
    }
    fs::remove_dir_all(&canonical)
        .map_err(|error| format!("cannot remove {}: {error}", canonical.display()))
}

fn inspect_cloned_repository(destination: &Path, project_root: &Path) -> Result<PathBuf, String> {
    let project_root = fs::canonicalize(project_root)
        .map_err(|error| format!("cannot resolve projects root after cloning: {error}"))?;
    let destination = fs::canonicalize(destination)
        .map_err(|error| format!("cannot resolve cloned repository: {error}"))?;
    if !destination.starts_with(&project_root) {
        return Err(format!(
            "cloned repository resolved outside the configured projects root: {}",
            destination.display()
        ));
    }
    match inspect_repository(&destination) {
        Ok(state) => Ok(state.root),
        Err(GitError::MissingHead(_)) => Ok(destination),
        Err(error) => Err(format!("cloned repository is invalid: {error}")),
    }
}

fn clone_destination(
    project_root: &Path,
    owner: &str,
    name: &str,
) -> Result<PathBuf, RequestFailure> {
    fs::create_dir_all(project_root).map_err(|error| RequestFailure {
        code: "projects_root_unavailable",
        message: format!("cannot create {}: {error}", project_root.display()),
    })?;
    let project_root = fs::canonicalize(project_root).map_err(|error| RequestFailure {
        code: "projects_root_unavailable",
        message: format!("cannot resolve {}: {error}", project_root.display()),
    })?;
    let simple = project_root.join(name);
    match fs::create_dir(&simple) {
        Ok(()) => return Ok(simple),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(RequestFailure {
                code: "clone_destination_failed",
                message: format!("cannot reserve {}: {error}", simple.display()),
            });
        }
    }
    let owner_directory = project_root.join(owner);
    match fs::symlink_metadata(&owner_directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(RequestFailure {
                code: "clone_destination_exists",
                message: format!(
                    "owner destination is not a regular directory: {}",
                    owner_directory.display()
                ),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(&owner_directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata =
                        fs::symlink_metadata(&owner_directory).map_err(|error| RequestFailure {
                            code: "clone_destination_failed",
                            message: format!(
                                "cannot inspect {} after a clone race: {error}",
                                owner_directory.display()
                            ),
                        })?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(RequestFailure {
                            code: "clone_destination_exists",
                            message: format!(
                                "owner destination is not a regular directory: {}",
                                owner_directory.display()
                            ),
                        });
                    }
                }
                Err(error) => {
                    return Err(RequestFailure {
                        code: "clone_destination_failed",
                        message: format!("cannot create {}: {error}", owner_directory.display()),
                    });
                }
            }
        }
        Err(error) => {
            return Err(RequestFailure {
                code: "clone_destination_failed",
                message: format!("cannot inspect {}: {error}", owner_directory.display()),
            });
        }
    }
    let namespaced = owner_directory.join(name);
    if let Err(error) = fs::create_dir(&namespaced) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(RequestFailure {
                code: "clone_destination_failed",
                message: format!("cannot reserve {}: {error}", namespaced.display()),
            });
        }
        return Err(RequestFailure {
            code: "clone_destination_exists",
            message: format!(
                "both {} and {} already exist; choose or move the intended repository manually",
                simple.display(),
                namespaced.display()
            ),
        });
    }
    Ok(namespaced)
}

fn valid_github_name_with_owner(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && owner != "."
        && owner != ".."
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case(".git")
        && owner
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && owner
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|character| character != '-')
        && owner
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    const LIMIT: usize = 4_096;
    let start = bytes.len().saturating_sub(LIMIT);
    let diagnostic = String::from_utf8_lossy(&bytes[start..]).trim().to_owned();
    if diagnostic.is_empty() {
        "no diagnostic output".to_owned()
    } else {
        diagnostic
    }
}

#[derive(Debug)]
struct BoundedCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
}

async fn run_bounded_command(
    mut command: AsyncCommand,
    duration: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<BoundedCommandOutput, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start process: {error}"))?;
    let process_group = child.id().and_then(|id| i32::try_from(id).ok());
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "process stdout is unavailable".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "process stderr is unavailable".to_owned())?;
    let mut stdout_task = tokio::spawn(read_bounded_bytes(stdout, stdout_limit));
    let mut stderr_task = tokio::spawn(read_bounded_bytes(stderr, stderr_limit));
    let completion = async {
        let status = child
            .wait()
            .await
            .map_err(|error| format!("cannot wait for process: {error}"))?;
        let stdout = (&mut stdout_task)
            .await
            .map_err(|error| format!("cannot join stdout reader: {error}"))?
            .map_err(|error| format!("cannot read process stdout: {error}"))?;
        let stderr = (&mut stderr_task)
            .await
            .map_err(|error| format!("cannot join stderr reader: {error}"))?
            .map_err(|error| format!("cannot read process stderr: {error}"))?;
        Ok::<_, String>(BoundedCommandOutput {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
        })
    };

    let Ok(output) = timeout(duration, completion).await else {
        if process_group
            .map(Pid::from_raw)
            .is_none_or(|group| killpg(group, Signal::SIGKILL).is_err())
        {
            let _ = child.kill().await;
        }
        let _ = child.wait().await;
        stdout_task.abort();
        stderr_task.abort();
        return Err(format!(
            "process timed out after {} seconds",
            duration.as_secs()
        ));
    };
    output
}

struct BoundedBytes {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded_bytes<R>(mut reader: R, maximum: usize) -> std::io::Result<BoundedBytes>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(maximum.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(bytes.len());
        let retained = read.min(remaining);
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok(BoundedBytes { bytes, truncated })
}

#[derive(Debug, Eq, PartialEq)]
struct PathCompletion {
    replacement: String,
    candidates: Vec<String>,
}

async fn complete_repository_path(path: String) -> Result<PathCompletion, RequestFailure> {
    if path.chars().count() > 4096 {
        return Err(RequestFailure {
            code: "invalid_path",
            message: "repository path must not exceed 4,096 characters".to_owned(),
        });
    }

    tokio::task::spawn_blocking(move || complete_repository_path_blocking(&path))
        .await
        .map_err(|error| RequestFailure {
            code: "path_completion_failed",
            message: format!("path completion task failed: {error}"),
        })?
}

fn complete_repository_path_blocking(input: &str) -> Result<PathCompletion, RequestFailure> {
    let expanded = expand_home_path(input)?;
    let path = PathBuf::from(&expanded);
    if !path.is_absolute() {
        return Err(RequestFailure {
            code: "invalid_path",
            message: "use an absolute repository path or a path beginning with ~/".to_owned(),
        });
    }

    let has_trailing_separator = expanded.ends_with(std::path::MAIN_SEPARATOR);
    let (directory, prefix) = if has_trailing_separator {
        (path, String::new())
    } else {
        let directory = path
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_path_buf();
        let prefix = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        (directory, prefix)
    };

    let entries = fs::read_dir(&directory).map_err(|error| RequestFailure {
        code: "path_completion_failed",
        message: format!("cannot read {}: {error}", directory.display()),
    })?;
    let include_hidden = prefix.starts_with('.');
    let mut matches = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if (!include_hidden && name.starts_with('.')) || !name.starts_with(&prefix) {
            continue;
        }
        if entry.path().is_dir() {
            matches.push(name);
        }
    }
    matches.sort_unstable();

    let candidates = matches
        .iter()
        .take(20)
        .filter_map(|name| directory.join(name).to_str().map(directory_path_string))
        .collect::<Vec<_>>();
    let replacement = match matches.as_slice() {
        [] => expanded,
        [only] => directory_path_string(directory.join(only).to_string_lossy().as_ref()),
        many => {
            let common = common_prefix(many);
            if common.len() > prefix.len() {
                directory.join(common).to_string_lossy().into_owned()
            } else {
                expanded
            }
        }
    };

    Ok(PathCompletion {
        replacement,
        candidates,
    })
}

fn expand_home_path(input: &str) -> Result<String, RequestFailure> {
    if !input.is_empty() && input != "~" && !input.starts_with("~/") {
        return Ok(input.to_owned());
    }

    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| RequestFailure {
            code: "path_completion_failed",
            message: "HOME is unavailable for repository path completion".to_owned(),
        })?;
    let needs_separator = input.is_empty() || input == "~" || input.ends_with('/');
    let expanded = if input.is_empty() || input == "~" {
        home
    } else {
        home.join(input.trim_start_matches("~/"))
    };
    let expanded = expanded.to_string_lossy();
    Ok(if needs_separator {
        directory_path_string(expanded.as_ref())
    } else {
        expanded.into_owned()
    })
}

fn directory_path_string(path: &str) -> String {
    if path.ends_with(std::path::MAIN_SEPARATOR) {
        path.to_owned()
    } else {
        format!("{path}{}", std::path::MAIN_SEPARATOR)
    }
}

fn common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in &values[1..] {
        let shared_bytes = prefix
            .char_indices()
            .zip(value.chars())
            .take_while(|((_, left), right)| *left == *right)
            .last()
            .map_or(0, |((index, character), _)| index + character.len_utf8());
        prefix.truncate(shared_bytes);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
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

/// Records an intended task worktree, then creates it. The durable record is
/// written first so an interrupted engine leaves a retryable reservation rather
/// than an unexplained directory. See ADR-0006.
async fn prepare_task_worktree(
    run_id: String,
    plan_id: String,
    task_id: String,
    storage: &StorageWorker,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    let run = load_active_run(storage, &run_id).await?;
    let plan = run
        .plan
        .as_ref()
        .filter(|plan| plan.id == plan_id && plan.status == PlanStatus::Approved)
        .ok_or_else(|| RequestFailure {
            code: "plan_not_approved",
            message: "isolated implementation requires the run's approved plan".to_owned(),
        })?;
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| RequestFailure {
            code: "task_not_found",
            message: "the selected task is not part of the approved plan".to_owned(),
        })?;

    let branch = task_branch_name(&run.id, task.position, &task.title);
    let path = task_worktree_path(
        storage.paths().worktrees(),
        &run.id,
        task.position,
        &task.title,
    );
    let reserved = storage
        .reserve_task_worktree(TaskWorktreeReservation {
            run_id: run.id.clone(),
            plan_id,
            task_id,
            branch: branch.clone(),
            path: path_to_string(&path)?,
            base_revision: run.base_revision.clone(),
        })
        .await
        .map_err(|error| match error {
            orchestrator_store::StorageError::WorktreeAlreadyLive => RequestFailure {
                code: "worktree_exists",
                message: error.to_string(),
            },
            other => RequestFailure::storage("cannot reserve the task worktree", other),
        })?;
    publish_newer_snapshot(state_sender, engine_snapshot(reserved.snapshot));

    let request = TaskWorktreeRequest {
        repository: PathBuf::from(&run.repository),
        path,
        branch,
        base_revision: run.base_revision,
    };
    let created =
        tokio::task::spawn_blocking(move || orchestrator_git::create_task_worktree(&request))
            .await
            .map_err(|error| format!("task worktree creation task failed: {error}"))
            .and_then(|result| result.map_err(|error| error.to_string()));

    match created {
        Ok(worktree) => storage
            .confirm_task_worktree(reserved.worktree_id, worktree.repository_dirty)
            .await
            .map_err(|error| RequestFailure::storage("cannot confirm the task worktree", error)),
        Err(message) => {
            let failed = storage
                .fail_task_worktree(reserved.worktree_id, message.clone())
                .await
                .map_err(|error| {
                    RequestFailure::storage("cannot record the worktree failure", error)
                })?;
            publish_newer_snapshot(state_sender, engine_snapshot(failed));
            Err(RequestFailure {
                code: "worktree_failed",
                message,
            })
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_task_implementation(
    run_id: String,
    plan_id: String,
    task_id: String,
    worktree_id: String,
    agent: AgentKind,
    storage: &StorageWorker,
    implementer: &ImplementerRunner,
    implementation_controls: &ImplementationControls,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    run_task_implementation_attempt(
        run_id,
        plan_id,
        task_id,
        worktree_id,
        agent,
        None,
        storage,
        implementer,
        implementation_controls,
        state_sender,
    )
    .await
}

struct ImplementationContinuation {
    parent_attempt_id: String,
    kind: ImplementationContinuationKind,
    instruction: String,
}

#[allow(clippy::too_many_arguments)]
async fn continue_task_implementation(
    run_id: String,
    attempt_id: String,
    kind: ImplementationContinuationKind,
    instruction: String,
    storage: &StorageWorker,
    implementer: &ImplementerRunner,
    implementation_controls: &ImplementationControls,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    let instruction = instruction.trim().to_owned();
    if instruction.is_empty() {
        return Err(RequestFailure {
            code: "invalid_instruction",
            message: "continuation instruction must not be empty".to_owned(),
        });
    }
    if instruction.chars().count() > 20_000 {
        return Err(RequestFailure {
            code: "invalid_instruction",
            message: "continuation instruction must not exceed 20,000 characters".to_owned(),
        });
    }

    let run = load_active_run(storage, &run_id).await?;
    let attempt = run
        .implementation_attempts
        .iter()
        .find(|attempt| attempt.id == attempt_id)
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "implementation_not_found",
            message: "the implementation attempt does not exist in the active run".to_owned(),
        })?;
    let plan_id = run
        .plan
        .as_ref()
        .filter(|plan| plan.status == PlanStatus::Approved)
        .map(|plan| plan.id.clone())
        .ok_or_else(|| RequestFailure {
            code: "plan_not_approved",
            message: "implementation continuation requires the approved plan".to_owned(),
        })?;
    let stop_reason = match kind {
        ImplementationContinuationKind::Redirect => ImplementationStopReason::Redirected,
        ImplementationContinuationKind::AdditionalContext => ImplementationStopReason::ContextAdded,
    };
    match (
        attempt.pending_continuation_kind,
        attempt.pending_user_instruction.as_deref(),
    ) {
        (None, None) if attempt.status == ImplementationStatus::Running => {
            let reserved = storage
                .reserve_implementation_continuation(ImplementationContinuationReservation {
                    attempt_id: attempt_id.clone(),
                    kind,
                    instruction: instruction.clone(),
                })
                .await
                .map_err(|error| {
                    RequestFailure::storage("cannot reserve implementation continuation", error)
                })?;
            publish_newer_snapshot(state_sender, engine_snapshot(reserved));
        }
        (Some(pending_kind), Some(pending_instruction))
            if pending_kind == kind && pending_instruction == instruction => {}
        (Some(_), Some(_)) => {
            return Err(RequestFailure {
                code: "continuation_already_pending",
                message: "a different continuation instruction is already pending".to_owned(),
            });
        }
        _ => {
            return Err(RequestFailure {
                code: "implementation_not_running",
                message: "the implementation is not running and has no pending continuation"
                    .to_owned(),
            });
        }
    }

    if attempt.status == ImplementationStatus::Running {
        match implementation_controls
            .stop(&run_id, &attempt_id, stop_reason)
            .await
        {
            Ok(()) => {}
            Err(error) if error.code == "implementation_not_running" => {}
            Err(error) => return Err(error),
        }
        wait_for_implementation_settlement(state_sender, &attempt_id, Duration::from_secs(10))
            .await?;
    }

    run_task_implementation_attempt(
        run_id,
        plan_id,
        attempt.task_id,
        attempt.worktree_id,
        attempt.agent,
        Some(ImplementationContinuation {
            parent_attempt_id: attempt_id,
            kind,
            instruction,
        }),
        storage,
        implementer,
        implementation_controls,
        state_sender,
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_task_implementation_attempt(
    run_id: String,
    plan_id: String,
    task_id: String,
    worktree_id: String,
    agent: AgentKind,
    continuation: Option<ImplementationContinuation>,
    storage: &StorageWorker,
    implementer: &ImplementerRunner,
    implementation_controls: &ImplementationControls,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    let context =
        load_implementation_context(storage, &run_id, &plan_id, &task_id, &worktree_id).await?;
    let prompt = continuation.as_ref().map_or_else(
        || build_implementation_prompt(&context.run, &context.task, &context.worktree),
        |continuation| {
            build_implementation_continuation_prompt(
                &context.run,
                &context.task,
                &context.worktree,
                continuation.kind,
                &continuation.instruction,
            )
        },
    );
    let worktree_path = context.worktree.path.clone();
    let started = storage
        .begin_implementation_attempt(ImplementationAttemptInput {
            run_id: run_id.clone(),
            plan_id,
            task_id,
            worktree_id,
            agent,
            prompt: prompt.clone(),
            parent_attempt_id: continuation
                .as_ref()
                .map(|continuation| continuation.parent_attempt_id.clone()),
            continuation_kind: continuation.as_ref().map(|continuation| continuation.kind),
            user_instruction: continuation
                .as_ref()
                .map(|continuation| continuation.instruction.clone()),
        })
        .await
        .map_err(|error| match error {
            orchestrator_store::StorageError::ImplementationNotReady(_) => RequestFailure {
                code: "implementation_not_ready",
                message: error.to_string(),
            },
            other => RequestFailure::storage("cannot start implementation", other),
        })?;
    let attempt_id = started.attempt_id;
    let (activity_sender, mut activity_receiver) =
        mpsc::channel(IMPLEMENTATION_ACTIVITY_CHANNEL_CAPACITY);
    let (cancellation_sender, cancellation_receiver) = implementer_stop_channel();
    let (control_sender, control_receiver) = mpsc::channel(4);
    implementation_controls
        .register(
            run_id,
            attempt_id.clone(),
            cancellation_sender.clone(),
            control_sender,
        )
        .await;
    publish_newer_snapshot(state_sender, engine_snapshot(started.snapshot));

    let implementation = implementer.implement_with_controls(
        agent,
        Path::new(&worktree_path),
        &prompt,
        Some(activity_sender),
        Some(cancellation_receiver),
        Some(control_receiver),
    );
    tokio::pin!(implementation);
    let mut activity_open = true;
    let mut activity_error = None;
    let mut activity_failure_owns_stop = false;
    let result = loop {
        tokio::select! {
            result = &mut implementation => break result,
            activity = activity_receiver.recv(), if activity_open => {
                if let Some(activity) = activity {
                    if activity_error.is_none()
                        && let Err(error) = persist_implementation_activity(
                            storage,
                            state_sender,
                            &attempt_id,
                            activity,
                        ).await
                    {
                        activity_error = Some(error);
                        activity_failure_owns_stop = cancellation_sender
                            .request(ImplementationStopReason::Cancelled)
                            == ImplementerStopRequestResult::Accepted;
                    }
                } else {
                    activity_open = false;
                }
            }
        }
    };
    implementation_controls.remove(&attempt_id).await;

    while activity_error.is_none() {
        let Ok(activity) = activity_receiver.try_recv() else {
            break;
        };
        if let Err(error) =
            persist_implementation_activity(storage, state_sender, &attempt_id, activity).await
        {
            activity_error = Some(error);
        }
    }

    if let Some(error) = activity_error {
        if !activity_failure_owns_stop
            && let Err(failure) = &result
            && failure.cancelled
        {
            return storage
                .cancel_implementation_attempt(ImplementationAttemptCancellation {
                    attempt_id,
                    final_output: failure.final_output.clone(),
                    diagnostic_output: failure.diagnostic_output.clone(),
                    error_message: failure.message.clone(),
                    stop_reason: failure
                        .stop_reason
                        .unwrap_or(ImplementationStopReason::Cancelled),
                })
                .await
                .map_err(|storage_error| {
                    RequestFailure::storage(
                        "cannot persist implementation cancellation",
                        storage_error,
                    )
                });
        }
        let (final_output, diagnostic_output, exit_code) = match result {
            Ok(output) => (
                output.final_output,
                output.diagnostic_output,
                Some(output.exit_code),
            ),
            Err(failure) => (
                failure.final_output,
                failure.diagnostic_output,
                failure.exit_code,
            ),
        };
        let message = format!("cannot persist implementation activity: {error}");
        let failed = storage
            .fail_implementation_attempt(ImplementationAttemptFailure {
                attempt_id,
                final_output,
                diagnostic_output,
                exit_code,
                error_message: message.clone(),
            })
            .await
            .map_err(|storage_error| {
                RequestFailure::storage("cannot persist implementation failure", storage_error)
            })?;
        publish_newer_snapshot(state_sender, engine_snapshot(failed));
        return Err(RequestFailure {
            code: "implementation_failed",
            message,
        });
    }

    match result {
        Ok(output) => storage
            .complete_implementation_attempt(ImplementationAttemptSuccess {
                attempt_id,
                final_output: output.final_output,
                diagnostic_output: output.diagnostic_output,
                exit_code: output.exit_code,
            })
            .await
            .map_err(|error| {
                RequestFailure::storage("cannot persist implementation result", error)
            }),
        Err(failure) => {
            let message = failure.message.clone();
            if failure.cancelled {
                return storage
                    .cancel_implementation_attempt(ImplementationAttemptCancellation {
                        attempt_id,
                        final_output: failure.final_output,
                        diagnostic_output: failure.diagnostic_output,
                        error_message: failure.message,
                        stop_reason: failure
                            .stop_reason
                            .unwrap_or(ImplementationStopReason::Cancelled),
                    })
                    .await
                    .map_err(|error| {
                        RequestFailure::storage("cannot persist implementation cancellation", error)
                    });
            }
            let failed = storage
                .fail_implementation_attempt(ImplementationAttemptFailure {
                    attempt_id,
                    final_output: failure.final_output,
                    diagnostic_output: failure.diagnostic_output,
                    exit_code: failure.exit_code,
                    error_message: failure.message,
                })
                .await
                .map_err(|error| {
                    RequestFailure::storage("cannot persist implementation failure", error)
                })?;
            publish_newer_snapshot(state_sender, engine_snapshot(failed));
            Err(RequestFailure {
                code: "implementation_failed",
                message,
            })
        }
    }
}

async fn persist_implementation_activity(
    storage: &StorageWorker,
    state_sender: &watch::Sender<EngineSnapshot>,
    attempt_id: &str,
    activity: ImplementerActivity,
) -> Result<(), orchestrator_store::StorageError> {
    for message in normalize_implementation_activity(&activity.message) {
        let snapshot = storage
            .append_implementation_activity(ImplementationActivityInput {
                attempt_id: attempt_id.to_owned(),
                kind: activity.kind,
                message,
            })
            .await?;
        publish_newer_snapshot(state_sender, engine_snapshot(snapshot));
    }
    Ok(())
}

#[derive(Clone)]
struct VerificationCommand {
    label: &'static str,
    program: &'static str,
    arguments: &'static [&'static str],
}

fn detected_verification_commands(worktree: &Path) -> Vec<VerificationCommand> {
    let mut commands = Vec::new();
    if worktree.join("Cargo.toml").is_file() {
        commands.extend([
            VerificationCommand {
                label: "Rust format",
                program: "cargo",
                arguments: &["fmt", "--all", "--", "--check"],
            },
            VerificationCommand {
                label: "Rust tests",
                program: "cargo",
                arguments: &["test", "--workspace"],
            },
            VerificationCommand {
                label: "Rust lint",
                program: "cargo",
                arguments: &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            },
        ]);
    }
    if worktree.join("manifest.json").is_file() {
        commands.push(VerificationCommand {
            label: "Omarchy plugin",
            program: "omarchy",
            arguments: &["plugin", "validate", "."],
        });
    }
    commands
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn verify_implementation(
    run_id: &str,
    plan_id: &str,
    task_id: &str,
    worktree_id: &str,
    implementation_attempt_id: &str,
    worktree: &Path,
    storage: &StorageWorker,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<(String, VerificationStatus, String, StoredSnapshot), RequestFailure> {
    let started_at = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RequestFailure {
                code: "clock_error",
                message: error.to_string(),
            })?
            .as_millis(),
    )
    .map_err(|_| RequestFailure {
        code: "clock_error",
        message: "system time is outside the supported range".to_owned(),
    })?;
    let commands = detected_verification_commands(worktree);
    let mut results = Vec::new();
    let mut infrastructure_error = None;
    if commands.is_empty() {
        infrastructure_error =
            Some("no supported deterministic verification commands were detected".to_owned());
    }
    for specification in commands {
        let began = Instant::now();
        let mut command = AsyncCommand::new(specification.program);
        command
            .args(specification.arguments)
            .current_dir(worktree)
            .env("LC_ALL", "C");
        match run_bounded_command(command, Duration::from_mins(15), 64 * 1024, 64 * 1024).await {
            Ok(output) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                if output.stdout_truncated {
                    stdout.push_str("\n[output truncated]");
                }
                results.push(VerificationCommandResult {
                    label: specification.label.to_owned(),
                    program: specification.program.to_owned(),
                    arguments: specification
                        .arguments
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                    working_directory: worktree.to_string_lossy().into_owned(),
                    status: if output.status.success() {
                        VerificationStatus::Passed
                    } else {
                        VerificationStatus::Failed
                    },
                    exit_code: output.status.code(),
                    stdout,
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    duration_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
            }
            Err(error) => {
                infrastructure_error = Some(format!("{}: {error}", specification.label));
                results.push(VerificationCommandResult {
                    label: specification.label.to_owned(),
                    program: specification.program.to_owned(),
                    arguments: specification
                        .arguments
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                    working_directory: worktree.to_string_lossy().into_owned(),
                    status: VerificationStatus::InfrastructureError,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: error,
                    duration_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
                });
                break;
            }
        }
    }
    let status = if infrastructure_error.is_some() {
        VerificationStatus::InfrastructureError
    } else if results
        .iter()
        .all(|result| result.status == VerificationStatus::Passed)
    {
        VerificationStatus::Passed
    } else {
        VerificationStatus::Failed
    };
    let evidence = results
        .iter()
        .map(|result| {
            format!(
                "{}: {} (exit {:?})\nstdout:\n{}\nstderr:\n{}",
                result.label,
                result.status.as_str(),
                result.exit_code,
                result.stdout,
                result.stderr
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let recorded = storage
        .record_verification_attempt(VerificationAttemptInput {
            run_id: run_id.to_owned(),
            plan_id: plan_id.to_owned(),
            task_id: task_id.to_owned(),
            worktree_id: worktree_id.to_owned(),
            implementation_attempt_id: implementation_attempt_id.to_owned(),
            status,
            commands: results,
            error_message: infrastructure_error,
            started_at,
        })
        .await
        .map_err(|error| RequestFailure::storage("cannot record verification", error))?;
    publish_newer_snapshot(state_sender, engine_snapshot(recorded.snapshot.clone()));
    Ok((recorded.attempt_id, status, evidence, recorded.snapshot))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn finish_task(
    run_id: String,
    plan_id: String,
    task_id: String,
    worktree_id: String,
    mut implementation_attempt_id: String,
    policy: ReviewPolicy,
    max_corrections: u8,
    should_commit: bool,
    storage: &StorageWorker,
    implementer: &ImplementerRunner,
    reviewer: &ReviewerRunner,
    implementation_controls: &ImplementationControls,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<StoredSnapshot, RequestFailure> {
    if max_corrections > 3 {
        return Err(RequestFailure {
            code: "invalid_correction_limit",
            message: "max_corrections must be between 0 and 3".to_owned(),
        });
    }
    let initial =
        load_implementation_context(storage, &run_id, &plan_id, &task_id, &worktree_id).await?;
    let worktree = PathBuf::from(&initial.worktree.path);
    let implementer_agent = initial
        .run
        .implementation_attempts
        .iter()
        .find(|attempt| {
            attempt.id == implementation_attempt_id
                && attempt.task_id == task_id
                && attempt.worktree_id == worktree_id
                && attempt.status == ImplementationStatus::Completed
        })
        .map(|attempt| attempt.agent)
        .ok_or_else(|| RequestFailure {
            code: "implementation_not_completed",
            message: "finish requires a completed implementation attempt".to_owned(),
        })?;
    let mut correction = 0_u8;
    loop {
        let (verification_id, verification_status, evidence, mut snapshot) = verify_implementation(
            &run_id,
            &plan_id,
            &task_id,
            &worktree_id,
            &implementation_attempt_id,
            &worktree,
            storage,
            state_sender,
        )
        .await?;
        let mut correction_reason = if verification_status == VerificationStatus::Passed {
            None
        } else {
            Some(format!(
                "Deterministic verification did not pass. Fix every failure and rerun the checks.\n\n{evidence}"
            ))
        };
        let mut approved_review = None;
        if verification_status == VerificationStatus::Passed {
            snapshot = run_task_review(
                run_id.clone(),
                plan_id.clone(),
                task_id.clone(),
                worktree_id.clone(),
                implementation_attempt_id.clone(),
                policy,
                storage,
                reviewer,
                state_sender,
                Some(&evidence),
            )
            .await?;
            let review = snapshot.active_run.as_ref().and_then(|run| {
                run.review_attempts
                    .iter()
                    .rev()
                    .find(|attempt| attempt.implementation_attempt_id == implementation_attempt_id)
            });
            match review {
                Some(review) if review.status == ReviewStatus::Approved => {
                    approved_review = Some(review.id.clone());
                }
                Some(review) if review.status == ReviewStatus::ChangesRequested => {
                    let result = review.result.as_ref();
                    correction_reason = Some(format!(
                        "An independent reviewer requested changes. Address all findings.\n\n{}",
                        result.map_or_else(
                            || review.error_message.clone().unwrap_or_default(),
                            |result| {
                                let findings = result
                                    .findings
                                    .iter()
                                    .map(|finding| {
                                        format!(
                                            "- {}: {} ({})",
                                            finding.severity.as_str(),
                                            finding.summary,
                                            finding.evidence
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                format!("{}\n{findings}", result.summary)
                            }
                        )
                    ));
                }
                _ => return Ok(snapshot),
            }
        }
        if let Some(review_id) = approved_review {
            if !should_commit {
                return Ok(snapshot);
            }
            let message = conventional_task_commit_message(&initial.task.title);
            let reserved = storage
                .record_task_commit(TaskCommitInput {
                    run_id: run_id.clone(),
                    task_id: task_id.clone(),
                    worktree_id: worktree_id.clone(),
                    implementation_attempt_id: implementation_attempt_id.clone(),
                    verification_attempt_id: verification_id,
                    review_attempt_id: review_id,
                    message: message.clone(),
                })
                .await
                .map_err(|error| RequestFailure::storage("cannot reserve task commit", error))?;
            publish_newer_snapshot(state_sender, engine_snapshot(reserved.snapshot));
            let commit = tokio::task::spawn_blocking({
                let worktree = worktree.clone();
                let branch = initial.worktree.branch.clone();
                let base = initial.worktree.base_revision.clone();
                let message = message.clone();
                move || create_task_commit(&worktree, &branch, &base, &message)
            })
            .await
            .map_err(|error| format!("task commit worker failed: {error}"))
            .and_then(|result| result.map_err(|error| error.to_string()));
            let (status, commit_hash, error_message) = match commit {
                Ok(hash) => (TaskCommitStatus::Created, Some(hash), None),
                Err(error) => (TaskCommitStatus::Failed, None, Some(error)),
            };
            let stored = storage
                .settle_task_commit(TaskCommitSettlement {
                    commit_id: reserved.commit_id,
                    status,
                    commit_hash,
                    error_message: error_message.clone(),
                })
                .await
                .map_err(|error| RequestFailure::storage("cannot record task commit", error))?;
            publish_newer_snapshot(state_sender, engine_snapshot(stored.clone()));
            if let Some(message) = error_message {
                return Err(RequestFailure {
                    code: "commit_failed",
                    message,
                });
            }
            return Ok(stored);
        }
        if correction >= max_corrections {
            return Ok(snapshot);
        }
        let instruction = correction_reason
            .unwrap_or_else(|| "Recheck and correct the implementation.".to_owned());
        let instruction = instruction.chars().take(19_000).collect::<String>();
        storage
            .reserve_implementation_continuation(ImplementationContinuationReservation {
                attempt_id: implementation_attempt_id.clone(),
                kind: ImplementationContinuationKind::AdditionalContext,
                instruction: instruction.clone(),
            })
            .await
            .map_err(|error| {
                RequestFailure::storage("cannot reserve automated correction", error)
            })?;
        let corrected = run_task_implementation_attempt(
            run_id.clone(),
            plan_id.clone(),
            task_id.clone(),
            worktree_id.clone(),
            implementer_agent,
            Some(ImplementationContinuation {
                parent_attempt_id: implementation_attempt_id,
                kind: ImplementationContinuationKind::AdditionalContext,
                instruction,
            }),
            storage,
            implementer,
            implementation_controls,
            state_sender,
        )
        .await?;
        implementation_attempt_id = corrected
            .active_run
            .as_ref()
            .and_then(|run| run.implementation_attempts.last())
            .map(|attempt| attempt.id.clone())
            .ok_or_else(|| RequestFailure {
                code: "implementation_failed",
                message: "corrected implementation attempt was not persisted".to_owned(),
            })?;
        correction += 1;
    }
}

fn conventional_task_commit_message(title: &str) -> String {
    let title = title.trim().trim_end_matches(['.', '!', '?']);
    let lowercase = title.to_ascii_lowercase();
    let (prefix, subject) = if lowercase.starts_with("fix ") {
        ("fix", lowercase.trim_start_matches("fix "))
    } else {
        ("feat", lowercase.as_str())
    };
    let mut message = format!("{prefix}: {subject}");
    if message.chars().count() > 72 {
        message = message.chars().take(72).collect();
    }
    message
}

fn normalize_implementation_activity(raw: &str) -> Vec<String> {
    let sanitized = strip_terminal_controls(raw);
    let mut messages = Vec::new();
    for line in sanitized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut chunk = String::new();
        let mut chunk_length = 0;
        for character in line.chars() {
            if chunk_length == MAX_IMPLEMENTATION_ACTIVITY_MESSAGE_CHARS {
                messages.push(std::mem::take(&mut chunk));
                chunk_length = 0;
            }
            chunk.push(character);
            chunk_length += 1;
        }
        if !chunk.is_empty() {
            messages.push(chunk);
        }
    }
    messages
}

fn strip_terminal_controls(raw: &str) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        Text,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }

    let mut state = EscapeState::Text;
    let mut output = String::with_capacity(raw.len());
    for character in raw.chars() {
        match state {
            EscapeState::Text if character == '\u{1b}' => state = EscapeState::Escape,
            EscapeState::Text if character == '\n' || character == '\t' => output.push(character),
            EscapeState::Text if !character.is_control() => output.push(character),
            EscapeState::Escape if character == '[' => state = EscapeState::Csi,
            EscapeState::Escape if character == ']' => state = EscapeState::Osc,
            EscapeState::Escape => state = EscapeState::Text,
            EscapeState::Csi if ('@'..='~').contains(&character) => state = EscapeState::Text,
            EscapeState::Osc if character == '\u{7}' => state = EscapeState::Text,
            EscapeState::Osc if character == '\u{1b}' => state = EscapeState::OscEscape,
            EscapeState::OscEscape if character == '\\' => state = EscapeState::Text,
            EscapeState::OscEscape => state = EscapeState::Osc,
            EscapeState::Text | EscapeState::Csi | EscapeState::Osc => {}
        }
    }
    output
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_task_review(
    run_id: String,
    plan_id: String,
    task_id: String,
    worktree_id: String,
    implementation_attempt_id: String,
    policy: ReviewPolicy,
    storage: &StorageWorker,
    reviewer_runner: &ReviewerRunner,
    state_sender: &watch::Sender<EngineSnapshot>,
    verification_evidence: Option<&str>,
) -> Result<StoredSnapshot, RequestFailure> {
    let snapshot = storage
        .current_snapshot()
        .await
        .map_err(|error| RequestFailure::storage("cannot load review context", error))?;
    let run = snapshot.active_run.as_ref().ok_or_else(|| RequestFailure {
        code: "run_not_found",
        message: "there is no active run to review".to_owned(),
    })?;
    if run.id != run_id {
        return Err(RequestFailure {
            code: "run_not_found",
            message: "the requested run is not active".to_owned(),
        });
    }
    let plan = run
        .plan
        .as_ref()
        .filter(|plan| plan.id == plan_id && plan.status == PlanStatus::Approved)
        .ok_or_else(|| RequestFailure {
            code: "plan_not_approved",
            message: "independent review requires the run's approved plan".to_owned(),
        })?;
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "task_not_found",
            message: "the selected task is not part of the approved plan".to_owned(),
        })?;
    let worktree = run
        .worktrees
        .iter()
        .find(|worktree| {
            worktree.id == worktree_id
                && worktree.task_id == task_id
                && worktree.status == TaskWorktreeStatus::Ready
        })
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "worktree_not_ready",
            message: "the selected task worktree is not ready for review".to_owned(),
        })?;
    let implementation = run
        .implementation_attempts
        .iter()
        .find(|attempt| {
            attempt.id == implementation_attempt_id
                && attempt.task_id == task_id
                && attempt.worktree_id == worktree_id
                && attempt.status == ImplementationStatus::Completed
        })
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "implementation_not_completed",
            message: "independent review requires a completed implementation attempt".to_owned(),
        })?;
    if run.review_attempts.iter().any(|attempt| {
        attempt.implementation_attempt_id == implementation_attempt_id
            && attempt.status == orchestrator_core::state::ReviewStatus::Approved
    }) {
        return Err(RequestFailure {
            code: "implementation_already_approved",
            message: "this implementation attempt already has an approved independent review"
                .to_owned(),
        });
    }
    let worktree_path = PathBuf::from(&worktree.path);
    let evidence =
        review_change_evidence(&worktree_path, &worktree.base_revision).map_err(|error| {
            RequestFailure {
                code: "review_evidence_failed",
                message: format!("cannot capture review evidence: {error}"),
            }
        })?;
    let evidence = format!(
        "Deterministic verification:\n{}\n\n{evidence}",
        verification_evidence.unwrap_or(
            "not run for this standalone review; block when a required criterion cannot be established"
        )
    );
    let prompt = build_review_prompt(run, &task, &worktree, implementation.agent, &evidence);
    let primary_reviewer = implementation.agent.other();
    let primary = execute_review_attempt(
        ReviewAttemptInput {
            run_id: run_id.clone(),
            plan_id: plan_id.clone(),
            task_id: task_id.clone(),
            worktree_id: worktree_id.clone(),
            implementation_attempt_id: implementation_attempt_id.clone(),
            implementer: implementation.agent,
            reviewer: primary_reviewer,
            policy,
            independence: ReviewIndependence::CrossProvider,
            prompt: prompt.clone(),
        },
        &worktree_path,
        storage,
        reviewer_runner,
        state_sender,
    )
    .await?;
    match primary {
        ReviewExecution::Settled(snapshot) => Ok(snapshot),
        ReviewExecution::LaunchFailed(snapshot) => {
            if policy == ReviewPolicy::CrossProviderRequired {
                return Ok(snapshot);
            }
            execute_review_attempt(
                ReviewAttemptInput {
                    run_id,
                    plan_id,
                    task_id,
                    worktree_id,
                    implementation_attempt_id,
                    implementer: implementation.agent,
                    reviewer: implementation.agent,
                    policy,
                    independence: ReviewIndependence::FreshSessionFallback,
                    prompt,
                },
                &worktree_path,
                storage,
                reviewer_runner,
                state_sender,
            )
            .await
            .map(|outcome| match outcome {
                ReviewExecution::Settled(snapshot) | ReviewExecution::LaunchFailed(snapshot) => {
                    snapshot
                }
            })
        }
    }
}

enum ReviewExecution {
    Settled(StoredSnapshot),
    LaunchFailed(StoredSnapshot),
}

async fn execute_review_attempt(
    input: ReviewAttemptInput,
    worktree: &Path,
    storage: &StorageWorker,
    reviewer_runner: &ReviewerRunner,
    state_sender: &watch::Sender<EngineSnapshot>,
) -> Result<ReviewExecution, RequestFailure> {
    let reviewer = input.reviewer;
    let prompt = input.prompt.clone();
    let started = storage
        .begin_review_attempt(input)
        .await
        .map_err(|error| RequestFailure::storage("cannot begin independent review", error))?;
    publish_newer_snapshot(state_sender, engine_snapshot(started.snapshot));
    match reviewer_runner.review(reviewer, worktree, &prompt).await {
        Ok(output) => storage
            .complete_review_attempt(ReviewAttemptSuccess {
                attempt_id: started.attempt_id,
                result: output.result,
                final_output: output.final_output,
                diagnostic_output: output.diagnostic_output,
                exit_code: output.exit_code,
            })
            .await
            .map(ReviewExecution::Settled)
            .map_err(|error| RequestFailure::storage("cannot complete independent review", error)),
        Err(failure) => {
            let launch_failed = failure.launch_failed;
            let snapshot = storage
                .fail_review_attempt(ReviewAttemptFailure {
                    attempt_id: started.attempt_id,
                    final_output: failure.final_output,
                    diagnostic_output: failure.diagnostic_output,
                    exit_code: failure.exit_code,
                    error_message: failure.message,
                })
                .await
                .map_err(|error| {
                    RequestFailure::storage("cannot fail independent review", error)
                })?;
            if launch_failed {
                Ok(ReviewExecution::LaunchFailed(snapshot))
            } else {
                Ok(ReviewExecution::Settled(snapshot))
            }
        }
    }
}

struct ImplementationContext {
    run: ActiveRunSummary,
    task: PlanTaskSummary,
    worktree: TaskWorktreeSummary,
}

async fn load_implementation_context(
    storage: &StorageWorker,
    run_id: &str,
    plan_id: &str,
    task_id: &str,
    worktree_id: &str,
) -> Result<ImplementationContext, RequestFailure> {
    let run = load_active_run(storage, run_id).await?;
    let plan = run
        .plan
        .as_ref()
        .filter(|plan| plan.id == plan_id && plan.status == PlanStatus::Approved)
        .ok_or_else(|| RequestFailure {
            code: "plan_not_approved",
            message: "implementation requires the run's approved plan".to_owned(),
        })?;
    let task = plan
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "task_not_found",
            message: "the selected task is not part of the approved plan".to_owned(),
        })?;
    let worktree = run
        .worktrees
        .iter()
        .find(|worktree| {
            worktree.id == worktree_id
                && worktree.task_id == task_id
                && worktree.status == TaskWorktreeStatus::Ready
        })
        .cloned()
        .ok_or_else(|| RequestFailure {
            code: "worktree_not_ready",
            message: "the selected task does not have that ready worktree".to_owned(),
        })?;

    let repository = PathBuf::from(&run.repository);
    let worktree_path = PathBuf::from(&worktree.path);
    let branch = worktree.branch.clone();
    let state = tokio::task::spawn_blocking(move || {
        task_worktree_state(&repository, &worktree_path, &branch)
    })
    .await
    .map_err(|error| RequestFailure {
        code: "worktree_not_ready",
        message: format!("cannot inspect the task worktree: {error}"),
    })?
    .map_err(|error| RequestFailure {
        code: "worktree_not_ready",
        message: error.to_string(),
    })?;
    if !matches!(state, TaskWorktreeState::Ready { .. }) {
        return Err(RequestFailure {
            code: "worktree_not_ready",
            message: match state {
                TaskWorktreeState::Missing => "the task worktree is missing".to_owned(),
                TaskWorktreeState::Diverged(detail) => {
                    format!("the task worktree diverged from its record: {detail}")
                }
                TaskWorktreeState::Ready { .. } => unreachable!(),
            },
        });
    }

    Ok(ImplementationContext {
        run,
        task,
        worktree,
    })
}

/// Compares recorded worktrees with the filesystem at startup and records what
/// diverged instead of repairing it. See ADR-0006.
async fn reconcile_task_worktrees(
    storage: &StorageWorker,
    snapshot: StoredSnapshot,
) -> StoredSnapshot {
    let Some(run) = snapshot.active_run.as_ref() else {
        return snapshot;
    };
    let repository = PathBuf::from(&run.repository);
    let recorded: Vec<_> = run
        .worktrees
        .iter()
        .filter(|worktree| worktree.status == TaskWorktreeStatus::Ready)
        .map(|worktree| {
            (
                worktree.id.clone(),
                PathBuf::from(&worktree.path),
                worktree.branch.clone(),
            )
        })
        .collect();
    if recorded.is_empty() {
        return snapshot;
    }

    let mut reconciled = snapshot;
    for (worktree_id, path, branch) in recorded {
        let repository = repository.clone();
        let inspected =
            tokio::task::spawn_blocking(move || task_worktree_state(&repository, &path, &branch))
                .await;
        let (status, detail) = match inspected {
            Ok(Ok(TaskWorktreeState::Ready { .. })) => continue,
            Ok(Ok(TaskWorktreeState::Missing)) => (TaskWorktreeStatus::Missing, None),
            Ok(Ok(TaskWorktreeState::Diverged(detail))) => {
                (TaskWorktreeStatus::Diverged, Some(detail))
            }
            Ok(Err(error)) => (TaskWorktreeStatus::Diverged, Some(error.to_string())),
            Err(error) => (
                TaskWorktreeStatus::Diverged,
                Some(format!("worktree reconciliation task failed: {error}")),
            ),
        };
        match storage
            .settle_task_worktree(worktree_id, status, detail)
            .await
        {
            Ok(updated) => reconciled = updated,
            Err(error) => eprintln!("cannot record task worktree reconciliation: {error}"),
        }
    }

    let pruned = repository.clone();
    if let Ok(Err(error)) =
        tokio::task::spawn_blocking(move || prune_missing_worktrees(&pruned)).await
    {
        eprintln!("cannot prune missing worktree records: {error}");
    }
    reconciled
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
                RunStatus::Running
                    if run.implementation_attempts.iter().any(|attempt| {
                        attempt.status == ImplementationStatus::Running && attempt.paused
                    }) =>
                {
                    (EngineStatus::WaitingForUser, true)
                }
                RunStatus::Planning | RunStatus::Running => (EngineStatus::Running, false),
                RunStatus::WaitingForUser => {
                    let review_needs_attention = run.review_attempts.last().is_some_and(|review| {
                        review.status == orchestrator_core::state::ReviewStatus::ChangesRequested
                    });
                    if !review_needs_attention
                        && run
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

    #[test]
    fn detects_fixed_rust_and_omarchy_verification_commands() {
        let directory = TempDir::new().expect("temporary directory should exist");
        fs::write(directory.path().join("Cargo.toml"), "[workspace]\n")
            .expect("Cargo marker should be written");
        fs::write(directory.path().join("manifest.json"), "{}")
            .expect("plugin marker should be written");
        let commands = detected_verification_commands(directory.path());
        assert_eq!(commands.len(), 4);
        assert_eq!(commands[0].arguments, &["fmt", "--all", "--", "--check"]);
        assert_eq!(commands[3].arguments, &["plugin", "validate", "."]);
    }

    #[test]
    fn creates_bounded_conventional_task_messages() {
        assert_eq!(
            conventional_task_commit_message("Fix broken review."),
            "fix: broken review"
        );
        assert!(conventional_task_commit_message(&"Improve pipeline ".repeat(10)).len() <= 72);
    }

    #[tokio::test]
    async fn first_stop_request_owns_the_implementation_transition() {
        let controls = ImplementationControls::default();
        let (stop_sender, _stop_receiver) = implementer_stop_channel();
        let (control_sender, _control_receiver) = mpsc::channel(1);
        controls
            .register(
                "run-1".to_owned(),
                "attempt-1".to_owned(),
                stop_sender,
                control_sender,
            )
            .await;

        controls
            .stop("run-1", "attempt-1", ImplementationStopReason::Cancelled)
            .await
            .expect("first stop should be accepted");
        let error = controls
            .stop("run-1", "attempt-1", ImplementationStopReason::Redirected)
            .await
            .expect_err("competing stop should be refused");

        assert_eq!(error.code, "implementation_stop_pending");
    }

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

    #[test]
    fn completes_repository_directories_with_terminal_style_prefixes() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        fs::create_dir(temporary.path().join("Projects")).expect("Projects directory should exist");
        fs::create_dir(temporary.path().join("Prototypes"))
            .expect("Prototypes directory should exist");
        fs::create_dir(temporary.path().join("Downloads"))
            .expect("Downloads directory should exist");
        fs::create_dir(temporary.path().join(".private")).expect("hidden directory should exist");
        fs::write(temporary.path().join("Profile.txt"), "not a directory")
            .expect("regular file should exist");

        let base = temporary.path().display().to_string();
        let ambiguous = complete_repository_path_blocking(&format!("{base}/Pr"))
            .expect("ambiguous prefix should complete");
        assert_eq!(ambiguous.replacement, format!("{base}/Pro"));
        assert_eq!(
            ambiguous.candidates,
            vec![format!("{base}/Projects/"), format!("{base}/Prototypes/")]
        );

        let unique = complete_repository_path_blocking(&format!("{base}/Down"))
            .expect("unique prefix should complete");
        assert_eq!(unique.replacement, format!("{base}/Downloads/"));
        assert_eq!(unique.candidates, vec![format!("{base}/Downloads/")]);

        let visible = complete_repository_path_blocking(&format!("{base}/"))
            .expect("directory should list visible children");
        assert!(
            !visible
                .candidates
                .iter()
                .any(|path| path.contains(".private"))
        );
        assert!(
            !visible
                .candidates
                .iter()
                .any(|path| path.contains("Profile.txt"))
        );
    }

    #[test]
    fn parses_and_deduplicates_paginated_github_repositories() {
        let output = br#"[[{"full_name":"owner/active","html_url":"https://github.com/owner/active","archived":false,"fork":false,"pushed_at":"2026-08-29T10:00:00Z"}],[{"full_name":"owner/active","html_url":"https://github.com/owner/active"},{"full_name":"team/archive","html_url":"https://github.com/team/archive","archived":true,"fork":true}]]"#;

        let repositories = parse_github_repositories(output).expect("GitHub metadata should parse");

        assert_eq!(repositories.len(), 2);
        assert_eq!(repositories[0].name_with_owner, "owner/active");
        assert!(repositories[1].archived);
        assert!(repositories[1].fork);
    }

    #[test]
    fn clone_destination_uses_owner_only_for_a_name_collision() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let projects = temporary.path().join("Projects");

        let simple = clone_destination(&projects, "owner", "project")
            .expect("simple destination should be selected");
        assert_eq!(simple, projects.join("project"));
        assert!(simple.is_dir());

        let namespaced = clone_destination(&projects, "owner", "project")
            .expect("namespaced destination should be selected");
        assert_eq!(namespaced, projects.join("owner").join("project"));
        assert!(namespaced.is_dir());

        let error = clone_destination(&projects, "owner", "project")
            .expect_err("both reserved destinations should be refused");
        assert_eq!(error.code, "clone_destination_exists");
    }

    #[test]
    fn rejects_unsafe_github_repository_names() {
        for value in [
            "owner/project",
            "the-chaos/project.rs",
            "owner/a_b",
            "owner/.github",
        ] {
            assert!(valid_github_name_with_owner(value));
        }
        for value in [
            "project",
            "../project",
            "-owner/project",
            "owner-/project",
            "owner/-project",
            "owner/.git",
            "owner/../project",
            "owner/",
        ] {
            assert!(!valid_github_name_with_owner(value));
        }
    }

    #[tokio::test]
    async fn bounds_process_output_while_draining_the_child() {
        let mut command = AsyncCommand::new("/bin/sh");
        command.args(["-c", "printf 'abcdefgh'; printf 'diagnostic' >&2"]);

        let output = run_bounded_command(command, Duration::from_secs(2), 4, 5)
            .await
            .expect("fixture process should complete");

        assert!(output.status.success());
        assert_eq!(output.stdout, b"abcd");
        assert_eq!(output.stderr, b"diagn");
        assert!(output.stdout_truncated);
    }

    #[tokio::test]
    async fn catalog_merges_local_and_github_identities_case_insensitively() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let projects = temporary.path().join("Projects");
        let local = projects.join("Forge");
        fs::create_dir_all(&local).expect("local repository directory should exist");
        initialize_repository_at(&local);
        git(
            &local,
            &["remote", "add", "origin", "git@github.com:Owner/Forge.git"],
        );
        let gh = temporary.path().join("gh");
        fs::write(
            &gh,
            "#!/bin/sh\nprintf '%s' '[[{\"full_name\":\"owner/forge\",\"html_url\":\"https://github.com/owner/forge\"},{\"full_name\":\"owner/remote\",\"html_url\":\"https://github.com/owner/remote\"}]]'\n",
        )
        .expect("fake GitHub CLI should be written");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700))
            .expect("fake GitHub CLI should be executable");

        let catalog = list_repositories(&RepositorySettings {
            project_roots: vec![projects],
            gh_bin: gh,
        })
        .await;

        assert_eq!(catalog.local.len(), 1);
        assert_eq!(catalog.github.len(), 1);
        assert_eq!(catalog.github[0].name_with_owner, "owner/remote");
        assert_eq!(catalog.local_error, None);
        assert_eq!(catalog.github_error, None);
    }

    #[tokio::test]
    async fn clones_with_an_injected_github_cli_and_reuses_the_local_identity() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let source = temporary.path().join("source");
        let projects = temporary.path().join("Projects");
        fs::create_dir(&source).expect("source directory should exist");
        initialize_repository_at(&source);
        let gh = temporary.path().join("gh");
        fs::write(
            &gh,
            format!(
                "#!/bin/sh\ngit clone --quiet '{}' \"$4\"\ngit -C \"$4\" remote set-url origin git@github.com:owner/project.git\n",
                source.display()
            ),
        )
        .expect("fake GitHub CLI should be written");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700))
            .expect("fake GitHub CLI should be executable");
        let settings = RepositorySettings {
            project_roots: vec![projects.clone()],
            gh_bin: gh,
        };

        let cloned = clone_github_repository("owner/project", &settings)
            .await
            .expect("repository should clone");
        let reused = clone_github_repository("OWNER/PROJECT", &settings)
            .await
            .expect("existing identity should be reused");

        assert_eq!(cloned, projects.join("project"));
        assert_eq!(reused, cloned);
    }

    #[tokio::test]
    async fn failed_clone_removes_its_reserved_destination() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let projects = temporary.path().join("Projects");
        let gh = temporary.path().join("gh");
        fs::write(&gh, "#!/bin/sh\nprintf 'network failed' >&2\nexit 2\n")
            .expect("fake GitHub CLI should be written");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700))
            .expect("fake GitHub CLI should be executable");
        let settings = RepositorySettings {
            project_roots: vec![projects.clone()],
            gh_bin: gh,
        };

        let failure = clone_github_repository("owner/project", &settings)
            .await
            .expect_err("clone should fail");

        assert_eq!(failure.code, "clone_failed");
        assert!(!projects.join("project").exists());
        assert_eq!(
            clone_destination(&projects, "owner", "project")
                .expect("simple destination should be reusable"),
            projects.join("project")
        );
    }

    #[tokio::test]
    async fn preserves_a_successfully_cloned_empty_repository() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let projects = temporary.path().join("Projects");
        let gh = temporary.path().join("gh");
        fs::write(
            &gh,
            "#!/bin/sh\ngit init --quiet \"$4\"\ngit -C \"$4\" remote add origin git@github.com:owner/empty.git\n",
        )
        .expect("fake GitHub CLI should be written");
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700))
            .expect("fake GitHub CLI should be executable");
        let settings = RepositorySettings {
            project_roots: vec![projects.clone()],
            gh_bin: gh,
        };

        let cloned = clone_github_repository("owner/empty", &settings)
            .await
            .expect("empty repository clone should be preserved");

        assert_eq!(cloned, projects.join("empty"));
        assert!(cloned.join(".git").is_dir());
    }

    #[tokio::test]
    async fn timeout_kills_the_command_process_group() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let marker = temporary.path().join("orphaned");
        let mut command = AsyncCommand::new("/bin/sh");
        command.args([
            "-c",
            &format!("(sleep 1; touch '{}') & exit 0", marker.display()),
        ]);

        let failure = run_bounded_command(command, Duration::from_millis(50), 1024, 1024)
            .await
            .expect_err("command should time out");
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        assert!(failure.contains("timed out"));
        assert!(!marker.exists(), "grandchild should have been killed");
    }

    #[test]
    fn clone_destination_does_not_follow_a_dangling_name_symlink() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let projects = temporary.path().join("Projects");
        fs::create_dir(&projects).expect("projects root should exist");
        std::os::unix::fs::symlink(temporary.path().join("outside"), projects.join("project"))
            .expect("dangling symlink should be created");

        let destination = clone_destination(&projects, "owner", "project")
            .expect("owner destination should be used");

        assert_eq!(destination, projects.join("owner").join("project"));
        assert!(destination.is_dir());
        assert!(!temporary.path().join("outside").exists());
    }

    #[test]
    fn rejects_relative_repository_path_completion() {
        let error = complete_repository_path_blocking("Projects/Fo")
            .expect_err("relative path completion should fail");

        assert_eq!(error.code, "invalid_path");
    }

    #[test]
    fn normalizes_implementation_activity_for_safe_snapshot_display() {
        let activity = normalize_implementation_activity(
            "\u{1b}]8;;https://example.com\u{1b}\\Editing src/main.rs\u{1b}]8;;\u{1b}\\\n\0\nDone\n",
        );

        assert_eq!(activity, vec!["Editing src/main.rs", "Done"]);

        let long = normalize_implementation_activity(&"x".repeat(8_001));
        assert_eq!(long.len(), 3);
        assert_eq!(long[0].chars().count(), 4_000);
        assert_eq!(long[1].chars().count(), 4_000);
        assert_eq!(long[2], "x");
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

        let proposed =
            generate_plan_past_a_busy_executable(run_id.clone(), &storage, &planner, &state_sender)
                .await;
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

    #[tokio::test]
    async fn creates_an_isolated_worktree_for_an_approved_task() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let paths = StatePaths::new(state.path().join("store"));
        let storage = StorageWorker::start(paths.clone()).expect("storage should start");
        fs::write(repository.path().join("in-progress.txt"), "user work")
            .expect("uncommitted user work should exist");

        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, _receiver) = watch::channel(EngineSnapshot::default());

        let created = prepare_task_worktree(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect("the approved task should get a worktree");

        let worktrees = created.active_run.expect("run should be active").worktrees;
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].status, TaskWorktreeStatus::Ready);
        assert!(worktrees[0].branch.starts_with("orchestrator/"));
        assert!(
            worktrees[0].path.starts_with(
                paths
                    .worktrees()
                    .to_str()
                    .expect("worktree root should be UTF-8")
            ),
            "worktrees live in engine state, not in the user's repository"
        );
        assert!(
            worktrees[0].repository_dirty,
            "the user's uncommitted work is recorded, not absorbed"
        );
        assert!(Path::new(&worktrees[0].path).join("README.md").is_file());
        assert!(
            !Path::new(&worktrees[0].path)
                .join("in-progress.txt")
                .exists()
        );
        assert_eq!(
            fs::read_to_string(repository.path().join("in-progress.txt"))
                .expect("user work should survive"),
            "user work"
        );
    }

    #[tokio::test]
    async fn supervises_an_implementer_in_the_task_worktree() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, _receiver) = watch::channel(EngineSnapshot::default());
        let created = prepare_task_worktree(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect("worktree should be created");
        let worktree = created
            .active_run
            .as_ref()
            .expect("run should stay active")
            .worktrees[0]
            .clone();
        let fake_codex = state.path().join("implementer-codex");
        fs::write(
            &fake_codex,
            "#!/bin/sh\ninput=$(cat)\nprintf implemented > implemented.txt\nprintf 'changed implemented.txt'\n",
        )
        .expect("fake implementer should be written");
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
            .expect("fake implementer should be executable");
        let implementer = ImplementerRunner::new(orchestrator_agents::AgentCommands {
            codex: fake_codex,
            claude: state.path().join("unused-claude"),
        });
        let implementation_controls = ImplementationControls::default();

        let completed = run_task_implementation(
            run_id,
            plan.id,
            plan.tasks[0].id.clone(),
            worktree.id.clone(),
            AgentKind::Codex,
            &storage,
            &implementer,
            &implementation_controls,
            &state_sender,
        )
        .await
        .expect("implementation should complete");
        let run = completed.active_run.expect("run should stay active");

        assert_eq!(run.run_status, RunStatus::WaitingForUser);
        assert_eq!(run.implementation_attempts.len(), 1);
        assert_eq!(run.implementation_activity.len(), 1);
        assert_eq!(
            run.implementation_activity[0].message,
            "changed implemented.txt"
        );
        assert_eq!(
            run.implementation_attempts[0].status,
            orchestrator_core::state::ImplementationStatus::Completed
        );
        assert_eq!(
            fs::read_to_string(Path::new(&worktree.path).join("implemented.txt"))
                .expect("implementer should edit its worktree"),
            "implemented"
        );
        assert!(!repository.path().join("implemented.txt").exists());
    }

    #[tokio::test]
    async fn routes_review_to_the_other_provider_in_a_fresh_attempt() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, _receiver) = watch::channel(EngineSnapshot::default());
        let created = prepare_task_worktree(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect("worktree should be created");
        let worktree = created
            .active_run
            .as_ref()
            .expect("run should stay active")
            .worktrees[0]
            .clone();
        let fake_codex = state.path().join("review-implementer-codex");
        fs::write(
            &fake_codex,
            "#!/bin/sh\ninput=$(cat)\nprintf implemented > implemented.txt\n",
        )
        .expect("fake implementer should be written");
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
            .expect("fake implementer should be executable");
        let implementer = ImplementerRunner::new(orchestrator_agents::AgentCommands {
            codex: fake_codex,
            claude: state.path().join("unused-implementation-claude"),
        });
        let implementation_controls = ImplementationControls::default();
        let implementation_snapshot = run_task_implementation(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            worktree.id.clone(),
            AgentKind::Codex,
            &storage,
            &implementer,
            &implementation_controls,
            &state_sender,
        )
        .await
        .expect("implementation should complete");
        let implementation_id = implementation_snapshot
            .active_run
            .as_ref()
            .and_then(|run| run.implementation_attempts.last())
            .expect("implementation should be stored")
            .id
            .clone();
        let fake_claude = state.path().join("reviewer-claude");
        let review = serde_json::json!({
            "structured_output": {
                "verdict": "approved",
                "summary": "The implementation satisfies the approved task.",
                "findings": []
            }
        });
        fs::write(
            &fake_claude,
            format!("#!/bin/sh\ninput=$(cat)\nprintf '%s' '{review}'\n"),
        )
        .expect("fake reviewer should be written");
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o700))
            .expect("fake reviewer should be executable");
        let reviewer = ReviewerRunner::new(orchestrator_agents::AgentCommands {
            codex: state.path().join("unused-review-codex"),
            claude: fake_claude,
        });

        let review_snapshot = run_task_review(
            run_id,
            plan.id,
            plan.tasks[0].id.clone(),
            worktree.id,
            implementation_id.clone(),
            ReviewPolicy::CrossProviderRequired,
            &storage,
            &reviewer,
            &state_sender,
            None,
        )
        .await
        .expect("review should settle");
        let run = review_snapshot.active_run.expect("run should stay active");
        let review = run.review_attempts.last().expect("review should be stored");

        assert_eq!(review.implementation_attempt_id, implementation_id);
        assert_eq!(review.implementer, AgentKind::Codex);
        assert_eq!(review.reviewer, AgentKind::Claude);
        assert_eq!(review.independence, ReviewIndependence::CrossProvider);
        assert_eq!(
            review.status,
            orchestrator_core::state::ReviewStatus::Approved
        );
    }

    #[tokio::test]
    async fn cancels_a_supervised_implementation_and_preserves_activity() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, mut state_receiver) = watch::channel(EngineSnapshot::default());
        let created = prepare_task_worktree(
            run_id.clone(),
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect("worktree should be created");
        let worktree = created
            .active_run
            .as_ref()
            .expect("run should stay active")
            .worktrees[0]
            .clone();
        let fake_codex = state.path().join("implementer-codex");
        fs::write(
            &fake_codex,
            "#!/bin/sh\ninput=$(cat)\nprintf 'editing files\\n'\nsleep 60\n",
        )
        .expect("fake implementer should be written");
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700))
            .expect("fake implementer should be executable");
        let implementer = ImplementerRunner::new(orchestrator_agents::AgentCommands {
            codex: fake_codex,
            claude: state.path().join("unused-claude"),
        });
        let implementation_controls = ImplementationControls::default();
        let implementation = run_task_implementation(
            run_id.clone(),
            plan.id,
            plan.tasks[0].id.clone(),
            worktree.id,
            AgentKind::Codex,
            &storage,
            &implementer,
            &implementation_controls,
            &state_sender,
        );
        tokio::pin!(implementation);

        let mut running_attempt_id = None;
        let attempt_id = loop {
            tokio::select! {
                result = &mut implementation => {
                    panic!("implementation settled before cancellation: {result:?}");
                }
                changed = state_receiver.changed() => {
                    changed.expect("state channel should remain open");
                    let snapshot = state_receiver.borrow_and_update();
                    if let Some(attempt) = snapshot
                        .active_run
                        .as_ref()
                        .and_then(|run| run.implementation_attempts.last())
                        .filter(|attempt| {
                            attempt.status
                                == orchestrator_core::state::ImplementationStatus::Running
                        })
                    {
                        running_attempt_id = Some(attempt.id.clone());
                    }
                    if snapshot
                        .active_run
                        .as_ref()
                        .is_some_and(|run| !run.implementation_activity.is_empty())
                    {
                        break running_attempt_id
                            .clone()
                            .expect("activity should belong to a running attempt");
                    }
                }
            }
        };
        implementation_controls
            .stop(&run_id, &attempt_id, ImplementationStopReason::Cancelled)
            .await
            .expect("running attempt should accept cancellation");
        let cancelled = timeout(Duration::from_secs(10), implementation)
            .await
            .expect("cancellation should settle promptly")
            .expect("user cancellation should be a settled workflow result");
        let run = cancelled.active_run.expect("run should stay active");

        assert_eq!(run.run_status, RunStatus::WaitingForUser);
        assert_eq!(
            run.implementation_attempts[0].status,
            orchestrator_core::state::ImplementationStatus::Cancelled
        );
        assert!(
            run.implementation_activity
                .iter()
                .any(|activity| activity.message == "editing files")
        );
    }

    #[tokio::test]
    async fn records_a_refused_worktree_as_a_retryable_failure() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, _receiver) = watch::channel(EngineSnapshot::default());
        // The engine must not adopt a branch the developer already owns.
        git(
            repository.path(),
            &[
                "branch",
                &task_branch_name(&run_id, 1, &plan.tasks[0].title),
            ],
        );

        let error = prepare_task_worktree(
            run_id,
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect_err("an existing branch should be refused");

        assert_eq!(error.code, "worktree_failed");
        let worktrees = storage
            .current_snapshot()
            .await
            .expect("snapshot should load")
            .active_run
            .expect("run should be active")
            .worktrees;
        assert_eq!(worktrees[0].status, TaskWorktreeStatus::Failed);
        assert!(
            worktrees[0]
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("already exists")),
            "the refusal is preserved for the user: {:?}",
            worktrees[0].last_error
        );
    }

    #[tokio::test]
    async fn reconciles_a_worktree_that_disappeared_while_the_engine_was_stopped() {
        let repository = initialized_repository();
        let state = TempDir::new().expect("state directory should exist");
        let storage = StorageWorker::start(StatePaths::new(state.path().join("store")))
            .expect("storage should start");
        let (run_id, plan) = approved_plan(repository.path(), &storage, &state).await;
        let (state_sender, _receiver) = watch::channel(EngineSnapshot::default());
        let created = prepare_task_worktree(
            run_id,
            plan.id.clone(),
            plan.tasks[0].id.clone(),
            &storage,
            &state_sender,
        )
        .await
        .expect("worktree should be created");
        let path = created.active_run.expect("run should be active").worktrees[0]
            .path
            .clone();
        fs::remove_dir_all(&path).expect("the user removed the directory");

        let snapshot = storage
            .current_snapshot()
            .await
            .expect("snapshot should load");
        let reconciled = reconcile_task_worktrees(&storage, snapshot).await;

        assert_eq!(
            reconciled
                .active_run
                .expect("run should be active")
                .worktrees[0]
                .status,
            TaskWorktreeStatus::Missing
        );
    }

    /// Drives a run through planning to an approved plan using a fake planner.
    async fn approved_plan(
        repository: &Path,
        storage: &StorageWorker,
        state: &TempDir,
    ) -> (String, PlanSummary) {
        let draft = create_draft_run(
            repository.display().to_string(),
            "Implement the change".to_owned(),
            storage,
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

        let fake_codex = state.path().join("planner-codex");
        let plan_json = serde_json::json!({
            "summary": "Implement the change safely",
            "tasks": [{
                "title": "Implement the change",
                "description": "Make the smallest verified change.",
                "acceptance_criteria": ["Tests pass."],
                "depends_on": []
            }]
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

        let proposed =
            generate_plan_past_a_busy_executable(run_id.clone(), storage, &planner, &state_sender)
                .await;
        let plan = proposed
            .active_run
            .and_then(|run| run.plan)
            .expect("plan should be visible");
        let approved = decide_plan(run_id.clone(), plan.id.clone(), true, None, storage)
            .await
            .expect("plan should be approved");
        let plan = approved
            .active_run
            .and_then(|run| run.plan)
            .expect("approved plan should be visible");
        (run_id, plan)
    }

    /// A sibling test that spawns a process can inherit the write descriptor of
    /// a fake planner this test just created, so the kernel reports `ETXTBSY`
    /// until that unrelated child execs. Only the test fixture is racy, so retry
    /// here rather than teaching the engine to retry a real CLI.
    async fn generate_plan_past_a_busy_executable(
        run_id: String,
        storage: &StorageWorker,
        planner: &PlannerRunner,
        state_sender: &watch::Sender<EngineSnapshot>,
    ) -> StoredSnapshot {
        for _ in 0..20 {
            match generate_plan(
                run_id.clone(),
                AgentKind::Codex,
                storage,
                planner,
                state_sender,
            )
            .await
            {
                Ok(snapshot) => return snapshot,
                Err(failure) if failure.message.contains("Text file busy") => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(failure) => panic!("planner should propose a plan: {failure:?}"),
            }
        }
        panic!("the fake planner stayed busy for every attempt");
    }

    fn initialized_repository() -> TempDir {
        let directory = TempDir::new().expect("repository directory should exist");
        initialize_repository_at(directory.path());
        directory
    }

    fn initialize_repository_at(directory: &Path) {
        git(directory, &["init", "--quiet"]);
        fs::write(directory.join("README.md"), "test").expect("tracked file should be created");
        git(directory, &["add", "README.md"]);
        git(
            directory,
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
