//! Local Git repository discovery and inspection.

use std::{
    collections::{HashSet, VecDeque},
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Output, Stdio},
    string::FromUtf8Error,
    time::Instant,
};

use orchestrator_core::state::{ChangedFileStatus, ChangedFileSummary};
use tempfile::TempDir;
use thiserror::Error;

mod worktree;

pub use worktree::{
    TASK_BRANCH_PREFIX, TaskWorktree, TaskWorktreeRequest, TaskWorktreeState, WorktreeError,
    create_task_worktree, prune_missing_worktrees, registered_worktrees, task_branch_name,
    task_slug, task_worktree_path, task_worktree_state,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryState {
    pub root: PathBuf,
    pub git_common_dir: PathBuf,
    pub head_revision: String,
    pub branch: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredRepository {
    pub state: RepositoryState,
    pub origin_url: Option<String>,
    pub github_name_with_owner: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryDiscovery {
    pub repositories: Vec<DiscoveredRepository>,
    pub truncated: bool,
    pub skipped_entries: usize,
}

const MAX_DISCOVERY_DEPTH: usize = 4;
const MAX_DISCOVERY_DIRECTORIES: usize = 4_000;
const MAX_REVIEW_DIFF_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskChangeSet {
    pub tree_hash: String,
    pub changed_files: Vec<ChangedFileSummary>,
    pub patch: String,
}

/// Read-only preflight evidence for integrating an approved task commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskIntegrationTarget {
    pub target_branch: String,
    pub expected_head: String,
    pub task_commit: String,
}

/// Validates that a local branch can be advanced to an approved task commit
/// without synthesizing a merge or rewriting history.
///
/// # Errors
///
/// Returns an error when the branch or commit is missing, the branch name is
/// invalid, or advancing the branch would not be a fast-forward.
pub fn prepare_task_integration(
    repository: &Path,
    target_branch: &str,
    task_commit: &str,
) -> Result<TaskIntegrationTarget, GitError> {
    let repository = inspect_repository(repository)?;
    let target_ref = validate_local_branch(&repository.root, target_branch)?;
    let expected_head = resolve_commit(
        &repository.root,
        &target_ref,
        "resolving the integration target branch",
    )?;
    let task_commit = resolve_commit(
        &repository.root,
        task_commit,
        "resolving the approved task commit",
    )?;
    if changes_gitlinks(&repository.root, &expected_head, &task_commit)? {
        return Err(GitError::IntegrationChangesSubmodules);
    }
    if expected_head != task_commit
        && !is_ancestor(&repository.root, &expected_head, &task_commit)?
        && !is_ancestor(&repository.root, &task_commit, &expected_head)?
    {
        return Err(GitError::IntegrationNotFastForward {
            branch: target_branch.to_owned(),
            head: expected_head,
            task_commit,
        });
    }
    Ok(TaskIntegrationTarget {
        target_branch: target_branch.to_owned(),
        expected_head,
        task_commit,
    })
}

/// Advances a local branch to an approved task commit after rechecking the
/// exact preflight head. The target must be checked out in a clean worktree so
/// its branch, index, and files can be advanced coherently under Git's locks.
///
/// # Errors
///
/// Returns an error when the target changed after preflight, is dirty, no
/// longer permits a fast-forward, or Git cannot update it safely.
pub fn integrate_task_commit(
    repository: &Path,
    target: &TaskIntegrationTarget,
) -> Result<String, GitError> {
    let repository = inspect_repository(repository)?;
    let target_ref = validate_local_branch(&repository.root, &target.target_branch)?;
    let current_head = resolve_commit(
        &repository.root,
        &target_ref,
        "rechecking the integration target branch",
    )?;
    let task_commit = resolve_commit(
        &repository.root,
        &target.task_commit,
        "rechecking the approved task commit",
    )?;
    if current_head != target.expected_head {
        return Err(GitError::IntegrationTargetChanged {
            branch: target.target_branch.clone(),
            expected: target.expected_head.clone(),
            actual: current_head,
        });
    }
    let worktree = branch_worktree(&repository.root, &target_ref)?
        .ok_or_else(|| GitError::IntegrationBranchNotCheckedOut(target.target_branch.clone()))?;
    let checked_out = inspect_repository(&worktree)?;
    if checked_out.branch.as_deref() != Some(target.target_branch.as_str())
        || checked_out.head_revision != target.expected_head
    {
        return Err(GitError::IntegrationTargetChanged {
            branch: target.target_branch.clone(),
            expected: target.expected_head.clone(),
            actual: checked_out.head_revision,
        });
    }
    if checked_out.dirty {
        return Err(GitError::DirtyIntegrationWorktree(worktree));
    }
    if changes_gitlinks(&repository.root, &current_head, &task_commit)? {
        return Err(GitError::IntegrationChangesSubmodules);
    }
    if current_head == task_commit || is_ancestor(&repository.root, &task_commit, &current_head)? {
        return Ok(current_head);
    }
    if !is_ancestor(&repository.root, &current_head, &task_commit)? {
        return Err(GitError::IntegrationNotFastForward {
            branch: target.target_branch.clone(),
            head: current_head,
            task_commit,
        });
    }
    guarded_fast_forward(
        &checked_out.root,
        &target_ref,
        &target.expected_head,
        &task_commit,
    )?;

    resolve_commit(
        &repository.root,
        &target_ref,
        "reading the integrated branch",
    )
}

/// Captures the exact tree, changed paths, and complete binary-safe Git patch
/// that a task commit would contain without modifying the worktree index or a
/// reference.
///
/// Git writes any new blob and tree objects into the shared object database,
/// but the temporary index is removed and no reference makes those objects
/// reachable until the user approves the commit.
///
/// # Errors
///
/// Returns an error when the worktree or base revision is invalid, a temporary
/// index cannot be created, Git fails, or Git returns invalid UTF-8 metadata.
pub fn capture_task_changes(
    worktree: &Path,
    base_revision: &str,
) -> Result<TaskChangeSet, GitError> {
    validate_revision(worktree, base_revision)?;
    let repository = inspect_repository(worktree)?;
    let temporary_index = TempDir::new().map_err(GitError::TemporaryIndex)?;
    let index = temporary_index.path().join("index");
    checked_git_with_index(
        &repository.root,
        &index,
        &["read-tree", base_revision],
        "initializing the inspection index",
    )?;
    checked_git_with_index(
        &repository.root,
        &index,
        &["add", "--all"],
        "capturing task changes",
    )?;
    let tree_hash = text_output(
        checked_git_with_index(
            &repository.root,
            &index,
            &["write-tree"],
            "fingerprinting task changes",
        )?,
        "fingerprinting task changes",
    )?;
    let patch = String::from_utf8_lossy(
        &checked_git_with_index(
            &repository.root,
            &index,
            &[
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
                "--no-color",
                base_revision,
                "--",
            ],
            "capturing the task patch",
        )?
        .stdout,
    )
    .into_owned();
    let names = checked_git_with_index(
        &repository.root,
        &index,
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            base_revision,
            "--",
        ],
        "capturing changed paths",
    )?
    .stdout;
    Ok(TaskChangeSet {
        tree_hash,
        changed_files: parse_changed_files(&names),
        patch,
    })
}

/// Stages all task-worktree changes and creates one local commit.
///
/// The caller must supply the branch and base revision recorded when Forge
/// created the worktree. This operation never merges or pushes.
///
/// # Errors
///
/// Returns an error when the worktree no longer matches its reservation, has
/// no changes, Git cannot stage or commit the change, or the commit hash cannot
/// be read.
pub fn create_task_commit(
    worktree: &Path,
    expected_branch: &str,
    expected_base: &str,
    expected_tree: &str,
    message: &str,
) -> Result<String, GitError> {
    let repository = inspect_repository(worktree)?;
    if repository.branch.as_deref() != Some(expected_branch)
        || repository.head_revision != expected_base
    {
        return Err(GitError::GitCommand {
            operation: "validating the task worktree before commit",
            status: None,
            stderr: "task worktree branch or HEAD no longer matches its reservation".to_owned(),
        });
    }
    let current = capture_task_changes(&repository.root, expected_base)?;
    if current.tree_hash != expected_tree {
        return Err(GitError::WorktreeChanged {
            expected: expected_tree.to_owned(),
            actual: current.tree_hash,
        });
    }
    checked_git(&repository.root, &["add", "--all"], "staging task changes")?;
    let staged_tree = text_output(
        checked_git(
            &repository.root,
            &["write-tree"],
            "validating staged task changes",
        )?,
        "validating staged task changes",
    )?;
    if staged_tree != expected_tree {
        return Err(GitError::WorktreeChanged {
            expected: expected_tree.to_owned(),
            actual: staged_tree,
        });
    }
    let staged = run_git(
        &repository.root,
        &["diff", "--cached", "--quiet", "--exit-code"],
        "checking staged task changes",
    )?;
    if staged.status.success() {
        return Err(GitError::GitCommand {
            operation: "creating the task commit",
            status: Some(1),
            stderr: "there are no task changes to commit".to_owned(),
        });
    }
    if staged.status.code() != Some(1) {
        return Err(command_error(&staged, "checking staged task changes"));
    }
    checked_git(
        &repository.root,
        &["commit", "-m", message],
        "creating the task commit",
    )?;
    text_output(
        checked_git(
            &repository.root,
            &["rev-parse", "HEAD"],
            "reading task commit",
        )?,
        "reading task commit",
    )
}

/// Captures bounded, read-only Git evidence for an independent reviewer.
///
/// The status includes untracked paths, while the patch compares tracked
/// content with the task's recorded base revision. Reviewers may inspect an
/// untracked path directly from the read-only worktree.
///
/// # Errors
///
/// Returns an error when the path is not a repository, the base revision is
/// invalid, Git fails, or its output is not UTF-8.
pub fn review_change_evidence(worktree: &Path, base_revision: &str) -> Result<String, GitError> {
    validate_revision(worktree, base_revision)?;
    let repository = inspect_repository(worktree)?;
    let status = review_text_output(
        checked_git(
            &repository.root,
            &["status", "--short", "--untracked-files=all"],
            "capturing review status",
        )?,
        "capturing review status",
    )?;
    let patch = review_text_output(
        checked_git(
            &repository.root,
            &["diff", "--no-ext-diff", "--no-color", base_revision, "--"],
            "capturing review diff",
        )?,
        "capturing review diff",
    )?;
    let mut evidence = format!("Git status:\n{status}\n\nPatch from {base_revision}:\n{patch}");
    if evidence.len() > MAX_REVIEW_DIFF_BYTES {
        evidence.truncate(floor_char_boundary(&evidence, MAX_REVIEW_DIFF_BYTES));
        evidence.push_str("\n[review evidence truncated at 2 MiB]");
    }
    Ok(evidence)
}

fn floor_char_boundary(value: &str, maximum: usize) -> usize {
    let mut boundary = maximum.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn review_text_output(output: Output, operation: &'static str) -> Result<String, GitError> {
    String::from_utf8(output.stdout)
        .map(|value| value.trim_end().to_owned())
        .map_err(|source| GitError::NonUtf8 { operation, source })
}

/// Finds Git worktrees below the configured project roots without following
/// symbolic links or descending into repositories and common build caches.
#[must_use]
pub fn discover_repositories(roots: &[PathBuf]) -> Vec<DiscoveredRepository> {
    discover_repositories_with_report(roots).repositories
}

/// Finds repositories and reports whether safety bounds or filesystem errors
/// made the result incomplete.
#[must_use]
pub fn discover_repositories_with_report(roots: &[PathBuf]) -> RepositoryDiscovery {
    discover_repositories_inner(roots, None)
}

/// Finds repositories until a monotonic deadline, returning partial results
/// with `truncated` set when the deadline is reached.
#[must_use]
pub fn discover_repositories_until(roots: &[PathBuf], deadline: Instant) -> RepositoryDiscovery {
    discover_repositories_inner(roots, Some(deadline))
}

fn discover_repositories_inner(
    roots: &[PathBuf],
    deadline: Option<Instant>,
) -> RepositoryDiscovery {
    let mut repositories = Vec::new();
    let mut seen = HashSet::new();
    let mut visited = 0_usize;
    let mut truncated = false;
    let mut skipped_entries = 0_usize;

    'roots: for root in roots {
        let root = match fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                skipped_entries += 1;
                continue;
            }
        };
        let mut queue = VecDeque::from([(root, 0_usize)]);
        while let Some((directory, depth)) = queue.pop_front() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                truncated = true;
                break 'roots;
            }
            visited += 1;
            if visited > MAX_DISCOVERY_DIRECTORIES {
                truncated = true;
                break 'roots;
            }

            if directory.join(".git").exists() {
                match inspect_repository(&directory) {
                    Ok(state) if seen.insert(state.root.clone()) => {
                        let origin_url = repository_origin(&state.root);
                        let github_name_with_owner =
                            origin_url.as_deref().and_then(github_name_with_owner);
                        repositories.push(DiscoveredRepository {
                            state,
                            origin_url,
                            github_name_with_owner,
                        });
                    }
                    Ok(_) => {}
                    Err(_) => skipped_entries += 1,
                }
                continue;
            }

            if depth >= MAX_DISCOVERY_DEPTH {
                continue;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                skipped_entries += 1;
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    skipped_entries += 1;
                    continue;
                };
                let Ok(file_type) = entry.file_type() else {
                    skipped_entries += 1;
                    continue;
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if name.starts_with('.') {
                    if name != ".git" && path.join(".git").exists() {
                        queue.push_back((path, depth + 1));
                    }
                    continue;
                }
                if !should_skip_directory(name) {
                    queue.push_back((path, depth + 1));
                }
            }
        }
    }

    repositories.sort_by(|left, right| {
        left.state
            .root
            .to_string_lossy()
            .to_lowercase()
            .cmp(&right.state.root.to_string_lossy().to_lowercase())
    });
    RepositoryDiscovery {
        repositories,
        truncated,
        skipped_entries,
    }
}

