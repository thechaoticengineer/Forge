//! Read-only Git repository inspection.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    string::FromUtf8Error,
};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryState {
    pub root: PathBuf,
    pub git_common_dir: PathBuf,
    pub head_revision: String,
    pub branch: Option<String>,
    pub dirty: bool,
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

fn checked_git(
    repository: &Path,
    arguments: &[&str],
    operation: &'static str,
) -> Result<Output, GitError> {
    let output = run_git(repository, arguments, operation)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(&output, operation))
    }
}

fn run_git(
    repository: &Path,
    arguments: &[&str],
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

fn command_error(output: &Output, operation: &'static str) -> GitError {
    GitError::GitCommand {
        operation,
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

fn text_output(output: Output, operation: &'static str) -> Result<String, GitError> {
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

    fn initialized_repository() -> TempDir {
        let directory = TempDir::new().expect("temporary directory should exist");
        git(directory.path(), &["init", "--quiet"]);
        fs::write(directory.path().join("README.md"), "test")
            .expect("tracked file should be created");
        git(directory.path(), &["add", "README.md"]);
        git(
            directory.path(),
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
        directory
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
