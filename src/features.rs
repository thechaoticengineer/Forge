//! Discovery and validation of `docs/features/<slug>/` folders (M1).
//!
//! Purely read-only: only `fs::read_dir`, `fs::metadata` and `fs::read_to_string`
//! are used, never a write, rename or delete. See docs/features/README.md and
//! docs/features/feature-specs/scenarios.md for the rules implemented here.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct Feature {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) path: PathBuf,
    pub(crate) reasons: Vec<String>,
    pub(crate) milestones: Vec<Milestone>,
}

/// One `## M<n>: <title>` section of `milestones.md` and its `Status:` line.
/// A milestone without a `Status:` line is planned.
pub(crate) struct Milestone {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) implemented: bool,
}

impl Feature {
    pub(crate) fn status(&self) -> &'static str {
        if self.reasons.is_empty() { "valid" } else { "invalid" }
    }

    /// `implemented` when every milestone is, `in progress` when some are,
    /// otherwise `planned` (also for a feature without milestones).
    pub(crate) fn progress(&self) -> &'static str {
        let done = self.milestones.iter().filter(|m| m.implemented).count();
        if done > 0 && done == self.milestones.len() {
            "implemented"
        } else if done > 0 {
            "in progress"
        } else {
            "planned"
        }
    }
}

const REQUIRED_FILES: [&str; 4] = ["README.md", "scenarios.md", "decisions.md", "milestones.md"];

/// Lines of `text` outside ``` / ~~~ fenced code blocks; fence delimiter
/// lines themselves are dropped since they are never headings or `Covers:`.
fn lines_outside_fences(text: &str) -> impl Iterator<Item = &str> {
    let mut in_fence = false;
    text.lines().filter(move |line| {
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            return false;
        }
        !in_fence
    })
}

