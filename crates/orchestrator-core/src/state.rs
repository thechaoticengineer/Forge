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
    #[serde(default)]
    pub review_attempts: Vec<ReviewAttemptSummary>,
    #[serde(default)]
    pub verification_attempts: Vec<VerificationAttemptSummary>,
    #[serde(default)]
    pub task_commits: Vec<TaskCommitSummary>,
    #[serde(default)]
    pub task_integrations: Vec<TaskIntegrationSummary>,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Running,
    Passed,
    Failed,
    InfrastructureError,
}

impl VerificationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::InfrastructureError => "infrastructure_error",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationCommandResult {
    pub label: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    /// Whether a failure of this command fails the whole verification attempt.
    /// Older attempts recorded before advisory checks existed load as required.
    #[serde(default = "required_check")]
    pub required: bool,
    pub status: VerificationStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

const fn required_check() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationAttemptSummary {
    pub id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub status: VerificationStatus,
    pub commands: Vec<VerificationCommandResult>,
    pub error_message: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangedFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown,
}

impl ChangedFileStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type_changed",
            Self::Unmerged => "unmerged",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangedFileSummary {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: ChangedFileStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCommitStatus {
    Proposed,
    Reserved,
    Created,
    Rejected,
    Stale,
    Failed,
}

impl TaskCommitStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Reserved => "reserved",
            Self::Created => "created",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskCommitSummary {
    pub id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub verification_attempt_id: String,
    pub review_attempt_id: String,
    pub status: TaskCommitStatus,
    pub message: String,
    #[serde(default)]
    pub tree_hash: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<ChangedFileSummary>,
    #[serde(default)]
    pub patch: Option<String>,
    pub commit_hash: Option<String>,
    pub error_message: Option<String>,
    #[serde(default)]
    pub decision_reason: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskIntegrationStatus {
    Reserved,
    Completed,
    Failed,
}

impl TaskIntegrationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskIntegrationSummary {
    pub id: String,
    pub task_commit_id: String,
    pub task_id: String,
    pub target_branch: String,
    pub expected_head: String,
    pub status: TaskIntegrationStatus,
    pub result_head: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

/// Policy used to choose an independent reviewer for an implementation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    CrossProviderRequired,
    CrossProviderOrFreshSession,
}

impl ReviewPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CrossProviderRequired => "cross_provider_required",
            Self::CrossProviderOrFreshSession => "cross_provider_or_fresh_session",
        }
    }
}

/// How the reviewer is independent from the implementation attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewIndependence {
    CrossProvider,
    FreshSessionFallback,
}

impl ReviewIndependence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CrossProvider => "cross_provider",
            Self::FreshSessionFallback => "fresh_session_fallback",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Running,
    Approved,
    ChangesRequested,
    Blocked,
    Failed,
}

impl ReviewStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
    Blocked,
}

impl ReviewVerdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    Critical,
    Major,
    Minor,
}

impl ReviewSeverity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub severity: ReviewSeverity,
    pub summary: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewResult {
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewAttemptSummary {
    pub id: String,
    pub task_id: String,
    pub worktree_id: String,
    pub implementation_attempt_id: String,
    pub implementer: AgentKind,
    pub reviewer: AgentKind,
    pub policy: ReviewPolicy,
    pub independence: ReviewIndependence,
    pub status: ReviewStatus,
    pub result: Option<ReviewResult>,
    pub error_message: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
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

/// Why a continuation attempt was started from an earlier implementation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationContinuationKind {
    Redirect,
    AdditionalContext,
}

impl ImplementationContinuationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::AdditionalContext => "additional_context",
        }
    }
}

/// Why an implementation attempt was stopped before normal completion.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationStopReason {
    Cancelled,
    Redirected,
    ContextAdded,
}

impl ImplementationStopReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Redirected => "redirected",
            Self::ContextAdded => "context_added",
        }
    }
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
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub parent_attempt_id: Option<String>,
    #[serde(default)]
    pub continuation_kind: Option<ImplementationContinuationKind>,
    #[serde(default)]
    pub stop_reason: Option<ImplementationStopReason>,
    #[serde(default)]
    pub pending_continuation_kind: Option<ImplementationContinuationKind>,
    #[serde(default)]
    pub pending_user_instruction: Option<String>,
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

    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Codex => Self::Claude,
            Self::Claude => Self::Codex,
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
