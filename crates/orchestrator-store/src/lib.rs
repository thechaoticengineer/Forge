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
    ActiveRunSummary, AgentKind, PlanProposal, PlanStatus, PlanSummary, PlanTaskSummary, RunStatus,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use thiserror::Error;
use tokio::sync::oneshot;

const APPLICATION_DIRECTORY: &str = "omarchy-ai-build-orchestrator";
const DATABASE_FILE: &str = "state.db";
const ARTIFACTS_DIRECTORY: &str = "artifacts";
const LATEST_SCHEMA_VERSION: i64 = 2;
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "initial", include_str!("../migrations/0001_initial.sql")),
    (
        2,
        "planning",
        include_str!("../migrations/0002_planning.sql"),
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
    #[error("database returned an invalid agent kind: {0}")]
    InvalidAgentKind(String),
    #[error("run does not exist: {0}")]
    RunNotFound(String),
    #[error("run is not ready for planning: {0}")]
    RunNotPlannable(String),
    #[error("planning attempt does not exist or is no longer running: {0}")]
    AttemptNotRunning(String),
    #[error("plan does not exist or is not the current proposal: {0}")]
    PlanNotCurrent(String),
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
}

impl StatePaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            database: root.join(DATABASE_FILE),
            artifacts: root.join(ARTIFACTS_DIRECTORY),
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
}

impl Drop for StorageWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(worker_thread) = self.thread.take() {
            let _ = worker_thread.join();
        }
    }
}

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

        let mut connection = Connection::open(paths.database())?;
        set_file_mode(paths.database(), 0o600)?;
        configure_connection(&connection)?;
        apply_migrations(&mut connection)?;
        recover_interrupted_planning(&mut connection)?;

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
        last_error: row.get(7)?,
    })
}

fn parse_agent_kind(agent: &str) -> Result<AgentKind, StorageError> {
    match agent {
        "codex" => Ok(AgentKind::Codex),
        "claude" => Ok(AgentKind::Claude),
        _ => Err(StorageError::InvalidAgentKind(agent.to_owned())),
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
        assert_eq!(migration_count, 2);
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
                found: 3,
                supported: 2
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

        let reopened = StorageWorker::start(paths).expect("storage should recover");
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
