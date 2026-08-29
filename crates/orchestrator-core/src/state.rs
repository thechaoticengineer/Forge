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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