fn is_valid_scenario_id(id: &str) -> bool {
    match id.strip_prefix('S') {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// First non-fenced `# ` heading's trimmed remainder, or `None` when there
/// is no such heading or the remainder is empty.
fn extract_title(readme: &str) -> Option<String> {
    for line in lines_outside_fences(readme) {
        if let Some(rest) = line.strip_prefix("# ") {
            let title = rest.trim();
            return if title.is_empty() { None } else { Some(title.to_string()) };
        }
    }
    None
}

/// The ID token of every non-fenced `## ` heading, in document order, with a
/// trailing `:` stripped. Shared by validation and `scenario_ids`, so both
/// read scenario headings exactly the same way.
fn heading_ids(content: &str) -> impl Iterator<Item = &str> {
    lines_outside_fences(content).filter_map(|line| {
        let rest = line.strip_prefix("## ")?;
        let token = rest.split_whitespace().next()?;
        Some(token.strip_suffix(':').unwrap_or(token))
    })
}

/// The valid scenario IDs of a `scenarios.md`, in document order and without
/// repeats. Read-only and non-judging: malformed IDs are simply skipped, since
/// `analyze_scenarios` is what reports them. Used to record the approved
/// scenario IDs of a feature (M2 S17).
pub(crate) fn scenario_ids(content: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in heading_ids(content) {
        if is_valid_scenario_id(id) && !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Scenario IDs defined by `## ` headings, and reasons for malformed or
/// duplicate IDs.
fn analyze_scenarios(content: &str) -> (HashSet<String>, Vec<String>) {
    let mut ids = HashSet::new();
    let mut duplicates_reported = HashSet::new();
    let mut reasons = Vec::new();
    for id in heading_ids(content) {
        if is_valid_scenario_id(id) {
            if !ids.insert(id.to_string()) && duplicates_reported.insert(id.to_string()) {
                reasons.push(format!("duplicate scenario ID: {id}"));
            }
        } else {
            reasons.push(format!("malformed scenario ID: {id}"));
        }
    }
    (ids, reasons)
}

/// The milestones of `milestones.md`, and reasons for malformed or unknown
/// `Covers:` entries and malformed or repeated `Status:` lines.
/// Unknown-reference checks are skipped when `scenarios_readable` is false.
fn analyze_milestones(
    content: &str,
    known_ids: &HashSet<String>,
    scenarios_readable: bool,
) -> (Vec<Milestone>, Vec<String>) {
    let mut reasons = Vec::new();
    let mut milestones: Vec<Milestone> = Vec::new();
    let mut milestone = String::new();
    let mut status_seen = false;
    for line in lines_outside_fences(content) {
        if let Some(rest) = line.strip_prefix("## ") {
            let token = rest.split_whitespace().next().unwrap_or("");
            milestone = token.strip_suffix(':').unwrap_or(token).to_string();
            let title = rest.trim()[token.len()..].trim().to_string();
            milestones.push(Milestone { id: milestone.clone(), title, implemented: false });
            status_seen = false;
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("Status:") {
            let value = rest.trim();
            match milestones.last_mut() {
                None => reasons.push(format!("Status line outside a milestone: {value}")),
                Some(_) if status_seen => reasons.push(format!("repeated Status line in {milestone}")),
                Some(current) => match value {
                    "implemented" => current.implemented = true,
                    "planned" => {},
                    _ => reasons.push(format!(
                        "malformed Status in {milestone}: {value} (expected implemented or planned)"
                    )),
                },
            }
            status_seen = true;
            continue;
        }
        let Some(rest) = line.trim().strip_prefix("Covers:") else { continue };
        let rest = rest.trim();
        if rest == "none yet" {
            continue;
        }
        for entry in rest.split(',') {
            let entry = entry.trim();
            if entry.is_empty() || !is_valid_scenario_id(entry) {
                reasons.push(format!("malformed Covers entry in {milestone}: {entry}"));
            } else if scenarios_readable && !known_ids.contains(entry) {
                reasons.push(format!("milestone {milestone} references unknown scenario ID: {entry}"));
            }
        }
    }
    (milestones, reasons)
}

fn build_feature(slug: String, path: PathBuf) -> Feature {
    let mut reasons = Vec::new();
    let mut contents: Vec<(&str, Option<String>)> = Vec::new();
    for name in REQUIRED_FILES {
        let file_path = path.join(name);
        let content = match fs::metadata(&file_path) {
            Ok(meta) if meta.is_file() => match fs::read_to_string(&file_path) {
                Ok(content) => Some(content),
                Err(_) => {
                    reasons.push(format!("unreadable file: {name}"));
                    None
                }
            },
            _ => {
                reasons.push(format!("missing required file: {name}"));
                None
            }
        };
        contents.push((name, content));
    }
    let get = |name: &str| contents.iter().find(|(n, _)| *n == name).and_then(|(_, c)| c.as_deref());

    let title = get("README.md").and_then(extract_title).unwrap_or_else(|| slug.clone());

    let scenarios_content = get("scenarios.md");
    let (known_ids, scenario_reasons) = match scenarios_content {
        Some(content) => analyze_scenarios(content),
        None => (HashSet::new(), Vec::new()),
    };
    reasons.extend(scenario_reasons);

    let mut milestones = Vec::new();
    if let Some(content) = get("milestones.md") {
        let (found, milestone_reasons) = analyze_milestones(content, &known_ids, scenarios_content.is_some());
        milestones = found;
        reasons.extend(milestone_reasons);
    }

    Feature { slug, title, path, reasons, milestones }
}

/// Discovers and validates every feature folder under `<project_root>/docs/features`.
/// Read-only: never creates, modifies, renames or deletes anything.
pub(crate) fn discover(project_root: &Path) -> Vec<Feature> {
    let root = project_root.join("docs/features");
    let Ok(entries) = fs::read_dir(&root) else { return Vec::new() };

    let mut features = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Some(slug) = entry.file_name().to_str().map(str::to_string) else { continue };
        if slug.starts_with('_') {
            continue;
        }
        let path = root.join(&slug);
        let Ok(meta) = fs::metadata(&path) else { continue };
        if !meta.is_dir() {
            continue;
        }
        features.push(build_feature(slug, path));
    }
    features.sort_by(|a, b| a.slug.cmp(&b.slug));
    features
}

#[cfg(test)]
mod tests {
    use super::discover;
    use std::path::Path;

    #[test]
    fn discovers_forge_repository_feature_specs() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let found = discover(repo_root);
        let feature_specs = found
            .iter()
            .find(|f| f.slug == "feature-specs")
            .expect("this repository's own docs/features/feature-specs should be discovered");
        assert_eq!(feature_specs.title, "Feature specs (self-specification)");
        assert_eq!(feature_specs.status(), "valid", "reasons: {:?}", feature_specs.reasons);
        assert!(!found.iter().any(|f| f.slug == "_template"));
    }
}
