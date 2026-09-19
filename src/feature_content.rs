//! Read-only view of one feature's content for `GET /api/features/content`
//! (M5 S37): the text of its four spec files, the parsed scenarios, the
//! discovered milestones and the files of `design/`.
//!
//! Nothing here creates, modifies or removes a file. No symlink under
//! `docs/features/<slug>/` is ever followed, the folder itself included: every
//! entry is inspected with `fs::symlink_metadata`, every file is opened with
//! `O_NOFOLLOW`, only plain relative components are joined, and a path whose
//! canonical form leaves the feature folder is never opened. Reads are capped
//! at [`MAX_FILE_BYTES`]; a file that cannot be served is reported with a
//! reason instead of failing the whole response.

use crate::features::{Feature, is_valid_scenario_id, lines_outside_fences};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// Largest file served as text.
pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Deepest directory level of `design/` that is walked.
const MAX_DESIGN_DEPTH: usize = 16;
/// Most `design/` entries listed; the walk stops with a note beyond this.
const MAX_DESIGN_ENTRIES: usize = 1000;

const FILES: [&str; 4] = ["README.md", "scenarios.md", "decisions.md", "milestones.md"];

/// The feature folder, verified to be a real directory (not a symlink) and
/// resolved to its canonical form, which bounds every path read below it.
struct Root {
    canonical: PathBuf,
}

impl Root {
    fn open(path: &Path) -> Result<Self, String> {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("could not read the feature folder: {e}"))?;
        if meta.is_symlink() {
            return Err("the feature folder is a symbolic link and is not followed".into());
        }
        if !meta.is_dir() {
            return Err("the feature folder is not a directory".into());
        }
        let canonical = fs::canonicalize(path).map_err(|e| format!("could not resolve the feature folder: {e}"))?;
        Ok(Self { canonical })
    }

    /// `rel` joined to the folder when it consists only of plain components,
    /// no component is a symlink and its canonical form stays inside.
    fn resolve(&self, rel: &Path) -> Result<PathBuf, String> {
        let mut current = self.canonical.clone();
        let mut components = rel.components().peekable();
        if components.peek().is_none() {
            return Err("empty path".into());
        }
        for component in components {
            let Component::Normal(part) = component else {
                return Err("the path has an absolute, '.' or '..' component and is refused".into());
            };
            current.push(part);
            let meta = fs::symlink_metadata(&current).map_err(|e| describe_io(&e))?;
            if meta.is_symlink() {
                return Err("symbolic link is not followed".into());
            }
        }
        let canonical = fs::canonicalize(&current).map_err(|e| describe_io(&e))?;
        if !canonical.starts_with(&self.canonical) {
            return Err("the path leaves the feature folder and is refused".into());
        }
        Ok(canonical)
    }

    /// The UTF-8 text of the regular file at `rel`, at most [`MAX_FILE_BYTES`].
    fn read_text(&self, rel: &Path) -> Result<String, String> {
        let path = self.resolve(rel)?;
        // O_NOFOLLOW refuses a link swapped in after the checks above, and
        // O_NONBLOCK keeps a FIFO from blocking before it is refused below.
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|e| describe_io(&e))?;
        let meta = file.metadata().map_err(|e| describe_io(&e))?;
        if !meta.is_file() {
            return Err("not a regular file".into());
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!("file too large ({} bytes, limit {MAX_FILE_BYTES})", meta.len()));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).map_err(|e| describe_io(&e))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("file too large (more than {MAX_FILE_BYTES} bytes)"));
        }
        String::from_utf8(bytes).map_err(|e| format!("not valid UTF-8 (invalid byte at offset {})", e.utf8_error().valid_up_to()))
    }
}

fn describe_io(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "file is missing".into(),
        _ if error.raw_os_error() == Some(libc::ELOOP) => "symbolic link is not followed".into(),
        _ => format!("could not read: {error}"),
    }
}

fn text_entry(result: Result<String, String>) -> Value {
    match result {
        Ok(text) => json!({"text": text, "error": null}),
        Err(error) => json!({"text": null, "error": error}),
    }
}

/// One `## S<n>: <title>` section of `scenarios.md`.
struct Scenario {
    id: String,
    title: String,
    given: Option<String>,
    when: Option<String>,
    then: Option<String>,
}

