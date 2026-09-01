//! Isolated task worktrees and their reserved task branches.
//!
//! The engine owns every worktree it creates here. Destructive Git options are
//! deliberately absent: creation never forces, and a refused precondition is
//! reported instead of being repaired. See ADR-0006.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{GitError, checked_git, command_error, inspect_repository, run_git};

/// Reserved namespace for branches the orchestrator creates.
pub const TASK_BRANCH_PREFIX: &str = "orchestrator/";

const MAX_SLUG_LENGTH: usize = 48;
const FALLBACK_SLUG: &str = "task";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskWorktreeRequest {
    pub repository: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskWorktree {
    pub repository: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub base_revision: String,
    pub repository_dirty: bool,
}

/// Condition of a recorded task worktree relative to the repository.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskWorktreeState {
    /// The worktree exists, is registered, and is on its recorded branch.
    Ready { head_revision: String, dirty: bool },
    /// The directory no longer exists.
    Missing,
    /// The directory exists but no longer matches its record.
    Diverged(String),
}

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("repository cannot host an isolated worktree: {0}")]
    Repository(#[source] GitError),
    #[error("worktree path must be absolute: {0}")]
    RelativePath(PathBuf),
    #[error("task branch must start with `{TASK_BRANCH_PREFIX}`: {0}")]
    UnreservedBranch(String),
    #[error("task branch is not a valid Git branch name: {0}")]
    InvalidBranch(String),
    #[error("task branch already exists: {0}")]
    BranchExists(String),
    #[error("base revision must be a full 40-character commit identifier: {0}")]
    InvalidBaseRevision(String),
    #[error("base revision is no longer present in the repository: {0}")]
    MissingBaseRevision(String),
    #[error("worktree destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("repository already registers a worktree at {0}")]
    AlreadyRegistered(PathBuf),
    #[error("cannot reserve worktree destination {path}: {source}")]
    Reserve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot create the task worktree: {source}")]
    Create {
        #[source]
        source: GitError,
    },
    #[error(
        "cannot create the task worktree ({source}); \
         the reserved directory {path} requires manual removal: {cleanup}"
    )]
    CreateAndRollback {
        path: PathBuf,
        cleanup: String,
        #[source]
        source: Box<GitError>,
    },
    #[error("created worktree does not match its request: {0}")]
    Unexpected(String),
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Reduces a task title to a bounded, path- and ref-safe identifier.
#[must_use]
pub fn task_slug(title: &str) -> String {
    let mut slug = String::with_capacity(MAX_SLUG_LENGTH);
    let mut pending_separator = false;
    let mut bounded = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            slug.push(character.to_ascii_lowercase());
            if slug.len() >= MAX_SLUG_LENGTH {
                bounded = true;
                break;
            }
        } else {
            pending_separator = true;
        }
    }

    // A branch name outlives its run, so end it on a whole word rather than
    // mid-word, unless doing so would leave too little of the title.
    if bounded && let Some(boundary) = slug.rfind('-').filter(|end| *end >= MAX_SLUG_LENGTH / 2) {
        slug.truncate(boundary);
    }

    if slug.is_empty() {
        FALLBACK_SLUG.to_owned()
    } else {
        slug
    }
}

/// Builds the reserved branch name for one task of one run.
#[must_use]
pub fn task_branch_name(run_id: &str, position: u32, title: &str) -> String {
    format!(
        "{TASK_BRANCH_PREFIX}{}/{position}-{}",
        task_slug(run_id),
        task_slug(title)
    )
}

/// Builds the engine-owned worktree directory for one task of one run.
#[must_use]
pub fn task_worktree_path(root: &Path, run_id: &str, position: u32, title: &str) -> PathBuf {
    root.join(task_slug(run_id))
        .join(format!("{position}-{}", task_slug(title)))
}

