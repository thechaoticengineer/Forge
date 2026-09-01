use std::{path::PathBuf, process};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use orchestrator_core::{
    ipc::default_socket_path,
    protocol::{
        ClientMessage, ClientRequest, MoveDirection, PROTOCOL_VERSION, RepositoryCatalog,
        ServerMessage,
    },
    state::{
        ActiveRunSummary, AgentKind, EngineSnapshot, ImplementationContinuationKind,
        ImplementationStatus, ReviewPolicy, TaskWorktreeStatus,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::{Duration, timeout},
};

mod service;

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
    /// Install, inspect, or remove the managed user engine service.
    Engine {
        #[command(subcommand)]
        command: EngineCommand,
    },
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
    /// Generate, revise, approve, or reject the active run's plan.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// Create the isolated Git worktree for one approved task.
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    /// Assign an agent to implement one approved task in its ready worktree.
    Implement {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long, value_enum)]
        agent: AgentArgument,
        #[arg(long)]
        json: bool,
    },
    /// Run a fresh independent review of one completed implementation.
    Review {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        /// Reviewer routing and fallback policy.
        #[arg(long, value_enum, default_value = "cross-provider-or-fresh-session")]
        policy: ReviewPolicyArgument,
        #[arg(long)]
        json: bool,
    },
    /// Verify, independently review, correct, and prepare one task for approval.
    Finish {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long, value_enum, default_value = "cross-provider-or-fresh-session")]
        policy: ReviewPolicyArgument,
        /// Maximum fresh correction sessions after failed gates.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=3))]
        max_corrections: u8,
        #[arg(long)]
        json: bool,
    },
    /// Approve the inspected result and create its local task commit.
    Approve {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long)]
        json: bool,
    },
    /// Reject the inspected result while preserving its worktree and history.
    Reject {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Fast-forward a selected local branch to an approved task commit.
    Integrate {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        /// Local target branch; defaults to the run's recorded branch.
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Cancel the active supervised implementation and preserve partial work.
    Cancel {
        /// Running attempt ID; defaults to the active run's running attempt.
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Pause the active supervised implementation process group.
    Pause {
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Resume the active paused implementation process group.
    Resume {
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Stop the current attempt and continue with a changed instruction.
    Redirect {
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        instruction: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop the current attempt and continue with additional context.
    Context {
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        instruction: String,
        #[arg(long)]
        json: bool,
    },
    /// List local and GitHub repositories or clone a missing repository.
    Repositories {
        #[command(subcommand)]
        command: RepositoryCommand,
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

#[derive(Debug, Subcommand)]
enum EngineCommand {
    /// Install and immediately start the systemd user service.
    Install {
        /// Engine executable; defaults to `orchestrator-engine` on PATH.
        #[arg(long, default_value = "orchestrator-engine")]
        engine_binary: PathBuf,
        #[arg(long, default_value = "codex")]
        codex_binary: PathBuf,
        #[arg(long, default_value = "claude")]
        claude_binary: PathBuf,
        #[arg(long, default_value = "gh")]
        gh_binary: PathBuf,
        /// Project root to scan and use for cloning (repeatable).
        #[arg(long = "projects-root")]
        project_roots: Vec<PathBuf>,
    },
    /// Show the systemd user service status.
    Status,
    /// Stop, disable, and remove the systemd user service unit.
    Uninstall,
}

#[derive(Debug, Subcommand)]
enum RepositoryCommand {
    /// List repositories from configured project roots and GitHub.
    List {
        /// Print the catalog as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Clone a GitHub repository into the configured projects root.
    Clone {
        /// GitHub repository in owner/name form.
        name_with_owner: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    /// Create the task worktree and its reserved branch.
    Create {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum PlanCommand {
    /// Ask Codex or Claude Code to inspect the repository and propose a plan.
    Generate {
        #[arg(long, value_enum)]
        agent: AgentArgument,
        #[arg(long)]
        json: bool,
    },
    /// Edit one task and create a new plan revision.
    Edit {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: String,
        /// Acceptance criterion; repeat for multiple criteria.
        #[arg(long = "acceptance", required = true)]
        acceptance_criteria: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Move one task while preserving dependency validity.
    Move {
        /// One-based task position shown by `status`.
        #[arg(long)]
        task: usize,
        #[arg(long, value_enum)]
        direction: DirectionArgument,
        #[arg(long)]
        json: bool,
    },
    /// Approve the current proposed plan.
    Approve {
        #[arg(long)]
        json: bool,
    },
    /// Reject the current proposed plan and return the run to draft state.
    Reject {
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AgentArgument {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ReviewPolicyArgument {
    CrossProviderRequired,
    CrossProviderOrFreshSession,
}

impl From<ReviewPolicyArgument> for ReviewPolicy {
    fn from(value: ReviewPolicyArgument) -> Self {
        match value {
            ReviewPolicyArgument::CrossProviderRequired => Self::CrossProviderRequired,
            ReviewPolicyArgument::CrossProviderOrFreshSession => Self::CrossProviderOrFreshSession,
        }
    }
}

impl From<AgentArgument> for AgentKind {
    fn from(value: AgentArgument) -> Self {
        match value {
            AgentArgument::Codex => Self::Codex,
            AgentArgument::Claude => Self::Claude,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DirectionArgument {
    Up,
    Down,
}

impl From<DirectionArgument> for MoveDirection {
    fn from(value: DirectionArgument) -> Self {
        match value {
            DirectionArgument::Up => Self::Up,
            DirectionArgument::Down => Self::Down,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let command = match arguments.command {
        Command::Engine { command } => return engine_command(command),
        command => command,
    };
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    match command {
        Command::Engine { .. } => unreachable!("engine command returned before socket discovery"),
        Command::Draft {
            repository,
            goal,
            json,
        } => create_draft(&socket_path, repository, goal, json).await,
        Command::Plan { command } => plan_command(&socket_path, command).await,
        Command::Worktree { command } => worktree_command(&socket_path, command).await,
        Command::Implement { task, agent, json } => {
            implement_task(&socket_path, task, agent.into(), json).await
        }
        Command::Review { task, policy, json } => {
            review_task(&socket_path, task, policy.into(), json).await
        }
        Command::Finish {
            task,
            policy,
            max_corrections,
            json,
        } => finish_task(&socket_path, task, policy.into(), max_corrections, json).await,
        Command::Approve { task, json } => {
            decide_task_commit(&socket_path, task, true, None, json).await
        }
        Command::Reject { task, reason, json } => {
            decide_task_commit(&socket_path, task, false, reason, json).await
        }
        Command::Integrate { task, branch, json } => {
            integrate_task(&socket_path, task, branch, json).await
        }
        Command::Cancel { attempt, json } => {
            cancel_implementation(&socket_path, attempt, json).await
        }
        Command::Pause { attempt, json } => {
            control_implementation(&socket_path, attempt, true, json).await
        }
        Command::Resume { attempt, json } => {
            control_implementation(&socket_path, attempt, false, json).await
        }
        Command::Redirect {
            attempt,
            instruction,
            json,
        } => {
            continue_implementation(
                &socket_path,
                attempt,
                ImplementationContinuationKind::Redirect,
                instruction,
                json,
            )
            .await
        }
        Command::Context {
            attempt,
            instruction,
            json,
        } => {
            continue_implementation(
                &socket_path,
                attempt,
                ImplementationContinuationKind::AdditionalContext,
                instruction,
                json,
            )
            .await
        }
        Command::Repositories { command } => repository_command(&socket_path, command).await,
        Command::Ping => ping(&socket_path).await,
        Command::Status { json } => status(&socket_path, json).await,
    }
}

fn engine_command(command: EngineCommand) -> Result<()> {
    match command {
        EngineCommand::Install {
            engine_binary,
            codex_binary,
            claude_binary,
            gh_binary,
            project_roots,
        } => {
            let path = service::install(service::InstallOptions {
                engine_binary,
                codex_binary,
                claude_binary,
                gh_binary,
                project_roots,
            })?;
            println!("Installed and started {}", path.display());
            Ok(())
        }
        EngineCommand::Status => service::status(),
        EngineCommand::Uninstall => {
            let path = service::uninstall()?;
            println!("Stopped and removed {}", path.display());
            Ok(())
        }
    }
}

async fn repository_command(socket_path: &PathBuf, command: RepositoryCommand) -> Result<()> {
    match command {
        RepositoryCommand::List { json } => {
            let catalog = request_repository_catalog(socket_path).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&catalog).context("cannot encode catalog")?
                );
            } else {
                print_repository_catalog(&catalog);
            }
        }
        RepositoryCommand::Clone {
            name_with_owner,
            json,
        } => {
            let path = request_repository_clone(socket_path, &name_with_owner).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name_with_owner": name_with_owner,
                        "path": path,
                    }))
                    .context("cannot encode clone result")?
                );
            } else {
                println!("Cloned {name_with_owner} to {path}");
            }
        }
    }
    Ok(())
}

async fn request_repository_catalog(socket_path: &PathBuf) -> Result<RepositoryCatalog> {
    let request_id = request_id();
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request: ClientRequest::ListRepositories,
    };
    let stream = connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    send_request(&mut write_half, &request).await?;
    let mut reader = BufReader::new(read_half);
    loop {
        match read_message(&mut reader, Duration::from_secs(35)).await? {
            ServerMessage::RepositoryCatalog {
                request_id: response_id,
                catalog,
                ..
            } if response_id == request_id => return Ok(catalog),
            ServerMessage::Error {
                request_id: response_id,
                code,
                message,
                ..
            } if response_id
                .as_deref()
                .is_none_or(|response_id| response_id == request_id) =>
            {
                bail!("engine rejected request ({code}): {message}")
            }
            _ => {}
        }
    }
}

async fn request_repository_clone(socket_path: &PathBuf, name_with_owner: &str) -> Result<String> {
    let request_id = request_id();
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request: ClientRequest::CloneRepository {
            name_with_owner: name_with_owner.to_owned(),
        },
    };
    let stream = connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    send_request(&mut write_half, &request).await?;
    let mut reader = BufReader::new(read_half);
    loop {
        match read_message(&mut reader, Duration::from_mins(16)).await? {
            ServerMessage::RepositoryCloned {
                request_id: response_id,
                path,
                ..
            } if response_id == request_id => return Ok(path),
            ServerMessage::Error {
                request_id: response_id,
                code,
                message,
                ..
            } if response_id
                .as_deref()
                .is_none_or(|response_id| response_id == request_id) =>
            {
                bail!("engine rejected request ({code}): {message}")
            }
            _ => {}
        }
    }
}

fn print_repository_catalog(catalog: &RepositoryCatalog) {
    println!("Project roots: {}", catalog.project_roots.join(", "));
    println!("Local repositories: {}", catalog.local.len());
    for repository in &catalog.local {
        println!(
            "  {}  {}{}",
            repository
                .name_with_owner
                .as_deref()
                .unwrap_or(&repository.name),
            repository.path,
            if repository.dirty { "  dirty" } else { "" }
        );
    }
    println!("GitHub repositories not cloned: {}", catalog.github.len());
    for repository in &catalog.github {
        println!(
            "  {}{}{}",
            repository.name_with_owner,
            if repository.fork { "  fork" } else { "" },
            if repository.archived {
                "  archived"
            } else {
                ""
            }
        );
    }
    if let Some(error) = &catalog.github_error {
        println!("GitHub unavailable: {error}");
    }
    if let Some(error) = &catalog.local_error {
        println!("Local discovery warning: {error}");
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
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::CreateDraftRun { repository, goal },
        Duration::from_secs(10),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn worktree_command(socket_path: &PathBuf, command: WorktreeCommand) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let WorktreeCommand::Create { task, json } = command;
    let plan = run
        .plan
        .as_ref()
        .context("the active run has no approved plan")?;
    let task = plan
        .tasks
        .get(
            task.checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the approved plan")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::CreateTaskWorktree {
            run_id: run.id.clone(),
            plan_id: plan.id.clone(),
            task_id: task.id.clone(),
        },
        Duration::from_secs(120),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn implement_task(
    socket_path: &PathBuf,
    task_position: usize,
    agent: AgentKind,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let plan = run
        .plan
        .as_ref()
        .context("the active run has no approved plan")?;
    let task = plan
        .tasks
        .get(
            task_position
                .checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the approved plan")?;
    let worktree = run
        .worktrees
        .iter()
        .rev()
        .find(|worktree| {
            worktree.task_id == task.id && worktree.status == TaskWorktreeStatus::Ready
        })
        .context("the task has no ready worktree; create it first")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::RunTaskImplementation {
            run_id: run.id.clone(),
            plan_id: plan.id.clone(),
            task_id: task.id.clone(),
            worktree_id: worktree.id.clone(),
            agent,
        },
        Duration::from_mins(61),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn review_task(
    socket_path: &PathBuf,
    task_position: usize,
    policy: ReviewPolicy,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let plan = run
        .plan
        .as_ref()
        .context("the active run has no approved plan")?;
    let task = plan
        .tasks
        .get(
            task_position
                .checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the approved plan")?;
    let worktree = run
        .worktrees
        .iter()
        .rev()
        .find(|worktree| {
            worktree.task_id == task.id && worktree.status == TaskWorktreeStatus::Ready
        })
        .context("the task has no ready worktree")?;
    let implementation = run
        .implementation_attempts
        .iter()
        .rev()
        .find(|attempt| {
            attempt.task_id == task.id
                && attempt.worktree_id == worktree.id
                && attempt.status == ImplementationStatus::Completed
        })
        .context("the task has no completed implementation in its ready worktree")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::RunTaskReview {
            run_id: run.id.clone(),
            plan_id: plan.id.clone(),
            task_id: task.id.clone(),
            worktree_id: worktree.id.clone(),
            implementation_attempt_id: implementation.id.clone(),
            policy,
        },
        Duration::from_mins(21),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn finish_task(
    socket_path: &PathBuf,
    task_position: usize,
    policy: ReviewPolicy,
    max_corrections: u8,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let plan = run
        .plan
        .as_ref()
        .context("the active run has no approved plan")?;
    let task = plan
        .tasks
        .get(
            task_position
                .checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the approved plan")?;
    let worktree = run
        .worktrees
        .iter()
        .rev()
        .find(|worktree| {
            worktree.task_id == task.id && worktree.status == TaskWorktreeStatus::Ready
        })
        .context("the task has no ready worktree")?;
    let implementation = run
        .implementation_attempts
        .iter()
        .rev()
        .find(|attempt| {
            attempt.task_id == task.id
                && attempt.worktree_id == worktree.id
                && attempt.status == ImplementationStatus::Completed
        })
        .context("the task has no completed implementation in its ready worktree")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::FinishTask {
            run_id: run.id.clone(),
            plan_id: plan.id.clone(),
            task_id: task.id.clone(),
            worktree_id: worktree.id.clone(),
            implementation_attempt_id: implementation.id.clone(),
            policy,
            max_corrections,
        },
        Duration::from_mins(91),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn decide_task_commit(
    socket_path: &PathBuf,
    task_position: usize,
    approve: bool,
    reason: Option<String>,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let plan = run.plan.as_ref().context("the active run has no plan")?;
    let task = plan
        .tasks
        .get(
            task_position
                .checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the plan")?;
    let proposal = run
        .task_commits
        .iter()
        .rev()
        .find(|proposal| {
            proposal.task_id == task.id
                && proposal.status == orchestrator_core::state::TaskCommitStatus::Proposed
        })
        .context("the task has no inspected commit awaiting approval")?;
    let request = if approve {
        ClientRequest::ApproveTaskCommit {
            run_id: run.id.clone(),
            task_commit_id: proposal.id.clone(),
        }
    } else {
        ClientRequest::RejectTaskCommit {
            run_id: run.id.clone(),
            task_commit_id: proposal.id.clone(),
            reason,
        }
    };
    let snapshot = send_workflow_request(socket_path, request, Duration::from_secs(120)).await?;
    print_result(&snapshot, json)
}

async fn integrate_task(
    socket_path: &PathBuf,
    task_position: usize,
    branch: Option<String>,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let plan = run.plan.as_ref().context("the active run has no plan")?;
    let task = plan
        .tasks
        .get(
            task_position
                .checked_sub(1)
                .context("task position must be at least 1")?,
        )
        .context("task position is outside the plan")?;
    let commit = run
        .task_commits
        .iter()
        .rev()
        .find(|commit| {
            commit.task_id == task.id
                && commit.status == orchestrator_core::state::TaskCommitStatus::Created
        })
        .context("the task has no approved local commit")?;
    let target_branch = branch
        .or_else(|| run.branch.clone())
        .context("the run started from detached HEAD; select --branch explicitly")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::IntegrateTaskCommit {
            run_id: run.id.clone(),
            task_commit_id: commit.id.clone(),
            target_branch,
        },
        Duration::from_secs(120),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn cancel_implementation(
    socket_path: &PathBuf,
    attempt_id: Option<String>,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot.active_run.context("there is no active run")?;
    let attempt_id = attempt_id
        .or_else(|| {
            run.implementation_attempts
                .iter()
                .rev()
                .find(|attempt| attempt.status == ImplementationStatus::Running)
                .map(|attempt| attempt.id.clone())
        })
        .context("the active run has no running implementation")?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::CancelTaskImplementation {
            run_id: run.id,
            attempt_id,
        },
        Duration::from_secs(10),
    )
    .await?;
    print_result(&snapshot, json)
}

async fn control_implementation(
    socket_path: &PathBuf,
    attempt_id: Option<String>,
    pause: bool,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot.active_run.context("there is no active run")?;
    let attempt_id = running_attempt_id(&run, attempt_id)?;
    let request = if pause {
        ClientRequest::PauseTaskImplementation {
            run_id: run.id,
            attempt_id,
        }
    } else {
        ClientRequest::ResumeTaskImplementation {
            run_id: run.id,
            attempt_id,
        }
    };
    let snapshot = send_workflow_request(socket_path, request, Duration::from_secs(10)).await?;
    print_result(&snapshot, json)
}

async fn continue_implementation(
    socket_path: &PathBuf,
    attempt_id: Option<String>,
    kind: ImplementationContinuationKind,
    instruction: String,
    json: bool,
) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot.active_run.context("there is no active run")?;
    let attempt_id = continuation_attempt_id(&run, attempt_id)?;
    let snapshot = send_workflow_request(
        socket_path,
        ClientRequest::ContinueTaskImplementation {
            run_id: run.id,
            attempt_id,
            kind,
            instruction,
        },
        Duration::from_mins(61),
    )
    .await?;
    print_result(&snapshot, json)
}

fn running_attempt_id(run: &ActiveRunSummary, requested: Option<String>) -> Result<String> {
    requested
        .or_else(|| {
            run.implementation_attempts
                .iter()
                .rev()
                .find(|attempt| attempt.status == ImplementationStatus::Running)
                .map(|attempt| attempt.id.clone())
        })
        .context("the active run has no running implementation")
}

fn continuation_attempt_id(run: &ActiveRunSummary, requested: Option<String>) -> Result<String> {
    requested
        .or_else(|| {
            run.implementation_attempts
                .iter()
                .rev()
                .find(|attempt| {
                    attempt.status == ImplementationStatus::Running
                        || attempt.pending_continuation_kind.is_some()
                })
                .map(|attempt| attempt.id.clone())
        })
        .context("the active run has no running or pending implementation continuation")
}

async fn plan_command(socket_path: &PathBuf, command: PlanCommand) -> Result<()> {
    let snapshot = fetch_snapshot(socket_path).await?;
    let run = snapshot
        .active_run
        .as_ref()
        .context("there is no active run")?;
    let (request, json, response_timeout) = match command {
        PlanCommand::Generate { agent, json } => (
            ClientRequest::GeneratePlan {
                run_id: run.id.clone(),
                agent: agent.into(),
            },
            json,
            Duration::from_mins(11),
        ),
        PlanCommand::Edit {
            task,
            title,
            description,
            acceptance_criteria,
            json,
        } => {
            let plan = run.plan.as_ref().context("the active run has no plan")?;
            let task = plan
                .tasks
                .get(
                    task.checked_sub(1)
                        .context("task position must be at least 1")?,
                )
                .context("task position is outside the current plan")?;
            (
                ClientRequest::UpdatePlanTask {
                    run_id: run.id.clone(),
                    plan_id: plan.id.clone(),
                    task_id: task.id.clone(),
                    title,
                    description,
                    acceptance_criteria,
                },
                json,
                Duration::from_secs(10),
            )
        }
        PlanCommand::Move {
            task,
            direction,
            json,
        } => {
            let plan = run.plan.as_ref().context("the active run has no plan")?;
            let task = plan
                .tasks
                .get(
                    task.checked_sub(1)
                        .context("task position must be at least 1")?,
                )
                .context("task position is outside the current plan")?;
            (
                ClientRequest::MovePlanTask {
                    run_id: run.id.clone(),
                    plan_id: plan.id.clone(),
                    task_id: task.id.clone(),
                    direction: direction.into(),
                },
                json,
                Duration::from_secs(10),
            )
        }
        PlanCommand::Approve { json } => {
            let plan = run.plan.as_ref().context("the active run has no plan")?;
            (
                ClientRequest::ApprovePlan {
                    run_id: run.id.clone(),
                    plan_id: plan.id.clone(),
                },
                json,
                Duration::from_secs(10),
            )
        }
        PlanCommand::Reject { reason, json } => {
            let plan = run.plan.as_ref().context("the active run has no plan")?;
            (
                ClientRequest::RejectPlan {
                    run_id: run.id.clone(),
                    plan_id: plan.id.clone(),
                    reason,
                },
                json,
                Duration::from_secs(10),
            )
        }
    };
    let snapshot = send_workflow_request(socket_path, request, response_timeout).await?;
    print_result(&snapshot, json)
}

async fn send_workflow_request(
    socket_path: &PathBuf,
    request: ClientRequest,
    response_timeout: Duration,
) -> Result<EngineSnapshot> {
    let request_id = request_id();
    let request = ClientMessage {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request,
    };
    let stream = connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    send_request(&mut write_half, &request).await?;
    let mut reader = BufReader::new(read_half);
    loop {
        match read_message(&mut reader, response_timeout).await? {
            ServerMessage::Snapshot {
                request_id: Some(response_id),
                snapshot,
                ..
            } if response_id == request_id => return Ok(*snapshot),
            ServerMessage::Error {
                request_id: response_id,
                code,
                message,
                ..
            } if response_id
                .as_deref()
                .is_none_or(|response_id| response_id == request_id) =>
            {
                bail!("engine rejected request ({code}): {message}")
            }
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
    let snapshot = fetch_snapshot(socket_path).await?;
    print_result(&snapshot, json)
}

async fn fetch_snapshot(socket_path: &PathBuf) -> Result<EngineSnapshot> {
    let stream = connect(socket_path).await?;
    let mut reader = BufReader::new(stream);
    let message = read_message(&mut reader, Duration::from_secs(10)).await?;
    let ServerMessage::Snapshot { snapshot, .. } = message else {
        bail!("engine did not send a state snapshot after connection");
    };
    Ok(*snapshot)
}

fn print_result(snapshot: &EngineSnapshot, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).context("cannot encode snapshot")?
        );
    } else {
        print_snapshot(snapshot);
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
        match read_message(&mut reader, Duration::from_secs(10)).await? {
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

async fn read_message<R>(reader: &mut R, response_timeout: Duration) -> Result<ServerMessage>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut line = String::new();
    let read = timeout(response_timeout, reader.read_line(&mut line))
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
            if let Some(error) = &run.last_error {
                println!("Last error: {error}");
            }
            if !run.worktrees.is_empty() {
                println!("Task worktrees: {}", run.worktrees.len());
                for worktree in &run.worktrees {
                    println!("  {}  {}", worktree.status.as_str(), worktree.branch);
                    println!("     {}", worktree.path);
                    if worktree.repository_dirty {
                        println!(
                            "     The repository had uncommitted work; \
                             the agent works from {}",
                            worktree.base_revision
                        );
                    }
                    if let Some(error) = &worktree.last_error {
                        println!("     {error}");
                    }
                }
            }
            print_implementation(run);
            print_reviews(run);
            print_task_results(run);
            if let Some(plan) = &run.plan {
                println!(
                    "Plan: revision {} · {} · {}",
                    plan.revision,
                    plan.planner.as_str(),
                    plan.status.as_str()
                );
                println!("Plan summary: {}", plan.summary);
                for task in &plan.tasks {
                    println!("  {}. {}", task.position, task.title);
                    println!("     {}", task.description);
                    for criterion in &task.acceptance_criteria {
                        println!("     - {criterion}");
                    }
                    if !task.depends_on.is_empty() {
                        let dependencies = task
                            .depends_on
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("     Depends on: {dependencies}");
                    }
                }
            }
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

fn print_task_results(run: &ActiveRunSummary) {
    if let Some(commit) = run.task_commits.last() {
        println!(
            "Task commit: {} · {} · {} changed file(s)",
            commit.status.as_str(),
            commit.message,
            commit.changed_files.len()
        );
        if let Some(hash) = &commit.commit_hash {
            println!("  Commit: {hash}");
        }
        if let Some(reason) = &commit.decision_reason {
            println!("  Decision: {reason}");
        }
    }
    if let Some(integration) = run.task_integrations.last() {
        println!(
            "Task integration: {} · {}",
            integration.status.as_str(),
            integration.target_branch
        );
        if let Some(head) = &integration.result_head {
            println!("  Result: {head}");
        }
        if let Some(error) = &integration.error_message {
            println!("  {error}");
        }
    }
}

fn print_implementation(run: &ActiveRunSummary) {
    if !run.implementation_attempts.is_empty() {
        println!(
            "Implementation attempts: {}",
            run.implementation_attempts.len()
        );
        for attempt in &run.implementation_attempts {
            println!(
                "  {}{}  {}  task {}",
                attempt.status.as_str(),
                if attempt.paused { " (paused)" } else { "" },
                attempt.agent.as_str(),
                attempt.task_id
            );
            if let Some(kind) = attempt.continuation_kind {
                println!("     continuation: {}", kind.as_str());
            }
            if let Some(reason) = attempt.stop_reason {
                println!("     stopped: {}", reason.as_str());
            }
            if let Some(exit_code) = attempt.exit_code {
                println!("     exit code: {exit_code}");
            }
            if let Some(error) = &attempt.error_message {
                println!("     {error}");
            }
        }
    }
    if !run.implementation_activity.is_empty() {
        println!("Recent implementation activity:");
        let start = run.implementation_activity.len().saturating_sub(10);
        for activity in &run.implementation_activity[start..] {
            println!(
                "  {}  {}: {}",
                activity.agent.as_str(),
                activity.kind.as_str(),
                activity.message
            );
        }
    }
}

fn print_reviews(run: &ActiveRunSummary) {
    if run.review_attempts.is_empty() {
        return;
    }
    println!("Independent review attempts: {}", run.review_attempts.len());
    for attempt in &run.review_attempts {
        println!(
            "  {}  {} -> {}  task {}  {}",
            attempt.status.as_str(),
            attempt.implementer.as_str(),
            attempt.reviewer.as_str(),
            attempt.task_id,
            attempt.independence.as_str(),
        );
        if let Some(result) = &attempt.result {
            println!("     {}", result.summary);
            for finding in &result.findings {
                println!(
                    "     - {}: {} ({})",
                    finding.severity.as_str(),
                    finding.summary,
                    finding.evidence
                );
            }
        }
        if let Some(error) = &attempt.error_message {
            println!("     {error}");
        }
    }
}

fn request_id() -> String {
    format!("cli-{}", process::id())
}