/// Extracts `owner/name` from common GitHub HTTPS and SSH remote forms.
#[must_use]
pub fn github_name_with_owner(remote: &str) -> Option<String> {
    let trimmed = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = trimmed.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = trimmed.strip_prefix("https://github.com/") {
        path
    } else {
        trimmed.strip_prefix("http://github.com/")?
    };
    let mut components = path.split('/');
    let owner = components.next()?;
    let name = components.next()?;
    if owner.is_empty() || name.is_empty() || components.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

fn repository_origin(repository: &Path) -> Option<String> {
    let output = run_git(
        repository,
        &["config", "--get", "remote.origin.url"],
        "reading origin remote",
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let origin = text_output(output, "reading origin remote").ok()?;
    (!origin.is_empty()).then_some(origin)
}

fn should_skip_directory(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | "target" | "vendor" | "dist" | "build" | "__pycache__"
    )
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("repository path must be absolute: {0}")]
    RelativePath(PathBuf),
    #[error("repository path does not exist or is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("cannot resolve repository path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot start Git while {operation}: {source}")]
    StartGit {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("Git failed while {operation} (exit {status:?}): {stderr}")]
    GitCommand {
        operation: &'static str,
        status: Option<i32>,
        stderr: String,
    },
    #[error("Git returned non-UTF-8 output while {operation}: {source}")]
    NonUtf8 {
        operation: &'static str,
        #[source]
        source: FromUtf8Error,
    },
    #[error("bare repositories are not supported: {0}")]
    BareRepository(PathBuf),
    #[error("repository has no committed HEAD revision: {0}")]
    MissingHead(PathBuf),
    #[error("cannot create a temporary Git index: {0}")]
    TemporaryIndex(std::io::Error),
    #[error("task worktree changed after inspection (expected tree {expected}, found {actual})")]
    WorktreeChanged { expected: String, actual: String },
    #[error("invalid local integration branch: {0}")]
    InvalidIntegrationBranch(String),
    #[error("symbolic local refs cannot be integration targets: {0}")]
    SymbolicIntegrationBranch(String),
    #[error(
        "integration target branch {branch} changed after confirmation (expected {expected}, found {actual})"
    )]
    IntegrationTargetChanged {
        branch: String,
        expected: String,
        actual: String,
    },
    #[error(
        "branch {branch} at {head} cannot be fast-forwarded to approved task commit {task_commit}"
    )]
    IntegrationNotFastForward {
        branch: String,
        head: String,
        task_commit: String,
    },
    #[error("integration target worktree has uncommitted changes: {0}")]
    DirtyIntegrationWorktree(PathBuf),
    #[error("integration target branch must be checked out in a worktree: {0}")]
    IntegrationBranchNotCheckedOut(String),
    #[error("integration target branch is checked out in multiple worktrees: {0}")]
    IntegrationBranchMultipleWorktrees(String),
    #[error("integration target branch checkout changed while acquiring Git locks: {0}")]
    IntegrationBranchOwnershipChanged(String),
    #[error("integration changes submodule pointers, which is not supported safely yet")]
    IntegrationChangesSubmodules,
    #[error("cannot prepare the guarded integration transaction: {0}")]
    IntegrationGuard(std::io::Error),
    #[error(
        "integration transaction failed after updating the worktree: {operation_error}; rollback failed: {rollback_error}"
    )]
    IntegrationWorktreeRollback {
        operation_error: String,
        rollback_error: String,
    },
}

