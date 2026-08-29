use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use orchestrator_agents::{AgentCommands, PlannerRunner};
use orchestrator_core::ipc::default_socket_path;
use orchestrator_store::StatePaths;

#[derive(Debug, Parser)]
#[command(about = "Run the local software build orchestration engine")]
struct Arguments {
    /// Override the Unix-domain socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Override the application state directory.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Override the Codex CLI executable.
    #[arg(long, default_value = "codex")]
    codex_bin: PathBuf,
    /// Override the Claude Code CLI executable.
    #[arg(long, default_value = "claude")]
    claude_bin: PathBuf,
    /// Project root to scan and use for cloning (repeatable; defaults to ~/Projects).
    #[arg(long = "projects-root")]
    project_roots: Vec<PathBuf>,
    /// Override the GitHub CLI executable.
    #[arg(long, default_value = "gh")]
    gh_bin: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let socket_path = match arguments.socket {
        Some(path) => path,
        None => default_socket_path().context("cannot determine the engine socket path")?,
    };

    let state_paths = match arguments.state_dir {
        Some(state_directory) => {
            if !state_directory.is_absolute() {
                bail!("state directory must be absolute");
            }
            StatePaths::new(state_directory)
        }
        None => StatePaths::discover().context("cannot determine state paths")?,
    };
    let planner = PlannerRunner::new(AgentCommands {
        codex: arguments.codex_bin,
        claude: arguments.claude_bin,
    });
    let repositories = if arguments.project_roots.is_empty() {
        let mut settings = orchestrator_engine::RepositorySettings::discover()?;
        settings.gh_bin = arguments.gh_bin;
        settings
    } else {
        for root in &arguments.project_roots {
            if !root.is_absolute() {
                bail!("projects root must be absolute: {}", root.display());
            }
        }
        orchestrator_engine::RepositorySettings {
            project_roots: arguments.project_roots,
            gh_bin: arguments.gh_bin,
        }
    };
    orchestrator_engine::serve_with_settings(socket_path, state_paths, planner, repositories).await
}
