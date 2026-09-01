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
        ActiveRunSummary, AgentKind, EngineSnapshot, ImplementationStatus, TaskWorktreeStatus,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::{Duration, timeout},
};

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
    /// Cancel the active supervised implementation and preserve partial work.
    Cancel {
        /// Running attempt ID; defaults to the active run's running attempt.
        #[arg(long)]
        attempt: Option<String>,
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
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    match arguments.command {
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
        Command::Cancel { attempt, json } => {
            cancel_implementation(&socket_path, attempt, json).await
        }
        Command::Repositories { command } => repository_command(&socket_path, command).await,
        Command::Ping => ping(&socket_path).await,
        Command::Status { json } => status(&socket_path, json).await,
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
            } if response_id == request_id => return Ok(snapshot),
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
    Ok(snapshot)
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

fn print_implementation(run: &ActiveRunSummary) {
    if !run.implementation_attempts.is_empty() {
        println!(
            "Implementation attempts: {}",
            run.implementation_attempts.len()
        );
        for attempt in &run.implementation_attempts {
            println!(
                "  {}  {}  task {}",
                attempt.status.as_str(),
                attempt.agent.as_str(),
                attempt.task_id
            );
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

fn request_id() -> String {
    format!("cli-{}", process::id())
}