/// Scenario sections outside fenced code, read like
/// [`crate::features::scenario_ids`]: the heading's first token with a
/// trailing `:` stripped, valid IDs only, first occurrence wins. Any other
/// `## ` heading ends the current section.
fn parse_scenarios(content: &str) -> Vec<Scenario> {
    let mut scenarios: Vec<Scenario> = Vec::new();
    let mut current: Option<usize> = None;
    for line in lines_outside_fences(content) {
        if let Some(rest) = line.strip_prefix("## ") {
            current = None;
            let Some(token) = rest.split_whitespace().next() else { continue };
            let id = token.strip_suffix(':').unwrap_or(token);
            if is_valid_scenario_id(id) && !scenarios.iter().any(|s| s.id == id) {
                let title = rest.trim_start()[token.len()..].trim().to_string();
                scenarios.push(Scenario { id: id.to_string(), title, given: None, when: None, then: None });
                current = Some(scenarios.len() - 1);
            }
            continue;
        }
        let Some(scenario) = current.map(|index| &mut scenarios[index]) else { continue };
        let item = line.trim().strip_prefix("- ").map(str::trim_start);
        let Some(item) = item else { continue };
        for (label, field) in [
            ("Given:", &mut scenario.given),
            ("When:", &mut scenario.when),
            ("Then:", &mut scenario.then),
        ] {
            if let Some(value) = item.strip_prefix(label)
                && field.is_none()
            {
                *field = Some(value.trim().to_string());
            }
        }
    }
    scenarios
}

/// The section of scenario `id` in `scenarios.md`: its `## <id>` heading
/// through the line before the next `## ` heading, with headings inside fenced
/// code ignored (fenced lines within the section are part of it) and trailing
/// whitespace trimmed. The first section wins, as in [`parse_scenarios`].
/// None when the scenario has no section.
pub(crate) fn scenario_section(content: &str, id: &str) -> Option<String> {
    let mut in_fence = false;
    let mut section: Option<Vec<&str>> = None;
    for line in content.lines() {
        let fence = line.starts_with("```") || line.starts_with("~~~");
        if !in_fence && !fence && let Some(rest) = line.strip_prefix("## ") {
            if section.is_some() {
                break;
            }
            let token = rest.split_whitespace().next().unwrap_or("");
            if token.strip_suffix(':').unwrap_or(token) == id {
                section = Some(Vec::new());
            }
        }
        if fence {
            in_fence = !in_fence;
        }
        if let Some(lines) = section.as_mut() {
            lines.push(line);
        }
    }
    section.map(|lines| lines.join("\n").trim_end().to_string())
}

/// Digest of scenario `id`'s section (see [`scenario_section`]), or None when
/// the scenario has no section. Recording a result and judging whether it is
/// out of date both use this, so they can never disagree (D18).
pub(crate) fn scenario_hash(content: &str, id: &str) -> Result<Option<String>, String> {
    scenario_section(content, id).map(|section| crate::util::digest(section.as_bytes())).transpose()
}

/// The text of `scenarios.md` in the feature folder `dir`, read under the same
/// no-symlink, containment and size rules as the content endpoint.
pub(crate) fn read_scenarios(dir: &Path) -> Result<String, String> {
    Root::open(dir)?.read_text(Path::new("scenarios.md"))
}

/// A scenario's latest recorded result as served (S39): null when none was
/// recorded, else the entry's fields plus `out_of_date`, true when the
/// scenario's current section hashes differently from the recorded hash.
fn result_view(state: &Value, id: &str, current_hash: Option<&str>) -> Value {
    let latest = crate::feature_state::latest_scenario_result(state, id);
    if latest.is_null() {
        return Value::Null;
    }
    json!({
        "status": latest["status"], "evidence": latest["evidence"], "role": latest["role"],
        "plan_id": latest["plan_id"], "milestone": latest["milestone"], "unix": latest["unix"],
        "out_of_date": current_hash.is_none() || latest["scenario_hash"].as_str() != current_hash,
    })
}

fn design_kind(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("pen") => "pen",
        Some("png") => "png",
        Some("mmd") => "mermaid",
        _ => "other",
    }
}

/// Walks `design/` in name order without following any link.
struct DesignWalk<'a> {
    root: &'a Root,
    entries: Vec<Value>,
}

