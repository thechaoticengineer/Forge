//! Durable SQLite storage owned by the orchestration engine.

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use rusqlite::{Connection, TransactionBehavior};
use thiserror::Error;
use tokio::sync::oneshot;

const APPLICATION_DIRECTORY: &str = "omarchy-ai-build-orchestrator";
const DATABASE_FILE: &str = "state.db";
const ARTIFACTS_DIRECTORY: &str = "artifacts";
const LATEST_SCHEMA_VERSION: i64 = 1;
const MIGRATIONS: &[(i64, &str, &str)] =
    &[(1, "initial", include_str!("../migrations/0001_initial.sql"))];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("XDG_STATE_HOME is empty")]
    EmptyStateHome,
    #[error("XDG_STATE_HOME must be an absolute path")]
    RelativeStateHome,
    #[error("HOME is not set and XDG_STATE_HOME is unavailable")]
    MissingHome,
    #[error("HOME is empty")]
    EmptyHome,
    #[error("HOME must be an absolute path")]
    RelativeHome,
    #[error("cannot {operation} {path}: {source}")]
    FileSystem {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than supported version {supported}")]
    FutureSchema { found: i64, supported: i64 },
    #[error("SQLite did not enable write-ahead logging; active mode is {0}")]
    JournalMode(String),
    #[error("storage worker stopped unexpectedly")]
    WorkerStopped,
    #[error("storage worker panicked")]
    WorkerPanicked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    root: PathBuf,
    database: PathBuf,
    artifacts: PathBuf,
}

impl StatePaths {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            database: root.join(DATABASE_FILE),
            artifacts: root.join(ARTIFACTS_DIRECTORY),
            root,
        }
    }

    /// Resolves the application state paths from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when neither a valid absolute `XDG_STATE_HOME` nor a
    /// valid absolute `HOME` is available.
    pub fn discover() -> Result<Self, StorageError> {
        Self::from_environment(env::var_os("XDG_STATE_HOME"), env::var_os("HOME"))
    }

    /// Resolves application state paths from explicit environment values.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied state or home directory is empty,
    /// relative, or entirely unavailable.
    pub fn from_environment(
        state_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, StorageError> {
        let base = if let Some(value) = state_home {
            if value.is_empty() {
                return Err(StorageError::EmptyStateHome);
            }
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(StorageError::RelativeStateHome);
            }
            path
        } else {
            let value = home.ok_or(StorageError::MissingHome)?;
            if value.is_empty() {
                return Err(StorageError::EmptyHome);
            }
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(StorageError::RelativeHome);
            }
            path.join(".local/state")
        };

        Ok(Self::new(base.join(APPLICATION_DIRECTORY)))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn database(&self) -> &Path {
        &self.database
    }

    #[must_use]
    pub fn artifacts(&self) -> &Path {
        &self.artifacts
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageHealth {
    pub database_path: PathBuf,
    pub schema_version: i64,
}

enum Command {
    Health(oneshot::Sender<Result<StorageHealth, StorageError>>),
    Shutdown,
}

pub struct StorageWorker {
    paths: StatePaths,
    sender: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StorageWorker {
    /// Starts the dedicated SQLite worker with default user state paths.
    ///
    /// # Errors
    ///
    /// Returns an error when state paths cannot be resolved or the database
    /// cannot be initialized and migrated.
    pub fn start_default() -> Result<Self, StorageError> {
        Self::start(StatePaths::discover()?)
    }

    /// Starts the dedicated SQLite worker for explicit state paths.
    ///
    /// # Errors
    ///
    /// Returns an error when directories, permissions, SQLite configuration,
    /// or migrations cannot be initialized.
    pub fn start(paths: StatePaths) -> Result<Self, StorageError> {
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_paths = paths.clone();
        let worker_thread = thread::Builder::new()
            .name("orchestrator-storage".to_owned())
            .spawn(move || {
                let database = match Database::open(&worker_paths) {
                    Ok(database) => {
                        if ready_sender.send(Ok(())).is_err() {
                            return;
                        }
                        database
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };

                run_worker(&database, &receiver);
            })
            .map_err(|source| StorageError::FileSystem {
                operation: "start storage worker for",
                path: paths.database().to_path_buf(),
                source,
            })?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                paths,
                sender,
                thread: Some(worker_thread),
            }),
            Ok(Err(error)) => {
                worker_thread
                    .join()
                    .map_err(|_| StorageError::WorkerPanicked)?;
                Err(error)
            }
            Err(_) => {
                worker_thread
                    .join()
                    .map_err(|_| StorageError::WorkerPanicked)?;
                Err(StorageError::WorkerStopped)
            }
        }
    }

    #[must_use]
    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    /// Reports the database path and current schema version.
    ///
    /// # Errors
    ///
    /// Returns an error when the worker has stopped or SQLite cannot read the
    /// current schema version.
    pub async fn health(&self) -> Result<StorageHealth, StorageError> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Command::Health(reply_sender))
            .map_err(|_| StorageError::WorkerStopped)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::WorkerStopped)?
    }
}

impl Drop for StorageWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(worker_thread) = self.thread.take() {
            let _ = worker_thread.join();
        }
    }
}

fn run_worker(database: &Database, receiver: &mpsc::Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Health(reply) => {
                let _ = reply.send(database.health());
            }
            Command::Shutdown => return,
        }
    }
}

struct Database {
    connection: Connection,
    path: PathBuf,
}

