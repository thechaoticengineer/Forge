//! Placeholder interfaces for milestone M4 (pen.dev integration).
//!
//! Every function here returns a neutral value (false/None/Ok(vec![])/empty
//! string) so that the business tests in src/pen_dev_tests.rs compile and
//! fail at runtime, before any later M4 stage implements real behavior.
//! See docs/features/feature-specs/scenarios.md S20-S26.

use serde_json::Value;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// Whether free text (stage instructions or acceptance) references a `.pen`
/// file or a feature's `design/` folder.
#[allow(dead_code)]
pub(crate) fn references_designs(_text: &str) -> bool {
    false
}

/// Whether a stage's instructions or acceptance criteria reference designs.
#[allow(dead_code)]
pub(crate) fn stage_references_designs(_stage: &Value) -> bool {
    false
}

/// Look up `pen` in an explicit PATH-like string, never the process PATH.
#[allow(dead_code)]
pub(crate) fn find_pen(_search_path: &OsStr) -> Option<PathBuf> {
    None
}

/// Resolve the pen.dev CLI's bundled SKILL.md next to the real `pen` entry point.
#[allow(dead_code)]
pub(crate) fn resolve_skill(_search_path: &OsStr) -> Option<PathBuf> {
    None
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum ExportError {
    Unavailable(String),
    Failed(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Unavailable(message) | ExportError::Failed(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// A top-level frame name turned into the PNG filename fragment: lowercased,
/// each non-alphanumeric run replaced by `-`.
#[allow(dead_code)]
pub(crate) fn frame_slug(_name: &str) -> String {
    String::new()
}

/// Repository-relative `.pen` files changed since `base`.
#[allow(dead_code)]
pub(crate) fn changed_pen_files(_root: &Path, _base: &str) -> Result<Vec<String>, String> {
    Ok(vec![])
}

/// Export one `.pen` file to PNGs next to it; returns the repository-relative
/// PNG paths written.
#[allow(dead_code)]
pub(crate) fn export_design(
    _root: &Path,
    _pen_file: &str,
    _search_path: &OsStr,
) -> Result<Vec<String>, ExportError> {
    Ok(vec![])
}

/// Export every `.pen` file changed since `base`.
#[allow(dead_code)]
pub(crate) fn export_changed(
    _root: &Path,
    _base: &str,
    _search_path: &OsStr,
) -> Result<Vec<String>, ExportError> {
    Ok(vec![])
}

/// Shell-based pen.dev editing instructions for implementer/fixer prompts.
#[allow(dead_code)]
pub(crate) fn editing_instructions(_skill: Option<&Path>) -> String {
    String::new()
}

/// Reviewer pointers to a snapshot's changed designs and their exported PNGs.
#[allow(dead_code)]
pub(crate) fn reviewer_instructions(_designs: &[(String, Vec<String>)]) -> String {
    String::new()
}
