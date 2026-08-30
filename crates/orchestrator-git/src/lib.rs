//! Local Git repository discovery and inspection.

use std::{
    collections::{HashSet, VecDeque},
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    string::FromUtf8Error,
    time::Instant,
};

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
