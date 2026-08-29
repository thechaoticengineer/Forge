//! Constrained adapters for authenticated Codex CLI and Claude Code CLI installations.

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use orchestrator_core::state::{ActiveRunSummary, AgentKind, PlanProposal};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(10);
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
            "items": { "type": "integer", "minimum": 1, "maximum": 20 },
            "uniqueItems": true
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
            prompt,
            self.timeout,
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
            });
        }

        let proposal = parse_proposal(agent, &stdout.text).map_err(|message| PlannerFailure {
            message,
            final_output: stdout.text.clone(),
            diagnostic_output: stderr.text.clone(),
            exit_code,
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
        match agent {
            AgentKind::Codex => &self.commands.codex,
            AgentKind::Claude => &self.commands.claude,
        }
    }
}

struct ProcessOutput {
    status: std::process::ExitStatus,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
}

async fn run_command(
    mut command: Command,
    command_path: &Path,
    agent: AgentKind,
    prompt: &str,
    duration: Duration,
) -> Result<ProcessOutput, PlannerFailure> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        PlannerFailure::new(format!(
            "cannot start {} CLI at {}: {error}",
            agent.as_str(),
            command_path.display()
        ))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PlannerFailure::new("planner stdout is unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PlannerFailure::new("planner stderr is unavailable".to_owned()))?;
    let stdout_task = tokio::spawn(read_bounded(stdout, MAX_STDOUT_BYTES));
    let stderr_task = tokio::spawn(read_bounded(stderr, MAX_STDERR_BYTES));
    send_prompt(&mut child, agent, prompt).await?;

    let status_result = wait_for_child(&mut child, duration, agent).await;
    let stdout = join_output(stdout_task, "stdout").await?;
    let stderr = join_output(stderr_task, "stderr").await?;
    let status = status_result.map_err(|message| PlannerFailure {
        message,
        final_output: stdout.text.clone(),
        diagnostic_output: stderr.text.clone(),
        exit_code: None,
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
    prompt: &str,
) -> Result<(), PlannerFailure> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PlannerFailure::new("planner stdin is unavailable".to_owned()))?;
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
}

impl PlannerFailure {
    fn new(message: String) -> Self {
        Self {
            message,
            final_output: String::new(),
            diagnostic_output: String::new(),
            exit_code: None,
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
) -> Result<std::process::ExitStatus, String> {
    if let Ok(result) = timeout(duration, child.wait()).await {
        result.map_err(|error| format!("cannot wait for {} CLI: {error}", agent.as_str()))
    } else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        Err(format!(
            "{} CLI exceeded the planning timeout",
            agent.as_str()
        ))
    }
}

struct BoundedOutput {
    text: String,
    truncated: bool,
}

async fn read_bounded<R>(reader: R, maximum: usize) -> std::io::Result<BoundedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let read_limit = u64::try_from(maximum)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| std::io::Error::other("planner output limit is invalid"))?;
    reader.take(read_limit).read_to_end(&mut bytes).await?;
    let truncated = bytes.len() > maximum;
    if truncated {
        bytes.truncate(maximum);
    }
    Ok(BoundedOutput {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    })
}

async fn join_output(
    task: tokio::task::JoinHandle<std::io::Result<BoundedOutput>>,
    stream: &str,
) -> Result<BoundedOutput, PlannerFailure> {
    task.await
        .map_err(|error| PlannerFailure::new(format!("cannot join planner {stream}: {error}")))?
        .map_err(|error| PlannerFailure::new(format!("cannot read planner {stream}: {error}")))
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
            let result = runner
                .generate(agent, temporary.path(), "Plan the change")
                .await
                .expect("adapter should return a plan");
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

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("fake CLI should be written");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fake CLI should be executable");
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