fn validate_local_branch(repository: &Path, branch: &str) -> Result<String, GitError> {
    if branch.trim() != branch || branch.is_empty() || branch.starts_with('-') {
        return Err(GitError::InvalidIntegrationBranch(branch.to_owned()));
    }
    let output = run_git(
        repository,
        &["check-ref-format", "--branch", branch],
        "validating the integration branch",
    )?;
    if !output.status.success() {
        return Err(GitError::InvalidIntegrationBranch(branch.to_owned()));
    }
    let target_ref = format!("refs/heads/{branch}");
    let symbolic = run_git(
        repository,
        &["symbolic-ref", "--quiet", &target_ref],
        "checking whether the integration branch is symbolic",
    )?;
    match symbolic.status.code() {
        Some(0) => Err(GitError::SymbolicIntegrationBranch(branch.to_owned())),
        Some(1) => Ok(target_ref),
        _ => Err(command_error(
            &symbolic,
            "checking whether the integration branch is symbolic",
        )),
    }
}

fn resolve_commit(
    repository: &Path,
    revision: &str,
    operation: &'static str,
) -> Result<String, GitError> {
    text_output(
        checked_git(
            repository,
            &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
            operation,
        )?,
        operation,
    )
}

fn is_ancestor(repository: &Path, ancestor: &str, descendant: &str) -> Result<bool, GitError> {
    let operation = "checking integration ancestry";
    let output = run_git(
        repository,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        operation,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(command_error(&output, operation)),
    }
}