impl Database {
    fn open(paths: &StatePaths) -> Result<Self, StorageError> {
        ensure_private_directory(paths.root())?;
        ensure_private_directory(paths.artifacts())?;

        let mut connection = Connection::open(paths.database())?;
        set_file_mode(paths.database(), 0o600)?;
        configure_connection(&connection)?;
        apply_migrations(&mut connection)?;

        Ok(Self {
            connection,
            path: paths.database().to_path_buf(),
        })
    }

    fn health(&self) -> Result<StorageHealth, StorageError> {
        let schema_version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(StorageHealth {
            database_path: self.path.clone(),
            schema_version,
        })
    }
}

fn configure_connection(connection: &Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "busy_timeout", 5000_i64)?;

    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        let enabled: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !enabled.eq_ignore_ascii_case("wal") {
            return Err(StorageError::JournalMode(enabled));
        }
    }
    Ok(())
}

fn apply_migrations(connection: &mut Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY NOT NULL, \
            name TEXT NOT NULL UNIQUE, \
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))\
        ) STRICT;",
    )?;

    let current_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::FutureSchema {
            found: current_version,
            supported: LATEST_SCHEMA_VERSION,
        });
    }

    for (version, name, sql) in MIGRATIONS {
        if *version <= current_version {
            continue;
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, name) VALUES (?1, ?2)",
            (version, name),
        )?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err(StorageError::FileSystem {
                    operation: "use non-directory state path",
                    path: path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "path is not a directory",
                    ),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|source| StorageError::FileSystem {
                    operation: "create directory",
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        Err(source) => {
            return Err(StorageError::FileSystem {
                operation: "inspect",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    set_file_mode(path, 0o700)
}

fn set_file_mode(path: &Path, mode: u32) -> Result<(), StorageError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        StorageError::FileSystem {
            operation: "set permissions on",
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    #[test]
    fn resolves_xdg_and_home_state_paths() {
        let xdg = StatePaths::from_environment(
            Some(OsString::from("/tmp/xdg-state")),
            Some(OsString::from("/home/tester")),
        )
        .expect("XDG path should resolve");
        assert_eq!(
            xdg.database(),
            Path::new("/tmp/xdg-state/omarchy-ai-build-orchestrator/state.db")
        );

        let fallback = StatePaths::from_environment(None, Some(OsString::from("/home/tester")))
            .expect("HOME fallback should resolve");
        assert_eq!(
            fallback.database(),
            Path::new("/home/tester/.local/state/omarchy-ai-build-orchestrator/state.db")
        );
    }

    #[test]
    fn rejects_relative_state_home() {
        let error = StatePaths::from_environment(Some(OsString::from("relative")), None)
            .expect_err("relative XDG state path should fail");
        assert!(matches!(error, StorageError::RelativeStateHome));
    }

    #[tokio::test]
    async fn initializes_private_migrated_store() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let worker = StorageWorker::start(paths.clone()).expect("storage should start");
        let health = worker.health().await.expect("health should succeed");

        assert_eq!(health.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(health.database_path, paths.database());
        assert_eq!(mode(paths.root()), 0o700);
        assert_eq!(mode(paths.artifacts()), 0o700);
        assert_eq!(mode(paths.database()), 0o600);

        drop(worker);

        let connection = Connection::open(paths.database()).expect("database should reopen");
        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign key pragma should be readable");
        assert_eq!(foreign_keys, 0, "pragmas are connection-local");
        let migration_count: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration table should be readable");
        assert_eq!(migration_count, 1);
        let project_table: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'projects'",
                [],
                |row| row.get(0),
            )
            .expect("schema should be readable");
        assert_eq!(project_table, 1);
    }

    #[tokio::test]
    async fn reopens_at_the_same_schema_version() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));

        let first = StorageWorker::start(paths.clone()).expect("first worker should start");
        drop(first);
        let second = StorageWorker::start(paths).expect("second worker should start");

        assert_eq!(
            second
                .health()
                .await
                .expect("health should succeed")
                .schema_version,
            LATEST_SCHEMA_VERSION
        );
    }

    #[test]
    fn configures_required_sqlite_pragmas() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let database = Database::open(&paths).expect("database should open");

        let foreign_keys: i64 = database
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign key pragma should be readable");
        let journal_mode: String = database
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode should be readable");
        let synchronous: i64 = database
            .connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("synchronous mode should be readable");

        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 2);
    }

    #[test]
    fn rejects_a_future_schema_version() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let paths = StatePaths::new(temporary.path().join("state"));
        let database = Database::open(&paths).expect("database should open");
        database
            .connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .expect("future version should be set for the test");
        drop(database);

        let Err(error) = StorageWorker::start(paths) else {
            panic!("future schema should be rejected");
        };
        assert!(matches!(
            error,
            StorageError::FutureSchema {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn refuses_a_symlink_as_the_state_root() {
        let temporary = TempDir::new().expect("temporary directory should exist");
        let actual = temporary.path().join("actual");
        fs::create_dir(&actual).expect("actual directory should exist");
        let linked = temporary.path().join("linked");
        symlink(&actual, &linked).expect("state symlink should exist");

        let Err(error) = StorageWorker::start(StatePaths::new(linked)) else {
            panic!("symlinked state root should be rejected");
        };
        assert!(matches!(error, StorageError::FileSystem { .. }));
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("path metadata should exist")
            .permissions()
            .mode()
            & 0o777
    }
}
