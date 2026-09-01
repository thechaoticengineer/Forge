use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    #[default]
    Idle,
    Running,
    Blocked,
    Failed,
    Completed,
    WaitingForUser,
}

impl EngineStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::WaitingForUser => "waiting_for_user",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActiveRunSummary {
    pub id: String,
    pub goal: String,
    pub repository: String,
    pub base_revision: String,
    pub branch: Option<String>,
    pub worktree_dirty: bool,
    pub run_status: RunStatus,
    pub plan: Option<PlanSummary>,
    pub worktrees: Vec<TaskWorktreeSummary>,
    #[serde(default)]
    pub implementation_attempts: Vec<ImplementationAttemptSummary>,
    #[serde(default)]
    pub implementation_activity: Vec<ImplementationActivitySummary>,
    pub last_error: Option<String>,
}

/// One bounded, durable activity update emitted by an implementation agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationActivitySummary {
    pub sequence: u64,
    pub attempt_id: String,
    pub task_id: String,
    pub agent: AgentKind,
    pub kind: ImplementationActivityKind,
    pub message: String,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationActivityKind {
    Output,
    Diagnostic,
}

impl ImplementationActivityKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// Durable state of one supervised implementation-agent invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ImplementationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationAttemptSummary {
    pub id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub agent: AgentKind,
    pub status: ImplementationStatus,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

/// Lifecycle of one engine-owned task worktree. See ADR-0006.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorktreeStatus {
    /// Recorded and reserved, but not yet confirmed on disk.
    Reserved,
    /// Present, registered, and on its own task branch.
    Ready,
    /// The directory is gone; the record is kept as history.
    Missing,
    /// The directory exists but no longer matches its record.
    Diverged,
    /// Creation failed or was interrupted; the task can be retried.
    Failed,
    /// Removed deliberately by the user.
    Retired,
}

impl TaskWorktreeStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Diverged => "diverged",
            Self::Failed => "failed",
            Self::Retired => "retired",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskWorktreeSummary {
    pub id: String,
    pub task_id: String,
    pub status: TaskWorktreeStatus,
    pub branch: String,
    pub path: String,
    pub base_revision: String,
    /// Whether the user's primary checkout had uncommitted work when this
    /// worktree was created; the agent cannot see it.
    pub repository_dirty: bool,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    Claude,
}

impl AgentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Proposed,
    Approved,
    Rejected,
    Superseded,
}

impl PlanStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanSummary {
    pub id: String,
    pub revision: u32,
    pub planner: AgentKind,
    pub status: PlanStatus,
    pub summary: String,
    pub tasks: Vec<PlanTaskSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanTaskSummary {
    pub id: String,
    pub position: u32,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub depends_on: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanProposal {
    pub summary: String,
    pub tasks: Vec<ProposedTask>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedTask {
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub depends_on: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Draft,
    Planning,
    WaitingForUser,
    Running,
    Blocked,
    Failed,
    Completed,
    Rejected,
    Cancelled,
}

impl RunStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Planning => "planning",
            Self::WaitingForUser => "waiting_for_user",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineSnapshot {
    pub sequence: u64,
    pub status: EngineStatus,
    pub active_run: Option<ActiveRunSummary>,
    pub requires_attention: bool,
}