/// Creates one linked worktree on a new reserved task branch.
///
/// Every precondition is checked before the repository is touched, and the
/// directory this function reserves is the only path it will ever remove.
///
/// # Errors
///
/// Returns an error when the repository is unusable, the branch is outside the
/// reserved namespace or already exists, the base revision is missing, the
/// destination cannot be reserved exclusively, or Git refuses the operation.
pub fn create_task_worktree(request: &TaskWorktreeRequest) -> Result<TaskWorktree, WorktreeError> {
    let repository = inspect_repository(&request.repository).map_err(WorktreeError::Repository)?;
    validate_branch(&request.branch)?;
    validate_base_revision(&request.base_revision)?;
    if !request.path.is_absolute() {
        return Err(WorktreeError::RelativePath(request.path.clone()));
    }
    if !commit_exists(&repository.root, &request.base_revision)? {
        return Err(WorktreeError::MissingBaseRevision(
            request.base_revision.clone(),
        ));
    }
    if reference_exists(&repository.root, &request.branch)? {
        return Err(WorktreeError::BranchExists(request.branch.clone()));
    }

    let destination = intended_destination(&request.path)?;
    if registered_worktrees(&repository.root)?
        .iter()
        .any(|registered| registered == &destination)
    {
        return Err(WorktreeError::AlreadyRegistered(destination));
    }
    let destination = reserve_destination(&destination)?;

    let created = checked_git(
        &repository.root,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--no-track"),
            OsStr::new("-b"),
            OsStr::new(&request.branch),
            destination.as_os_str(),
            OsStr::new(&request.base_revision),
        ],
        "creating the task worktree",
    );
    if let Err(source) = created {
        return Err(roll_back_reservation(
            &repository.root,
            request,
            &destination,
            source,
        ));
    }

    let worktree = match inspect_repository(&destination) {
        Ok(worktree) => worktree,
        Err(source) => {
            return Err(roll_back_reservation(
                &repository.root,
                request,
                &destination,
                source,
            ));
        }
    };
    if worktree.root != destination
        || worktree.branch.as_deref() != Some(request.branch.as_str())
        || worktree.head_revision != request.base_revision
    {
        return Err(WorktreeError::Unexpected(format!(
            "expected {} on {} at {}, found {} on {:?} at {}",
            destination.display(),
            request.branch,
            request.base_revision,
            worktree.root.display(),
            worktree.branch,
            worktree.head_revision
        )));
    }

    Ok(TaskWorktree {
        repository: repository.root,
        path: destination,
        branch: request.branch.clone(),
        base_revision: request.base_revision.clone(),
        repository_dirty: repository.dirty,
    })
}

/// Reports whether a recorded task worktree still matches its record.
///
/// # Errors
///
/// Returns an error when the repository cannot be queried.
pub fn task_worktree_state(
    repository: &Path,
    path: &Path,
    branch: &str,
) -> Result<TaskWorktreeState, GitError> {
    if !path.exists() {
        return Ok(TaskWorktreeState::Missing);
    }
    let registered = registered_worktrees(repository)?;
    let Ok(canonical) = fs::canonicalize(path) else {
        return Ok(TaskWorktreeState::Missing);
    };
    if !registered.iter().any(|entry| entry == &canonical) {
        return Ok(TaskWorktreeState::Diverged(format!(
            "{} is no longer registered by the repository",
            canonical.display()
        )));
    }
    let state = match inspect_repository(&canonical) {
        Ok(state) => state,
        Err(error) => return Ok(TaskWorktreeState::Diverged(error.to_string())),
    };
    if state.branch.as_deref() != Some(branch) {
        return Ok(TaskWorktreeState::Diverged(format!(
            "expected branch {branch}, found {:?}",
            state.branch
        )));
    }
    Ok(TaskWorktreeState::Ready {
        head_revision: state.head_revision,
        dirty: state.dirty,
    })
}

/// Lists the canonical paths of every worktree the repository registers.
///
/// # Errors
///
/// Returns an error when Git cannot be started or reports a failure.
pub fn registered_worktrees(repository: &Path) -> Result<Vec<PathBuf>, GitError> {
    let operation = "listing registered worktrees";
    let output = checked_git(
        repository,
        &["worktree", "list", "--porcelain", "-z"],
        operation,
    )?;
    let listing = String::from_utf8(output.stdout)
        .map_err(|source| GitError::NonUtf8 { operation, source })?;
    Ok(listing
        .split('\0')
        .filter_map(|record| record.strip_prefix("worktree "))
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)))
        .collect())
}

/// Drops administrative records for worktree directories that no longer exist.
///
/// # Errors
///
/// Returns an error when Git cannot be started or reports a failure.
pub fn prune_missing_worktrees(repository: &Path) -> Result<(), GitError> {
    checked_git(
        repository,
        &["worktree", "prune"],
        "pruning missing worktree records",
    )
    .map(|_| ())
}

fn validate_branch(branch: &str) -> Result<(), WorktreeError> {
    if !branch.starts_with(TASK_BRANCH_PREFIX) {
        return Err(WorktreeError::UnreservedBranch(branch.to_owned()));
    }
    if branch.len() > 255
        || branch.contains("..")
        || branch
            .chars()
            .any(|character| !character.is_ascii_alphanumeric() && !matches!(character, '-' | '/'))
    {
        return Err(WorktreeError::InvalidBranch(branch.to_owned()));
    }
    Ok(())
}