fn changes_gitlinks(repository: &Path, old: &str, new: &str) -> Result<bool, GitError> {
    let operation = "checking integration submodule changes";
    let output = checked_git(
        repository,
        &["diff-tree", "--no-commit-id", "-r", "--raw", old, new],
        operation,
    )?;
    Ok(output.stdout.split(|byte| *byte == b'\n').any(|line| {
        let mut fields = line.split(u8::is_ascii_whitespace);
        let old_mode = fields.next().unwrap_or_default();
        let new_mode = fields.next().unwrap_or_default();
        old_mode.strip_prefix(b":") == Some(b"160000") || new_mode == b"160000"
    }))
}

fn branch_worktree(repository: &Path, target_ref: &str) -> Result<Option<PathBuf>, GitError> {
    let operation = "locating the integration branch worktree";
    let output = checked_git(
        repository,
        &["worktree", "list", "--porcelain", "-z"],
        operation,
    )?;
    let listing = String::from_utf8(output.stdout)
        .map_err(|source| GitError::NonUtf8 { operation, source })?;
    let mut current_path = None;
    let mut matched_path = None;
    for field in listing.split('\0') {
        if field.is_empty() {
            current_path = None;
        } else if let Some(path) = field.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if field.strip_prefix("branch ") == Some(target_ref) {
            if matched_path.is_some() {
                return Err(GitError::IntegrationBranchMultipleWorktrees(
                    target_ref
                        .strip_prefix("refs/heads/")
                        .unwrap_or(target_ref)
                        .to_owned(),
                ));
            }
            matched_path.clone_from(&current_path);
        }
    }
    Ok(matched_path)
}

fn guarded_fast_forward(
    worktree: &Path,
    target_ref: &str,
    expected_head: &str,
    task_commit: &str,
) -> Result<(), GitError> {
    // Preparing the exact ref transaction takes Git's target-ref lock and,
    // when that target is checked out here, its HEAD lock. This closes the
    // branch-switch window before any index or worktree update begins.
    let transaction =
        PreparedRefTransaction::start(worktree, target_ref, expected_head, task_commit)?;
    if let Err(error) = verify_locked_branch_worktree(worktree, target_ref) {
        let _ = transaction.abort();
        return Err(error);
    }
    let checked_out = inspect_repository(worktree)?;
    let expected_branch = target_ref
        .strip_prefix("refs/heads/")
        .expect("validated local branch ref");
    if checked_out.branch.as_deref() != Some(expected_branch)
        || checked_out.head_revision != expected_head
    {
        let _ = transaction.abort();
        return Err(GitError::IntegrationTargetChanged {
            branch: expected_branch.to_owned(),
            expected: expected_head.to_owned(),
            actual: checked_out.head_revision,
        });
    }
    if checked_out.dirty {
        let _ = transaction.abort();
        return Err(GitError::DirtyIntegrationWorktree(checked_out.root.clone()));
    }
    let worktree_update = checked_git(
        worktree,
        &["read-tree", "-u", "-m", expected_head, task_commit],
        "updating the guarded integration worktree",
    );
    if let Err(error) = worktree_update {
        let abort = transaction.abort();
        return match abort {
            Ok(()) => Err(error),
            Err(abort_error) => Err(GitError::IntegrationWorktreeRollback {
                operation_error: error.to_string(),
                rollback_error: abort_error.to_string(),
            }),
        };
    }
    if let Err(error) = transaction.commit() {
        let rollback = checked_git(
            worktree,
            &["read-tree", "-u", "-m", task_commit, expected_head],
            "rolling back the guarded integration worktree",
        );
        return match rollback {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(GitError::IntegrationWorktreeRollback {
                operation_error: error.to_string(),
                rollback_error: rollback_error.to_string(),
            }),
        };
    }
    let integrated = inspect_repository(worktree)?;
    if integrated.branch.as_deref() != Some(expected_branch)
        || integrated.head_revision != task_commit
        || integrated.dirty
    {
        return Err(GitError::IntegrationWorktreeRollback {
            operation_error:
                "Git committed the reference transaction but the checkout is not coherent"
                    .to_owned(),
            rollback_error: "automatic rollback is unsafe after the target reference moved"
                .to_owned(),
        });
    }
    Ok(())
}

fn verify_locked_branch_worktree(worktree: &Path, target_ref: &str) -> Result<(), GitError> {
    match branch_worktree(worktree, target_ref)? {
        Some(locked_worktree) if locked_worktree == worktree => Ok(()),
        _ => Err(GitError::IntegrationBranchOwnershipChanged(
            target_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(target_ref)
                .to_owned(),
        )),
    }
}

struct PreparedRefTransaction {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    error: ChildStderr,
    finished: bool,
}

