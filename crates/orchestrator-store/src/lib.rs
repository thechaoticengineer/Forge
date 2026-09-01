//! Durable SQLite storage owned by the orchestration engine.

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use orchestrator_core::state::{
    ActiveRunSummary, AgentKind, ChangedFileSummary, ImplementationActivityKind,
    ImplementationActivitySummary, ImplementationAttemptSummary, ImplementationContinuationKind,
    ImplementationStatus, ImplementationStopReason, PlanProposal, PlanStatus, PlanSummary,
    PlanTaskSummary, ReviewAttemptSummary, ReviewIndependence, ReviewPolicy, ReviewResult,
    ReviewStatus, ReviewVerdict, RunStatus, TaskCommitStatus, TaskCommitSummary,
    TaskWorktreeStatus, TaskWorktreeSummary, VerificationAttemptSummary, VerificationCommandResult,
    VerificationStatus,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use thiserror::Error;
use tokio::sync::oneshot;

const APPLICATION_DIRECTORY: &str = "omarchy-ai-build-orchestrator";
const DATABASE_FILE: &str = "state.db";
const ARTIFACTS_DIRECTORY: &str = "artifacts";
const WORKTREES_DIRECTORY: &str = "worktrees";
const LATEST_SCHEMA_VERSION: i64 = 9;
const RECENT_IMPLEMENTATION_ACTIVITY_LIMIT: i64 = 50;
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial", include_str!("../migrations/0001_initial.sql")),
    (
        2,
        "planning",
        include_str!("../migrations/0002_planning.sql"),
    ),
    (
        3,
        "worktrees",
        include_str!("../migrations/0003_worktrees.sql"),
    ),
    (
        4,
        "implementation_attempts",
        include_str!("../migrations/0004_implementation_attempts.sql"),
    ),
    (
        5,
        "implementation_activity",
        include_str!("../migrations/0005_implementation_activity.sql"),
    ),
    (
        6,
        "implementation_controls",
        include_str!("../migrations/0006_implementation_controls.sql"),
    ),
    (
        7,
        "independent_reviews",
        include_str!("../migrations/0007_independent_reviews.sql"),
    ),
    (
        8,
        "completion_pipeline",
        include_str!("../migrations/0008_completion_pipeline.sql"),
    ),
    (
        9,
        "task_commit_approval",
        include_str!("../migrations/0009_task_commit_approval.sql"),
    ),
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("XDG_STATE_HOME is empty")]
    EmptyStateHome,
    #[error("XDG_STATE_HOME must be an absolute path")]
    RelativeStateHome,
    #[error("HOME is not set and XDG_STATE_HOME is unavailable")]
    MissingHome,
    #[error("HOME is empty")]
    EmptyHome,
    #[error("HOME must be an absolute path")]
    RelativeHome,
    #[error("cannot {operation} {path}: {source}")]
    FileSystem {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot serialize or parse durable JSON: {0}")]
    Json(String),
    #[error("database schema version {found} is newer than supported version {supported}")]
    FutureSchema { found: i64, supported: i64 },
    #[error("SQLite did not enable write-ahead logging; active mode is {0}")]
    JournalMode(String),
    #[error("storage worker stopped unexpectedly")]
    WorkerStopped,
    #[error("storage worker panicked")]
    WorkerPanicked,
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch,
    #[error("database returned an invalid run status: {0}")]
    InvalidRunStatus(String),
    #[error("database returned an invalid plan status: {0}")]
    InvalidPlanStatus(String),
    #[error("database returned an invalid worktree status: {0}")]
    InvalidWorktreeStatus(String),
    #[error("database returned an invalid agent kind: {0}")]
    InvalidAgentKind(String),
    #[error("database returned an invalid implementation status: {0}")]
    InvalidImplementationStatus(String),
    #[error("database returned an invalid implementation continuation kind: {0}")]
    InvalidImplementationContinuationKind(String),
    #[error("database returned an invalid implementation stop reason: {0}")]
    InvalidImplementationStopReason(String),
    #[error("database returned an invalid review policy: {0}")]
    InvalidReviewPolicy(String),
    #[error("database returned invalid review independence: {0}")]
    InvalidReviewIndependence(String),
    #[error("database returned an invalid review status: {0}")]
    InvalidReviewStatus(String),
    #[error("run does not exist: {0}")]
    RunNotFound(String),
    #[error("run is not ready for planning: {0}")]
    RunNotPlannable(String),
    #[error("planning attempt does not exist or is no longer running: {0}")]
    AttemptNotRunning(String),
    #[error("plan does not exist or is not the current proposal: {0}")]
    PlanNotCurrent(String),
    #[error("task is not ready for isolated implementation: {0}")]
    TaskNotImplementable(String),
    #[error("task worktree does not exist or is no longer reserved: {0}")]
    WorktreeNotReserved(String),
    #[error("this task already has a live worktree; retire it before creating another")]
    WorktreeAlreadyLive,
    #[error("task implementation is not ready to run: {0}")]
    ImplementationNotReady(String),
    #[error("implementation attempt does not exist or is no longer running: {0}")]
    ImplementationAttemptNotRunning(String),
    #[error("implementation activity is empty or exceeds the configured bound")]
    InvalidImplementationActivity,
    #[error("database returned an invalid implementation activity kind: {0}")]
    InvalidImplementationActivityKind(String),
    #[error("task review is not ready to run: {0}")]
    ReviewNotReady(String),
    #[error("review attempt does not exist or is no longer running: {0}")]
    ReviewAttemptNotRunning(String),
    #[error("database sequence is negative: {0}")]
    NegativeSequence(i64),
    #[error("database position is outside the supported range: {0}")]
    InvalidPosition(i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    root: PathBuf,
    database: PathBuf,
    artifacts: PathBuf,
    worktrees: PathBuf,
}

impl StatePaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            database: root.join(DATABASE_FILE),
            artifacts: root.join(ARTIFACTS_DIRECTORY),
            worktrees: root.join(WORKTREES_DIRECTORY),
            root,
        }
    }

    /// Resolves the application state paths from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when neither a valid absolute `XDG_STATE_HOME` nor a
    /// valid absolute `HOME` is available.
    pub fn discover() -> Result<Self, StorageError> {
        Self::from_environment(env::var_os("XDG_STATE_HOME"), env::var_os("HOME"))
    }

    /// Resolves application state paths from explicit environment values.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied state or home directory is empty,
    /// relative, or entirely unavailable.
    pub fn from_environment(
        state_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, StorageError> {
        let base = if let Some(value) = state_home {
            if value.is_empty() {
                return Err(StorageError::EmptyStateHome);
            }
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(StorageError::RelativeStateHome);
            }
            path
        } else {
            let value = home.ok_or(StorageError::MissingHome)?;
            if value.is_empty() {
                return Err(StorageError::EmptyHome);
            }
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(StorageError::RelativeHome);
            }
            path.join(".local/state")
        };

        Ok(Self::new(base.join(APPLICATION_DIRECTORY)))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn database(&self) -> &Path {
        &self.database
    }

    #[must_use]
    pub fn artifacts(&self) -> &Path {
        &self.artifacts
    }

    /// Root of the engine-owned task worktrees described by ADR-0006.
    #[must_use]
    pub fn worktrees(&self) -> &Path {
        &self.worktrees
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageHealth {
    pub database_path: PathBuf,
    pub schema_version: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftRunInput {
    pub repository_path: String,
    pub git_common_dir: String,
    pub goal: String,
    pub base_revision: String,
    pub branch: Option<String>,
    pub worktree_dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanAttemptInput {
    pub run_id: String,
    pub agent: AgentKind,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedPlanAttempt {
    pub attempt_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanAttemptSuccess {
    pub attempt_id: String,
    pub proposal: PlanProposal,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanAttemptFailure {
    pub attempt_id: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: Option<i32>,
    pub error_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanRevisionInput {
    pub run_id: String,
    pub based_on_plan_id: String,
    pub proposal: PlanProposal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskWorktreeReservation {
    pub run_id: String,
    pub plan_id: String,
    pub task_id: String,
    pub branch: String,
    pub path: String,
    pub base_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReservedTaskWorktree {
    pub worktree_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationAttemptInput {
    pub run_id: String,
    pub plan_id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub agent: AgentKind,
    pub prompt: String,
    pub parent_attempt_id: Option<String>,
    pub continuation_kind: Option<ImplementationContinuationKind>,
    pub user_instruction: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedImplementationAttempt {
    pub attempt_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationAttemptSuccess {
    pub attempt_id: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationAttemptFailure {
    pub attempt_id: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: Option<i32>,
    pub error_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationActivityInput {
    pub attempt_id: String,
    pub kind: ImplementationActivityKind,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationAttemptCancellation {
    pub attempt_id: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub error_message: String,
    pub stop_reason: ImplementationStopReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementationContinuationReservation {
    pub attempt_id: String,
    pub kind: ImplementationContinuationKind,
    pub instruction: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewAttemptInput {
    pub run_id: String,
    pub plan_id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub implementer: AgentKind,
    pub reviewer: AgentKind,
    pub policy: ReviewPolicy,
    pub independence: ReviewIndependence,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartedReviewAttempt {
    pub attempt_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewAttemptSuccess {
    pub attempt_id: String,
    pub result: ReviewResult,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewAttemptFailure {
    pub attempt_id: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: Option<i32>,
    pub error_message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationAttemptInput {
    pub run_id: String,
    pub plan_id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub status: VerificationStatus,
    pub commands: Vec<VerificationCommandResult>,
    pub error_message: Option<String>,
    pub started_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedVerificationAttempt {
    pub attempt_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskCommitInput {
    pub run_id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub verification_attempt_id: String,
    pub review_attempt_id: String,
    pub message: String,
    pub tree_hash: String,
    pub changed_files: Vec<ChangedFileSummary>,
    pub patch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedTaskCommit {
    pub commit_id: String,
    pub snapshot: StoredSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskCommitSettlement {
    pub commit_id: String,
    pub status: TaskCommitStatus,
    pub commit_hash: Option<String>,
    pub error_message: Option<String>,
    pub decision_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoredSnapshot {
    pub sequence: u64,
    pub active_run: Option<ActiveRunSummary>,
}

enum Command {
    CreateDraftRun(
        DraftRunInput,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    BeginPlanAttempt(
        PlanAttemptInput,
        oneshot::Sender<Result<StartedPlanAttempt, StorageError>>,
    ),
    CompletePlanAttempt(
        PlanAttemptSuccess,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    FailPlanAttempt(
        PlanAttemptFailure,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    RevisePlan(
        PlanRevisionInput,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    ApprovePlan {
        run_id: String,
        plan_id: String,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    RejectPlan {
        run_id: String,
        plan_id: String,
        reason: Option<String>,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    ReserveTaskWorktree(
        TaskWorktreeReservation,
        oneshot::Sender<Result<ReservedTaskWorktree, StorageError>>,
    ),
    ConfirmTaskWorktree {
        worktree_id: String,
        repository_dirty: bool,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    FailTaskWorktree {
        worktree_id: String,
        error_message: String,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    SettleTaskWorktree {
        worktree_id: String,
        status: TaskWorktreeStatus,
        detail: Option<String>,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    BeginImplementationAttempt(
        ImplementationAttemptInput,
        oneshot::Sender<Result<StartedImplementationAttempt, StorageError>>,
    ),
    CompleteImplementationAttempt(
        ImplementationAttemptSuccess,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    FailImplementationAttempt(
        ImplementationAttemptFailure,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    AppendImplementationActivity(
        ImplementationActivityInput,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    CancelImplementationAttempt(
        ImplementationAttemptCancellation,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    SetImplementationPaused {
        attempt_id: String,
        paused: bool,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    ReserveImplementationContinuation(
        ImplementationContinuationReservation,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    BeginReviewAttempt(
        ReviewAttemptInput,
        oneshot::Sender<Result<StartedReviewAttempt, StorageError>>,
    ),
    CompleteReviewAttempt(
        ReviewAttemptSuccess,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    FailReviewAttempt(
        ReviewAttemptFailure,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    RecordVerificationAttempt(
        VerificationAttemptInput,
        oneshot::Sender<Result<RecordedVerificationAttempt, StorageError>>,
    ),
    RecordTaskCommit(
        TaskCommitInput,
        oneshot::Sender<Result<RecordedTaskCommit, StorageError>>,
    ),
    ReserveTaskCommit {
        commit_id: String,
        reply: oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    },
    SettleTaskCommit(
        TaskCommitSettlement,
        oneshot::Sender<Result<StoredSnapshot, StorageError>>,
    ),
    CurrentSnapshot(oneshot::Sender<Result<StoredSnapshot, StorageError>>),
    Health(oneshot::Sender<Result<StorageHealth, StorageError>>),
    Shutdown,
}

pub struct StorageWorker {
    paths: StatePaths,
    sender: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StorageWorker {
    /// Starts the dedicated SQLite worker with default user state paths.
    ///
    /// # Errors
    ///
    /// Returns an error when state paths cannot be resolved or the database
    /// cannot be initialized and migrated.
    pub fn start_default() -> Result<Self, StorageError> {
        Self::start(StatePaths::discover()?)
    }

    /// Starts the dedicated SQLite worker for explicit state paths.
    ///
    /// # Errors
    ///
    /// Returns an error when directories, permissions, SQLite configuration,
    /// or migrations cannot be initialized.
    pub fn start(paths: StatePaths) -> Result<Self, StorageError> {
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_paths = paths.clone();
        let worker_thread = thread::Builder::new()
            .name("orchestrator-storage".to_owned())
            .spawn(move || {
                let database = match Database::open(&worker_paths) {
                    Ok(database) => {
                        if ready_sender.send(Ok(())).is_err() {
                            return;
                        }
                        database
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };

                run_worker(&database, &receiver);
            })
            .map_err(|source| StorageError::FileSystem {
                operation: "start storage worker for",
                path: paths.database().to_path_buf(),
                source,
            })?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                paths,
                sender,
                thread: Some(worker_thread),
            }),
            Ok(Err(error)) => {
                worker_thread
                    .join()
                    .map_err(|_| StorageError::WorkerPanicked)?;
                Err(error)
            }
            Err(_) => {
                worker_thread
                    .join()
                    .map_err(|_| StorageError::WorkerPanicked)?;
                Err(StorageError::WorkerStopped)
            }
        }
    }

    #[must_use]
    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    /// Reports the database path and current schema version.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker has stopped or SQLite cannot read the
    /// current schema version.
    pub async fn health(&self) -> Result<StorageHealth, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::Health(reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Loads the newest non-terminal run and latest audit sequence.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker stops or persisted data is invalid.
    pub async fn current_snapshot(&self) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CurrentSnapshot(reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Creates a durable draft run and its audit event transactionally.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker stops, the data violates the schema,
    /// the clock is unavailable, or SQLite cannot commit the transaction.
    pub async fn create_draft_run(
        &self,
        input: DraftRunInput,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CreateDraftRun(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Starts a durable planner attempt and moves the run into planning.
    ///
    /// # Errors
    ///
    /// Returns an error when the run is unavailable, not plannable, or storage fails.
    pub async fn begin_plan_attempt(
        &self,
        input: PlanAttemptInput,
    ) -> Result<StartedPlanAttempt, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::BeginPlanAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Stores a validated proposal and completes its planner attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is not running or storage fails.
    pub async fn complete_plan_attempt(
        &self,
        input: PlanAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CompletePlanAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Stores planner failure evidence and marks the run failed.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is not running or storage fails.
    pub async fn fail_plan_attempt(
        &self,
        input: PlanAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::FailPlanAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Creates a new proposed revision without overwriting the prior plan.
    ///
    /// # Errors
    ///
    /// Returns an error when the base plan is not current or storage fails.
    pub async fn revise_plan(
        &self,
        input: PlanRevisionInput,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::RevisePlan(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Approves the current proposal transactionally with its audit event.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not current or storage fails.
    pub async fn approve_plan(
        &self,
        run_id: String,
        plan_id: String,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::ApprovePlan {
                run_id,
                plan_id,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Rejects the current proposal and returns its run to draft state.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not current or storage fails.
    pub async fn reject_plan(
        &self,
        run_id: String,
        plan_id: String,
        reason: Option<String>,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::RejectPlan {
                run_id,
                plan_id,
                reason,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records an intended task worktree before any Git command runs.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not part of the run's approved plan,
    /// the task already holds a live worktree, or storage fails.
    pub async fn reserve_task_worktree(
        &self,
        input: TaskWorktreeReservation,
    ) -> Result<ReservedTaskWorktree, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::ReserveTaskWorktree(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Marks a reserved worktree as present on disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the worktree is no longer reserved or storage fails.
    pub async fn confirm_task_worktree(
        &self,
        worktree_id: String,
        repository_dirty: bool,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::ConfirmTaskWorktree {
                worktree_id,
                repository_dirty,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records why a reserved worktree could not be created.
    ///
    /// # Errors
    ///
    /// Returns an error when the worktree is no longer reserved or storage fails.
    pub async fn fail_task_worktree(
        &self,
        worktree_id: String,
        error_message: String,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::FailTaskWorktree {
                worktree_id,
                error_message,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records that a previously ready worktree has disappeared or no longer
    /// matches its record.
    ///
    /// # Errors
    ///
    /// Returns an error when the worktree is not ready, the status is not a
    /// reconciliation outcome, or storage fails.
    pub async fn settle_task_worktree(
        &self,
        worktree_id: String,
        status: TaskWorktreeStatus,
        detail: Option<String>,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::SettleTaskWorktree {
                worktree_id,
                status,
                detail,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records a user-selected implementation assignment before the agent starts.
    ///
    /// # Errors
    ///
    /// Returns an error when the approved task, ready worktree, or run state no
    /// longer matches the request, or when storage is unavailable.
    pub async fn begin_implementation_attempt(
        &self,
        input: ImplementationAttemptInput,
    ) -> Result<StartedImplementationAttempt, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::BeginImplementationAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists a successfully exited implementation process.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running or storage fails.
    pub async fn complete_implementation_attempt(
        &self,
        input: ImplementationAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CompleteImplementationAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists a failed, timed-out, or refused implementation process.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running or storage fails.
    pub async fn fail_implementation_attempt(
        &self,
        input: ImplementationAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::FailImplementationAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Appends one bounded activity update for a running implementation.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running, the message is
    /// outside its bound, or storage fails.
    pub async fn append_implementation_activity(
        &self,
        input: ImplementationActivityInput,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::AppendImplementationActivity(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists a user-cancelled implementation and its retained partial output.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running or storage fails.
    pub async fn cancel_implementation_attempt(
        &self,
        input: ImplementationAttemptCancellation,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CancelImplementationAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records whether a live implementation process group is paused.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running, already has the
    /// requested state, or storage fails.
    pub async fn set_implementation_paused(
        &self,
        attempt_id: String,
        paused: bool,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::SetImplementationPaused {
                attempt_id,
                paused,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Durably records a continuation instruction before the current process
    /// is stopped.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running, another
    /// continuation is already pending, the instruction is invalid, or
    /// storage fails.
    pub async fn reserve_implementation_continuation(
        &self,
        input: ImplementationContinuationReservation,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::ReserveImplementationContinuation(
                input,
                reply_sender,
            ))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records reviewer identity and independence before starting a fresh
    /// read-only review process.
    ///
    /// # Errors
    ///
    /// Returns an error when the completed implementation, worktree, policy,
    /// or run state does not match, or storage is unavailable.
    pub async fn begin_review_attempt(
        &self,
        input: ReviewAttemptInput,
    ) -> Result<StartedReviewAttempt, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::BeginReviewAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists a validated independent-review verdict.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running, its result
    /// cannot be serialized, or storage is unavailable.
    pub async fn complete_review_attempt(
        &self,
        input: ReviewAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::CompleteReviewAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists reviewer process or structured-output failure evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when the attempt is no longer running or storage is
    /// unavailable.
    pub async fn fail_review_attempt(
        &self,
        input: ReviewAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::FailReviewAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists one completed deterministic-verification attempt.
    ///
    /// # Errors
    ///
    /// Returns an error when the implementation identity is not completed,
    /// command evidence cannot be serialized, or storage is unavailable.
    pub async fn record_verification_attempt(
        &self,
        input: VerificationAttemptInput,
    ) -> Result<RecordedVerificationAttempt, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::RecordVerificationAttempt(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Persists the outcome of the gated local task-commit operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the evidence references are invalid or storage is
    /// unavailable.
    pub async fn record_task_commit(
        &self,
        input: TaskCommitInput,
    ) -> Result<RecordedTaskCommit, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::RecordTaskCommit(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Records the user's approval before the proposed task commit is created.
    ///
    /// # Errors
    ///
    /// Returns an error when the proposal is no longer awaiting a decision or
    /// storage is unavailable.
    pub async fn reserve_task_commit(
        &self,
        commit_id: String,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::ReserveTaskCommit {
                commit_id,
                reply: reply_sender,
            })
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }

    /// Settles a previously reserved local task commit.
    ///
    /// # Errors
    ///
    /// Returns an error when the reservation is no longer pending or storage
    /// is unavailable.
    pub async fn settle_task_commit(
        &self,
        input: TaskCommitSettlement,
    ) -> Result<StoredSnapshot, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::SettleTaskCommit(input, reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }
}

impl Drop for StorageWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(worker_thread) = self.thread.take() {
            let _ = worker_thread.join();
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run_worker(database: &Database, receiver: &mpsc::Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::CreateDraftRun(input, reply) => {
                let _ = reply.send(database.create_draft_run(&input));
            }
            Command::BeginPlanAttempt(input, reply) => {
                let _ = reply.send(database.begin_plan_attempt(&input));
            }
            Command::CompletePlanAttempt(input, reply) => {
                let _ = reply.send(database.complete_plan_attempt(&input));
            }
            Command::FailPlanAttempt(input, reply) => {
                let _ = reply.send(database.fail_plan_attempt(&input));
            }
            Command::RevisePlan(input, reply) => {
                let _ = reply.send(database.revise_plan(&input));
            }
            Command::ApprovePlan {
                run_id,
                plan_id,
                reply,
            } => {
                let _ = reply.send(database.decide_plan(&run_id, &plan_id, true, None));
            }
            Command::RejectPlan {
                run_id,
                plan_id,
                reason,
                reply,
            } => {
                let _ =
                    reply.send(database.decide_plan(&run_id, &plan_id, false, reason.as_deref()));
            }
            Command::ReserveTaskWorktree(input, reply) => {
                let _ = reply.send(database.reserve_task_worktree(&input));
            }
            Command::ConfirmTaskWorktree {
                worktree_id,
                repository_dirty,
                reply,
            } => {
                let _ = reply.send(database.confirm_task_worktree(&worktree_id, repository_dirty));
            }
            Command::FailTaskWorktree {
                worktree_id,
                error_message,
                reply,
            } => {
                let _ = reply.send(database.settle_task_worktree(
                    &worktree_id,
                    TaskWorktreeStatus::Failed,
                    Some(&error_message),
                ));
            }
            Command::SettleTaskWorktree {
                worktree_id,
                status,
                detail,
                reply,
            } => {
                let _ = reply.send(database.settle_task_worktree(
                    &worktree_id,
                    status,
                    detail.as_deref(),
                ));
            }
            Command::BeginImplementationAttempt(input, reply) => {
                let _ = reply.send(database.begin_implementation_attempt(&input));
            }
            Command::CompleteImplementationAttempt(input, reply) => {
                let _ = reply.send(database.complete_implementation_attempt(&input));
            }
            Command::FailImplementationAttempt(input, reply) => {
                let _ = reply.send(database.fail_implementation_attempt(&input));
            }
            Command::AppendImplementationActivity(input, reply) => {
                let _ = reply.send(database.append_implementation_activity(&input));
            }
            Command::CancelImplementationAttempt(input, reply) => {
                let _ = reply.send(database.cancel_implementation_attempt(&input));
            }
            Command::SetImplementationPaused {
                attempt_id,
                paused,
                reply,
            } => {
                let _ = reply.send(database.set_implementation_paused(&attempt_id, paused));
            }
            Command::ReserveImplementationContinuation(input, reply) => {
                let _ = reply.send(database.reserve_implementation_continuation(&input));
            }
            Command::BeginReviewAttempt(input, reply) => {
                let _ = reply.send(database.begin_review_attempt(&input));
            }
            Command::CompleteReviewAttempt(input, reply) => {
                let _ = reply.send(database.complete_review_attempt(&input));
            }
            Command::FailReviewAttempt(input, reply) => {
                let _ = reply.send(database.fail_review_attempt(&input));
            }
            Command::RecordVerificationAttempt(input, reply) => {
                let _ = reply.send(database.record_verification_attempt(&input));
            }
            Command::RecordTaskCommit(input, reply) => {
                let _ = reply.send(database.record_task_commit(&input));
            }
            Command::ReserveTaskCommit { commit_id, reply } => {
                let _ = reply.send(database.reserve_task_commit(&commit_id));
            }
            Command::SettleTaskCommit(input, reply) => {
                let _ = reply.send(database.settle_task_commit(&input));
            }
            Command::CurrentSnapshot(reply) => {
                let _ = reply.send(database.current_snapshot());
            }
            Command::Health(reply) => {
                let _ = reply.send(database.health());
            }
            Command::Shutdown => return,
        }
    }
}

struct Database {
    connection: Connection,
    path: PathBuf,
}

impl Database {
    fn open(paths: &StatePaths) -> Result<Self, StorageError> {
        ensure_private_directory(paths.root())?;
        ensure_private_directory(paths.artifacts())?;
        ensure_private_directory(paths.worktrees())?;

        let mut connection = Connection::open(paths.database())?;
        set_file_mode(paths.database(), 0o600)?;
        configure_connection(&connection)?;
        apply_migrations(&mut connection)?;
        recover_interrupted_planning(&mut connection)?;
        recover_interrupted_worktrees(&mut connection)?;
        recover_interrupted_implementations(&mut connection)?;
        recover_interrupted_reviews(&mut connection)?;
        recover_interrupted_task_commits(&mut connection)?;

        Ok(Self {
            connection,
            path: paths.database().to_path_buf(),
        })
    }

    fn health(&self) -> Result<StorageHealth, StorageError> {
        let schema_version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(StorageHealth {
            database_path: self.path.clone(),
            schema_version,
        })
    }

    fn current_snapshot(&self) -> Result<StoredSnapshot, StorageError> {
        let sequence = self.connection.query_row(
            "SELECT coalesce(max(sequence), 0) FROM run_events",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let mut active_run = self
            .connection
            .query_row(
                "SELECT r.id, r.goal, p.repository_path, r.base_revision, r.branch, \
                        r.worktree_dirty, r.status, r.last_error \
                 FROM runs r \
                 JOIN projects p ON p.id = r.project_id \
                 WHERE r.status NOT IN ('completed', 'rejected', 'cancelled') \
                 ORDER BY r.updated_at DESC, r.rowid DESC \
                 LIMIT 1",
                [],
                row_to_run,
            )
            .optional()?;

        if let Some(run) = &mut active_run {
            run.plan = self.load_latest_plan(&run.id)?;
            run.worktrees = self.load_task_worktrees(&run.id)?;
            run.implementation_attempts = self.load_implementation_attempts(&run.id)?;
            run.implementation_activity = self.load_implementation_activity(&run.id)?;
            run.review_attempts = self.load_review_attempts(&run.id)?;
            run.verification_attempts = self.load_verification_attempts(&run.id)?;
            run.task_commits = self.load_task_commits(&run.id)?;
        }

        Ok(StoredSnapshot {
            sequence: sequence_to_u64(sequence)?,
            active_run,
        })
    }

    fn create_draft_run(&self, input: &DraftRunInput) -> Result<StoredSnapshot, StorageError> {
        let created_at = unix_milliseconds()?;
        let project_candidate = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7().to_string();
        let transaction = self.connection.unchecked_transaction()?;

        let project_id: String = transaction.query_row(
            "INSERT INTO projects(\
                id, repository_path, git_common_dir, created_at, last_opened_at\
             ) VALUES (?1, ?2, ?3, ?4, ?4) \
             ON CONFLICT(repository_path) DO UPDATE SET \
                git_common_dir = excluded.git_common_dir, \
                last_opened_at = excluded.last_opened_at \
             RETURNING id",
            (
                &project_candidate,
                &input.repository_path,
                &input.git_common_dir,
                created_at,
            ),
            |row| row.get(0),
        )?;

        transaction.execute(
            "INSERT INTO runs(\
                id, project_id, goal, base_revision, branch, worktree_dirty, \
                status, created_at, updated_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'draft', ?7, ?7)",
            (
                &run_id,
                &project_id,
                &input.goal,
                &input.base_revision,
                &input.branch,
                input.worktree_dirty,
                created_at,
            ),
        )?;

        let payload = json!({
            "repository": input.repository_path,
            "base_revision": input.base_revision,
            "branch": input.branch,
            "worktree_dirty": input.worktree_dirty,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'draft_created', 'user', ?2, ?3)",
            (&run_id, payload.to_string(), created_at),
        )?;
        let sequence = transaction.last_insert_rowid();
        transaction.commit()?;

        Ok(StoredSnapshot {
            sequence: sequence_to_u64(sequence)?,
            active_run: Some(ActiveRunSummary {
                id: run_id,
                goal: input.goal.clone(),
                repository: input.repository_path.clone(),
                base_revision: input.base_revision.clone(),
                branch: input.branch.clone(),
                worktree_dirty: input.worktree_dirty,
                run_status: RunStatus::Draft,
                plan: None,
                worktrees: Vec::new(),
                implementation_attempts: Vec::new(),
                implementation_activity: Vec::new(),
                review_attempts: Vec::new(),
                verification_attempts: Vec::new(),
                task_commits: Vec::new(),
                last_error: None,
            }),
        })
    }

    fn begin_plan_attempt(
        &self,
        input: &PlanAttemptInput,
    ) -> Result<StartedPlanAttempt, StorageError> {
        let started_at = unix_milliseconds()?;
        let attempt_id = uuid::Uuid::now_v7().to_string();
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE runs SET status = 'planning', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1 AND status IN ('draft', 'failed')",
            (&input.run_id, started_at),
        )?;
        if changed == 0 {
            return Err(if run_exists(&transaction, &input.run_id)? {
                StorageError::RunNotPlannable(input.run_id.clone())
            } else {
                StorageError::RunNotFound(input.run_id.clone())
            });
        }

        transaction.execute(
            "INSERT INTO plan_attempts(\
                id, run_id, agent, status, prompt, started_at\
             ) VALUES (?1, ?2, ?3, 'running', ?4, ?5)",
            (
                &attempt_id,
                &input.run_id,
                input.agent.as_str(),
                &input.prompt,
                started_at,
            ),
        )?;
        let payload = json!({
            "attempt_id": attempt_id,
            "agent": input.agent,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'planning_started', ?2, ?3, ?4)",
            (
                &input.run_id,
                input.agent.as_str(),
                payload.to_string(),
                started_at,
            ),
        )?;
        transaction.commit()?;

        Ok(StartedPlanAttempt {
            attempt_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn complete_plan_attempt(
        &self,
        input: &PlanAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, agent) = running_attempt(&transaction, &input.attempt_id)?;

        transaction.execute(
            "UPDATE plans SET status = 'superseded' \
             WHERE run_id = ?1 AND status = 'proposed'",
            [&run_id],
        )?;
        let revision = next_plan_revision(&transaction, &run_id)?;
        let plan_id = insert_plan(
            &transaction,
            &run_id,
            None,
            Some(&input.attempt_id),
            agent,
            revision,
            &input.proposal,
            completed_at,
        )?;
        let changed = transaction.execute(
            "UPDATE plan_attempts SET \
                status = 'completed', final_output = ?2, diagnostic_output = ?3, \
                exit_code = ?4, completed_at = ?5 \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::AttemptNotRunning(input.attempt_id.clone()));
        }
        transaction.execute(
            "UPDATE runs SET status = 'waiting_for_user', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1",
            (&run_id, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "plan_id": plan_id,
            "revision": revision,
            "agent": agent,
            "task_count": input.proposal.tasks.len(),
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'plan_proposed', ?2, ?3, ?4)",
            (&run_id, agent.as_str(), payload.to_string(), completed_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn fail_plan_attempt(
        &self,
        input: &PlanAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, agent) = running_attempt(&transaction, &input.attempt_id)?;
        let changed = transaction.execute(
            "UPDATE plan_attempts SET \
                status = 'failed', final_output = ?2, diagnostic_output = ?3, \
                exit_code = ?4, error_message = ?5, completed_at = ?6 \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                &input.error_message,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::AttemptNotRunning(input.attempt_id.clone()));
        }
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1",
            (&run_id, &input.error_message, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "agent": agent,
            "exit_code": input.exit_code,
            "error": input.error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'planning_failed', ?2, ?3, ?4)",
            (&run_id, agent.as_str(), payload.to_string(), completed_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn revise_plan(&self, input: &PlanRevisionInput) -> Result<StoredSnapshot, StorageError> {
        let created_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let planner =
            current_proposed_plan_agent(&transaction, &input.run_id, &input.based_on_plan_id)?;
        transaction.execute(
            "UPDATE plans SET status = 'superseded' WHERE id = ?1",
            [&input.based_on_plan_id],
        )?;
        let revision = next_plan_revision(&transaction, &input.run_id)?;
        let plan_id = insert_plan(
            &transaction,
            &input.run_id,
            Some(&input.based_on_plan_id),
            None,
            planner,
            revision,
            &input.proposal,
            created_at,
        )?;
        transaction.execute(
            "UPDATE runs SET status = 'waiting_for_user', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1",
            (&input.run_id, created_at),
        )?;
        let payload = json!({
            "plan_id": plan_id,
            "based_on_plan_id": input.based_on_plan_id,
            "revision": revision,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'plan_revised', 'user', ?2, ?3)",
            (&input.run_id, payload.to_string(), created_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn decide_plan(
        &self,
        run_id: &str,
        plan_id: &str,
        approved: bool,
        reason: Option<&str>,
    ) -> Result<StoredSnapshot, StorageError> {
        let decided_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let plan_status = if approved { "approved" } else { "rejected" };
        let changed = transaction.execute(
            "UPDATE plans SET status = ?3, decided_at = ?4 \
             WHERE id = ?2 AND run_id = ?1 AND status = 'proposed' \
               AND revision = (SELECT max(revision) FROM plans WHERE run_id = ?1)",
            (run_id, plan_id, plan_status, decided_at),
        )?;
        if changed != 1 {
            return Err(StorageError::PlanNotCurrent(plan_id.to_owned()));
        }
        let run_status = if approved {
            "waiting_for_user"
        } else {
            "draft"
        };
        transaction.execute(
            "UPDATE runs SET status = ?2, last_error = NULL, updated_at = ?3 WHERE id = ?1",
            (run_id, run_status, decided_at),
        )?;
        let kind = if approved {
            "plan_approved"
        } else {
            "plan_rejected"
        };
        let payload = json!({
            "plan_id": plan_id,
            "reason": reason,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, ?2, 'user', ?3, ?4)",
            (run_id, kind, payload.to_string(), decided_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn reserve_task_worktree(
        &self,
        input: &TaskWorktreeReservation,
    ) -> Result<ReservedTaskWorktree, StorageError> {
        let created_at = unix_milliseconds()?;
        let worktree_id = uuid::Uuid::now_v7().to_string();
        let transaction = self.connection.unchecked_transaction()?;

        let approved = transaction
            .query_row(
                "SELECT 1 FROM plans p \
                 JOIN runs r ON r.id = p.run_id \
                 JOIN plan_tasks t ON t.plan_id = p.id \
                 WHERE p.id = ?2 AND p.run_id = ?1 AND t.id = ?3 \
                   AND p.status = 'approved' \
                   AND p.revision = (SELECT max(revision) FROM plans WHERE run_id = ?1) \
                   AND r.status NOT IN ('completed', 'rejected', 'cancelled')",
                (&input.run_id, &input.plan_id, &input.task_id),
                |_| Ok(true),
            )
            .optional()?;
        if approved.is_none() {
            return Err(if run_exists(&transaction, &input.run_id)? {
                StorageError::TaskNotImplementable(input.task_id.clone())
            } else {
                StorageError::RunNotFound(input.run_id.clone())
            });
        }

        transaction
            .execute(
                "INSERT INTO task_worktrees(\
                id, run_id, plan_id, task_id, status, branch, path, \
                base_revision, repository_dirty, created_at, updated_at\
             ) VALUES (?1, ?2, ?3, ?4, 'reserved', ?5, ?6, ?7, 0, ?8, ?8)",
                (
                    &worktree_id,
                    &input.run_id,
                    &input.plan_id,
                    &input.task_id,
                    &input.branch,
                    &input.path,
                    &input.base_revision,
                    created_at,
                ),
            )
            .map_err(|error| {
                // The live partial indexes are the reservation itself, so a
                // constraint violation here means the task, branch, or
                // directory is already held rather than that storage broke.
                if is_constraint_violation(&error) {
                    StorageError::WorktreeAlreadyLive
                } else {
                    StorageError::Sqlite(error)
                }
            })?;
        let payload = json!({
            "worktree_id": worktree_id,
            "task_id": input.task_id,
            "branch": input.branch,
            "path": input.path,
            "base_revision": input.base_revision,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'worktree_reserved', 'engine', ?2, ?3)",
            (&input.run_id, payload.to_string(), created_at),
        )?;
        transaction.commit()?;

        Ok(ReservedTaskWorktree {
            worktree_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn confirm_task_worktree(
        &self,
        worktree_id: &str,
        repository_dirty: bool,
    ) -> Result<StoredSnapshot, StorageError> {
        let confirmed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE task_worktrees SET \
                status = 'ready', repository_dirty = ?2, last_error = NULL, updated_at = ?3 \
             WHERE id = ?1 AND status = 'reserved'",
            (worktree_id, repository_dirty, confirmed_at),
        )?;
        if changed != 1 {
            return Err(StorageError::WorktreeNotReserved(worktree_id.to_owned()));
        }
        let (run_id, branch, path) = worktree_identity(&transaction, worktree_id)?;
        let payload = json!({
            "worktree_id": worktree_id,
            "branch": branch,
            "path": path,
            "repository_dirty": repository_dirty,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'worktree_ready', 'engine', ?2, ?3)",
            (&run_id, payload.to_string(), confirmed_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn settle_task_worktree(
        &self,
        worktree_id: &str,
        status: TaskWorktreeStatus,
        error_message: Option<&str>,
    ) -> Result<StoredSnapshot, StorageError> {
        let settled_at = unix_milliseconds()?;
        let (expected, kind) = match status {
            TaskWorktreeStatus::Failed => ("reserved", "worktree_failed"),
            TaskWorktreeStatus::Missing => ("ready", "worktree_missing"),
            TaskWorktreeStatus::Diverged => ("ready", "worktree_diverged"),
            other => {
                return Err(StorageError::InvalidWorktreeStatus(
                    other.as_str().to_owned(),
                ));
            }
        };
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE task_worktrees SET status = ?2, last_error = ?3, updated_at = ?4 \
             WHERE id = ?1 AND status = ?5",
            (
                worktree_id,
                status.as_str(),
                error_message,
                settled_at,
                expected,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::WorktreeNotReserved(worktree_id.to_owned()));
        }
        let (run_id, branch, path) = worktree_identity(&transaction, worktree_id)?;
        let payload = json!({
            "worktree_id": worktree_id,
            "branch": branch,
            "path": path,
            "error": error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, ?2, 'engine', ?3, ?4)",
            (&run_id, kind, payload.to_string(), settled_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    #[allow(clippy::too_many_lines)]
    fn begin_implementation_attempt(
        &self,
        input: &ImplementationAttemptInput,
    ) -> Result<StartedImplementationAttempt, StorageError> {
        let started_at = unix_milliseconds()?;
        let attempt_id = uuid::Uuid::now_v7().to_string();
        let transaction = self.connection.unchecked_transaction()?;
        let ready = transaction
            .query_row(
                "SELECT 1 FROM task_worktrees w \
                 JOIN plans p ON p.id = w.plan_id \
                 JOIN runs r ON r.id = w.run_id \
                 WHERE w.id = ?4 AND w.run_id = ?1 AND w.plan_id = ?2 AND w.task_id = ?3 \
                   AND w.status = 'ready' AND p.status = 'approved' \
                   AND p.revision = (SELECT max(revision) FROM plans WHERE run_id = ?1) \
                   AND r.status IN ('waiting_for_user', 'failed')",
                (
                    &input.run_id,
                    &input.plan_id,
                    &input.task_id,
                    &input.worktree_id,
                ),
                |_| Ok(true),
            )
            .optional()?;
        if ready.is_none() {
            return Err(if run_exists(&transaction, &input.run_id)? {
                StorageError::ImplementationNotReady(input.task_id.clone())
            } else {
                StorageError::RunNotFound(input.run_id.clone())
            });
        }

        match (
            input.parent_attempt_id.as_deref(),
            input.continuation_kind,
            input.user_instruction.as_deref(),
        ) {
            (None, None, None) => {}
            (Some(parent_id), Some(kind), Some(instruction))
                if !instruction.trim().is_empty() && instruction.chars().count() <= 20_000 =>
            {
                let valid_parent = transaction
                    .query_row(
                        "SELECT 1 FROM implementation_attempts \
                         WHERE id = ?1 AND run_id = ?2 AND plan_id = ?3 AND task_id = ?4 \
                           AND worktree_id = ?5 AND agent = ?6 \
                           AND status IN ('completed', 'failed', 'cancelled') \
                           AND pending_continuation_kind = ?7 \
                           AND pending_user_instruction = ?8",
                        (
                            parent_id,
                            &input.run_id,
                            &input.plan_id,
                            &input.task_id,
                            &input.worktree_id,
                            input.agent.as_str(),
                            kind.as_str(),
                            instruction,
                        ),
                        |_| Ok(true),
                    )
                    .optional()?;
                if valid_parent.is_none() {
                    return Err(StorageError::ImplementationNotReady(input.task_id.clone()));
                }
            }
            _ => return Err(StorageError::ImplementationNotReady(input.task_id.clone())),
        }

        let changed = transaction.execute(
            "UPDATE runs SET status = 'running', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1 AND status IN ('waiting_for_user', 'failed')",
            (&input.run_id, started_at),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationNotReady(input.task_id.clone()));
        }
        transaction.execute(
            "INSERT INTO implementation_attempts(\
                id, run_id, plan_id, task_id, worktree_id, agent, status, prompt, started_at, \
                parent_attempt_id, continuation_kind, user_instruction\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?9, ?10, ?11)",
            (
                &attempt_id,
                &input.run_id,
                &input.plan_id,
                &input.task_id,
                &input.worktree_id,
                input.agent.as_str(),
                &input.prompt,
                started_at,
                &input.parent_attempt_id,
                input
                    .continuation_kind
                    .map(ImplementationContinuationKind::as_str),
                &input.user_instruction,
            ),
        )?;
        if let Some(parent_attempt_id) = &input.parent_attempt_id {
            let consumed = transaction.execute(
                "UPDATE implementation_attempts SET \
                    pending_continuation_kind = NULL, pending_user_instruction = NULL \
                 WHERE id = ?1 AND pending_continuation_kind = ?2 \
                   AND pending_user_instruction = ?3",
                (
                    parent_attempt_id,
                    input
                        .continuation_kind
                        .map(ImplementationContinuationKind::as_str),
                    &input.user_instruction,
                ),
            )?;
            if consumed != 1 {
                return Err(StorageError::ImplementationNotReady(input.task_id.clone()));
            }
        }
        let payload = json!({
            "attempt_id": attempt_id,
            "task_id": input.task_id,
            "worktree_id": input.worktree_id,
            "agent": input.agent,
            "parent_attempt_id": input.parent_attempt_id,
            "continuation_kind": input.continuation_kind,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_started', ?2, ?3, ?4)",
            (
                &input.run_id,
                input.agent.as_str(),
                payload.to_string(),
                started_at,
            ),
        )?;
        transaction.commit()?;

        Ok(StartedImplementationAttempt {
            attempt_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn complete_implementation_attempt(
        &self,
        input: &ImplementationAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, worktree_id, agent) =
            running_implementation_attempt(&transaction, &input.attempt_id)?;
        let changed = transaction.execute(
            "UPDATE implementation_attempts SET \
                status = 'completed', final_output = ?2, diagnostic_output = ?3, \
                exit_code = ?4, completed_at = ?5, paused_at = NULL \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationAttemptNotRunning(
                input.attempt_id.clone(),
            ));
        }
        transaction.execute(
            "UPDATE runs SET status = 'waiting_for_user', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
            "exit_code": input.exit_code,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_completed', ?2, ?3, ?4)",
            (&run_id, agent.as_str(), payload.to_string(), completed_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn fail_implementation_attempt(
        &self,
        input: &ImplementationAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, worktree_id, agent) =
            running_implementation_attempt(&transaction, &input.attempt_id)?;
        let retryable_continuation = retryable_continuation(&transaction, &input.attempt_id)?;
        let changed = transaction.execute(
            "UPDATE implementation_attempts SET \
                status = 'failed', final_output = ?2, diagnostic_output = ?3, \
                exit_code = ?4, error_message = ?5, completed_at = ?6, paused_at = NULL \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                &input.error_message,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationAttemptNotRunning(
                input.attempt_id.clone(),
            ));
        }
        restore_retryable_continuation(&transaction, retryable_continuation.as_ref())?;
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, &input.error_message, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
            "exit_code": input.exit_code,
            "error": input.error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_failed', ?2, ?3, ?4)",
            (&run_id, agent.as_str(), payload.to_string(), completed_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn append_implementation_activity(
        &self,
        input: &ImplementationActivityInput,
    ) -> Result<StoredSnapshot, StorageError> {
        let message_length = input.message.chars().count();
        if message_length == 0 || message_length > 8192 {
            return Err(StorageError::InvalidImplementationActivity);
        }
        let created_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, _worktree_id, agent) =
            running_implementation_attempt(&transaction, &input.attempt_id)?;
        transaction.execute(
            "INSERT INTO implementation_activity(\
                attempt_id, run_id, task_id, agent, kind, message, created_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &input.attempt_id,
                &run_id,
                &task_id,
                agent.as_str(),
                input.kind.as_str(),
                &input.message,
                created_at,
            ),
        )?;
        let activity_sequence = transaction.last_insert_rowid();
        let payload = json!({
            "attempt_id": input.attempt_id,
            "activity_sequence": activity_sequence,
            "kind": input.kind,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_activity', ?2, ?3, ?4)",
            (&run_id, agent.as_str(), payload.to_string(), created_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn cancel_implementation_attempt(
        &self,
        input: &ImplementationAttemptCancellation,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, worktree_id, agent) =
            running_implementation_attempt(&transaction, &input.attempt_id)?;
        let changed = transaction.execute(
            "UPDATE implementation_attempts SET \
                status = 'cancelled', final_output = ?2, diagnostic_output = ?3, \
                error_message = ?4, completed_at = ?5, stop_reason = ?6, paused_at = NULL, \
                pending_continuation_kind = CASE WHEN ?6 = 'cancelled' THEN NULL \
                    ELSE pending_continuation_kind END, \
                pending_user_instruction = CASE WHEN ?6 = 'cancelled' THEN NULL \
                    ELSE pending_user_instruction END \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                &input.error_message,
                completed_at,
                input.stop_reason.as_str(),
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationAttemptNotRunning(
                input.attempt_id.clone(),
            ));
        }
        transaction.execute(
            "UPDATE runs SET status = 'waiting_for_user', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
            "stop_reason": input.stop_reason,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, ?2, 'user', ?3, ?4)",
            (
                &run_id,
                match input.stop_reason {
                    ImplementationStopReason::Cancelled => "implementation_cancelled",
                    ImplementationStopReason::Redirected => "implementation_redirected",
                    ImplementationStopReason::ContextAdded => "implementation_context_added",
                },
                payload.to_string(),
                completed_at,
            ),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn set_implementation_paused(
        &self,
        attempt_id: &str,
        paused: bool,
    ) -> Result<StoredSnapshot, StorageError> {
        let changed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, worktree_id, agent) =
            running_implementation_attempt(&transaction, attempt_id)?;
        let changed = if paused {
            transaction.execute(
                "UPDATE implementation_attempts SET paused_at = ?2 \
                 WHERE id = ?1 AND status = 'running' AND paused_at IS NULL",
                (attempt_id, changed_at),
            )?
        } else {
            transaction.execute(
                "UPDATE implementation_attempts SET paused_at = NULL \
                 WHERE id = ?1 AND status = 'running' AND paused_at IS NOT NULL",
                [attempt_id],
            )?
        };
        if changed != 1 {
            return Err(StorageError::ImplementationAttemptNotRunning(
                attempt_id.to_owned(),
            ));
        }
        let payload = json!({
            "attempt_id": attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, ?2, 'user', ?3, ?4)",
            (
                &run_id,
                if paused {
                    "implementation_paused"
                } else {
                    "implementation_resumed"
                },
                payload.to_string(),
                changed_at,
            ),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn reserve_implementation_continuation(
        &self,
        input: &ImplementationContinuationReservation,
    ) -> Result<StoredSnapshot, StorageError> {
        let instruction = input.instruction.trim();
        if instruction.is_empty() || instruction.chars().count() > 20_000 {
            return Err(StorageError::ImplementationNotReady(
                input.attempt_id.clone(),
            ));
        }
        let requested_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, worktree_id, agent) = transaction
            .query_row(
                "SELECT run_id, task_id, worktree_id, agent FROM implementation_attempts \
                 WHERE id = ?1 AND status IN ('running', 'completed')",
                [&input.attempt_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(run_id, task_id, worktree_id, agent)| {
                parse_agent_kind(&agent).map(|agent| (run_id, task_id, worktree_id, agent))
            })
            .transpose()?
            .ok_or_else(|| {
                StorageError::ImplementationAttemptNotRunning(input.attempt_id.clone())
            })?;
        let changed = transaction.execute(
            "UPDATE implementation_attempts SET \
                pending_continuation_kind = ?2, pending_user_instruction = ?3 \
             WHERE id = ?1 AND status IN ('running', 'completed') \
               AND pending_continuation_kind IS NULL AND pending_user_instruction IS NULL",
            (&input.attempt_id, input.kind.as_str(), instruction),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationNotReady(
                input.attempt_id.clone(),
            ));
        }
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
            "continuation_kind": input.kind,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_continuation_requested', 'user', ?2, ?3)",
            (&run_id, payload.to_string(), requested_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn begin_review_attempt(
        &self,
        input: &ReviewAttemptInput,
    ) -> Result<StartedReviewAttempt, StorageError> {
        let valid_independence = match input.independence {
            ReviewIndependence::CrossProvider => input.reviewer != input.implementer,
            ReviewIndependence::FreshSessionFallback => {
                input.policy == ReviewPolicy::CrossProviderOrFreshSession
                    && input.reviewer == input.implementer
            }
        };
        if !valid_independence || input.prompt.trim().is_empty() {
            return Err(StorageError::ReviewNotReady(input.task_id.clone()));
        }
        let started_at = unix_milliseconds()?;
        let attempt_id = uuid::Uuid::now_v7().to_string();
        let transaction = self.connection.unchecked_transaction()?;
        let ready = transaction
            .query_row(
                "SELECT 1 FROM implementation_attempts i \
                 JOIN task_worktrees w ON w.id = i.worktree_id \
                 JOIN plans p ON p.id = i.plan_id \
                 JOIN runs r ON r.id = i.run_id \
                 WHERE i.id = ?5 AND i.run_id = ?1 AND i.plan_id = ?2 \
                   AND i.task_id = ?3 AND i.worktree_id = ?4 AND i.agent = ?6 \
                   AND i.status = 'completed' AND w.status = 'ready' \
                   AND p.status = 'approved' AND r.status IN ('waiting_for_user', 'failed')",
                (
                    &input.run_id,
                    &input.plan_id,
                    &input.task_id,
                    &input.worktree_id,
                    &input.implementation_attempt_id,
                    input.implementer.as_str(),
                ),
                |_| Ok(true),
            )
            .optional()?;
        if ready.is_none() {
            return Err(if run_exists(&transaction, &input.run_id)? {
                StorageError::ReviewNotReady(input.task_id.clone())
            } else {
                StorageError::RunNotFound(input.run_id.clone())
            });
        }
        let changed = transaction.execute(
            "UPDATE runs SET status = 'running', last_error = NULL, updated_at = ?2 \
             WHERE id = ?1 AND status IN ('waiting_for_user', 'failed')",
            (&input.run_id, started_at),
        )?;
        if changed != 1 {
            return Err(StorageError::ReviewNotReady(input.task_id.clone()));
        }
        transaction.execute(
            "INSERT INTO review_attempts(\
                id, run_id, plan_id, task_id, worktree_id, implementation_attempt_id, \
                implementer, reviewer, policy, independence, status, prompt, started_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'running', ?11, ?12)",
            (
                &attempt_id,
                &input.run_id,
                &input.plan_id,
                &input.task_id,
                &input.worktree_id,
                &input.implementation_attempt_id,
                input.implementer.as_str(),
                input.reviewer.as_str(),
                input.policy.as_str(),
                input.independence.as_str(),
                &input.prompt,
                started_at,
            ),
        )?;
        let payload = json!({
            "attempt_id": attempt_id,
            "task_id": input.task_id,
            "implementation_attempt_id": input.implementation_attempt_id,
            "implementer": input.implementer,
            "reviewer": input.reviewer,
            "policy": input.policy,
            "independence": input.independence,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'review_started', ?2, ?3, ?4)",
            (
                &input.run_id,
                input.reviewer.as_str(),
                payload.to_string(),
                started_at,
            ),
        )?;
        transaction.commit()?;
        Ok(StartedReviewAttempt {
            attempt_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn complete_review_attempt(
        &self,
        input: &ReviewAttemptSuccess,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, reviewer) = running_review_attempt(&transaction, &input.attempt_id)?;
        let status = match input.result.verdict {
            ReviewVerdict::Approved => ReviewStatus::Approved,
            ReviewVerdict::ChangesRequested => ReviewStatus::ChangesRequested,
            ReviewVerdict::Blocked => ReviewStatus::Blocked,
        };
        let result_json = serde_json::to_string(&input.result)
            .map_err(|error| StorageError::Json(error.to_string()))?;
        let changed = transaction.execute(
            "UPDATE review_attempts SET status = ?2, result_json = ?3, final_output = ?4, \
                diagnostic_output = ?5, exit_code = ?6, completed_at = ?7 \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                status.as_str(),
                &result_json,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ReviewAttemptNotRunning(
                input.attempt_id.clone(),
            ));
        }
        let run_status = if status == ReviewStatus::Blocked {
            "blocked"
        } else {
            "waiting_for_user"
        };
        transaction.execute(
            "UPDATE runs SET status = ?2, last_error = NULL, updated_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, run_status, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "reviewer": reviewer,
            "verdict": input.result.verdict,
            "finding_count": input.result.findings.len(),
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'review_completed', ?2, ?3, ?4)",
            (
                &run_id,
                reviewer.as_str(),
                payload.to_string(),
                completed_at,
            ),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn fail_review_attempt(
        &self,
        input: &ReviewAttemptFailure,
    ) -> Result<StoredSnapshot, StorageError> {
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let (run_id, task_id, reviewer) = running_review_attempt(&transaction, &input.attempt_id)?;
        let changed = transaction.execute(
            "UPDATE review_attempts SET status = 'failed', final_output = ?2, \
                diagnostic_output = ?3, exit_code = ?4, error_message = ?5, completed_at = ?6 \
             WHERE id = ?1 AND status = 'running'",
            (
                &input.attempt_id,
                &input.final_output,
                &input.diagnostic_output,
                input.exit_code,
                &input.error_message,
                completed_at,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ReviewAttemptNotRunning(
                input.attempt_id.clone(),
            ));
        }
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, &input.error_message, completed_at),
        )?;
        let payload = json!({
            "attempt_id": input.attempt_id,
            "task_id": task_id,
            "reviewer": reviewer,
            "error": input.error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'review_failed', ?2, ?3, ?4)",
            (
                &run_id,
                reviewer.as_str(),
                payload.to_string(),
                completed_at,
            ),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn load_implementation_activity(
        &self,
        run_id: &str,
    ) -> Result<Vec<ImplementationActivitySummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT sequence, attempt_id, task_id, agent, kind, message, created_at \
             FROM (\
               SELECT sequence, attempt_id, task_id, agent, kind, message, created_at \
               FROM implementation_activity WHERE run_id = ?1 \
               ORDER BY sequence DESC LIMIT ?2\
             ) ORDER BY sequence",
        )?;
        let rows = statement.query_map((run_id, RECENT_IMPLEMENTATION_ACTIVITY_LIMIT), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;

        let mut activity = Vec::new();
        for row in rows {
            let (sequence, attempt_id, task_id, agent, kind, message, created_at) = row?;
            activity.push(ImplementationActivitySummary {
                sequence: sequence_to_u64(sequence)?,
                attempt_id,
                task_id,
                agent: parse_agent_kind(&agent)?,
                kind: parse_implementation_activity_kind(&kind)?,
                message,
                created_at,
            });
        }
        Ok(activity)
    }

    fn load_implementation_attempts(
        &self,
        run_id: &str,
    ) -> Result<Vec<ImplementationAttemptSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, worktree_id, agent, status, paused_at IS NOT NULL, \
                    parent_attempt_id, continuation_kind, stop_reason, \
                    pending_continuation_kind, pending_user_instruction, \
                    exit_code, error_message, started_at, completed_at \
             FROM implementation_attempts WHERE run_id = ?1 ORDER BY started_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<i32>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, i64>(13)?,
                row.get::<_, Option<i64>>(14)?,
            ))
        })?;

        let mut attempts = Vec::new();
        for row in rows {
            let (
                id,
                task_id,
                worktree_id,
                agent,
                status,
                paused,
                parent_attempt_id,
                continuation_kind,
                stop_reason,
                pending_continuation_kind,
                pending_user_instruction,
                exit_code,
                error_message,
                started_at,
                completed_at,
            ) = row?;
            attempts.push(ImplementationAttemptSummary {
                id,
                task_id,
                worktree_id,
                agent: parse_agent_kind(&agent)?,
                status: parse_implementation_status(&status)?,
                paused,
                parent_attempt_id,
                continuation_kind: continuation_kind
                    .as_deref()
                    .map(parse_implementation_continuation_kind)
                    .transpose()?,
                stop_reason: stop_reason
                    .as_deref()
                    .map(parse_implementation_stop_reason)
                    .transpose()?,
                pending_continuation_kind: pending_continuation_kind
                    .as_deref()
                    .map(parse_implementation_continuation_kind)
                    .transpose()?,
                pending_user_instruction,
                exit_code,
                error_message,
                started_at,
                completed_at,
            });
        }
        Ok(attempts)
    }

    fn load_review_attempts(
        &self,
        run_id: &str,
    ) -> Result<Vec<ReviewAttemptSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, worktree_id, implementation_attempt_id, implementer, \
                    reviewer, policy, independence, status, result_json, error_message, \
                    started_at, completed_at \
             FROM review_attempts WHERE run_id = ?1 ORDER BY started_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, Option<i64>>(12)?,
            ))
        })?;
        let mut attempts = Vec::new();
        for row in rows {
            let (
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                implementer,
                reviewer,
                policy,
                independence,
                status,
                result_json,
                error_message,
                started_at,
                completed_at,
            ) = row?;
            let result = result_json
                .map(|value| {
                    serde_json::from_str::<ReviewResult>(&value)
                        .map_err(|error| StorageError::Json(error.to_string()))
                })
                .transpose()?;
            attempts.push(ReviewAttemptSummary {
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                implementer: parse_agent_kind(&implementer)?,
                reviewer: parse_agent_kind(&reviewer)?,
                policy: parse_review_policy(&policy)?,
                independence: parse_review_independence(&independence)?,
                status: parse_review_status(&status)?,
                result,
                error_message,
                started_at,
                completed_at,
            });
        }
        Ok(attempts)
    }

    fn record_verification_attempt(
        &self,
        input: &VerificationAttemptInput,
    ) -> Result<RecordedVerificationAttempt, StorageError> {
        let completed_at = unix_milliseconds()?;
        let attempt_id = uuid::Uuid::now_v7().to_string();
        let commands_json = serde_json::to_string(&input.commands)
            .map_err(|error| StorageError::Json(error.to_string()))?;
        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO verification_attempts(\
                id, run_id, plan_id, task_id, worktree_id, implementation_attempt_id, \
                status, commands_json, error_message, started_at, completed_at\
             ) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11 \
             WHERE EXISTS (SELECT 1 FROM implementation_attempts \
               WHERE id = ?6 AND run_id = ?2 AND plan_id = ?3 AND task_id = ?4 \
                 AND worktree_id = ?5 AND status = 'completed')",
            (
                &attempt_id,
                &input.run_id,
                &input.plan_id,
                &input.task_id,
                &input.worktree_id,
                &input.implementation_attempt_id,
                input.status.as_str(),
                &commands_json,
                &input.error_message,
                input.started_at,
                completed_at,
            ),
        )?;
        if inserted != 1 {
            return Err(StorageError::ImplementationNotReady(input.task_id.clone()));
        }
        let payload = json!({
            "verification_attempt_id": attempt_id,
            "task_id": input.task_id,
            "implementation_attempt_id": input.implementation_attempt_id,
            "status": input.status,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'verification_completed', 'engine', ?2, ?3)",
            (&input.run_id, payload.to_string(), completed_at),
        )?;
        transaction.commit()?;
        Ok(RecordedVerificationAttempt {
            attempt_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn record_task_commit(
        &self,
        input: &TaskCommitInput,
    ) -> Result<RecordedTaskCommit, StorageError> {
        let created_at = unix_milliseconds()?;
        let commit_id = uuid::Uuid::now_v7().to_string();
        let changed_files_json = serde_json::to_string(&input.changed_files)
            .map_err(|error| StorageError::Json(error.to_string()))?;
        let transaction = self.connection.unchecked_transaction()?;
        let inserted = transaction.execute(
            "INSERT INTO task_commits(\
                id, run_id, task_id, worktree_id, implementation_attempt_id, \
                verification_attempt_id, review_attempt_id, status, message, tree_hash, \
                changed_files_json, patch, commit_hash, error_message, decision_reason, \
                created_at, completed_at\
             ) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'proposed', ?8, ?9, ?10, ?11, \
                NULL, NULL, NULL, ?12, NULL \
             WHERE EXISTS (SELECT 1 FROM verification_attempts \
               WHERE id = ?6 AND run_id = ?2 AND task_id = ?3 AND worktree_id = ?4 \
                 AND implementation_attempt_id = ?5 AND status = 'passed') \
               AND EXISTS (SELECT 1 FROM review_attempts \
               WHERE id = ?7 AND run_id = ?2 AND task_id = ?3 AND worktree_id = ?4 \
                 AND implementation_attempt_id = ?5 AND status = 'approved') \
               AND NOT EXISTS (SELECT 1 FROM task_commits \
               WHERE run_id = ?2 AND task_id = ?3 \
                 AND status IN ('proposed', 'reserved', 'created'))",
            (
                &commit_id,
                &input.run_id,
                &input.task_id,
                &input.worktree_id,
                &input.implementation_attempt_id,
                &input.verification_attempt_id,
                &input.review_attempt_id,
                &input.message,
                &input.tree_hash,
                &changed_files_json,
                &input.patch,
                created_at,
            ),
        )?;
        if inserted != 1 {
            return Err(StorageError::ImplementationNotReady(input.task_id.clone()));
        }
        let payload = json!({
            "task_commit_id": commit_id,
            "task_id": input.task_id,
            "status": TaskCommitStatus::Proposed,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'task_commit_proposed', 'engine', ?2, ?3)",
            (&input.run_id, payload.to_string(), created_at),
        )?;
        transaction.commit()?;
        Ok(RecordedTaskCommit {
            commit_id,
            snapshot: self.current_snapshot()?,
        })
    }

    fn reserve_task_commit(&self, commit_id: &str) -> Result<StoredSnapshot, StorageError> {
        let approved_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let run_id = transaction
            .query_row(
                "SELECT run_id FROM task_commits WHERE id = ?1 AND status = 'proposed'",
                [commit_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::ImplementationNotReady(commit_id.to_owned()))?;
        let changed = transaction.execute(
            "UPDATE task_commits SET status = 'reserved', decision_reason = 'approved by user' \
             WHERE id = ?1 AND status = 'proposed'",
            [commit_id],
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationNotReady(commit_id.to_owned()));
        }
        let payload = json!({
            "task_commit_id": commit_id,
            "status": TaskCommitStatus::Reserved,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'task_commit_approved', 'user', ?2, ?3)",
            (&run_id, payload.to_string(), approved_at),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn settle_task_commit(
        &self,
        input: &TaskCommitSettlement,
    ) -> Result<StoredSnapshot, StorageError> {
        if matches!(
            input.status,
            TaskCommitStatus::Proposed | TaskCommitStatus::Reserved
        ) {
            return Err(StorageError::ImplementationNotReady(
                input.commit_id.clone(),
            ));
        }
        let completed_at = unix_milliseconds()?;
        let transaction = self.connection.unchecked_transaction()?;
        let expected_status = match input.status {
            TaskCommitStatus::Rejected => "proposed",
            TaskCommitStatus::Created | TaskCommitStatus::Failed => "reserved",
            TaskCommitStatus::Stale => "proposed_or_reserved",
            TaskCommitStatus::Proposed | TaskCommitStatus::Reserved => unreachable!(),
        };
        let run_id = transaction
            .query_row(
                "SELECT run_id FROM task_commits WHERE id = ?1 \
                   AND (?2 = 'proposed_or_reserved' AND status IN ('proposed', 'reserved') \
                     OR status = ?2)",
                (&input.commit_id, expected_status),
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::ImplementationNotReady(input.commit_id.clone()))?;
        let changed = transaction.execute(
            "UPDATE task_commits SET status = ?2, commit_hash = ?3, error_message = ?4, \
                decision_reason = coalesce(?5, decision_reason), completed_at = ?6 \
             WHERE id = ?1 AND (?7 = 'proposed_or_reserved' AND status IN ('proposed', 'reserved') \
                OR status = ?7)",
            (
                &input.commit_id,
                input.status.as_str(),
                &input.commit_hash,
                &input.error_message,
                &input.decision_reason,
                completed_at,
                expected_status,
            ),
        )?;
        if changed != 1 {
            return Err(StorageError::ImplementationNotReady(
                input.commit_id.clone(),
            ));
        }
        let payload = json!({
            "task_commit_id": input.commit_id,
            "status": input.status,
            "commit_hash": input.commit_hash,
            "reason": input.decision_reason,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'task_commit_settled', ?2, ?3, ?4)",
            (
                &run_id,
                if input.status == TaskCommitStatus::Rejected {
                    "user"
                } else {
                    "engine"
                },
                payload.to_string(),
                completed_at,
            ),
        )?;
        transaction.commit()?;
        self.current_snapshot()
    }

    fn load_verification_attempts(
        &self,
        run_id: &str,
    ) -> Result<Vec<VerificationAttemptSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, worktree_id, implementation_attempt_id, status, commands_json, \
                    error_message, started_at, completed_at \
             FROM verification_attempts WHERE run_id = ?1 ORDER BY started_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                status,
                commands_json,
                error_message,
                started_at,
                completed_at,
            ) = row?;
            let commands = serde_json::from_str(&commands_json)
                .map_err(|error| StorageError::Json(error.to_string()))?;
            Ok(VerificationAttemptSummary {
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                status: parse_verification_status(&status)?,
                commands,
                error_message,
                started_at,
                completed_at,
            })
        })
        .collect()
    }

    fn load_task_commits(&self, run_id: &str) -> Result<Vec<TaskCommitSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, worktree_id, implementation_attempt_id, verification_attempt_id, \
                    review_attempt_id, status, message, tree_hash, changed_files_json, patch, \
                    commit_hash, error_message, decision_reason, created_at, completed_at \
             FROM task_commits WHERE run_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, Option<i64>>(15)?,
            ))
        })?;
        let mut commits = Vec::new();
        for row in rows {
            let (
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                verification_attempt_id,
                review_attempt_id,
                status,
                message,
                tree_hash,
                changed_files_json,
                patch,
                commit_hash,
                error_message,
                decision_reason,
                created_at,
                completed_at,
            ) = row?;
            let changed_files = changed_files_json
                .map(|value| {
                    serde_json::from_str(&value)
                        .map_err(|error| StorageError::Json(error.to_string()))
                })
                .transpose()?
                .unwrap_or_default();
            commits.push(TaskCommitSummary {
                id,
                task_id,
                worktree_id,
                implementation_attempt_id,
                verification_attempt_id,
                review_attempt_id,
                status: parse_task_commit_status_sql(&status)?,
                message,
                tree_hash,
                changed_files,
                patch,
                commit_hash,
                error_message,
                decision_reason,
                created_at,
                completed_at,
            });
        }
        Ok(commits)
    }

    fn load_task_worktrees(&self, run_id: &str) -> Result<Vec<TaskWorktreeSummary>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, status, branch, path, base_revision, \
                    repository_dirty, last_error \
             FROM task_worktrees WHERE run_id = ?1 ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;

        let mut worktrees = Vec::new();
        for row in rows {
            let (id, task_id, status, branch, path, base_revision, repository_dirty, last_error) =
                row?;
            worktrees.push(TaskWorktreeSummary {
                id,
                task_id,
                status: parse_worktree_status(&status)?,
                branch,
                path,
                base_revision,
                repository_dirty,
                last_error,
            });
        }
        Ok(worktrees)
    }

    fn load_latest_plan(&self, run_id: &str) -> Result<Option<PlanSummary>, StorageError> {
        let plan = self
            .connection
            .query_row(
                "SELECT id, revision, planner_agent, status, summary \
                 FROM plans WHERE run_id = ?1 ORDER BY revision DESC LIMIT 1",
                [run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((plan_id, revision, planner, status, summary)) = plan else {
            return Ok(None);
        };

        let mut statement = self.connection.prepare(
            "SELECT id, position, title, description \
             FROM plan_tasks WHERE plan_id = ?1 ORDER BY position",
        )?;
        let task_rows = statement.query_map([&plan_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut tasks = Vec::new();
        for task in task_rows {
            let (task_id, position, title, description) = task?;
            let acceptance_criteria = self.load_acceptance_criteria(&task_id)?;
            let depends_on = self.load_dependencies(&plan_id, &task_id)?;
            tasks.push(PlanTaskSummary {
                id: task_id,
                position: position_to_u32(position)?,
                title,
                description,
                acceptance_criteria,
                depends_on,
            });
        }

        Ok(Some(PlanSummary {
            id: plan_id,
            revision: position_to_u32(revision)?,
            planner: parse_agent_kind(&planner)?,
            status: parse_plan_status(&status)?,
            summary,
            tasks,
        }))
    }

    fn load_acceptance_criteria(&self, task_id: &str) -> Result<Vec<String>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT criterion FROM plan_acceptance_criteria \
             WHERE task_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map([task_id], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn load_dependencies(&self, plan_id: &str, task_id: &str) -> Result<Vec<u32>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT dependency.position \
             FROM plan_task_dependencies link \
             JOIN plan_tasks dependency ON dependency.id = link.depends_on_task_id \
             WHERE link.plan_id = ?1 AND link.task_id = ?2 \
             ORDER BY dependency.position",
        )?;
        let rows = statement.query_map((plan_id, task_id), |row| row.get::<_, i64>(0))?;
        rows.map(|row| position_to_u32(row?)).collect()
    }
}

fn run_exists(transaction: &Transaction<'_>, run_id: &str) -> Result<bool, StorageError> {
    transaction
        .query_row("SELECT 1 FROM runs WHERE id = ?1", [run_id], |_| Ok(true))
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(Into::into)
}

fn running_attempt(
    transaction: &Transaction<'_>,
    attempt_id: &str,
) -> Result<(String, AgentKind), StorageError> {
    let attempt = transaction
        .query_row(
            "SELECT run_id, agent FROM plan_attempts \
             WHERE id = ?1 AND status = 'running'",
            [attempt_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((run_id, agent)) = attempt else {
        return Err(StorageError::AttemptNotRunning(attempt_id.to_owned()));
    };
    Ok((run_id, parse_agent_kind(&agent)?))
}

fn running_implementation_attempt(
    transaction: &Transaction<'_>,
    attempt_id: &str,
) -> Result<(String, String, String, AgentKind), StorageError> {
    let attempt = transaction
        .query_row(
            "SELECT run_id, task_id, worktree_id, agent FROM implementation_attempts \
             WHERE id = ?1 AND status = 'running'",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((run_id, task_id, worktree_id, agent)) = attempt else {
        return Err(StorageError::ImplementationAttemptNotRunning(
            attempt_id.to_owned(),
        ));
    };
    Ok((run_id, task_id, worktree_id, parse_agent_kind(&agent)?))
}

fn running_review_attempt(
    transaction: &Transaction<'_>,
    attempt_id: &str,
) -> Result<(String, String, AgentKind), StorageError> {
    let attempt = transaction
        .query_row(
            "SELECT run_id, task_id, reviewer FROM review_attempts \
             WHERE id = ?1 AND status = 'running'",
            [attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((run_id, task_id, reviewer)) = attempt else {
        return Err(StorageError::ReviewAttemptNotRunning(attempt_id.to_owned()));
    };
    Ok((run_id, task_id, parse_agent_kind(&reviewer)?))
}

fn current_proposed_plan_agent(
    transaction: &Transaction<'_>,
    run_id: &str,
    plan_id: &str,
) -> Result<AgentKind, StorageError> {
    let agent = transaction
        .query_row(
            "SELECT planner_agent FROM plans \
             WHERE id = ?2 AND run_id = ?1 AND status = 'proposed' \
               AND revision = (SELECT max(revision) FROM plans WHERE run_id = ?1)",
            (run_id, plan_id),
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    agent
        .as_deref()
        .map(parse_agent_kind)
        .transpose()?
        .ok_or_else(|| StorageError::PlanNotCurrent(plan_id.to_owned()))
}

fn next_plan_revision(transaction: &Transaction<'_>, run_id: &str) -> Result<u32, StorageError> {
    let revision = transaction.query_row(
        "SELECT coalesce(max(revision), 0) + 1 FROM plans WHERE run_id = ?1",
        [run_id],
        |row| row.get::<_, i64>(0),
    )?;
    position_to_u32(revision)
}

#[allow(clippy::too_many_arguments)]
fn insert_plan(
    transaction: &Transaction<'_>,
    run_id: &str,
    based_on_plan_id: Option<&str>,
    source_attempt_id: Option<&str>,
    planner: AgentKind,
    revision: u32,
    proposal: &PlanProposal,
    created_at: i64,
) -> Result<String, StorageError> {
    let plan_id = uuid::Uuid::now_v7().to_string();
    transaction.execute(
        "INSERT INTO plans(\
            id, run_id, revision, based_on_plan_id, source_attempt_id, \
            planner_agent, status, summary, created_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'proposed', ?7, ?8)",
        (
            &plan_id,
            run_id,
            revision,
            based_on_plan_id,
            source_attempt_id,
            planner.as_str(),
            &proposal.summary,
            created_at,
        ),
    )?;

    let task_ids: Vec<String> = proposal
        .tasks
        .iter()
        .map(|_| uuid::Uuid::now_v7().to_string())
        .collect();
    for (index, task) in proposal.tasks.iter().enumerate() {
        let position =
            u32::try_from(index + 1).map_err(|_| StorageError::InvalidPosition(i64::MAX))?;
        transaction.execute(
            "INSERT INTO plan_tasks(id, plan_id, position, title, description) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                &task_ids[index],
                &plan_id,
                position,
                &task.title,
                &task.description,
            ),
        )?;
        for (criterion_index, criterion) in task.acceptance_criteria.iter().enumerate() {
            let criterion_position = u32::try_from(criterion_index + 1)
                .map_err(|_| StorageError::InvalidPosition(i64::MAX))?;
            transaction.execute(
                "INSERT INTO plan_acceptance_criteria(\
                    id, task_id, position, criterion\
                 ) VALUES (?1, ?2, ?3, ?4)",
                (
                    uuid::Uuid::now_v7().to_string(),
                    &task_ids[index],
                    criterion_position,
                    criterion,
                ),
            )?;
        }
    }

    for (index, task) in proposal.tasks.iter().enumerate() {
        for dependency_position in &task.depends_on {
            let dependency_index = usize::try_from(*dependency_position)
                .ok()
                .and_then(|position| position.checked_sub(1))
                .filter(|position| *position < task_ids.len())
                .ok_or_else(|| StorageError::InvalidPosition(i64::from(*dependency_position)))?;
            transaction.execute(
                "INSERT INTO plan_task_dependencies(plan_id, task_id, depends_on_task_id) \
                 VALUES (?1, ?2, ?3)",
                (&plan_id, &task_ids[index], &task_ids[dependency_index]),
            )?;
        }
    }

    Ok(plan_id)
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActiveRunSummary> {
    let status: String = row.get(6)?;
    let run_status = parse_run_status(&status).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(ActiveRunSummary {
        id: row.get(0)?,
        goal: row.get(1)?,
        repository: row.get(2)?,
        base_revision: row.get(3)?,
        branch: row.get(4)?,
        worktree_dirty: row.get(5)?,
        run_status,
        plan: None,
        worktrees: Vec::new(),
        implementation_attempts: Vec::new(),
        implementation_activity: Vec::new(),
        review_attempts: Vec::new(),
        verification_attempts: Vec::new(),
        task_commits: Vec::new(),
        last_error: row.get(7)?,
    })
}

fn parse_verification_status(value: &str) -> Result<VerificationStatus, StorageError> {
    match value {
        "running" => Ok(VerificationStatus::Running),
        "passed" => Ok(VerificationStatus::Passed),
        "failed" => Ok(VerificationStatus::Failed),
        "infrastructure_error" => Ok(VerificationStatus::InfrastructureError),
        _ => Err(StorageError::Json(format!(
            "invalid verification status: {value}"
        ))),
    }
}

fn parse_task_commit_status_sql(value: &str) -> rusqlite::Result<TaskCommitStatus> {
    match value {
        "proposed" => Ok(TaskCommitStatus::Proposed),
        "reserved" => Ok(TaskCommitStatus::Reserved),
        "created" => Ok(TaskCommitStatus::Created),
        "rejected" => Ok(TaskCommitStatus::Rejected),
        "stale" => Ok(TaskCommitStatus::Stale),
        "failed" => Ok(TaskCommitStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_agent_kind(agent: &str) -> Result<AgentKind, StorageError> {
    match agent {
        "codex" => Ok(AgentKind::Codex),
        "claude" => Ok(AgentKind::Claude),
        _ => Err(StorageError::InvalidAgentKind(agent.to_owned())),
    }
}

fn parse_review_policy(value: &str) -> Result<ReviewPolicy, StorageError> {
    match value {
        "cross_provider_required" => Ok(ReviewPolicy::CrossProviderRequired),
        "cross_provider_or_fresh_session" => Ok(ReviewPolicy::CrossProviderOrFreshSession),
        _ => Err(StorageError::InvalidReviewPolicy(value.to_owned())),
    }
}

fn parse_review_independence(value: &str) -> Result<ReviewIndependence, StorageError> {
    match value {
        "cross_provider" => Ok(ReviewIndependence::CrossProvider),
        "fresh_session_fallback" => Ok(ReviewIndependence::FreshSessionFallback),
        _ => Err(StorageError::InvalidReviewIndependence(value.to_owned())),
    }
}

fn parse_review_status(value: &str) -> Result<ReviewStatus, StorageError> {
    match value {
        "running" => Ok(ReviewStatus::Running),
        "approved" => Ok(ReviewStatus::Approved),
        "changes_requested" => Ok(ReviewStatus::ChangesRequested),
        "blocked" => Ok(ReviewStatus::Blocked),
        "failed" => Ok(ReviewStatus::Failed),
        _ => Err(StorageError::InvalidReviewStatus(value.to_owned())),
    }
}

fn parse_implementation_status(status: &str) -> Result<ImplementationStatus, StorageError> {
    match status {
        "running" => Ok(ImplementationStatus::Running),
        "completed" => Ok(ImplementationStatus::Completed),
        "failed" => Ok(ImplementationStatus::Failed),
        "cancelled" => Ok(ImplementationStatus::Cancelled),
        _ => Err(StorageError::InvalidImplementationStatus(status.to_owned())),
    }
}

fn parse_implementation_continuation_kind(
    kind: &str,
) -> Result<ImplementationContinuationKind, StorageError> {
    match kind {
        "redirect" => Ok(ImplementationContinuationKind::Redirect),
        "additional_context" => Ok(ImplementationContinuationKind::AdditionalContext),
        _ => Err(StorageError::InvalidImplementationContinuationKind(
            kind.to_owned(),
        )),
    }
}

fn parse_implementation_stop_reason(
    reason: &str,
) -> Result<ImplementationStopReason, StorageError> {
    match reason {
        "cancelled" => Ok(ImplementationStopReason::Cancelled),
        "redirected" => Ok(ImplementationStopReason::Redirected),
        "context_added" => Ok(ImplementationStopReason::ContextAdded),
        _ => Err(StorageError::InvalidImplementationStopReason(
            reason.to_owned(),
        )),
    }
}

fn parse_implementation_activity_kind(
    kind: &str,
) -> Result<ImplementationActivityKind, StorageError> {
    match kind {
        "output" => Ok(ImplementationActivityKind::Output),
        "diagnostic" => Ok(ImplementationActivityKind::Diagnostic),
        _ => Err(StorageError::InvalidImplementationActivityKind(
            kind.to_owned(),
        )),
    }
}

fn worktree_identity(
    transaction: &Transaction<'_>,
    worktree_id: &str,
) -> Result<(String, String, String), StorageError> {
    transaction
        .query_row(
            "SELECT run_id, branch, path FROM task_worktrees WHERE id = ?1",
            [worktree_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(Into::into)
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn parse_worktree_status(status: &str) -> Result<TaskWorktreeStatus, StorageError> {
    match status {
        "reserved" => Ok(TaskWorktreeStatus::Reserved),
        "ready" => Ok(TaskWorktreeStatus::Ready),
        "missing" => Ok(TaskWorktreeStatus::Missing),
        "diverged" => Ok(TaskWorktreeStatus::Diverged),
        "failed" => Ok(TaskWorktreeStatus::Failed),
        "retired" => Ok(TaskWorktreeStatus::Retired),
        _ => Err(StorageError::InvalidWorktreeStatus(status.to_owned())),
    }
}

fn parse_plan_status(status: &str) -> Result<PlanStatus, StorageError> {
    match status {
        "proposed" => Ok(PlanStatus::Proposed),
        "approved" => Ok(PlanStatus::Approved),
        "rejected" => Ok(PlanStatus::Rejected),
        "superseded" => Ok(PlanStatus::Superseded),
        _ => Err(StorageError::InvalidPlanStatus(status.to_owned())),
    }
}

fn parse_run_status(status: &str) -> Result<RunStatus, StorageError> {
    match status {
        "draft" => Ok(RunStatus::Draft),
        "planning" => Ok(RunStatus::Planning),
        "waiting_for_user" => Ok(RunStatus::WaitingForUser),
        "running" => Ok(RunStatus::Running),
        "blocked" => Ok(RunStatus::Blocked),
        "failed" => Ok(RunStatus::Failed),
        "completed" => Ok(RunStatus::Completed),
        "rejected" => Ok(RunStatus::Rejected),
        "cancelled" => Ok(RunStatus::Cancelled),
        _ => Err(StorageError::InvalidRunStatus(status.to_owned())),
    }
}

fn sequence_to_u64(sequence: i64) -> Result<u64, StorageError> {
    u64::try_from(sequence).map_err(|_| StorageError::NegativeSequence(sequence))
}

fn position_to_u32(position: i64) -> Result<u32, StorageError> {
    u32::try_from(position).map_err(|_| StorageError::InvalidPosition(position))
}

fn unix_milliseconds() -> Result<i64, StorageError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| StorageError::ClockBeforeEpoch)?;
    i64::try_from(duration.as_millis()).map_err(|_| StorageError::ClockBeforeEpoch)
}

fn configure_connection(connection: &Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "busy_timeout", 5000_i64)?;

    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        let enabled: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !enabled.eq_ignore_ascii_case("wal") {
            return Err(StorageError::JournalMode(enabled));
        }
    }
    Ok(())
}

fn apply_migrations(connection: &mut Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY NOT NULL, \
            name TEXT NOT NULL UNIQUE, \
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))\
        ) STRICT;",
    )?;

    let current_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::FutureSchema {
            found: current_version,
            supported: LATEST_SCHEMA_VERSION,
        });
    }

    for (version, name, sql) in MIGRATIONS {
        if *version <= current_version {
            continue;
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, name) VALUES (?1, ?2)",
            (version, name),
        )?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    Ok(())
}

fn recover_interrupted_planning(connection: &mut Connection) -> Result<(), StorageError> {
    let interrupted = {
        let mut statement = connection
            .prepare("SELECT id, run_id, agent FROM plan_attempts WHERE status = 'running'")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if interrupted.is_empty() {
        return Ok(());
    }

    let recovered_at = unix_milliseconds()?;
    let error_message = "engine stopped before the planner completed";
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (attempt_id, run_id, agent) in interrupted {
        transaction.execute(
            "UPDATE plan_attempts SET \
                status = 'failed', error_message = ?2, completed_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&attempt_id, error_message, recovered_at),
        )?;
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'planning'",
            (&run_id, error_message, recovered_at),
        )?;
        let payload = json!({
            "attempt_id": attempt_id,
            "agent": agent,
            "error": error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'planning_interrupted', 'engine', ?2, ?3)",
            (&run_id, payload.to_string(), recovered_at),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Settles worktrees that were reserved but never confirmed, so an interrupted
/// engine leaves a retryable record instead of a permanently pending one.
fn recover_interrupted_worktrees(connection: &mut Connection) -> Result<(), StorageError> {
    let interrupted = {
        let mut statement = connection.prepare(
            "SELECT id, run_id, branch, path FROM task_worktrees WHERE status = 'reserved'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if interrupted.is_empty() {
        return Ok(());
    }

    let recovered_at = unix_milliseconds()?;
    let error_message = "engine stopped before the task worktree was created";
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (worktree_id, run_id, branch, path) in interrupted {
        transaction.execute(
            "UPDATE task_worktrees SET \
                status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'reserved'",
            (&worktree_id, error_message, recovered_at),
        )?;
        let payload = json!({
            "worktree_id": worktree_id,
            "branch": branch,
            "path": path,
            "error": error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'worktree_interrupted', 'engine', ?2, ?3)",
            (&run_id, payload.to_string(), recovered_at),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn recover_interrupted_implementations(connection: &mut Connection) -> Result<(), StorageError> {
    let interrupted = {
        let mut statement = connection.prepare(
            "SELECT id, run_id, task_id, worktree_id, agent, parent_attempt_id, \
                    continuation_kind, user_instruction, pending_continuation_kind, \
                    pending_user_instruction \
             FROM implementation_attempts WHERE status = 'running'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if interrupted.is_empty() {
        return Ok(());
    }

    let recovered_at = unix_milliseconds()?;
    let error_message = "engine stopped before the implementer completed";
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (
        attempt_id,
        run_id,
        task_id,
        worktree_id,
        agent,
        parent_attempt_id,
        continuation_kind,
        user_instruction,
        pending_continuation_kind,
        pending_user_instruction,
    ) in interrupted
    {
        let retryable_continuation = match (
            parent_attempt_id,
            continuation_kind,
            user_instruction,
            pending_continuation_kind,
            pending_user_instruction,
        ) {
            (Some(parent_id), Some(kind), Some(instruction), None, None) => {
                Some((parent_id, kind, instruction))
            }
            _ => None,
        };
        transaction.execute(
            "UPDATE implementation_attempts SET \
                status = 'failed', error_message = ?2, completed_at = ?3, paused_at = NULL \
             WHERE id = ?1 AND status = 'running'",
            (&attempt_id, error_message, recovered_at),
        )?;
        restore_retryable_continuation(&transaction, retryable_continuation.as_ref())?;
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, error_message, recovered_at),
        )?;
        let payload = json!({
            "attempt_id": attempt_id,
            "task_id": task_id,
            "worktree_id": worktree_id,
            "agent": agent,
            "error": error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'implementation_interrupted', 'engine', ?2, ?3)",
            (&run_id, payload.to_string(), recovered_at),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn recover_interrupted_reviews(connection: &mut Connection) -> Result<(), StorageError> {
    let interrupted = {
        let mut statement = connection.prepare(
            "SELECT id, run_id, task_id, reviewer FROM review_attempts WHERE status = 'running'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if interrupted.is_empty() {
        return Ok(());
    }
    let recovered_at = unix_milliseconds()?;
    let error_message = "engine stopped before the reviewer completed";
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (attempt_id, run_id, task_id, reviewer) in interrupted {
        transaction.execute(
            "UPDATE review_attempts SET status = 'failed', error_message = ?2, completed_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&attempt_id, error_message, recovered_at),
        )?;
        transaction.execute(
            "UPDATE runs SET status = 'failed', last_error = ?2, updated_at = ?3 \
             WHERE id = ?1 AND status = 'running'",
            (&run_id, error_message, recovered_at),
        )?;
        let payload = json!({
            "attempt_id": attempt_id,
            "task_id": task_id,
            "reviewer": reviewer,
            "error": error_message,
        });
        transaction.execute(
            "INSERT INTO run_events(run_id, kind, actor, payload_json, created_at) \
             VALUES (?1, 'review_interrupted', 'engine', ?2, ?3)",
            (&run_id, payload.to_string(), recovered_at),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn recover_interrupted_task_commits(connection: &mut Connection) -> Result<(), StorageError> {
    let recovered_at = unix_milliseconds()?;
    let error_message = "engine stopped while the local task commit was being created";
    connection.execute(
        "UPDATE task_commits SET status = 'failed', error_message = ?1, completed_at = ?2 \
         WHERE status = 'reserved'",
        (error_message, recovered_at),
    )?;
    Ok(())
}

fn retryable_continuation(
    transaction: &Transaction<'_>,
    attempt_id: &str,
) -> Result<Option<(String, String, String)>, StorageError> {
    let metadata = transaction.query_row(
        "SELECT parent_attempt_id, continuation_kind, user_instruction, \
                pending_continuation_kind, pending_user_instruction \
         FROM implementation_attempts WHERE id = ?1",
        [attempt_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    Ok(match metadata {
        (Some(parent_id), Some(kind), Some(instruction), None, None) => {
            Some((parent_id, kind, instruction))
        }
        _ => None,
    })
}

fn restore_retryable_continuation(
    transaction: &Transaction<'_>,
    continuation: Option<&(String, String, String)>,
) -> Result<(), StorageError> {
    let Some((parent_attempt_id, kind, instruction)) = continuation else {
        return Ok(());
    };
    transaction.execute(
        "UPDATE implementation_attempts SET \
            pending_continuation_kind = ?2, pending_user_instruction = ?3 \
         WHERE id = ?1 AND pending_continuation_kind IS NULL \
           AND pending_user_instruction IS NULL",
        (parent_attempt_id, kind, instruction),
    )?;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), StorageError> {
    let created = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(StorageError::FileSystem {
                    operation: "use non-directory state path",
                    path: path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "path is not a directory",
                    ),
                });
            }
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(StorageError::FileSystem {
                    operation: "use non-private state directory",
                    path: path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("directory mode {mode:04o} is not owner-only"),
                    ),
                });
            }
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|source| StorageError::FileSystem {
                    operation: "create directory",
                    path: path.to_path_buf(),
                    source,
                })?;
            true
        }
        Err(source) => {
            return Err(StorageError::FileSystem {
                operation: "inspect",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if created {
        set_file_mode(path, 0o700)?;
    }
    Ok(())
}

fn set_file_mode(path: &Path, mode: u32) -> Result<(), StorageError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        StorageError::FileSystem {
            operation: "set permissions on",
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    #[test]
    fn resolves_xdg_and_home_state_paths() {
        let xdg = StatePaths::from_environment(
            Some(OsString::from("/tmp/xdg-state")),
            Some(OsString::from("/home/tester")),
        )
        .expect("XDG path should resolve");
        assert_eq!(
            xdg.database(),
            Path::new("/tmp/xdg-state/omarchy-ai-build-orchestrator/state.db")
        );

        let fallback = StatePaths::from_environment(None, Some(OsString::from("/home/tester")))
            .expect("HOME fallback should resolve");
        assert_eq!(
            fallback.database(),
            Path::new("/home/tester/.local/state/omarchy-ai-build-orchestrator/state.db")
        );
    }

    #[test]
    fn rejects_relative_state_home() {
        let error = StatePaths::from_environment(Some(OsString::from("relative")), None)
            .expect_err("relative XDG state path should fail");
        assert!(matches!(error, StorageError::RelativeStateHome));
    }

    #[tokio::test]
    async fn initializes_private_migrated_store() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let health = worker.health().await.expect("health should succeed");

        assert_eq!(health.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(health.database_path, paths.database());
        assert_eq!(mode(paths.root()), 0o700);
        assert_eq!(mode(paths.artifacts()), 0o700);
        assert_eq!(mode(paths.database()), 0o600);

        drop(worker);

        let connection = Connection::open(paths.database()).expect("database should reopen");
        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign key pragma should be readable");
        assert_eq!(foreign_keys, 0, "pragmas are connection-local");
        let migration_count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration table should be readable");
        assert_eq!(migration_count, LATEST_SCHEMA_VERSION);
        let project_table: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'projects'",
                [],
                |row| row.get(0),
            )
            .expect("schema should be readable");
        assert_eq!(project_table, 1);
    }

    #[tokio::test]
    async fn reopens_at_the_same_schema_version() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));

        let first = StorageWorker::start(paths.clone()).expect("first worker should start");
        drop(first);
        let second = StorageWorker::start(paths).expect("second worker should start");

        assert_eq!(
            second
                .health()
                .await
                .expect("health should succeed")
                .schema_version,
            LATEST_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn persists_draft_runs_and_restores_the_latest() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");

        assert_eq!(
            worker
                .current_snapshot()
                .await
                .expect("empty snapshot should load"),
            StoredSnapshot::default()
        );

        let first = worker
            .create_draft_run(draft_input("First goal", false))
            .await
            .expect("first draft should persist");
        assert_eq!(first.sequence, 1);
        assert_eq!(
            first.active_run.as_ref().map(|run| run.goal.as_str()),
            Some("First goal")
        );

        let second = worker
            .create_draft_run(draft_input("Second goal", true))
            .await
            .expect("second draft should persist");
        assert_eq!(second.sequence, 2);
        assert_eq!(
            second.active_run.as_ref().map(|run| run.goal.as_str()),
            Some("Second goal")
        );
        assert!(
            second
                .active_run
                .as_ref()
                .is_some_and(|run| run.worktree_dirty)
        );
        drop(worker);

        let reopened = StorageWorker::start(paths.clone()).expect("storage should reopen");
        assert_eq!(
            reopened
                .current_snapshot()
                .await
                .expect("snapshot should restore"),
            second
        );
        drop(reopened);

        let connection = Connection::open(paths.database()).expect("database should reopen");
        let project_count: i64 = connection
            .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
            .expect("projects should be countable");
        let run_count: i64 = connection
            .query_row("SELECT count(*) FROM runs", [], |row| row.get(0))
            .expect("runs should be countable");
        let event_count: i64 = connection
            .query_row("SELECT count(*) FROM run_events", [], |row| row.get(0))
            .expect("events should be countable");
        assert_eq!(project_count, 1);
        assert_eq!(run_count, 2);
        assert_eq!(event_count, 2);
    }

    #[test]
    fn configures_required_sqlite_pragmas() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let database = Database::open(&paths).expect("database should open");

        let foreign_keys: i64 = database
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign key pragma should be readable");
        let journal_mode: String = database
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode should be readable");
        let synchronous: i64 = database
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("synchronous mode should be readable");

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
    }

    #[test]
    fn rejects_a_future_schema_version() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let database = Database::open(&paths).expect("database should open");
        database
            .connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("future version should be set for the test");
        drop(database);

        let Err(error) = StorageWorker::start(paths) else {
            panic!("future schema should be rejected");
        };
        assert!(matches!(
            error,
            StorageError::FutureSchema {
                found: 10,
                supported: 9
            }
        ));
    }

    #[tokio::test]
    async fn persists_plan_attempts_revisions_and_approval() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let draft = worker
            .create_draft_run(draft_input("Plan the change", false))
            .await
            .expect("draft should persist");
        let run_id = draft.active_run.expect("run should exist").id;

        let started = worker
            .begin_plan_attempt(PlanAttemptInput {
                run_id: run_id.clone(),
                agent: AgentKind::Codex,
                prompt: "Inspect the repository and propose a plan".to_owned(),
            })
            .await
            .expect("attempt should start");
        assert_eq!(
            started
                .snapshot
                .active_run
                .as_ref()
                .map(|run| &run.run_status),
            Some(&RunStatus::Planning)
        );

        let proposed = worker
            .complete_plan_attempt(PlanAttemptSuccess {
                attempt_id: started.attempt_id,
                proposal: sample_proposal(),
                final_output: "{\"summary\":\"Safe plan\"}".to_owned(),
                diagnostic_output: "planner progress".to_owned(),
                exit_code: 0,
            })
            .await
            .expect("proposal should persist");
        let plan = proposed
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("plan should be visible");
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.status, PlanStatus::Proposed);
        assert_eq!(plan.planner, AgentKind::Codex);
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[1].depends_on, vec![1]);

        let mut revised_proposal = sample_proposal();
        revised_proposal.tasks[0].title = "Inspect current behavior".to_owned();
        let revised = worker
            .revise_plan(PlanRevisionInput {
                run_id: run_id.clone(),
                based_on_plan_id: plan.id.clone(),
                proposal: revised_proposal,
            })
            .await
            .expect("revision should persist");
        let revised_plan = revised
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("revised plan should be visible");
        assert_eq!(revised_plan.revision, 2);
        assert_eq!(revised_plan.tasks[0].title, "Inspect current behavior");

        let approved = worker
            .approve_plan(run_id, revised_plan.id.clone())
            .await
            .expect("plan should be approved");
        assert_eq!(
            approved
                .active_run
                .and_then(|run| run.plan)
                .map(|plan| plan.status),
            Some(PlanStatus::Approved)
        );

        drop(worker);
        let connection = Connection::open(paths.database()).expect("database should reopen");
        let plan_count: i64 = connection
            .query_row("SELECT count(*) FROM plans", [], |row| row.get(0))
            .expect("plans should be countable");
        let attempt_count: i64 = connection
            .query_row("SELECT count(*) FROM plan_attempts", [], |row| row.get(0))
            .expect("attempts should be countable");
        assert_eq!(plan_count, 2);
        assert_eq!(attempt_count, 1);
    }

    #[tokio::test]
    async fn rejects_a_plan_and_returns_the_run_to_draft() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let worker = StorageWorker::start(StatePaths::new(temporary.path().join("state")))
            .expect("storage should start");
        let draft = worker
            .create_draft_run(draft_input("Plan the change", false))
            .await
            .expect("draft should persist");
        let run_id = draft.active_run.expect("run should exist").id;
        let started = worker
            .begin_plan_attempt(PlanAttemptInput {
                run_id: run_id.clone(),
                agent: AgentKind::Claude,
                prompt: "Propose a plan".to_owned(),
            })
            .await
            .expect("attempt should start");
        let proposed = worker
            .complete_plan_attempt(PlanAttemptSuccess {
                attempt_id: started.attempt_id,
                proposal: sample_proposal(),
                final_output: "result".to_owned(),
                diagnostic_output: String::new(),
                exit_code: 0,
            })
            .await
            .expect("proposal should persist");
        let plan_id = proposed
            .active_run
            .as_ref()
            .and_then(|run| run.plan.as_ref())
            .expect("plan should exist")
            .id
            .clone();

        let rejected = worker
            .reject_plan(run_id, plan_id, Some("Too broad".to_owned()))
            .await
            .expect("plan should be rejected");
        let run = rejected.active_run.expect("run should remain active");
        assert_eq!(run.run_status, RunStatus::Draft);
        assert_eq!(run.plan.map(|plan| plan.status), Some(PlanStatus::Rejected));
    }

    #[tokio::test]
    async fn recovers_an_interrupted_planning_attempt_as_failed() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let draft = worker
            .create_draft_run(draft_input("Plan the change", false))
            .await
            .expect("draft should persist");
        let run_id = draft.active_run.expect("run should exist").id;
        worker
            .begin_plan_attempt(PlanAttemptInput {
                run_id,
                agent: AgentKind::Codex,
                prompt: "Propose a plan".to_owned(),
            })
            .await
            .expect("attempt should start");
        drop(worker);

        let reopened = StorageWorker::start(paths.clone()).expect("storage should recover");
        let run = reopened
            .current_snapshot()
            .await
            .expect("snapshot should load")
            .active_run
            .expect("run should remain active");
        assert_eq!(run.run_status, RunStatus::Failed);
        assert_eq!(
            run.last_error.as_deref(),
            Some("engine stopped before the planner completed")
        );
    }

    #[tokio::test]
    async fn records_a_task_worktree_before_and_after_it_exists() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;

        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("reservation should persist");
        let pending = reserved
            .snapshot
            .active_run
            .as_ref()
            .expect("run should be active")
            .worktrees
            .clone();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, TaskWorktreeStatus::Reserved);
        assert_eq!(pending[0].task_id, task_id);
        assert!(!pending[0].repository_dirty);

        let ready = worker
            .confirm_task_worktree(reserved.worktree_id.clone(), true)
            .await
            .expect("confirmation should persist");
        let worktrees = ready.active_run.expect("run should be active").worktrees;
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].status, TaskWorktreeStatus::Ready);
        assert!(
            worktrees[0].repository_dirty,
            "the primary checkout condition is preserved for the user"
        );

        let missing = worker
            .settle_task_worktree(
                reserved.worktree_id.clone(),
                TaskWorktreeStatus::Missing,
                None,
            )
            .await
            .expect("missing worktree should persist");
        assert_eq!(
            missing.active_run.expect("run should be active").worktrees[0].status,
            TaskWorktreeStatus::Missing
        );
    }

    #[tokio::test]
    async fn refuses_a_second_live_worktree_and_allows_a_retry_after_failure() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;

        let first = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("first reservation should persist");
        let conflict = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 2))
            .await
            .expect_err("a task holds at most one live worktree");
        assert!(matches!(conflict, StorageError::WorktreeAlreadyLive));

        let failed = worker
            .fail_task_worktree(first.worktree_id, "Git refused the destination".to_owned())
            .await
            .expect("failure should persist");
        assert_eq!(
            failed.active_run.expect("run should be active").worktrees[0]
                .last_error
                .as_deref(),
            Some("Git refused the destination")
        );

        let retried = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("a failed record should not block a retry");
        let worktrees = retried
            .snapshot
            .active_run
            .expect("run should be active")
            .worktrees;
        assert_eq!(worktrees.len(), 2, "history is preserved beside the retry");
        assert_eq!(worktrees[0].status, TaskWorktreeStatus::Failed);
        assert_eq!(worktrees[1].status, TaskWorktreeStatus::Reserved);
    }

    #[tokio::test]
    async fn refuses_a_worktree_for_a_plan_that_is_not_approved() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths).expect("storage should start");
        let (run_id, plan_id, task_id) = proposed_plan(&worker).await;

        let error = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect_err("an unapproved plan cannot reserve a worktree");
        assert!(matches!(error, StorageError::TaskNotImplementable(_)));
    }

    #[tokio::test]
    async fn recovers_an_interrupted_reservation_as_failed() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("reservation should persist");
        drop(worker);

        let reopened = StorageWorker::start(paths).expect("storage should recover");
        let worktrees = reopened
            .current_snapshot()
            .await
            .expect("snapshot should load")
            .active_run
            .expect("run should remain active")
            .worktrees;
        assert_eq!(worktrees[0].status, TaskWorktreeStatus::Failed);
        assert_eq!(
            worktrees[0].last_error.as_deref(),
            Some("engine stopped before the task worktree was created")
        );
    }

    #[tokio::test]
    async fn persists_a_supervised_implementation_attempt() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        let worktree_id = reserved.worktree_id;
        worker
            .confirm_task_worktree(worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");

        let started = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id: run_id.clone(),
                plan_id,
                task_id: task_id.clone(),
                worktree_id: worktree_id.clone(),
                agent: AgentKind::Claude,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        let running = started
            .snapshot
            .active_run
            .as_ref()
            .expect("run should stay active");
        assert_eq!(running.run_status, RunStatus::Running);
        assert_eq!(
            running.implementation_attempts[0].status,
            ImplementationStatus::Running
        );

        let activity = worker
            .append_implementation_activity(ImplementationActivityInput {
                attempt_id: started.attempt_id.clone(),
                kind: ImplementationActivityKind::Output,
                message: "Editing the engine boundary".to_owned(),
            })
            .await
            .expect("activity should persist");
        let activity = &activity
            .active_run
            .as_ref()
            .expect("run should stay active")
            .implementation_activity;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].message, "Editing the engine boundary");
        assert_eq!(activity[0].kind, ImplementationActivityKind::Output);

        let completed = worker
            .complete_implementation_attempt(ImplementationAttemptSuccess {
                attempt_id: started.attempt_id,
                final_output: "Changed the requested files".to_owned(),
                diagnostic_output: String::new(),
                exit_code: 0,
            })
            .await
            .expect("implementation should complete");
        let run = completed.active_run.expect("run should stay active");
        assert_eq!(run.run_status, RunStatus::WaitingForUser);
        assert_eq!(
            run.implementation_attempts[0].status,
            ImplementationStatus::Completed
        );
        assert_eq!(run.implementation_attempts[0].task_id, task_id);
        assert_eq!(run.implementation_attempts[0].worktree_id, worktree_id);

        drop(worker);
        let reopened = StorageWorker::start(paths).expect("storage should reopen");
        assert_eq!(
            reopened
                .current_snapshot()
                .await
                .expect("snapshot should load")
                .active_run
                .expect("run should stay active")
                .implementation_attempts[0]
                .status,
            ImplementationStatus::Completed
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn persists_task_commit_proposal_approval_and_restart() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let worktree = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        worker
            .confirm_task_worktree(worktree.worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");
        let implementation = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id: run_id.clone(),
                plan_id: plan_id.clone(),
                task_id: task_id.clone(),
                worktree_id: worktree.worktree_id.clone(),
                agent: AgentKind::Codex,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        worker
            .complete_implementation_attempt(ImplementationAttemptSuccess {
                attempt_id: implementation.attempt_id.clone(),
                final_output: "Implemented".to_owned(),
                diagnostic_output: String::new(),
                exit_code: 0,
            })
            .await
            .expect("implementation should complete");
        let review = worker
            .begin_review_attempt(ReviewAttemptInput {
                run_id: run_id.clone(),
                plan_id: plan_id.clone(),
                task_id: task_id.clone(),
                worktree_id: worktree.worktree_id.clone(),
                implementation_attempt_id: implementation.attempt_id.clone(),
                implementer: AgentKind::Codex,
                reviewer: AgentKind::Claude,
                policy: ReviewPolicy::CrossProviderRequired,
                independence: ReviewIndependence::CrossProvider,
                prompt: "Review the exact task diff".to_owned(),
            })
            .await
            .expect("review should start");
        worker
            .complete_review_attempt(ReviewAttemptSuccess {
                attempt_id: review.attempt_id.clone(),
                result: ReviewResult {
                    verdict: ReviewVerdict::Approved,
                    summary: "The task is ready".to_owned(),
                    findings: Vec::new(),
                },
                final_output: "approved".to_owned(),
                diagnostic_output: String::new(),
                exit_code: 0,
            })
            .await
            .expect("review should complete");
        let verification = worker
            .record_verification_attempt(VerificationAttemptInput {
                run_id: run_id.clone(),
                plan_id,
                task_id: task_id.clone(),
                worktree_id: worktree.worktree_id.clone(),
                implementation_attempt_id: implementation.attempt_id.clone(),
                status: VerificationStatus::Passed,
                commands: Vec::new(),
                error_message: None,
                started_at: 1,
            })
            .await
            .expect("verification should persist");
        let input = TaskCommitInput {
            run_id: run_id.clone(),
            task_id: task_id.clone(),
            worktree_id: worktree.worktree_id.clone(),
            implementation_attempt_id: implementation.attempt_id,
            verification_attempt_id: verification.attempt_id,
            review_attempt_id: review.attempt_id,
            message: "feat: implement the change".to_owned(),
            tree_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            changed_files: vec![ChangedFileSummary {
                path: "src/lib.rs".to_owned(),
                previous_path: None,
                status: orchestrator_core::state::ChangedFileStatus::Modified,
            }],
            patch: "diff --git a/src/lib.rs b/src/lib.rs".to_owned(),
        };
        let rejected = worker
            .record_task_commit(input.clone())
            .await
            .expect("proposal should persist");
        let proposed = rejected
            .snapshot
            .active_run
            .as_ref()
            .expect("run should stay active")
            .task_commits
            .last()
            .expect("proposal should be visible");
        assert_eq!(proposed.status, TaskCommitStatus::Proposed);
        assert_eq!(proposed.changed_files[0].path, "src/lib.rs");
        assert!(
            proposed
                .patch
                .as_deref()
                .is_some_and(|patch| patch.contains("diff --git"))
        );
        let rejected = worker
            .settle_task_commit(TaskCommitSettlement {
                commit_id: rejected.commit_id,
                status: TaskCommitStatus::Rejected,
                commit_hash: None,
                error_message: None,
                decision_reason: Some("Needs another pass".to_owned()),
            })
            .await
            .expect("rejection should persist");
        assert_eq!(
            rejected
                .active_run
                .as_ref()
                .expect("run should stay active")
                .task_commits[0]
                .status,
            TaskCommitStatus::Rejected
        );

        let proposal = worker
            .record_task_commit(input)
            .await
            .expect("a new proposal should persist after rejection");

        worker
            .reserve_task_commit(proposal.commit_id.clone())
            .await
            .expect("user approval should reserve the commit");
        let created = worker
            .settle_task_commit(TaskCommitSettlement {
                commit_id: proposal.commit_id,
                status: TaskCommitStatus::Created,
                commit_hash: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned()),
                error_message: None,
                decision_reason: None,
            })
            .await
            .expect("created commit should settle");
        assert_eq!(
            created
                .active_run
                .expect("run should stay active")
                .task_commits[1]
                .status,
            TaskCommitStatus::Created
        );

        drop(worker);
        let reopened = StorageWorker::start(paths).expect("storage should reopen");
        let commit = reopened
            .current_snapshot()
            .await
            .expect("snapshot should load")
            .active_run
            .expect("run should stay active")
            .task_commits
            .pop()
            .expect("commit should survive restart");
        assert_eq!(commit.status, TaskCommitStatus::Created);
        assert_eq!(commit.changed_files.len(), 1);
    }

    #[tokio::test]
    async fn preserves_partial_activity_when_implementation_is_cancelled() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let worker = StorageWorker::start(StatePaths::new(temporary.path().join("state")))
            .expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        worker
            .confirm_task_worktree(reserved.worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");
        let started = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id,
                plan_id,
                task_id,
                worktree_id: reserved.worktree_id,
                agent: AgentKind::Codex,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        worker
            .append_implementation_activity(ImplementationActivityInput {
                attempt_id: started.attempt_id.clone(),
                kind: ImplementationActivityKind::Diagnostic,
                message: "Inspecting files".to_owned(),
            })
            .await
            .expect("activity should persist");
        worker
            .reserve_implementation_continuation(ImplementationContinuationReservation {
                attempt_id: started.attempt_id.clone(),
                kind: ImplementationContinuationKind::Redirect,
                instruction: "Use a different API".to_owned(),
            })
            .await
            .expect("a competing continuation should be reserved");

        let cancelled = worker
            .cancel_implementation_attempt(ImplementationAttemptCancellation {
                attempt_id: started.attempt_id,
                final_output: "partial result".to_owned(),
                diagnostic_output: "Inspecting files".to_owned(),
                error_message: "codex implementer was cancelled by the user".to_owned(),
                stop_reason: ImplementationStopReason::Cancelled,
            })
            .await
            .expect("cancellation should persist");
        let run = cancelled.active_run.expect("run should stay active");
        assert_eq!(run.run_status, RunStatus::WaitingForUser);
        assert_eq!(
            run.implementation_attempts[0].status,
            ImplementationStatus::Cancelled
        );
        assert!(
            run.implementation_attempts[0]
                .pending_continuation_kind
                .is_none()
        );
        assert!(
            run.implementation_attempts[0]
                .pending_user_instruction
                .is_none()
        );
        assert_eq!(run.implementation_activity.len(), 1);
        assert_eq!(run.implementation_activity[0].message, "Inspecting files");
    }

    #[tokio::test]
    async fn persists_pause_and_linked_implementation_continuation() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let worker = StorageWorker::start(StatePaths::new(temporary.path().join("state")))
            .expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        let worktree_id = reserved.worktree_id;
        worker
            .confirm_task_worktree(worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");
        let started = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id: run_id.clone(),
                plan_id: plan_id.clone(),
                task_id: task_id.clone(),
                worktree_id: worktree_id.clone(),
                agent: AgentKind::Codex,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        let paused = worker
            .set_implementation_paused(started.attempt_id.clone(), true)
            .await
            .expect("pause should persist");
        assert!(
            paused
                .active_run
                .as_ref()
                .expect("run should stay active")
                .implementation_attempts[0]
                .paused
        );
        worker
            .set_implementation_paused(started.attempt_id.clone(), false)
            .await
            .expect("resume should persist");
        worker
            .reserve_implementation_continuation(ImplementationContinuationReservation {
                attempt_id: started.attempt_id.clone(),
                kind: ImplementationContinuationKind::Redirect,
                instruction: "Use the corrected approach".to_owned(),
            })
            .await
            .expect("continuation instruction should be reserved before stopping");
        worker
            .cancel_implementation_attempt(ImplementationAttemptCancellation {
                attempt_id: started.attempt_id.clone(),
                final_output: "partial change".to_owned(),
                diagnostic_output: String::new(),
                error_message: "stopped for redirect".to_owned(),
                stop_reason: ImplementationStopReason::Redirected,
            })
            .await
            .expect("redirect stop should persist");

        let continuation = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id,
                plan_id,
                task_id,
                worktree_id,
                agent: AgentKind::Codex,
                prompt: "Inspect partial changes and use the corrected approach".to_owned(),
                parent_attempt_id: Some(started.attempt_id.clone()),
                continuation_kind: Some(ImplementationContinuationKind::Redirect),
                user_instruction: Some("Use the corrected approach".to_owned()),
            })
            .await
            .expect("linked continuation should start");
        let run = continuation
            .snapshot
            .active_run
            .expect("run should stay active");
        assert_eq!(run.implementation_attempts.len(), 2);
        assert_eq!(
            run.implementation_attempts[0].stop_reason,
            Some(ImplementationStopReason::Redirected)
        );
        assert_eq!(
            run.implementation_attempts[1].parent_attempt_id.as_deref(),
            Some(started.attempt_id.as_str())
        );
        assert_eq!(
            run.implementation_attempts[1].continuation_kind,
            Some(ImplementationContinuationKind::Redirect)
        );
        assert!(
            run.implementation_attempts[0]
                .pending_continuation_kind
                .is_none()
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn recovers_a_reserved_continuation_after_engine_interruption() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        let worktree_id = reserved.worktree_id;
        worker
            .confirm_task_worktree(worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");
        let started = worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id: run_id.clone(),
                plan_id: plan_id.clone(),
                task_id: task_id.clone(),
                worktree_id: worktree_id.clone(),
                agent: AgentKind::Claude,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        worker
            .reserve_implementation_continuation(ImplementationContinuationReservation {
                attempt_id: started.attempt_id.clone(),
                kind: ImplementationContinuationKind::AdditionalContext,
                instruction: "Keep the public API stable".to_owned(),
            })
            .await
            .expect("continuation should be durable before stopping");
        drop(worker);

        let reopened = StorageWorker::start(paths.clone()).expect("storage should recover");
        let recovered = reopened
            .current_snapshot()
            .await
            .expect("recovered snapshot should load");
        let parent = &recovered
            .active_run
            .as_ref()
            .expect("run should stay active")
            .implementation_attempts[0];
        assert_eq!(parent.status, ImplementationStatus::Failed);
        assert_eq!(
            parent.pending_continuation_kind,
            Some(ImplementationContinuationKind::AdditionalContext)
        );
        assert_eq!(
            parent.pending_user_instruction.as_deref(),
            Some("Keep the public API stable")
        );

        let continued = reopened
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id: run_id.clone(),
                plan_id: plan_id.clone(),
                task_id: task_id.clone(),
                worktree_id: worktree_id.clone(),
                agent: AgentKind::Claude,
                prompt: "Inspect partial changes and keep the public API stable".to_owned(),
                parent_attempt_id: Some(started.attempt_id.clone()),
                continuation_kind: Some(ImplementationContinuationKind::AdditionalContext),
                user_instruction: Some("Keep the public API stable".to_owned()),
            })
            .await
            .expect("reserved continuation should remain startable");
        assert_eq!(
            continued
                .snapshot
                .active_run
                .expect("run should stay active")
                .implementation_attempts
                .len(),
            2
        );

        drop(reopened);
        let recovered_child = StorageWorker::start(paths.clone())
            .expect("storage should recover a continuation interrupted before launch");
        let recovered = recovered_child
            .current_snapshot()
            .await
            .expect("recovered continuation snapshot should load");
        let attempts = &recovered
            .active_run
            .as_ref()
            .expect("run should stay active")
            .implementation_attempts;
        assert_eq!(attempts[1].status, ImplementationStatus::Failed);
        assert_eq!(
            attempts[0].pending_continuation_kind,
            Some(ImplementationContinuationKind::AdditionalContext)
        );
        assert_eq!(
            attempts[0].pending_user_instruction.as_deref(),
            Some("Keep the public API stable")
        );

        let retried = recovered_child
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id,
                plan_id,
                task_id,
                worktree_id,
                agent: AgentKind::Claude,
                prompt: "Retry with the saved context".to_owned(),
                parent_attempt_id: Some(started.attempt_id.clone()),
                continuation_kind: Some(ImplementationContinuationKind::AdditionalContext),
                user_instruction: Some("Keep the public API stable".to_owned()),
            })
            .await
            .expect("recovered continuation should remain retryable");
        let failed = recovered_child
            .fail_implementation_attempt(ImplementationAttemptFailure {
                attempt_id: retried.attempt_id,
                final_output: String::new(),
                diagnostic_output: "provider executable not found".to_owned(),
                exit_code: None,
                error_message: "cannot start Claude CLI".to_owned(),
            })
            .await
            .expect("launch failure should persist");
        let attempts = &failed
            .active_run
            .as_ref()
            .expect("run should stay active")
            .implementation_attempts;
        assert_eq!(
            attempts[0].pending_continuation_kind,
            Some(ImplementationContinuationKind::AdditionalContext)
        );
        assert_eq!(
            attempts[0].pending_user_instruction.as_deref(),
            Some("Keep the public API stable")
        );
    }

    #[tokio::test]
    async fn recovers_an_interrupted_implementation_as_failed() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let (run_id, plan_id, task_id) = approved_plan(&worker).await;
        let reserved = worker
            .reserve_task_worktree(reservation(&run_id, &plan_id, &task_id, 1))
            .await
            .expect("worktree should be reserved");
        worker
            .confirm_task_worktree(reserved.worktree_id.clone(), false)
            .await
            .expect("worktree should be ready");
        worker
            .begin_implementation_attempt(ImplementationAttemptInput {
                run_id,
                plan_id,
                task_id,
                worktree_id: reserved.worktree_id,
                agent: AgentKind::Codex,
                prompt: "Implement the approved task".to_owned(),
                parent_attempt_id: None,
                continuation_kind: None,
                user_instruction: None,
            })
            .await
            .expect("implementation should start");
        drop(worker);

        let reopened = StorageWorker::start(paths).expect("storage should recover");
        let run = reopened
            .current_snapshot()
            .await
            .expect("snapshot should load")
            .active_run
            .expect("run should remain active");
        assert_eq!(run.run_status, RunStatus::Failed);
        assert_eq!(
            run.implementation_attempts[0].status,
            ImplementationStatus::Failed
        );
        assert_eq!(
            run.last_error.as_deref(),
            Some("engine stopped before the implementer completed")
        );
    }

    #[test]
    fn refuses_a_symlink_as_the_state_root() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let actual = temporary.path().join("actual");
        fs::create_dir(&actual).expect("actual directory should exist");
        let linked = temporary.path().join("linked");
        symlink(&actual, &linked).expect("state symlink should exist");

        let Err(error) = StorageWorker::start(StatePaths::new(linked)) else {
            panic!("symlinked state root should be rejected");
        };
        assert!(matches!(error, StorageError::FileSystem { .. }));
    }

    #[test]
    fn refuses_a_permissive_existing_state_root() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let state_root = temporary.path().join("state");
        fs::create_dir(&state_root).expect("state root should exist");
        fs::set_permissions(&state_root, fs::Permissions::from_mode(0o755))
            .expect("test permissions should be set");

        let Err(error) = StorageWorker::start(StatePaths::new(state_root)) else {
            panic!("permissive state root should be rejected");
        };
        assert!(matches!(error, StorageError::FileSystem { .. }));
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("path metadata should exist")
            .permissions()
            .mode()
            & 0o777
    }

    fn reservation(
        run_id: &str,
        plan_id: &str,
        task_id: &str,
        attempt: u32,
    ) -> TaskWorktreeReservation {
        TaskWorktreeReservation {
            run_id: run_id.to_owned(),
            plan_id: plan_id.to_owned(),
            task_id: task_id.to_owned(),
            branch: format!("orchestrator/run/{attempt}-inspect-behavior"),
            path: format!("/tmp/state/worktrees/run/{attempt}-inspect-behavior"),
            base_revision: "0123456789012345678901234567890123456789".to_owned(),
        }
    }

    /// Drives a run to a proposed plan and returns its run, plan, and first task.
    async fn proposed_plan(worker: &StorageWorker) -> (String, String, String) {
        let draft = worker
            .create_draft_run(draft_input("Implement the change", false))
            .await
            .expect("draft should persist");
        let run_id = draft.active_run.expect("run should exist").id;
        let started = worker
            .begin_plan_attempt(PlanAttemptInput {
                run_id: run_id.clone(),
                agent: AgentKind::Codex,
                prompt: "Propose a plan".to_owned(),
            })
            .await
            .expect("attempt should start");
        let proposed = worker
            .complete_plan_attempt(PlanAttemptSuccess {
                attempt_id: started.attempt_id,
                proposal: sample_proposal(),
                final_output: "{}".to_owned(),
                diagnostic_output: String::new(),
                exit_code: 0,
            })
            .await
            .expect("proposal should persist");
        let plan = proposed
            .active_run
            .and_then(|run| run.plan)
            .expect("plan should be visible");
        (run_id, plan.id, plan.tasks[0].id.clone())
    }

    /// Drives a run all the way to an approved plan.
    async fn approved_plan(worker: &StorageWorker) -> (String, String, String) {
        let (run_id, plan_id, task_id) = proposed_plan(worker).await;
        worker
            .approve_plan(run_id.clone(), plan_id.clone())
            .await
            .expect("plan should be approved");
        (run_id, plan_id, task_id)
    }

    fn draft_input(goal: &str, worktree_dirty: bool) -> DraftRunInput {
        DraftRunInput {
            repository_path: "/tmp/project".to_owned(),
            git_common_dir: "/tmp/project/.git".to_owned(),
            goal: goal.to_owned(),
            base_revision: "0123456789012345678901234567890123456789".to_owned(),
            branch: Some("main".to_owned()),
            worktree_dirty,
        }
    }

    fn sample_proposal() -> PlanProposal {
        PlanProposal {
            summary: "Safe plan".to_owned(),
            tasks: vec![
                orchestrator_core::state::ProposedTask {
                    title: "Inspect behavior".to_owned(),
                    description: "Understand the current implementation.".to_owned(),
                    acceptance_criteria: vec!["Relevant behavior is documented.".to_owned()],
                    depends_on: vec![],
                },
                orchestrator_core::state::ProposedTask {
                    title: "Implement change".to_owned(),
                    description: "Make the smallest verified change.".to_owned(),
                    acceptance_criteria: vec!["Relevant tests pass.".to_owned()],
                    depends_on: vec![1],
                },
            ],
        }
    }
}
