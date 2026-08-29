use std::{
    env,
    path::{Path, PathBuf},
};

use thiserror::Error;

pub const RUNTIME_DIRECTORY: &str = "omarchy-ai-build-orchestrator";
pub const SOCKET_FILE: &str = "engine.sock";

#[derive(Debug, Error)]
pub enum RuntimePathError {
    #[error("XDG_RUNTIME_DIR is not set")]
    Missing,
    #[error("XDG_RUNTIME_DIR is not valid UTF-8")]
    NotUnicode,
    #[error("XDG_RUNTIME_DIR is empty")]
    Empty,
    #[error("XDG_RUNTIME_DIR must be an absolute path")]
    NotAbsolute,
}

pub fn socket_path(runtime_directory: impl AsRef<Path>) -> PathBuf {
    runtime_directory
        .as_ref()
        .join(RUNTIME_DIRECTORY)
        .join(SOCKET_FILE)
}

/// Returns the default engine socket path for the current user session.
///
/// # Errors
///
/// Returns an error when `XDG_RUNTIME_DIR` is missing, empty, not valid
/// UTF-8, or not an absolute path.
pub fn default_socket_path() -> Result<PathBuf, RuntimePathError> {
    let runtime_directory = match env::var("XDG_RUNTIME_DIR") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Err(RuntimePathError::Missing),
        Err(env::VarError::NotUnicode(_)) => return Err(RuntimePathError::NotUnicode),
    };

    if runtime_directory.is_empty() {
        return Err(RuntimePathError::Empty);
    }

    let runtime_directory = PathBuf::from(runtime_directory);
    if !runtime_directory.is_absolute() {
        return Err(RuntimePathError::NotAbsolute);
    }

    Ok(socket_path(runtime_directory))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_socket_path_below_runtime_directory() {
        assert_eq!(
            socket_path("/run/user/1000"),
            PathBuf::from("/run/user/1000/omarchy-ai-build-orchestrator/engine.sock")
        );
    }
}