impl PreparedRefTransaction {
    fn start(
        worktree: &Path,
        target_ref: &str,
        expected_head: &str,
        task_commit: &str,
    ) -> Result<Self, GitError> {
        let operation = "preparing the integration reference transaction";
        let mut child = Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args([
                "update-ref",
                "-m",
                "forge: integrate approved task commit",
                "--stdin",
            ])
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| GitError::StartGit { operation, source })?;
        let input = child.stdin.take().ok_or_else(|| {
            GitError::IntegrationGuard(std::io::Error::other("Git stdin was unavailable"))
        })?;
        let output = child.stdout.take().ok_or_else(|| {
            GitError::IntegrationGuard(std::io::Error::other("Git stdout was unavailable"))
        })?;
        let error = child.stderr.take().ok_or_else(|| {
            GitError::IntegrationGuard(std::io::Error::other("Git stderr was unavailable"))
        })?;
        let mut transaction = Self {
            child,
            input: Some(input),
            output: BufReader::new(output),
            error,
            finished: false,
        };
        transaction.command("start", Some("start: ok"))?;
        transaction.command(
            &format!("update {target_ref} {task_commit} {expected_head}"),
            None,
        )?;
        transaction.command("prepare", Some("prepare: ok"))?;
        Ok(transaction)
    }

    fn commit(mut self) -> Result<(), GitError> {
        self.command("commit", Some("commit: ok"))?;
        self.finish()
    }

    fn abort(mut self) -> Result<(), GitError> {
        self.command("abort", Some("abort: ok"))?;
        self.finish()
    }

    fn command(&mut self, command: &str, expected: Option<&str>) -> Result<(), GitError> {
        let input = self.input.as_mut().expect("live transaction input");
        writeln!(input, "{command}").map_err(GitError::IntegrationGuard)?;
        input.flush().map_err(GitError::IntegrationGuard)?;
        if let Some(expected) = expected {
            let mut response = String::new();
            self.output
                .read_line(&mut response)
                .map_err(GitError::IntegrationGuard)?;
            if response.trim_end() != expected {
                return Err(GitError::GitCommand {
                    operation: "running the integration reference transaction",
                    status: None,
                    stderr: format!(
                        "expected Git response {expected:?}, received {:?}",
                        response.trim_end()
                    ),
                });
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), GitError> {
        self.input.take();
        let status = self.child.wait().map_err(GitError::IntegrationGuard)?;
        let mut stderr = String::new();
        self.error
            .read_to_string(&mut stderr)
            .map_err(GitError::IntegrationGuard)?;
        self.finished = true;
        if status.success() {
            Ok(())
        } else {
            Err(GitError::GitCommand {
                operation: "running the integration reference transaction",
                status: status.code(),
                stderr: stderr.trim().to_owned(),
            })
        }
    }
}

impl Drop for PreparedRefTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn validate_revision(worktree: &Path, revision: &str) -> Result<(), GitError> {
    if !(4..=64).contains(&revision.len()) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(GitError::MissingHead(worktree.to_path_buf()));
    }
    Ok(())
}

fn parse_changed_files(output: &[u8]) -> Vec<ChangedFileSummary> {
    let fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status_code = fields[index].first().copied().unwrap_or(b'?');
        index += 1;
        if index >= fields.len() {
            break;
        }
        let first_path = String::from_utf8_lossy(fields[index]).into_owned();
        index += 1;
        let (previous_path, path) = if matches!(status_code, b'R' | b'C') && index < fields.len() {
            let path = String::from_utf8_lossy(fields[index]).into_owned();
            index += 1;
            (Some(first_path), path)
        } else {
            (None, first_path)
        };
        let status = match status_code {
            b'A' => ChangedFileStatus::Added,
            b'M' => ChangedFileStatus::Modified,
            b'D' => ChangedFileStatus::Deleted,
            b'R' => ChangedFileStatus::Renamed,
            b'C' => ChangedFileStatus::Copied,
            b'T' => ChangedFileStatus::TypeChanged,
            b'U' => ChangedFileStatus::Unmerged,
            _ => ChangedFileStatus::Unknown,
        };
        files.push(ChangedFileSummary {
            path,
            previous_path,
            status,
        });
    }
    files
}

/// Inspects a local Git worktree without modifying it.
///
/// # Errors
///
/// Returns an error when the path is relative, inaccessible, not a Git
/// worktree, bare, missing a committed `HEAD`, or produces invalid Git output.
pub fn inspect_repository(path: &Path) -> Result<RepositoryState, GitError> {
    if !path.is_absolute() {
        return Err(GitError::RelativePath(path.to_path_buf()));
    }
    if !path.is_dir() {
        return Err(GitError::NotDirectory(path.to_path_buf()));
    }

    let canonical_input = fs::canonicalize(path).map_err(|source| GitError::Canonicalize {
        path: path.to_path_buf(),
        source,
    })?;
    let bare = text_output(
        checked_git(
            &canonical_input,
            &["rev-parse", "--is-bare-repository"],
            "checking whether the repository is bare",
        )?,
        "checking whether the repository is bare",
    )?;
    if bare == "true" {
        return Err(GitError::BareRepository(canonical_input));
    }

    let root_output = checked_git(
        &canonical_input,
        &["rev-parse", "--show-toplevel"],
        "finding the repository root",
    )?;
    let root = path_output(root_output, "finding the repository root")?;
    let root =
        fs::canonicalize(&root).map_err(|source| GitError::Canonicalize { path: root, source })?;

    let git_common_dir = path_output(
        checked_git(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            "finding the Git common directory",
        )?,
        "finding the Git common directory",
    )?;
    let git_common_dir =
        fs::canonicalize(&git_common_dir).map_err(|source| GitError::Canonicalize {
            path: git_common_dir,
            source,
        })?;

    let head = run_git(&root, &["rev-parse", "--verify", "HEAD"], "reading HEAD")?;
    if !head.status.success() {
        return Err(GitError::MissingHead(root));
    }
    let head_revision = text_output(head, "reading HEAD")?;

    let branch_output = run_git(
        &root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "reading the current branch",
    )?;
    let branch = if branch_output.status.success() {
        Some(text_output(branch_output, "reading the current branch")?)
    } else if branch_output.status.code() == Some(1) {
        None
    } else {
        return Err(command_error(&branch_output, "reading the current branch"));
    };

    let dirty = worktree_is_dirty(&root)?;
    Ok(RepositoryState {
        root,
        git_common_dir,
        head_revision,
        branch,
        dirty,
    })
}

fn worktree_is_dirty(repository: &Path) -> Result<bool, GitError> {
    let operation = "reading worktree status";
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| GitError::StartGit { operation, source })?;
    let mut stdout = child.stdout.take().expect("piped Git stdout should exist");
    let mut first_byte = [0_u8; 1];
    let bytes_read = stdout
        .read(&mut first_byte)
        .map_err(|source| GitError::StartGit { operation, source })?;
    drop(stdout);

    if bytes_read > 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(true);
    }

    let output = child
        .wait_with_output()
        .map_err(|source| GitError::StartGit { operation, source })?;
    if output.status.success() {
        Ok(false)
    } else {
        Err(command_error(&output, operation))
    }
}

pub(crate) fn checked_git<S: AsRef<OsStr>>(
    repository: &Path,
    arguments: &[S],
    operation: &'static str,
) -> Result<Output, GitError> {
    let output = run_git(repository, arguments, operation)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(&output, operation))
    }
}