fn validate_base_revision(revision: &str) -> Result<(), WorktreeError> {
    if revision.len() == 40
        && revision
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(WorktreeError::InvalidBaseRevision(revision.to_owned()))
    }
}

fn commit_exists(repository: &Path, revision: &str) -> Result<bool, GitError> {
    let operation = "resolving the base revision";
    let output = run_git(
        repository,
        &["cat-file", "-e", &format!("{revision}^{{commit}}")],
        operation,
    )?;
    Ok(output.status.success())
}

fn reference_exists(repository: &Path, reference: &str) -> Result<bool, GitError> {
    let operation = "checking whether the task branch exists";
    let output = run_git(
        repository,
        &["rev-parse", "--verify", "--quiet", reference],
        operation,
    )?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Err(command_error(&output, operation))
}

fn intended_destination(path: &Path) -> Result<PathBuf, WorktreeError> {
    let parent = path
        .parent()
        .ok_or_else(|| WorktreeError::RelativePath(path.to_path_buf()))?;
    let name = path
        .file_name()
        .ok_or_else(|| WorktreeError::RelativePath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| WorktreeError::Reserve {
        path: parent.to_path_buf(),
        source,
    })?;
    let parent = fs::canonicalize(parent).map_err(|source| WorktreeError::Reserve {
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(parent.join(name))
}

fn reserve_destination(destination: &Path) -> Result<PathBuf, WorktreeError> {
    match fs::create_dir(destination) {
        Ok(()) => Ok(destination.to_path_buf()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(WorktreeError::DestinationExists(destination.to_path_buf()))
        }
        Err(source) => Err(WorktreeError::Reserve {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

/// Undoes exactly what this module reserved: the destination directory, the
/// incomplete registration, and the branch `git worktree add` creates before it
/// can fail. The branch is released only while it still points at the base
/// revision, so a branch that has moved is never lost.
fn roll_back_reservation(
    repository: &Path,
    request: &TaskWorktreeRequest,
    destination: &Path,
    source: GitError,
) -> WorktreeError {
    let mut problems = Vec::new();
    if let Err(problem) = remove_reserved_destination(destination) {
        problems.push(problem);
    }
    if let Err(problem) = prune_missing_worktrees(repository) {
        problems.push(problem.to_string());
    }
    if let Err(problem) = release_reserved_branch(repository, request) {
        problems.push(problem);
    }

    if problems.is_empty() {
        WorktreeError::Create { source }
    } else {
        WorktreeError::CreateAndRollback {
            path: destination.to_path_buf(),
            cleanup: problems.join("; "),
            source: Box::new(source),
        }
    }
}

fn release_reserved_branch(repository: &Path, request: &TaskWorktreeRequest) -> Result<(), String> {
    let reference = format!("refs/heads/{}", request.branch);
    let existing = run_git(
        repository,
        &["rev-parse", "--verify", "--quiet", &reference],
        "checking the reserved task branch",
    )
    .map_err(|error| error.to_string())?;
    if !existing.status.success() {
        return Ok(());
    }

    let released = run_git(
        repository,
        &["update-ref", "-d", &reference, &request.base_revision],
        "releasing the reserved task branch",
    )
    .map_err(|error| error.to_string())?;
    if released.status.success() {
        Ok(())
    } else {
        Err(format!(
            "reserved branch {} no longer points at {} and was kept",
            request.branch, request.base_revision
        ))
    }
}

fn remove_reserved_destination(destination: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", destination.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "reserved destination became a symbolic link: {}",
            destination.display()
        ));
    }
    fs::remove_dir_all(destination)
        .map_err(|error| format!("cannot remove {}: {error}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    use tempfile::TempDir;

    #[test]
    fn builds_bounded_reserved_names_from_titles() {
        assert_eq!(
            task_slug("Add the Repository Chooser!"),
            "add-the-repository-chooser"
        );
        assert_eq!(task_slug("  ../../etc/passwd  "), "etc-passwd");
        assert_eq!(task_slug("🙂"), FALLBACK_SLUG);
        assert!(task_slug(&"long".repeat(40)).len() <= MAX_SLUG_LENGTH);
        assert_eq!(
            task_slug("Add panel-side worktree actions and task associations"),
            "add-panel-side-worktree-actions-and-task",
            "a bounded slug ends on a whole word"
        );
        assert!(
            !task_slug(&"word ".repeat(30)).ends_with('-'),
            "a bounded slug never ends on a separator"
        );

        let branch = task_branch_name("019-run", 2, "Refuse dirty worktrees");
        assert_eq!(branch, "orchestrator/019-run/2-refuse-dirty-worktrees");
        assert_eq!(
            task_worktree_path(Path::new("/state/worktrees"), "019-run", 2, "Refuse it"),
            Path::new("/state/worktrees/019-run/2-refuse-it")
        );
    }

    #[test]
    fn creates_an_isolated_worktree_without_touching_the_primary_checkout() {
        let fixture = Fixture::new();
        fixture.write_untracked("scratch.txt", "user work in progress");
        let before = inspect_repository(fixture.repository()).expect("repository should inspect");

        let worktree = create_task_worktree(&fixture.request("orchestrator/run/1-task"))
            .expect("worktree should be created");

        assert_eq!(worktree.branch, "orchestrator/run/1-task");
        assert_eq!(worktree.base_revision, fixture.base_revision);
        assert!(worktree.repository_dirty);
        assert!(worktree.path.join("README.md").is_file());

        let after = inspect_repository(fixture.repository()).expect("repository should inspect");
        assert_eq!(after.branch, before.branch);
        assert_eq!(after.head_revision, before.head_revision);
        assert_eq!(
            fs::read_to_string(fixture.repository().join("scratch.txt"))
                .expect("user file should survive"),
            "user work in progress"
        );
        assert!(
            !worktree.path.join("scratch.txt").exists(),
            "the task worktree starts from the committed base revision"
        );

        let state = task_worktree_state(fixture.repository(), &worktree.path, &worktree.branch)
            .expect("state should resolve");
        assert_eq!(
            state,
            TaskWorktreeState::Ready {
                head_revision: fixture.base_revision.clone(),
                dirty: false,
            }
        );
    }

    #[test]
    fn refuses_a_branch_outside_the_reserved_namespace() {
        let fixture = Fixture::new();
        let error = create_task_worktree(&fixture.request("feature/user-branch"))
            .expect_err("unreserved branch should be refused");
        assert!(matches!(error, WorktreeError::UnreservedBranch(_)));
    }

    #[test]
    fn refuses_a_branch_that_already_exists() {
        let fixture = Fixture::new();
        fixture.git(&["branch", "orchestrator/run/1-task"]);

        let error = create_task_worktree(&fixture.request("orchestrator/run/1-task"))
            .expect_err("existing branch should be refused");
        assert!(matches!(error, WorktreeError::BranchExists(_)));
    }

    #[test]
    fn refuses_a_base_revision_the_repository_no_longer_has() {
        let fixture = Fixture::new();
        let mut request = fixture.request("orchestrator/run/1-task");
        request.base_revision = "0".repeat(40);

        let error =
            create_task_worktree(&request).expect_err("missing base revision should be refused");
        assert!(matches!(error, WorktreeError::MissingBaseRevision(_)));

        request.base_revision = "HEAD".to_owned();
        let error =
            create_task_worktree(&request).expect_err("abbreviated revision should be refused");
        assert!(matches!(error, WorktreeError::InvalidBaseRevision(_)));
    }

    #[test]
    fn refuses_an_occupied_destination_without_reading_it() {
        let fixture = Fixture::new();
        let request = fixture.request("orchestrator/run/1-task");
        fs::create_dir_all(&request.path).expect("destination should be created");
        fs::write(request.path.join("existing.txt"), "user data")
            .expect("existing file should be written");

        let error =
            create_task_worktree(&request).expect_err("occupied destination should be refused");
        assert!(matches!(error, WorktreeError::DestinationExists(_)));
        assert_eq!(
            fs::read_to_string(request.path.join("existing.txt")).expect("file should survive"),
            "user data"
        );
    }

    #[test]
    fn refuses_a_destination_the_repository_still_registers() {
        let fixture = Fixture::new();
        let first = create_task_worktree(&fixture.request("orchestrator/run/1-task"))
            .expect("first worktree should be created");
        fs::remove_dir_all(&first.path).expect("worktree directory should be removable");

        let mut request = fixture.request("orchestrator/run/2-task");
        request.path = first.path.clone();
        let error =
            create_task_worktree(&request).expect_err("a registered destination should be refused");
        assert!(matches!(error, WorktreeError::AlreadyRegistered(_)));
    }

    #[test]
    fn rolls_back_the_directory_and_branch_when_creation_fails() {
        let fixture = Fixture::new();
        let administration = fixture.repository().join(".git").join("worktrees");
        fs::write(&administration, "not a directory")
            .expect("worktree administration path should be blocked");
        let request = fixture.request("orchestrator/run/1-task");

        let error = create_task_worktree(&request).expect_err("creation should fail");

        assert!(
            matches!(error, WorktreeError::Create { .. }),
            "rollback should succeed: {error}"
        );
        assert!(
            !request.path.exists(),
            "reserved directory should be removed"
        );
        assert!(
            !fixture.reference_exists(&request.branch),
            "the branch reserved by the failed attempt should be released"
        );
    }

    #[test]
    fn keeps_a_reserved_branch_that_no_longer_points_at_the_base_revision() {
        let fixture = Fixture::new();
        fixture.git(&[
            "branch",
            "orchestrator/run/1-task",
            &fixture.second_revision,
        ]);
        let request = fixture.request("orchestrator/run/1-task");

        let kept = release_reserved_branch(fixture.repository(), &request)
            .expect_err("a moved branch should be kept");

        assert!(kept.contains("was kept"), "{kept}");
        assert!(fixture.reference_exists("orchestrator/run/1-task"));
    }

    #[test]
    fn reports_a_missing_or_diverged_worktree() {
        let fixture = Fixture::new();
        let worktree = create_task_worktree(&fixture.request("orchestrator/run/1-task"))
            .expect("worktree should be created");

        run_command(&worktree.path, &["checkout", "--quiet", "--detach"]);
        let diverged = task_worktree_state(fixture.repository(), &worktree.path, &worktree.branch)
            .expect("state should resolve");
        assert!(matches!(diverged, TaskWorktreeState::Diverged(_)));

        fs::remove_dir_all(&worktree.path).expect("worktree directory should be removable");
        let missing = task_worktree_state(fixture.repository(), &worktree.path, &worktree.branch)
            .expect("state should resolve");
        assert_eq!(missing, TaskWorktreeState::Missing);

        prune_missing_worktrees(fixture.repository()).expect("pruning should succeed");
        assert!(
            !registered_worktrees(fixture.repository())
                .expect("registrations should list")
                .contains(&worktree.path)
        );
    }

    struct Fixture {
        directory: TempDir,
        repository: PathBuf,
        base_revision: String,
        second_revision: String,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("temporary directory should exist");
            let repository = directory.path().join("repository");
            fs::create_dir(&repository).expect("repository directory should exist");
            run_command(&repository, &["init", "--quiet"]);
            fs::write(repository.join("README.md"), "base").expect("file should be written");
            run_command(&repository, &["add", "README.md"]);
            commit(&repository, "test: base");
            let base_revision = revision(&repository);
            fs::write(repository.join("README.md"), "second").expect("file should be written");
            run_command(&repository, &["add", "README.md"]);
            commit(&repository, "test: second");
            let second_revision = revision(&repository);
            run_command(&repository, &["checkout", "--quiet", &base_revision]);
            run_command(&repository, &["checkout", "--quiet", "-B", "main"]);
            run_command(&repository, &["reset", "--quiet", "--hard", &base_revision]);

            Self {
                directory,
                repository,
                base_revision,
                second_revision,
            }
        }

        fn repository(&self) -> &Path {
            &self.repository
        }

        fn worktree_root(&self) -> PathBuf {
            self.directory.path().join("worktrees")
        }

        fn request(&self, branch: &str) -> TaskWorktreeRequest {
            TaskWorktreeRequest {
                repository: self.repository().to_path_buf(),
                path: self.worktree_root().join(branch.replace('/', "-")),
                branch: branch.to_owned(),
                base_revision: self.base_revision.clone(),
            }
        }

        fn write_untracked(&self, name: &str, contents: &str) {
            fs::write(self.repository().join(name), contents).expect("file should be written");
        }

        fn reference_exists(&self, reference: &str) -> bool {
            super::reference_exists(self.repository(), reference).expect("reference should resolve")
        }

        fn git(&self, arguments: &[&str]) {
            run_command(self.repository(), arguments);
        }
    }

    fn commit(repository: &Path, message: &str) {
        run_command(
            repository,
            &[
                "-c",
                "user.name=Test User",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                message,
            ],
        );
    }

    fn revision(repository: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-parse", "HEAD"])
            .env("LC_ALL", "C")
            .output()
            .expect("Git should start");
        String::from_utf8(output.stdout)
            .expect("revision should be UTF-8")
            .trim()
            .to_owned()
    }

    fn run_command(repository: &Path, arguments: &[&str]) {
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
