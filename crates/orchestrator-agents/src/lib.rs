//! Constrained adapters for authenticated Codex CLI and Claude Code CLI installations.

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use orchestrator_core::state::{
    ActiveRunSummary, AgentKind, ImplementationActivityKind, PlanProposal, PlanTaskSummary,
    TaskWorktreeSummary,
};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{mpsc, watch},
    time::timeout,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(10);
const DEFAULT_IMPLEMENTATION_TIMEOUT: Duration = Duration::from_mins(60);
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 1024 * 1024;
const MAX_TASKS: usize = 20;
const MAX_CRITERIA_PER_TASK: usize = 20;

pub const PLAN_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["summary", "tasks"],
  "properties": {
    "summary": { "type": "string", "minLength": 1, "maxLength": 4000 },
    "tasks": {
      "type": "array",
      "minItems": 1,
      "maxItems": 20,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "description", "acceptance_criteria", "depends_on"],
        "properties": {
          "title": { "type": "string", "minLength": 1, "maxLength": 200 },
          "description": { "type": "string", "minLength": 1, "maxLength": 4000 },
          "acceptance_criteria": {
            "type": "array",
            "minItems": 1,
            "maxItems": 20,
            "items": { "type": "string", "minLength": 1, "maxLength": 1000 }
          },
          "depends_on": {
            "type": "array",
            "items": { "type": "integer", "minimum": 1, "maximum": 20 }
          }
        }
      }
    }
  }
}"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCommands {
    pub codex: PathBuf,
    pub claude: PathBuf,
}

impl Default for AgentCommands {
    fn default() -> Self {
        Self {
            codex: PathBuf::from("codex"),
            claude: PathBuf::from("claude"),
        }
    }
}

