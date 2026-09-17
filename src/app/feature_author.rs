//! Read-only co-authoring of one feature spec folder (M2, S11/S12, D8).
//!
//! The agent never writes: it returns a reply plus proposed file contents as
//! JSON, and the engine validates every path against `docs/features/<slug>/`
//! and publishes the files itself. A response is an all-or-nothing write set:
//! validation covers the whole set before anything is mutated, and every
//! destination is checked again immediately before the write, so a symlink or
//! directory that appears in between cannot let a write escape the folder
//! (M2-ARCH-002). Every rejection travels the shared correction budget in
//! `crate::response`, so the agent gets the reason and one bounded chance to
//! fix it.
use super::{Ctx, WorkerGuard};
use crate::feature_state;
use crate::prompts::FEATURE_CHAT_PROMPT;
use crate::util::{fill_template, json_payload, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

/// Bounds on one proposed write set.
pub(crate) const MAX_FILES: usize = 32;
pub(crate) const MAX_CONTENT_BYTES: usize = 256 * 1024;
/// Bounds on the folder text quoted into the prompt.
const MAX_PROMPT_FILE_BYTES: usize = 64 * 1024;
const MAX_PROMPT_TOTAL_BYTES: usize = 256 * 1024;
/// How much of the transcript the agent sees, and how much is retained.
const PROMPT_TRANSCRIPT_ENTRIES: usize = 20;
const MAX_CHAT_ENTRIES: usize = 200;

/// A validated co-authoring response: the reply plus the proposed write set as
/// repository-relative paths with the complete new content of each file.
#[derive(Debug)]
pub(crate) struct Proposal {
    pub(crate) reply: String,
    pub(crate) files: Vec<(String, String)>,
}

// ---------------------------------------------------------------- validation

/// The absolute destination of one proposed path, or the reason it is refused.
///
/// Refuses anything that is not a relative path strictly inside
/// `docs/features/<slug>/`, and any symlink or non-directory found on the way.
/// Components are compared one by one, so a prefix such as
/// `docs/features/<slug>-x/` never passes, and every existing component is
/// inspected with `symlink_metadata`, so no link is ever followed.
pub(crate) fn check_path(project: &Path, slug: &str, path: &str) -> Result<PathBuf, String> {
    let prefix = ["docs", "features", slug];
    if path.is_empty() {
        return Err("a proposed file needs a non-empty path".into());
    }
    if path.starts_with('/') {
        return Err(format!("{path:?} is not a relative path inside docs/features/{slug}/"));
    }
    if path.contains('\0') {
        return Err(format!("{path:?} contains a NUL byte"));
    }
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts {
        match *part {
            "" => return Err(format!("{path:?} has an empty path component")),
            "." | ".." => return Err(format!("{path:?} contains a {part:?} component")),
            _ => {},
        }
    }
    if parts.len() <= prefix.len() || parts[..prefix.len()] != prefix[..] {
        return Err(format!("{path:?} is not a file inside docs/features/{slug}/"));
    }
    // `docs`, `docs/features` and the feature folder must already exist as
    // real directories; anything below them may still be missing, because the
    // engine creates the missing part of the write set itself.
    let mut current = project.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let last = index + 1 == parts.len();
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if index < prefix.len() {
                    return Err(format!("{path:?}: {} does not exist: {e}", current.display()));
                }
                break;
            },
            Err(e) => return Err(format!("{path:?}: could not check {}: {e}", current.display())),
        };
        if meta.is_symlink() {
            return Err(format!("{path:?} passes through the symlink {}", current.display()));
        }
        if last {
            if !meta.is_file() {
                return Err(format!("{path:?} exists and is not a regular file"));
            }
        } else if !meta.is_dir() {
            return Err(format!("{path:?}: {} is not a directory", current.display()));
        }
    }
    Ok(project.join(path))
}