pub(crate) fn run_git<S: AsRef<OsStr>>(
    repository: &Path,
    arguments: &[S],
    operation: &'static str,
) -> Result<Output, GitError> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| GitError::StartGit { operation, source })
}

fn checked_git_with_index<S: AsRef<OsStr>>(
    repository: &Path,
    index: &Path,
    arguments: &[S],
    operation: &'static str,
) -> Result<Output, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env("GIT_INDEX_FILE", index)
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| GitError::StartGit { operation, source })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(&output, operation))
    }
}

pub(crate) fn command_error(output: &Output, operation: &'static str) -> GitError {
    GitError::GitCommand {
        operation,
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

pub(crate) fn text_output(output: Output, operation: &'static str) -> Result<String, GitError> {
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|source| GitError::NonUtf8 { operation, source })
}

fn path_output(output: Output, operation: &'static str) -> Result<PathBuf, GitError> {
    text_output(output, operation).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn inspects_clean_and_dirty_repository_state() {
        let repository = initialized_repository();
        let clean = inspect_repository(repository.path()).expect("repository should inspect");

        assert_eq!(clean.root, repository.path());
        assert_eq!(clean.git_common_dir, repository.path().join(".git"));
        assert_eq!(clean.head_revision.len(), 40);
        assert!(clean.branch.is_some());
        assert!(!clean.dirty);

        fs::write(repository.path().join("changed.txt"), "changed")
            .expect("untracked file should be created");
        let dirty = inspect_repository(repository.path()).expect("repository should inspect");
        assert!(dirty.dirty);
    }

    #[test]
    fn reports_detached_head_without_a_branch() {
        let repository = initialized_repository();
        git(repository.path(), &["checkout", "--detach"]);

        let state = inspect_repository(repository.path()).expect("repository should inspect");
        assert_eq!(state.branch, None);
    }

    #[test]
    fn rejects_non_repository_directory() {
        let directory = TempDir::new().expect("temporary directory should exist");
        let error = inspect_repository(directory.path()).expect_err("directory is not a repo");
        assert!(matches!(error, GitError::GitCommand { .. }));
    }

    #[test]
    fn rejects_bare_repository() {
        let directory = TempDir::new().expect("temporary directory should exist");
        git(directory.path(), &["init", "--bare", "--quiet"]);

        let error = inspect_repository(directory.path()).expect_err("bare repo should fail");
        assert!(matches!(error, GitError::BareRepository(_)));
    }

    #[test]
    fn rejects_repository_without_a_commit() {
        let directory = TempDir::new().expect("temporary directory should exist");
        git(directory.path(), &["init", "--quiet"]);

        let error = inspect_repository(directory.path()).expect_err("empty repo should fail");
        assert!(matches!(error, GitError::MissingHead(_)));
    }

    #[test]
    fn resolves_a_subdirectory_to_the_worktree_root() {
        let repository = initialized_repository();
        let nested = repository.path().join("nested");
        fs::create_dir(&nested).expect("nested directory should exist");

        let state = inspect_repository(&nested).expect("nested path should inspect");
        assert_eq!(state.root, repository.path());
    }

    #[test]
    fn rejects_relative_path() {
        let error = inspect_repository(Path::new("relative"))
            .expect_err("relative repository path should fail");
        assert!(matches!(error, GitError::RelativePath(_)));
    }

    #[test]
    fn captures_bounded_review_change_evidence() {
        let repository = initialized_repository();
        let base = inspect_repository(repository.path())
            .expect("repository should inspect")
            .head_revision;
        fs::write(repository.path().join("README.md"), "changed")
            .expect("tracked file should change");
        fs::write(repository.path().join("new.txt"), "new").expect("untracked file should exist");

        let evidence = review_change_evidence(repository.path(), &base)
            .expect("review evidence should be captured");

        assert!(evidence.contains(" M README.md"));
        assert!(evidence.contains("?? new.txt"));
        assert!(evidence.contains("-test"));
        assert!(evidence.contains("+changed"));
    }

    #[test]
    fn creates_a_local_task_commit_only_from_the_expected_base() {
        let repository = initialized_repository();
        git(repository.path(), &["config", "user.name", "Test User"]);
        git(
            repository.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        fs::write(repository.path().join("change.txt"), "verified")
            .expect("task change should be written");
        let inspected = capture_task_changes(repository.path(), &state.head_revision)
            .expect("task changes should be inspectable");
        assert_eq!(inspected.changed_files.len(), 1);
        assert_eq!(inspected.changed_files[0].path, "change.txt");
        assert_eq!(inspected.changed_files[0].status, ChangedFileStatus::Added);
        assert!(inspected.patch.contains("+verified"));
        let hash = create_task_commit(
            repository.path(),
            state.branch.as_deref().expect("branch should exist"),
            &state.head_revision,
            &inspected.tree_hash,
            "feat: record verified change",
        )
        .expect("task commit should be created");
        assert_eq!(hash.len(), 40);
        assert!(
            !inspect_repository(repository.path())
                .expect("repository should inspect")
                .dirty
        );
    }

    #[test]
    fn refuses_to_commit_a_tree_that_changed_after_inspection() {
        let repository = initialized_repository();
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        fs::write(repository.path().join("change.txt"), "inspected")
            .expect("task change should be written");
        let inspected = capture_task_changes(repository.path(), &state.head_revision)
            .expect("task changes should be inspectable");
        fs::write(repository.path().join("change.txt"), "changed afterward")
            .expect("task change should mutate");

        let error = create_task_commit(
            repository.path(),
            state.branch.as_deref().expect("branch should exist"),
            &state.head_revision,
            &inspected.tree_hash,
            "feat: unsafe stale change",
        )
        .expect_err("stale inspection must not be committed");

        assert!(matches!(error, GitError::WorktreeChanged { .. }));
        assert_eq!(
            inspect_repository(repository.path())
                .expect("repository should inspect")
                .head_revision,
            state.head_revision
        );
    }

    #[test]
    fn fast_forwards_a_clean_checked_out_target_branch() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let (task_worktree, task_commit) = committed_task(&repository, &worktrees);
        let branch = inspect_repository(repository.path())
            .expect("repository should inspect")
            .branch
            .expect("repository should have a branch");
        let target = prepare_task_integration(repository.path(), &branch, &task_commit)
            .expect("integration should preflight");

        let integrated = integrate_task_commit(repository.path(), &target)
            .expect("clean target should fast-forward");

        assert_eq!(integrated, task_commit);
        assert_eq!(
            inspect_repository(repository.path())
                .expect("repository should inspect")
                .head_revision,
            task_commit
        );
        assert!(repository.path().join("task.txt").exists());
        assert!(!task_worktree.as_os_str().is_empty());
    }

    #[test]
    fn refuses_a_dirty_checked_out_target_branch() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let (_, task_commit) = committed_task(&repository, &worktrees);
        let branch = inspect_repository(repository.path())
            .expect("repository should inspect")
            .branch
            .expect("repository should have a branch");
        let target = prepare_task_integration(repository.path(), &branch, &task_commit)
            .expect("integration should preflight");
        fs::write(repository.path().join("local.txt"), "uncommitted")
            .expect("target should become dirty");

        let error = integrate_task_commit(repository.path(), &target)
            .expect_err("dirty target must be refused");

        assert!(matches!(error, GitError::DirtyIntegrationWorktree(_)));
    }

    #[test]
    fn refuses_a_branch_that_is_not_checked_out() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        git(
            repository.path(),
            &["branch", "release", &state.head_revision],
        );
        let (_, task_commit) = committed_task(&repository, &worktrees);
        let target = prepare_task_integration(repository.path(), "release", &task_commit)
            .expect("integration should preflight");

        let error = integrate_task_commit(repository.path(), &target)
            .expect_err("unowned branch must be refused");

        assert!(matches!(
            error,
            GitError::IntegrationBranchNotCheckedOut(branch) if branch == "release"
        ));
        let release = text_output(
            checked_git(
                repository.path(),
                &["rev-parse", "release"],
                "reading release branch",
            )
            .expect("release should resolve"),
            "reading release branch",
        )
        .expect("release hash should be text");
        assert_eq!(release, state.head_revision);
    }

    #[test]
    fn refuses_a_branch_checked_out_in_multiple_worktrees() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let branch = state.branch.expect("branch should exist");
        let duplicate = worktrees.path().join("duplicate");
        git(
            repository.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "--force",
                duplicate.to_str().expect("worktree path should be UTF-8"),
                &branch,
            ],
        );
        let (_, task_commit) = committed_task(&repository, &worktrees);
        let target = prepare_task_integration(repository.path(), &branch, &task_commit)
            .expect("integration should preflight");

        let error = integrate_task_commit(repository.path(), &target)
            .expect_err("duplicate checked-out branch must be refused");

        assert!(matches!(
            error,
            GitError::IntegrationBranchMultipleWorktrees(error_branch)
                if error_branch == branch
        ));
        for checkout in [repository.path(), duplicate.as_path()] {
            let unchanged = inspect_repository(checkout).expect("checkout should inspect");
            assert_eq!(unchanged.head_revision, state.head_revision);
            assert!(!unchanged.dirty);
            assert!(!checkout.join("task.txt").exists());
        }
    }

    #[test]
    fn locked_ownership_recheck_detects_a_duplicate_checkout() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let branch = state.branch.expect("branch should exist");
        let duplicate = worktrees.path().join("duplicate");
        git(
            repository.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "--force",
                duplicate.to_str().expect("worktree path should be UTF-8"),
                &branch,
            ],
        );
        let (_, task_commit) = committed_task(&repository, &worktrees);
        let transaction = PreparedRefTransaction::start(
            repository.path(),
            &format!("refs/heads/{branch}"),
            &state.head_revision,
            &task_commit,
        )
        .expect("reference transaction should prepare despite an earlier forced checkout");

        let error =
            verify_locked_branch_worktree(repository.path(), &format!("refs/heads/{branch}"))
                .expect_err("locked ownership recheck must see both worktrees");

        assert!(matches!(
            error,
            GitError::IntegrationBranchMultipleWorktrees(error_branch)
                if error_branch == branch
        ));
        transaction.abort().expect("transaction should abort");
        for checkout in [repository.path(), duplicate.as_path()] {
            let unchanged = inspect_repository(checkout).expect("checkout should inspect");
            assert_eq!(unchanged.head_revision, state.head_revision);
            assert!(!unchanged.dirty);
            assert!(!checkout.join("task.txt").exists());
        }
    }

    #[test]
    fn rejects_a_symbolic_local_integration_branch() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let (_, task_commit) = committed_task(&repository, &worktrees);
        git(
            repository.path(),
            &[
                "symbolic-ref",
                "refs/heads/alias",
                &format!("refs/heads/{}", state.branch.expect("branch should exist")),
            ],
        );

        let error = prepare_task_integration(repository.path(), "alias", &task_commit)
            .expect_err("symbolic target must be refused");

        assert!(matches!(error, GitError::SymbolicIntegrationBranch(branch) if branch == "alias"));
    }

    #[test]
    fn refuses_an_integration_that_changes_a_submodule_pointer() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let (task_worktree, _) = committed_task(&repository, &worktrees);
        git(
            &task_worktree,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{},module", state.head_revision),
            ],
        );
        git(
            &task_worktree,
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: add submodule pointer",
            ],
        );
        let task_commit = inspect_repository(&task_worktree)
            .expect("task worktree should inspect")
            .head_revision;

        let error = prepare_task_integration(
            repository.path(),
            state.branch.as_deref().expect("branch should exist"),
            &task_commit,
        )
        .expect_err("submodule pointer change must be refused");

        assert!(matches!(error, GitError::IntegrationChangesSubmodules));
        let unchanged = inspect_repository(repository.path()).expect("repository should inspect");
        assert_eq!(unchanged.head_revision, state.head_revision);
        assert!(!unchanged.dirty);
        assert!(!repository.path().join("module").exists());
    }

    #[test]
    fn guarded_fast_forward_refuses_to_advance_a_different_checked_out_branch() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let target_branch = state.branch.expect("branch should exist");
        let base = state.head_revision;
        let (_, task_commit) = committed_task(&repository, &worktrees);
        git(repository.path(), &["checkout", "--quiet", "-b", "other"]);

        let error = guarded_fast_forward(
            repository.path(),
            &format!("refs/heads/{target_branch}"),
            &base,
            &task_commit,
        )
        .expect_err("guard must reject a branch ownership race");

        assert!(matches!(
            error,
            GitError::IntegrationBranchOwnershipChanged(error_branch)
                if error_branch == target_branch
        ));
        for branch in [target_branch.as_str(), "other"] {
            assert_eq!(
                resolve_commit(repository.path(), branch, "checking guarded branch")
                    .expect("branch should resolve"),
                base
            );
        }
        assert!(
            !inspect_repository(repository.path())
                .expect("rejected checkout should inspect")
                .dirty
        );
        assert!(!repository.path().join("task.txt").exists());
    }

    #[test]
    fn prepared_ref_transaction_blocks_a_concurrent_branch_switch() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let state = inspect_repository(repository.path()).expect("repository should inspect");
        let target_branch = state.branch.expect("branch should exist");
        let (_, task_commit) = committed_task(&repository, &worktrees);
        git(
            repository.path(),
            &["branch", "other", &state.head_revision],
        );
        let transaction = PreparedRefTransaction::start(
            repository.path(),
            &format!("refs/heads/{target_branch}"),
            &state.head_revision,
            &task_commit,
        )
        .expect("reference transaction should prepare");

        let checkout = Command::new("git")
            .arg("-C")
            .arg(repository.path())
            .args(["checkout", "--quiet", "other"])
            .env("LC_ALL", "C")
            .output()
            .expect("concurrent Git should start");

        assert!(!checkout.status.success());
        assert!(String::from_utf8_lossy(&checkout.stderr).contains("HEAD.lock"));
        transaction.abort().expect("transaction should abort");
        let unchanged = inspect_repository(repository.path()).expect("repository should inspect");
        assert_eq!(unchanged.branch.as_deref(), Some(target_branch.as_str()));
        assert_eq!(unchanged.head_revision, state.head_revision);
        assert!(!unchanged.dirty);
        assert!(!repository.path().join("task.txt").exists());
    }

    #[test]
    fn refuses_integration_after_the_target_branch_changes() {
        let repository = initialized_repository();
        let worktrees = TempDir::new().expect("worktree root should exist");
        let (_, task_commit) = committed_task(&repository, &worktrees);
        let branch = inspect_repository(repository.path())
            .expect("repository should inspect")
            .branch
            .expect("repository should have a branch");
        let target = prepare_task_integration(repository.path(), &branch, &task_commit)
            .expect("integration should preflight");
        fs::write(repository.path().join("main.txt"), "advanced")
            .expect("main change should be written");
        git(repository.path(), &["add", "main.txt"]);
        git(
            repository.path(),
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: advance main",
            ],
        );

        let error = integrate_task_commit(repository.path(), &target)
            .expect_err("changed target must be refused");

        assert!(matches!(error, GitError::IntegrationTargetChanged { .. }));
    }

    #[test]
    fn discovers_nested_repositories_and_github_origins() {
        let projects = TempDir::new().expect("projects root should exist");
        let first = projects.path().join("Forge");
        let second = projects.path().join("team").join("website");
        let hidden = projects.path().join(".github");
        fs::create_dir_all(&first).expect("first repository should be created");
        fs::create_dir_all(&second).expect("second repository should be created");
        fs::create_dir_all(&hidden).expect("hidden repository should be created");
        initialize_repository(&first);
        initialize_repository(&second);
        initialize_repository(&hidden);
        git(
            &first,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:thechaoticengineer/Forge.git",
            ],
        );

        let repositories = discover_repositories(&[projects.path().to_path_buf()]);

        assert_eq!(repositories.len(), 3);
        let forge = repositories
            .iter()
            .find(|repository| repository.state.root == first)
            .expect("Forge should be discovered");
        assert_eq!(
            forge.github_name_with_owner.as_deref(),
            Some("thechaoticengineer/Forge")
        );
        assert!(
            repositories
                .iter()
                .any(|repository| repository.state.root == hidden)
        );
    }

    #[test]
    fn parses_supported_github_remote_forms() {
        for remote in [
            "git@github.com:owner/project.git",
            "ssh://git@github.com/owner/project.git",
            "https://github.com/owner/project",
            "http://github.com/owner/project/",
        ] {
            assert_eq!(
                github_name_with_owner(remote).as_deref(),
                Some("owner/project")
            );
        }
        assert_eq!(
            github_name_with_owner("https://gitlab.com/owner/project"),
            None
        );
        assert_eq!(github_name_with_owner("https://github.com/owner"), None);
    }

    #[test]
    fn returns_partial_discovery_when_the_deadline_is_reached() {
        let projects = TempDir::new().expect("projects root should exist");
        let repository = projects.path().join("project");
        fs::create_dir(&repository).expect("repository directory should exist");
        initialize_repository(&repository);

        let report = discover_repositories_until(&[projects.path().to_path_buf()], Instant::now());

        assert!(report.truncated);
        assert!(report.repositories.is_empty());
    }

    fn initialized_repository() -> TempDir {
        let directory = TempDir::new().expect("temporary directory should exist");
        initialize_repository(directory.path());
        directory
    }

    fn committed_task(repository: &TempDir, worktrees: &TempDir) -> (PathBuf, String) {
        let task_worktree = worktrees.path().join("task");
        git(
            repository.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "orchestrator/test/task",
                task_worktree
                    .to_str()
                    .expect("worktree path should be UTF-8"),
            ],
        );
        fs::write(task_worktree.join("task.txt"), "approved")
            .expect("task change should be written");
        git(&task_worktree, &["add", "task.txt"]);
        git(
            &task_worktree,
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "feat: approved task",
            ],
        );
        let commit = inspect_repository(&task_worktree)
            .expect("task worktree should inspect")
            .head_revision;
        (task_worktree, commit)
    }

    fn initialize_repository(directory: &Path) {
        git(directory, &["init", "--quiet"]);
        fs::write(directory.join("README.md"), "test").expect("tracked file should be created");
        git(directory, &["add", "README.md"]);
        git(
            directory,
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "test: initialize",
            ],
        );
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .env("LC_ALL", "C")
            .output()
            .expect("Git should start");
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