impl DesignWalk<'_> {
    fn full(&mut self) -> bool {
        if self.entries.len() >= MAX_DESIGN_ENTRIES {
            if self.entries.len() == MAX_DESIGN_ENTRIES {
                self.entries.push(json!({"path": "design", "kind": "other",
                    "error": format!("more than {MAX_DESIGN_ENTRIES} design entries; the rest are not listed")}));
            }
            return true;
        }
        false
    }

    fn walk(&mut self, rel: &Path, depth: usize) {
        let dir = match self.root.resolve(rel) {
            Ok(dir) => dir,
            Err(error) => {
                self.entries.push(json!({"path": display(rel), "kind": "other", "error": error}));
                return;
            },
        };
        let mut names: Vec<_> = match fs::read_dir(&dir) {
            Ok(entries) => entries.filter_map(|e| e.ok()).map(|e| e.file_name()).collect(),
            Err(e) => {
                self.entries.push(json!({"path": display(rel), "kind": "other", "error": describe_io(&e)}));
                return;
            },
        };
        names.sort();
        for name in names {
            if self.full() {
                return;
            }
            let child = rel.join(&name);
            let path = display(&child);
            let Some(name) = name.to_str() else {
                self.entries.push(json!({"path": path, "kind": "other", "error": "file name is not UTF-8"}));
                continue;
            };
            let kind = design_kind(name);
            let meta = match fs::symlink_metadata(dir.join(name)) {
                Ok(meta) => meta,
                Err(e) => {
                    self.entries.push(json!({"path": path, "kind": kind, "error": describe_io(&e)}));
                    continue;
                },
            };
            if meta.is_symlink() {
                self.entries.push(json!({"path": path, "kind": kind, "error": "symbolic link is not followed"}));
            } else if meta.is_dir() {
                if depth >= MAX_DESIGN_DEPTH {
                    self.entries.push(json!({"path": path, "kind": "other", "error": "directory nested too deeply"}));
                } else {
                    self.walk(&child, depth + 1);
                }
            } else if !meta.is_file() {
                self.entries.push(json!({"path": path, "kind": kind, "error": "not a regular file"}));
            } else {
                let entry = self.file_entry(&child, &path, kind);
                self.entries.push(entry);
            }
        }
    }

    fn file_entry(&self, rel: &Path, path: &str, kind: &str) -> Value {
        match kind {
            "pen" => {
                // The engine's export naming rule (`<stem>.png`, or
                // `<stem>.<frame>.png` per top-level frame) decides the PNGs.
                let pngs = crate::pen::exported_pngs(&self.root.canonical, path);
                let stem_png = format!("{}.png", path.trim_end_matches(".pen"));
                let png = pngs.iter().find(|p| **p == stem_png).or(pngs.first()).cloned();
                json!({"path": path, "kind": kind, "png": png, "pngs": pngs, "error": null})
            },
            "png" => match self.root.resolve(rel) {
                Ok(absolute) => json!({"path": path, "kind": kind, "absolute_path": absolute, "error": null}),
                Err(error) => json!({"path": path, "kind": kind, "absolute_path": null, "error": error}),
            },
            "mermaid" => {
                let mut entry = text_entry(self.root.read_text(rel));
                entry["path"] = json!(path);
                entry["kind"] = json!(kind);
                entry
            },
            _ => json!({"path": path, "kind": kind, "error": null}),
        }
    }
}

fn display(rel: &Path) -> String {
    rel.to_string_lossy().into_owned()
}