impl AgentCommands {
    fn command_path(&self, agent: AgentKind) -> &Path {
        match agent {
            AgentKind::Codex => &self.codex,
            AgentKind::Claude => &self.claude,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlannerRunner {
    commands: AgentCommands,
    timeout: Duration,
}

impl Default for PlannerRunner {
    fn default() -> Self {
        Self::new(AgentCommands::default())
    }
}

impl PlannerRunner {
    #[must_use]
    pub const fn new(commands: AgentCommands) -> Self {
        Self {
            commands,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub const fn commands(&self) -> &AgentCommands {
        &self.commands
    }

    /// Runs the selected planner and returns a validated structured proposal.
    ///
    /// # Errors
    ///
    /// Returns captured failure evidence when the CLI cannot start, times out,
    /// exits unsuccessfully, exceeds output bounds, or emits an invalid plan.
    pub async fn generate(
        &self,
        agent: AgentKind,
        repository: &Path,
        prompt: &str,
    ) -> Result<PlannerOutput, PlannerFailure> {
        let (command, _schema_file) = self.prepare_command(agent, repository)?;
        let output = run_command(
            command,
            self.command_path(agent),
            agent,
            "planner",
            prompt,
            self.timeout,
            RunControl::default(),
        )
        .await?;
        let stdout = output.stdout;
        let stderr = output.stderr;
        let status = output.status;
        let exit_code = status.code();

        if stdout.truncated || stderr.truncated {
            return Err(PlannerFailure {
                message: "planner output exceeded the configured capture limit".to_owned(),
                final_output: stdout.text,
                diagnostic_output: stderr.text,
                exit_code,
                cancelled: false,
            });
        }
        if !status.success() {
            return Err(PlannerFailure {
                message: format!(
                    "{} CLI exited unsuccessfully{}",
                    agent.as_str(),
                    exit_code.map_or_else(String::new, |code| format!(" with status {code}"))
                ),
                final_output: stdout.text,
                diagnostic_output: stderr.text,
                exit_code,
                cancelled: false,
            });
        }

        let proposal = parse_proposal(agent, &stdout.text).map_err(|message| PlannerFailure {
            message,
            final_output: stdout.text.clone(),
            diagnostic_output: stderr.text.clone(),
            exit_code,
            cancelled: false,
        })?;
        Ok(PlannerOutput {
            proposal,
            final_output: stdout.text,
            diagnostic_output: stderr.text,
            exit_code: exit_code.unwrap_or(0),
        })
    }

    fn prepare_command(
        &self,
        agent: AgentKind,
        repository: &Path,
    ) -> Result<(Command, Option<NamedTempFile>), PlannerFailure> {
        match agent {
            AgentKind::Codex => self.prepare_codex(repository),
            AgentKind::Claude => Ok((self.prepare_claude(repository), None)),
        }
    }

    fn prepare_codex(
        &self,
        repository: &Path,
    ) -> Result<(Command, Option<NamedTempFile>), PlannerFailure> {
        let mut file = NamedTempFile::new().map_err(|error| {
            PlannerFailure::new(format!("cannot create plan schema file: {error}"))
        })?;
        file.write_all(PLAN_SCHEMA.as_bytes()).map_err(|error| {
            PlannerFailure::new(format!("cannot write plan schema file: {error}"))
        })?;
        let mut command = Command::new(&self.commands.codex);
        command.args([
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--sandbox",
            "read-only",
            "--color",
            "never",
            "--output-schema",
        ]);
        command.arg(file.path());
        command.arg("-C").arg(repository).arg("-");
        Ok((command, Some(file)))
    }

    fn prepare_claude(&self, repository: &Path) -> Command {
        let mut command = Command::new(&self.commands.claude);
        command.args([
            "--print",
            "--safe-mode",
            "--disable-slash-commands",
            "--permission-mode",
            "plan",
            "--tools",
            "Read,Glob,Grep",
            "--output-format",
            "json",
            "--json-schema",
            PLAN_SCHEMA,
            "--no-session-persistence",
        ]);
        command.current_dir(repository);
        command
    }

    fn command_path(&self, agent: AgentKind) -> &Path {
        self.commands.command_path(agent)
    }
}

#[derive(Clone, Debug)]
pub struct ImplementerRunner {
    commands: AgentCommands,
    timeout: Duration,
}

impl Default for ImplementerRunner {
    fn default() -> Self {
        Self::new(AgentCommands::default())
    }
}

impl ImplementerRunner {
    #[must_use]
    pub const fn new(commands: AgentCommands) -> Self {
        Self {
            commands,
            timeout: DEFAULT_IMPLEMENTATION_TIMEOUT,
        }
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Runs one write-capable agent inside an existing task worktree.
    ///
    /// # Errors
    ///
    /// Returns bounded failure evidence when the CLI cannot start, times out,
    /// exits unsuccessfully, or exceeds the configured capture limits.
    pub async fn implement(
        &self,
        agent: AgentKind,
        worktree: &Path,
        prompt: &str,
    ) -> Result<ImplementerOutput, ImplementerFailure> {
        self.implement_with_activity(agent, worktree, prompt, None, None)
            .await
    }

    /// Runs an implementer while streaming bounded activity and accepting an
    /// engine-owned cancellation signal.
    ///
    /// # Errors
    ///
    /// Returns bounded failure evidence when the CLI cannot start, is
    /// cancelled, times out, exits unsuccessfully, or exceeds capture limits.
    pub async fn implement_with_activity(
        &self,
        agent: AgentKind,
        worktree: &Path,
        prompt: &str,
        activity: Option<mpsc::Sender<ImplementerActivity>>,
        cancellation: Option<watch::Receiver<bool>>,
    ) -> Result<ImplementerOutput, ImplementerFailure> {
        let command = self.prepare_command(agent, worktree);
        let output = run_command(
            command,
            self.commands.command_path(agent),
            agent,
            "implementer",
            prompt,
            self.timeout,
            RunControl {
                activity,
                cancellation,
            },
        )
        .await
        .map_err(ImplementerFailure::from)?;
        let exit_code = output.status.code();

        if output.stdout.truncated || output.stderr.truncated {
            return Err(ImplementerFailure {
                message: "implementer output exceeded the configured capture limit".to_owned(),
                final_output: output.stdout.text,
                diagnostic_output: output.stderr.text,
                exit_code,
                cancelled: false,
            });
        }
        if !output.status.success() {
            return Err(ImplementerFailure {
                message: format!(
                    "{} CLI exited unsuccessfully{}",
                    agent.as_str(),
                    exit_code.map_or_else(String::new, |code| format!(" with status {code}"))
                ),
                final_output: output.stdout.text,
                diagnostic_output: output.stderr.text,
                exit_code,
                cancelled: false,
            });
        }

        Ok(ImplementerOutput {
            final_output: output.stdout.text,
            diagnostic_output: output.stderr.text,
            exit_code: exit_code.unwrap_or(0),
        })
    }

    fn prepare_command(&self, agent: AgentKind, worktree: &Path) -> Command {
        match agent {
            AgentKind::Codex => {
                let mut command = Command::new(&self.commands.codex);
                command.args([
                    "exec",
                    "--ephemeral",
                    "--sandbox",
                    "workspace-write",
                    "--color",
                    "never",
                ]);
                command.arg("-C").arg(worktree).arg("-");
                command.current_dir(worktree);
                command
            }
            AgentKind::Claude => {
                let mut command = Command::new(&self.commands.claude);
                command.args([
                    "--print",
                    "--safe-mode",
                    "--restricted",
                    "--disable-slash-commands",
                    "--permission-mode",
                    "acceptEdits",
                    "--tools",
                    "Read,Glob,Grep,Edit,Write",
                    "--output-format",
                    "text",
                    "--no-session-persistence",
                ]);
                command.current_dir(worktree);
                command
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementerOutput {
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementerFailure {
    pub message: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
}

impl From<PlannerFailure> for ImplementerFailure {
    fn from(failure: PlannerFailure) -> Self {
        Self {
            message: failure.message,
            final_output: failure.final_output,
            diagnostic_output: failure.diagnostic_output,
            exit_code: failure.exit_code,
            cancelled: failure.cancelled,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImplementerActivity {
    pub kind: ImplementationActivityKind,
    pub message: String,
}

struct ProcessOutput {
    status: std::process::ExitStatus,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
}

#[derive(Default)]
struct RunControl {
    activity: Option<mpsc::Sender<ImplementerActivity>>,
    cancellation: Option<watch::Receiver<bool>>,
}

async fn run_command(
    mut command: Command,
    command_path: &Path,
    agent: AgentKind,
    role: &str,
    prompt: &str,
    duration: Duration,
    control: RunControl,
) -> Result<ProcessOutput, PlannerFailure> {
    let RunControl {
        activity,
        cancellation,
    } = control;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        PlannerFailure::new(format!(
            "cannot start {} CLI at {}: {error}",
            agent.as_str(),
            command_path.display()
        ))
    })?;
    let process_group = child
        .id()
        .ok_or_else(|| PlannerFailure::new(format!("{role} process ID is unavailable")))?;
    let mut process_guard = ProcessGroupGuard::new(process_group)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PlannerFailure::new(format!("{role} stdout is unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PlannerFailure::new(format!("{role} stderr is unavailable")))?;
    let stdout_task = tokio::spawn(read_bounded_with_activity(
        stdout,
        MAX_STDOUT_BYTES,
        activity.clone(),
        ImplementationActivityKind::Output,
    ));
    let stderr_task = tokio::spawn(read_bounded_with_activity(
        stderr,
        MAX_STDERR_BYTES,
        activity,
        ImplementationActivityKind::Diagnostic,
    ));
    let status_result = wait_for_child(
        &mut child,
        duration,
        agent,
        role,
        prompt,
        &mut process_guard,
        cancellation,
    )
    .await;
    let stdout = join_output(stdout_task, role, "stdout").await?;
    let stderr = join_output(stderr_task, role, "stderr").await?;
    let status = status_result.map_err(|failure| PlannerFailure {
        message: failure.message,
        final_output: stdout.text.clone(),
        diagnostic_output: stderr.text.clone(),
        exit_code: None,
        cancelled: failure.cancelled,
    })?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

async fn send_prompt(
    child: &mut Child,
    agent: AgentKind,
    role: &str,
    prompt: &str,
) -> Result<(), PlannerFailure> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PlannerFailure::new(format!("{role} stdin is unavailable")))?;
    if let Err(error) = stdin.write_all(prompt.as_bytes()).await {
        let _ = child.kill().await;
        return Err(PlannerFailure::new(format!(
            "cannot send prompt to {} CLI: {error}",
            agent.as_str()
        )));
    }
    if let Err(error) = stdin.shutdown().await {
        let _ = child.kill().await;
        return Err(PlannerFailure::new(format!(
            "cannot close {} CLI input: {error}",
            agent.as_str()
        )));
    }
    drop(stdin);
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerOutput {
    pub proposal: PlanProposal,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerFailure {
    pub message: String,
    pub final_output: String,
    pub diagnostic_output: String,
    pub exit_code: Option<i32>,
    cancelled: bool,
}

impl PlannerFailure {
    fn new(message: String) -> Self {
        Self {
            message,
            final_output: String::new(),
            diagnostic_output: String::new(),
            exit_code: None,
            cancelled: false,
        }
    }
}

#[must_use]
pub fn build_planning_prompt(run: &ActiveRunSummary) -> String {
    let branch = run.branch.as_deref().unwrap_or("detached HEAD");
    let working_tree = if run.worktree_dirty { "dirty" } else { "clean" };
    format!(
        "You are the planning worker in a local software workshop.\n\
         Inspect the repository without modifying files, Git state, configuration, or external systems.\n\
         Produce a concrete implementation plan for the goal below. Each task must be independently understandable, ordered, and have objective acceptance criteria. Dependencies use one-based positions and may refer only to earlier tasks. Keep the plan focused; do not implement it. Return only the requested JSON object.\n\n\
         Goal:\n{}\n\n\
         Repository: {}\n\
         Base revision: {}\n\
         Branch: {}\n\
         Working tree: {}\n",
        run.goal, run.repository, run.base_revision, branch, working_tree
    )
}

#[must_use]
pub fn build_implementation_prompt(
    run: &ActiveRunSummary,
    task: &PlanTaskSummary,
    worktree: &TaskWorktreeSummary,
) -> String {
    let criteria = task
        .acceptance_criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dependencies = if task.depends_on.is_empty() {
        "none".to_owned()
    } else {
        task.depends_on
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "You are the implementation worker in a local software workshop.\n\
         Work only inside the current task worktree. Read the repository's applicable contributor instructions before editing. Implement only the assigned task and keep existing unrelated changes intact.\n\
         Do not commit, merge, push, rebase, reset, rewrite Git history, delete branches or worktrees, modify another checkout, or change external systems. Do not claim that tests or verification passed; deterministic verification is a separate engine stage.\n\
         Inspect the current worktree first because a retry may contain a previous partial attempt. Make the smallest coherent code and test changes required by the task. End with a concise summary of changed files, remaining concerns, and checks you recommend the engine run.\n\n\
         Overall goal:\n{}\n\n\
         Assigned task {}: {}\n{}\n\n\
         Acceptance criteria:\n{}\n\n\
         Dependencies by plan position: {}\n\
         Base revision: {}\n\
         Task branch: {}\n\
         Task worktree: {}\n",
        run.goal,
        task.position,
        task.title,
        task.description,
        criteria,
        dependencies,
        worktree.base_revision,
        worktree.branch,
        worktree.path
    )
}

fn parse_proposal(agent: AgentKind, output: &str) -> Result<PlanProposal, String> {
    let value: serde_json::Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("{} CLI returned invalid JSON: {error}", agent.as_str()))?;
    let proposal_value = match agent {
        AgentKind::Codex => value,
        AgentKind::Claude => {
            if let Some(structured) = value.get("structured_output") {
                structured.clone()
            } else if let Some(result) = value.get("result").and_then(serde_json::Value::as_str) {
                serde_json::from_str(result).map_err(|error| {
                    format!("Claude CLI result did not contain valid plan JSON: {error}")
                })?
            } else {
                value
            }
        }
    };
    let mut proposal: PlanProposal = serde_json::from_value(proposal_value)
        .map_err(|error| format!("planner output does not match the plan schema: {error}"))?;
    validate_proposal(&mut proposal)?;
    Ok(proposal)
}

/// Normalizes and validates a plan before it enters durable workflow state.
///
/// # Errors
///
/// Returns a precise validation message for empty, oversized, duplicate, or
/// non-topological plan content.
pub fn validate_proposal(proposal: &mut PlanProposal) -> Result<(), String> {
    proposal.summary = proposal.summary.trim().to_owned();
    validate_text("plan summary", &proposal.summary, 4_000)?;
    if proposal.tasks.is_empty() || proposal.tasks.len() > MAX_TASKS {
        return Err(format!("plan must contain between 1 and {MAX_TASKS} tasks"));
    }

    for (index, task) in proposal.tasks.iter_mut().enumerate() {
        let position = index + 1;
        task.title = task.title.trim().to_owned();
        task.description = task.description.trim().to_owned();
        validate_text(&format!("task {position} title"), &task.title, 200)?;
        validate_text(
            &format!("task {position} description"),
            &task.description,
            4_000,
        )?;
        if task.acceptance_criteria.is_empty()
            || task.acceptance_criteria.len() > MAX_CRITERIA_PER_TASK
        {
            return Err(format!(
                "task {position} must contain between 1 and {MAX_CRITERIA_PER_TASK} acceptance criteria"
            ));
        }
        for criterion in &mut task.acceptance_criteria {
            *criterion = criterion.trim().to_owned();
            validate_text(
                &format!("task {position} acceptance criterion"),
                criterion,
                1_000,
            )?;
        }

        let mut dependencies = HashSet::new();
        for dependency in &task.depends_on {
            let dependency_index = usize::try_from(*dependency)
                .map_err(|_| format!("task {position} has an invalid dependency"))?;
            if dependency_index == 0 || dependency_index >= position {
                return Err(format!(
                    "task {position} dependency {dependency} must refer to an earlier task"
                ));
            }
            if !dependencies.insert(*dependency) {
                return Err(format!("task {position} repeats dependency {dependency}"));
            }
        }
        task.depends_on.sort_unstable();
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, maximum: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length == 0 {
        return Err(format!("{label} must not be empty"));
    }
    if length > maximum {
        return Err(format!("{label} must not exceed {maximum} characters"));
    }
    Ok(())
}

async fn wait_for_child(
    child: &mut Child,
    duration: Duration,
    agent: AgentKind,
    role: &str,
    prompt: &str,
    process_guard: &mut ProcessGroupGuard,
    mut cancellation: Option<watch::Receiver<bool>>,
) -> Result<std::process::ExitStatus, WaitFailure> {
    let invocation = async {
        send_prompt(child, agent, role, prompt)
            .await
            .map_err(|failure| failure.message)?;
        child
            .wait()
            .await
            .map_err(|error| format!("cannot wait for {} CLI: {error}", agent.as_str()))
    };
    let cancellation_requested = async {
        let Some(receiver) = cancellation.as_mut() else {
            std::future::pending::<()>().await;
            return;
        };
        if *receiver.borrow() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow() {
                return;
            }
        }
        std::future::pending::<()>().await;
    };
    tokio::select! {
        result = timeout(duration, invocation) => match result {
        Ok(Ok(status)) => {
            process_guard.kill_remaining();
            Ok(status)
        }
        Ok(Err(message)) => {
            process_guard.terminate(child).await;
            Err(WaitFailure { message, cancelled: false })
        }
        Err(_) => {
            process_guard.terminate(child).await;
            Err(WaitFailure {
                message: format!("{} CLI exceeded the {role} timeout", agent.as_str()),
                cancelled: false,
            })
        }
        },
        () = cancellation_requested => {
            process_guard.terminate(child).await;
            Err(WaitFailure {
                message: format!("{} {role} was cancelled by the user", agent.as_str()),
                cancelled: true,
            })
        }
    }
}

struct WaitFailure {
    message: String,
    cancelled: bool,
}

struct ProcessGroupGuard {
    process_group: Option<Pid>,
}

impl ProcessGroupGuard {
    fn new(process_id: u32) -> Result<Self, PlannerFailure> {
        let process_group = i32::try_from(process_id)
            .map(Pid::from_raw)
            .map_err(|_| PlannerFailure::new("agent process ID is out of range".to_owned()))?;
        Ok(Self {
            process_group: Some(process_group),
        })
    }

    fn disarm(&mut self) {
        self.process_group = None;
    }

    fn kill_remaining(&mut self) {
        if let Some(process_group) = self.process_group {
            let _ = killpg(process_group, Signal::SIGKILL);
        }
        self.disarm();
    }

    async fn terminate(&mut self, child: &mut Child) {
        let Some(process_group) = self.process_group else {
            return;
        };
        let _ = killpg(process_group, Signal::SIGTERM);
        let exited = matches!(
            timeout(Duration::from_secs(5), child.wait()).await,
            Ok(Ok(_))
        );
        let _ = killpg(process_group, Signal::SIGKILL);
        if !exited {
            let _ = child.wait().await;
        }
        self.disarm();
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(process_group) = self.process_group {
            let _ = killpg(process_group, Signal::SIGKILL);
        }
    }
}

struct BoundedOutput {
    text: String,
    truncated: bool,
}

async fn read_bounded_with_activity<R>(
    mut reader: R,
    maximum: usize,
    activity: Option<mpsc::Sender<ImplementerActivity>>,
    kind: ImplementationActivityKind,
) -> std::io::Result<BoundedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    let mut pending_activity_bytes = Vec::new();
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(bytes.len());
        let retained = remaining.min(count);
        if retained > 0
            && let Some(sender) = &activity
        {
            pending_activity_bytes.extend_from_slice(&buffer[..retained]);
            emit_complete_utf8_activity(sender, kind, &mut pending_activity_bytes, false).await;
        }
        bytes.extend_from_slice(&buffer[..retained]);
        if retained < count {
            truncated = true;
        }
    }
    if let Some(sender) = &activity {
        emit_complete_utf8_activity(sender, kind, &mut pending_activity_bytes, true).await;
    }
    Ok(BoundedOutput {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    })
}

async fn emit_complete_utf8_activity(
    sender: &mpsc::Sender<ImplementerActivity>,
    kind: ImplementationActivityKind,
    pending: &mut Vec<u8>,
    flush: bool,
) {
    loop {
        if pending.is_empty() {
            return;
        }
        match std::str::from_utf8(pending) {
            Ok(text) => {
                let message = text.to_owned();
                pending.clear();
                let _ = sender.send(ImplementerActivity { kind, message }).await;
                return;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    let message = String::from_utf8_lossy(&pending[..valid]).into_owned();
                    pending.drain(..valid);
                    let _ = sender.send(ImplementerActivity { kind, message }).await;
                    continue;
                }
                if let Some(invalid_length) = error.error_len() {
                    pending.drain(..invalid_length);
                    let _ = sender
                        .send(ImplementerActivity {
                            kind,
                            message: "\u{fffd}".to_owned(),
                        })
                        .await;
                    continue;
                }
                if flush {
                    let message = String::from_utf8_lossy(pending).into_owned();
                    pending.clear();
                    let _ = sender.send(ImplementerActivity { kind, message }).await;
                }
                return;
            }
        }
    }
}

async fn join_output(
    task: tokio::task::JoinHandle<std::io::Result<BoundedOutput>>,
    role: &str,
    stream: &str,
) -> Result<BoundedOutput, PlannerFailure> {
    task.await
        .map_err(|error| PlannerFailure::new(format!("cannot join {role} {stream}: {error}")))?
        .map_err(|error| PlannerFailure::new(format!("cannot read {role} {stream}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, os::unix::fs::PermissionsExt};

    use orchestrator_core::state::{ProposedTask, RunStatus};
    use tempfile::TempDir;

    #[test]
    fn builds_a_prompt_with_repository_evidence() {
        let prompt = build_planning_prompt(&sample_run());
        assert!(prompt.contains("Add plan support"));
        assert!(prompt.contains("/tmp/project"));
        assert!(prompt.contains("0123456789"));
        assert!(prompt.contains("Working tree: dirty"));
        assert!(prompt.contains("without modifying"));
    }

    #[test]
    fn builds_an_implementation_prompt_with_task_boundaries() {
        let run = sample_run();
        let task = PlanTaskSummary {
            id: "task-1".to_owned(),
            position: 1,
            title: "Implement the change".to_owned(),
            description: "Make the focused edit.".to_owned(),
            acceptance_criteria: vec!["The behavior is covered.".to_owned()],
            depends_on: Vec::new(),
        };
        let worktree = TaskWorktreeSummary {
            id: "worktree-1".to_owned(),
            task_id: task.id.clone(),
            status: orchestrator_core::state::TaskWorktreeStatus::Ready,
            branch: "orchestrator/run/1-implement".to_owned(),
            path: "/tmp/worktree".to_owned(),
            base_revision: "0123456789".to_owned(),
            repository_dirty: false,
            last_error: None,
        };

        let prompt = build_implementation_prompt(&run, &task, &worktree);
        assert!(prompt.contains("Implement the change"));
        assert!(prompt.contains("The behavior is covered"));
        assert!(prompt.contains("/tmp/worktree"));
        assert!(prompt.contains("Do not commit"));
        assert!(prompt.contains("verification is a separate engine stage"));
    }

    #[test]
    fn parses_codex_and_claude_structured_results() {
        let plan = serde_json::to_string(&sample_proposal()).expect("plan should serialize");
        assert_eq!(
            parse_proposal(AgentKind::Codex, &plan).expect("Codex plan should parse"),
            sample_proposal()
        );
        let claude = serde_json::json!({ "structured_output": sample_proposal() });
        assert_eq!(
            parse_proposal(AgentKind::Claude, &claude.to_string())
                .expect("Claude plan should parse"),
            sample_proposal()
        );
    }

    #[test]
    fn rejects_forward_and_repeated_dependencies() {
        let mut forward = sample_proposal();
        forward.tasks[0].depends_on = vec![1];
        assert!(
            validate_proposal(&mut forward)
                .expect_err("forward dependency should fail")
                .contains("earlier task")
        );

        let mut repeated = sample_proposal();
        repeated.tasks[1].depends_on = vec![1, 1];
        assert!(
            validate_proposal(&mut repeated)
                .expect_err("repeated dependency should fail")
                .contains("repeats dependency")
        );
    }

    #[test]
    fn plan_schema_avoids_keywords_the_structured_output_apis_reject() {
        let schema: serde_json::Value =
            serde_json::from_str(PLAN_SCHEMA).expect("plan schema should be valid JSON");

        // Codex rejects the request outright with `invalid_json_schema` when the
        // schema carries a keyword its structured-output mode does not permit.
        // Rust validation, not the schema, is authoritative for these rules.
        let mut pending = vec![&schema];
        while let Some(node) = pending.pop() {
            match node {
                serde_json::Value::Object(fields) => {
                    for rejected in ["uniqueItems", "minContains", "maxContains"] {
                        assert!(
                            !fields.contains_key(rejected),
                            "plan schema must not use `{rejected}`"
                        );
                    }
                    pending.extend(fields.values());
                }
                serde_json::Value::Array(items) => pending.extend(items.iter()),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn runs_both_cli_adapters_without_a_shell() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let codex = temporary.path().join("codex");
        let claude = temporary.path().join("claude");
        let direct = serde_json::to_string(&sample_proposal()).expect("plan should serialize");
        let wrapped = serde_json::json!({ "structured_output": sample_proposal() }).to_string();
        write_executable(
            &codex,
            &format!("#!/bin/sh\nread prompt\nprintf '%s' '{direct}'\n"),
        );
        write_executable(
            &claude,
            &format!("#!/bin/sh\nread prompt\nprintf '%s' '{wrapped}'\n"),
        );
        let runner = PlannerRunner::new(AgentCommands { codex, claude });

        for agent in [AgentKind::Codex, AgentKind::Claude] {
            let result = generate_past_a_busy_executable(&runner, agent, temporary.path()).await;
            assert_eq!(result.proposal, sample_proposal());
            assert_eq!(result.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn preserves_nonzero_exit_evidence() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let failing = temporary.path().join("failing");
        write_executable(
            &failing,
            "#!/bin/sh\nprintf 'partial'\nprintf 'not authenticated' >&2\nexit 2\n",
        );
        let runner = PlannerRunner::new(AgentCommands {
            codex: failing.clone(),
            claude: failing,
        });
        let failure = runner
            .generate(AgentKind::Codex, temporary.path(), "Plan")
            .await
            .expect_err("nonzero exit should fail");
        assert_eq!(failure.exit_code, Some(2));
        assert_eq!(failure.final_output, "partial");
        assert_eq!(failure.diagnostic_output, "not authenticated");
    }

    #[tokio::test]
    async fn runs_implementers_inside_the_selected_worktree() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let codex = temporary.path().join("codex");
        let claude = temporary.path().join("claude");
        let marker = temporary.path().join("implemented");
        let script = format!(
            "#!/bin/sh\nread prompt\nprintf done > '{}'\nprintf 'implemented'\n",
            marker.display()
        );
        write_executable(&codex, &script);
        write_executable(&claude, &script);
        let runner = ImplementerRunner::new(AgentCommands { codex, claude });

        for agent in [AgentKind::Codex, AgentKind::Claude] {
            let output = runner
                .implement(agent, temporary.path(), "Implement the task")
                .await
                .expect("implementation should complete");
            assert_eq!(output.final_output, "implemented");
            assert_eq!(output.exit_code, 0);
        }
        assert!(marker.is_file());
    }

    #[tokio::test]
    async fn bounds_prompt_delivery_by_the_implementation_timeout() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let stalled = temporary.path().join("stalled");
        write_executable(&stalled, "#!/bin/sh\nsleep 60\n");
        let runner = ImplementerRunner::new(AgentCommands {
            codex: stalled.clone(),
            claude: stalled,
        })
        .with_timeout(Duration::from_millis(50));
        let prompt = "x".repeat(1024 * 1024);

        let failure = runner
            .implement(AgentKind::Codex, temporary.path(), &prompt)
            .await
            .expect_err("stalled prompt delivery should time out");

        assert!(failure.message.contains("implementer timeout"));
    }

    #[tokio::test]
    async fn streams_activity_and_cancels_the_implementation_process_group() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let stalled = temporary.path().join("stalled");
        write_executable(
            &stalled,
            "#!/bin/sh\nread prompt\nprintf 'editing files\\n'\nprintf 'inspecting\\n' >&2\nsleep 60\n",
        );
        let runner = ImplementerRunner::new(AgentCommands {
            codex: stalled.clone(),
            claude: stalled,
        });
        let worktree = temporary.path().to_path_buf();
        let (activity_sender, mut activity_receiver) = mpsc::channel(8);
        let (cancellation_sender, cancellation_receiver) = watch::channel(false);
        let implementation = tokio::spawn(async move {
            runner
                .implement_with_activity(
                    AgentKind::Codex,
                    &worktree,
                    "Implement",
                    Some(activity_sender),
                    Some(cancellation_receiver),
                )
                .await
        });

        let mut activity = Vec::new();
        while activity.len() < 2 {
            activity.push(
                timeout(Duration::from_secs(2), activity_receiver.recv())
                    .await
                    .expect("activity should arrive before timeout")
                    .expect("activity channel should remain open"),
            );
        }
        cancellation_sender
            .send(true)
            .expect("supervisor should accept cancellation");
        let failure = timeout(Duration::from_secs(10), implementation)
            .await
            .expect("cancelled implementation should stop promptly")
            .expect("implementation task should join")
            .expect_err("cancelled implementation should not complete");

        assert!(failure.cancelled);
        assert!(failure.message.contains("cancelled by the user"));
        assert!(activity.iter().any(|item| {
            item.kind == ImplementationActivityKind::Output
                && item.message.contains("editing files")
        }));
        assert!(activity.iter().any(|item| {
            item.kind == ImplementationActivityKind::Diagnostic
                && item.message.contains("inspecting")
        }));
    }

    #[tokio::test]
    async fn preserves_utf8_code_points_split_across_output_reads() {
        let (sender, mut receiver) = mpsc::channel(4);
        let mut pending = vec![0xc3];

        emit_complete_utf8_activity(
            &sender,
            ImplementationActivityKind::Output,
            &mut pending,
            false,
        )
        .await;
        assert!(receiver.try_recv().is_err());

        pending.push(0xa9);
        emit_complete_utf8_activity(
            &sender,
            ImplementationActivityKind::Output,
            &mut pending,
            false,
        )
        .await;
        assert_eq!(
            receiver
                .recv()
                .await
                .expect("completed UTF-8 should emit")
                .message,
            "é"
        );
        assert!(pending.is_empty());
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("fake CLI should be written");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fake CLI should be executable");
    }

    /// A sibling test that spawns a process can inherit the write descriptor of
    /// a fake CLI this test just created, so the kernel reports `ETXTBSY` until
    /// that unrelated child execs. Only the test fixture is racy, so retry here
    /// rather than teaching the adapter to retry a real CLI.
    async fn generate_past_a_busy_executable(
        runner: &PlannerRunner,
        agent: AgentKind,
        repository: &Path,
    ) -> PlannerOutput {
        for _ in 0..20 {
            match runner.generate(agent, repository, "Plan the change").await {
                Ok(output) => return output,
                Err(failure) if failure.message.contains("Text file busy") => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(failure) => panic!("adapter should return a plan: {failure:?}"),
            }
        }
        panic!("the fake CLI stayed busy for every attempt");
    }

    fn sample_run() -> ActiveRunSummary {
        ActiveRunSummary {
            id: "run-1".to_owned(),
            goal: "Add plan support".to_owned(),
            repository: "/tmp/project".to_owned(),
            base_revision: "0123456789".to_owned(),
            branch: Some("main".to_owned()),
            worktree_dirty: true,
            run_status: RunStatus::Draft,
            plan: None,
            worktrees: Vec::new(),
            implementation_attempts: Vec::new(),
            implementation_activity: Vec::new(),
            last_error: None,
        }
    }

    fn sample_proposal() -> PlanProposal {
        PlanProposal {
            summary: "Implement safely".to_owned(),
            tasks: vec![
                ProposedTask {
                    title: "Inspect".to_owned(),
                    description: "Inspect current behavior.".to_owned(),
                    acceptance_criteria: vec!["Behavior is understood.".to_owned()],
                    depends_on: vec![],
                },
                ProposedTask {
                    title: "Implement".to_owned(),
                    description: "Make the focused change.".to_owned(),
                    acceptance_criteria: vec!["Tests pass.".to_owned()],
                    depends_on: vec![1],
                },
            ],
        }
    }
}