/// Pure validation of one complete co-authoring response, called inside the
/// shared correction budget: every rejection here is returned to the agent.
pub(crate) fn validate_response(project: &Path, slug: &str, text: &str) -> Result<Proposal, String> {
    let output: Value = crate::response::parse_json(json_payload(text))
        .map_err(|e| format!("invalid co-authoring JSON: {e}"))?;
    crate::response::object_fields(&output, &["reply", "files"])?;
    let reply = output["reply"]
        .as_str()
        .filter(|reply| !reply.trim().is_empty())
        .ok_or("agent did not produce a non-empty reply string")?
        .to_string();
    let files = match &output["files"] {
        Value::Null => Vec::new(),
        Value::Array(files) => files.clone(),
        _ => return Err("`files` must be an array of {path, content} objects".into()),
    };
    if files.len() > MAX_FILES {
        return Err(format!("too many files: {} (at most {MAX_FILES})", files.len()));
    }
    let mut proposed: Vec<(String, String)> = Vec::new();
    for file in &files {
        crate::response::object_fields(file, &["path", "content"])?;
        let path = file["path"]
            .as_str()
            .ok_or("every proposed file needs a `path` string")?;
        let content = file["content"]
            .as_str()
            .ok_or_else(|| format!("proposed file {path:?} needs a `content` string"))?;
        if content.len() > MAX_CONTENT_BYTES {
            return Err(format!(
                "proposed file {path:?} is too large: {} bytes (at most {MAX_CONTENT_BYTES})",
                content.len()
            ));
        }
        check_path(project, slug, path)?;
        if proposed.iter().any(|(existing, _)| existing == path) {
            return Err(format!("duplicate proposed file path {path:?}"));
        }
        proposed.push((path.to_string(), content.to_string()));
    }
    Ok(Proposal { reply, files: proposed })
}

// ---------------------------------------------------------------- publication

/// Publishes a validated write set, or nothing at all.
///
/// Every destination is re-checked before the first mutation, each file is
/// then staged as a temporary file in its own destination directory, and only
/// a fully staged set is renamed into place. A failure at any point leaves no
/// published file and no temporary file behind.
pub(crate) fn publish(project: &Path, slug: &str, files: &[(String, String)]) -> Result<(), String> {
    let mut targets = Vec::new();
    for (path, content) in files {
        targets.push((path.as_str(), check_path(project, slug, path)?, content));
    }
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| -> Result<(), String> {
        for (path, target, content) in &targets {
            ensure_dirs(project, path)?;
            let dir = target.parent().ok_or_else(|| format!("{path:?} has no parent directory"))?;
            let tmp = dir.join(format!(".{}.tmp", crate::durable_json::identity()));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| format!("could not create {}: {e}", tmp.display()))?;
            file.write_all(content.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
            staged.push((tmp, target.clone()));
        }
        for (tmp, target) in &staged {
            fs::rename(tmp, target)
                .map_err(|e| format!("could not publish {}: {e}", target.display()))?;
            crate::durable_json::sync_dir(target.parent().unwrap())?;
        }
        Ok(())
    })();
    // Renamed files are gone from their temporary name already; this only
    // cleans up after a failure, so no temp file is ever left under docs/.
    for (tmp, _) in &staged {
        let _ = fs::remove_file(tmp);
    }
    result
}

