//! Discovery and validation of `docs/features/<slug>/` folders (M1 stub).
//!
//! This module currently only defines the interface later stages implement:
//! discovery, validation and the read-only API and panel depend on the
//! shape here, not on any behavior yet.

use std::path::PathBuf;

#[allow(dead_code)]
pub(crate) struct Feature {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) path: PathBuf,
    pub(crate) reasons: Vec<String>,
}

impl Feature {
    #[allow(dead_code)]
    pub(crate) fn status(&self) -> &'static str {
        if self.reasons.is_empty() { "valid" } else { "invalid" }
    }
}

#[allow(dead_code)]
pub(crate) fn discover(_project_root: &std::path::Path) -> Vec<Feature> {
    Vec::new()
}