/// The full read-only content of a discovered `feature`, or an error when the
/// feature folder itself cannot be served (for example because it is a link).
/// `state` is the feature's runtime state, which supplies each scenario's
/// latest recorded result.
pub(crate) fn read(feature: &Feature, state: &Value) -> Result<Value, String> {
    let root = Root::open(&feature.path)?;

    let mut files = Map::new();
    let mut scenarios_text = None;
    for name in FILES {
        let result = root.read_text(Path::new(name));
        if name == "scenarios.md" {
            scenarios_text = result.as_ref().ok().cloned();
        }
        files.insert(name.to_string(), text_entry(result));
    }

    let milestone_of = |id: &str| {
        feature.milestones.iter().find(|m| m.covers.iter().any(|c| c == id)).map(|m| m.id.clone())
    };
    let scenarios: Vec<Value> = scenarios_text
        .as_deref()
        .map(parse_scenarios)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            // A hash that cannot be computed never matches, so the result
            // shows as possibly out of date rather than failing the response.
            let current_hash = scenarios_text.as_deref().and_then(|text| scenario_hash(text, &s.id).ok().flatten());
            let result = result_view(state, &s.id, current_hash.as_deref());
            json!({"id": s.id, "title": s.title, "given": s.given, "when": s.when, "then": s.then,
                "milestone": milestone_of(&s.id), "result": result})
        })
        .collect();

    let milestones: Vec<Value> = feature
        .milestones
        .iter()
        .map(|m| {
            json!({"id": m.id, "title": m.title, "status": m.status(), "covers": m.covers,
                "business_tests": m.business_tests})
        })
        .collect();

    let mut design = DesignWalk { root: &root, entries: Vec::new() };
    match fs::symlink_metadata(root.canonical.join("design")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        _ => design.walk(Path::new("design"), 1),
    }

    Ok(json!({
        "slug": feature.slug,
        "files": files,
        "scenarios": scenarios,
        "milestones": milestones,
        "design": design.entries,
    }))
}

#[cfg(test)]
mod tests {
    use super::{MAX_FILE_BYTES, Root, parse_scenarios, scenario_hash, scenario_section};
    use std::fs;
    use std::path::Path;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-feature-content-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("feature")).unwrap();
        dir
    }

    #[test]
    fn a_linked_feature_folder_is_refused() {
        let dir = temp_dir("linked-root");
        std::os::unix::fs::symlink(dir.join("feature"), dir.join("link")).unwrap();
        assert!(Root::open(&dir.join("link")).is_err());
        assert!(Root::open(&dir.join("feature")).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parent_absolute_and_oversized_paths_are_refused() {
        let dir = temp_dir("guards");
        fs::write(dir.join("outside.md"), "outside").unwrap();
        fs::write(dir.join("feature/big.md"), vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
        fs::write(dir.join("feature/ok.md"), "inside").unwrap();
        let root = Root::open(&dir.join("feature")).unwrap();
        assert!(root.read_text(Path::new("../outside.md")).is_err());
        assert!(root.read_text(&dir.join("outside.md")).is_err());
        assert!(root.read_text(Path::new("big.md")).unwrap_err().contains("too large"));
        assert_eq!(root.read_text(Path::new("ok.md")).unwrap(), "inside");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_scenario_section_runs_to_the_next_heading_outside_fences() {
        let text = "# Scenarios\n\n## S1: One\n\n- Given: a\n```\n## S9: Fenced\n```\n\n## S2: Two\n- Then: b\n\n## Notes\nx\n";
        assert_eq!(scenario_section(text, "S1").unwrap(), "## S1: One\n\n- Given: a\n```\n## S9: Fenced\n```");
        assert_eq!(scenario_section(text, "S2").unwrap(), "## S2: Two\n- Then: b");
        assert_eq!(scenario_section(text, "S9"), None, "a fenced heading is no section");
        assert_eq!(scenario_section(text, "S3"), None);

        let edited = text.replace("- Then: b", "- Then: c");
        assert_eq!(scenario_hash(text, "S1").unwrap(), scenario_hash(&edited, "S1").unwrap());
        assert_ne!(scenario_hash(text, "S2").unwrap(), scenario_hash(&edited, "S2").unwrap());
        assert_eq!(scenario_hash(text, "S3").unwrap(), None);
    }

    #[test]
    fn scenarios_skip_fenced_headings_repeats_and_other_sections() {
        let text = "# Scenarios\n\n## S1: One\n\n- Given: a\n- When: b\n- Then: c\n\n```\n## S9: Fenced\n- Given: no\n```\n\
            ## Notes\n\n- Given: ignored\n\n## S1: Again\n\n## S2 Two words\n- Then: done\n";
        let parsed = parse_scenarios(text);
        let ids: Vec<&str> = parsed.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["S1", "S2"]);
        assert_eq!(parsed[0].title, "One");
        assert_eq!(
            (parsed[0].given.as_deref(), parsed[0].when.as_deref(), parsed[0].then.as_deref()),
            (Some("a"), Some("b"), Some("c"))
        );
        assert_eq!(parsed[1].title, "Two words");
        assert_eq!((parsed[1].given.as_deref(), parsed[1].then.as_deref()), (None, Some("done")));
    }
}