/// Creates the missing directories of one destination, one component at a
/// time, re-checking each one so an existing entry is always a real directory.
fn ensure_dirs(project: &Path, path: &str) -> Result<(), String> {
    let parts: Vec<&str> = path.split('/').collect();
    let mut current = project.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => {},
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {},
            Err(e) => return Err(format!("could not create {}: {e}", current.display())),
        }
        let meta = fs::symlink_metadata(&current)
            .map_err(|e| format!("could not check {}: {e}", current.display()))?;
        if meta.is_symlink() || !meta.is_dir() {
            return Err(format!("{} is not a directory", current.display()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- prompt

/// The folder's current text, bounded per file and in total, with every
/// truncation or omission noted so the agent never treats a cut file as
/// complete. Symlinks are listed but never followed.
pub(crate) fn folder_text(dir: &Path, folder: &str) -> Result<String, String> {
    let mut entries = Vec::new();
    collect(dir, Path::new(""), &mut entries)?;
    entries.sort();
    let mut out = String::new();
    let mut total = 0usize;
    for (rel, kind) in entries {
        let rel = rel.display().to_string();
        if kind == "symlink" {
            out.push_str(&format!("--- {folder}/{rel} (symlink, not spec content) ---\n"));
            continue;
        }
        if total >= MAX_PROMPT_TOTAL_BYTES {
            out.push_str(&format!("--- {folder}/{rel} (omitted, prompt size limit reached) ---\n"));
            continue;
        }
        let path = dir.join(&rel);
        let Ok(text) = fs::read_to_string(&path) else {
            out.push_str(&format!("--- {folder}/{rel} (not UTF-8 text, omitted) ---\n"));
            continue;
        };
        let budget = MAX_PROMPT_FILE_BYTES.min(MAX_PROMPT_TOTAL_BYTES - total);
        let (text, note) = if text.len() > budget {
            let mut end = budget;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            (&text[..end], " (TRUNCATED, not the complete file)")
        } else {
            (text.as_str(), "")
        };
        total += text.len();
        out.push_str(&format!("--- {folder}/{rel}{note} ---\n{text}\n"));
    }
    if out.is_empty() {
        out.push_str("(the folder holds no files)\n");
    }
    Ok(out)
}

fn collect(root: &Path, rel: &Path, out: &mut Vec<(PathBuf, &'static str)>) -> Result<(), String> {
    let dir = root.join(rel);
    for entry in fs::read_dir(&dir).map_err(|e| format!("could not read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        let child = rel.join(entry.file_name());
        let full = root.join(&child);
        let meta = fs::symlink_metadata(&full)
            .map_err(|e| format!("could not read {}: {e}", full.display()))?;
        if meta.is_symlink() {
            out.push((child, "symlink"));
        } else if meta.is_file() {
            out.push((child, "file"));
        } else if meta.is_dir() {
            collect(root, &child, out)?;
        }
    }
    Ok(())
}

/// The tail of the transcript, oldest of the shown entries first.
fn transcript_text(chat: &Value) -> String {
    let entries: Vec<&Value> = chat.as_array().into_iter().flatten().collect();
    let start = entries.len().saturating_sub(PROMPT_TRANSCRIPT_ENTRIES);
    serde_json::to_string_pretty(&entries[start..]).unwrap_or_else(|_| "[]".to_string())
}

// ---------------------------------------------------------------- worker

impl Ctx {
    /// Answers one co-authoring message. Only a fully validated and published
    /// write set extends the transcript; a failure writes no file, leaves the
    /// chat history untouched and reports the reason as the activity error.
    pub(crate) fn feature_chat_worker(&self, slug: &str, message: &str, request_id: i64) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "co-authoring a feature spec");
        let result = self.run_feature_chat(slug, message);
        let (activity, kind, text) = match &result {
            Ok(paths) => (
                json!({"kind":"chat","slug":slug,"request_id":request_id,"status":"ready",
                    "unix":unix_timestamp()}),
                "features",
                format!("co-authoring reply for {slug} ready ({} file(s) written)", paths.len()),
            ),
            Err(error) => (
                json!({"kind":"chat","slug":slug,"request_id":request_id,"status":"failed",
                    "error":error,"unix":unix_timestamp()}),
                "error",
                format!("co-authoring {slug} failed: {error}"),
            ),
        };
        let current = {
            let mut state = self.session.state.lock().unwrap();
            let current = state.feature_serial == request_id;
            if current {
                state.feature_activity = activity;
            }
            current
        };
        if current {
            self.log_event(kind, &text);
        }
    }

    /// The published relative paths on success. Nothing here commits or stages
    /// anything: approval owns the feature folder's history (S15).
    fn run_feature_chat(&self, slug: &str, message: &str) -> Result<Vec<String>, String> {
        self.ensure_forge_dir();
        match fs::remove_file(self.forge_path("answer.json")) {
            Ok(()) => {},
            Err(e) if e.kind() == ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not remove previous answer: {e}")),
        }
        let project = PathBuf::from(self.project());
        let folder = format!("docs/features/{slug}");
        let transcript = {
            let _guard = self.session.feature_lock.lock().unwrap();
            feature_state::load(self, slug)?["chat"].clone()
        };
        let prompt = fill_template(FEATURE_CHAT_PROMPT, &[
            ("{slug}", slug),
            ("{folder}", &folder),
            ("{folder_text}", &folder_text(&feature_state::feature_dir(self, slug), &folder)?),
            ("{history}", &transcript_text(&transcript)),
            ("{message}", message),
            ("{max_files}", &MAX_FILES.to_string()),
            ("{max_kib}", &(MAX_CONTENT_BYTES / 1024).to_string()),
        ]);
        let reply = self.readonly_response("chat", &prompt, Some("answer.json"), |text| {
            validate_response(&project, slug, text)
        })?;
        let Proposal { reply, files } = reply.value;
        publish(&project, slug, &files)?;
        let paths: Vec<String> = files.iter().map(|(path, _)| path.clone()).collect();
        let recorded = paths.clone();
        let unix = unix_timestamp();
        feature_state::update(self, slug, |state| {
            let chat = state["chat"]
                .as_array_mut()
                .ok_or("feature state chat is not an array")?;
            chat.push(json!({"role":"user","text":message,"unix":unix}));
            chat.push(json!({"role":"assistant","text":reply,"files":recorded,"unix":unix}));
            if chat.len() > MAX_CHAT_ENTRIES {
                chat.drain(..chat.len() - MAX_CHAT_ENTRIES);
            }
            Ok(())
        })?;
        Ok(paths)
    }
}
