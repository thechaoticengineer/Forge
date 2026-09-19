//! Runtime state, content hashing and creation for feature specs (M2).
//!
//! This module is the single authority for the feature-spec spec phase's
//! durable state: schema defaults, slug/title validation, the folder content
//! hash, serialized load-modify-publish, derived status and feature creation.
//! See docs/features/feature-specs/decisions.md D6 (runtime state lives in
//! `.forge/`, never under `docs/features/`) and D10 (approvals are bound to a
//! commit and a content hash; any later change returns the feature to draft).
//!
//! `crate::features` stays strictly read-only discovery; the only write this
//! module performs under `docs/features/` is creating a feature from the
//! template.

use crate::app::Ctx;
use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub(crate) const MAX_SLUG_BYTES: usize = 64;
pub(crate) const MAX_TITLE_CHARS: usize = 200;

/// A refused request, carrying the HTTP status the API should answer with.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) status: u32,
    pub(crate) message: String,
}

/// Everything a read of one feature needs: its persisted state plus the values
/// derived from the folder's current content hash.
pub(crate) struct Snapshot {
    pub(crate) state: Value,
    pub(crate) content_hash: String,
    pub(crate) spec_status: &'static str,
    pub(crate) latest_review: Value,
    pub(crate) review_current: bool,
}

// ---------------------------------------------------------------- validation

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 bytes.
fn slug_shape(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_BYTES
        && slug
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

pub(crate) fn validate_slug(slug: &str) -> Result<(), String> {
    if slug_shape(slug) {
        Ok(())
    } else {
        Err(format!(
            "invalid slug: {slug:?} (expected lowercase words joined by '-', at most {MAX_SLUG_BYTES} bytes)"
        ))
    }
}

/// The trimmed title, or the reason it is unusable as a README heading.
pub(crate) fn validate_title(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("title required".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("title too long (at most {MAX_TITLE_CHARS} characters)"));
    }
    if title.contains('\n') || title.contains('\r') {
        return Err("title must be a single line".into());
    }
    Ok(title.to_string())
}

// ---------------------------------------------------------------- content hash

pub(crate) fn features_root(ctx: &Ctx) -> PathBuf {
    PathBuf::from(ctx.project()).join("docs/features")
}

pub(crate) fn feature_dir(ctx: &Ctx, slug: &str) -> PathBuf {
    features_root(ctx).join(slug)
}

/// Hash of everything `dir` holds, as `<relpath>\0<kind>\0<hash>\n` records
/// sorted by relative path. Symlinks are hashed by their target text and never
/// followed, so a link can neither hide content nor pull outside content in.
/// Directories contribute nothing by themselves.
pub(crate) fn content_hash(dir: &Path) -> Result<String, String> {
    let mut records = Vec::new();
    collect(dir, Path::new(""), &mut records)?;
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buffer = Vec::new();
    for (rel, kind, hash) in records {
        buffer.extend_from_slice(rel.as_os_str().as_encoded_bytes());
        buffer.push(0);
        buffer.extend_from_slice(kind.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(hash.as_bytes());
        buffer.push(b'\n');
    }
    crate::util::digest(&buffer)
}

fn collect(
    root: &Path,
    rel: &Path,
    out: &mut Vec<(PathBuf, &'static str, String)>,
) -> Result<(), String> {
    let dir = root.join(rel);
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("could not read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        names.push(entry.file_name());
    }
    names.sort();
    for name in names {
        let child = rel.join(&name);
        let full = root.join(&child);
        let meta = fs::symlink_metadata(&full)
            .map_err(|e| format!("could not read {}: {e}", full.display()))?;
        if meta.is_symlink() {
            let target =
                fs::read_link(&full).map_err(|e| format!("could not read {}: {e}", full.display()))?;
            let hash = crate::util::digest(target.as_os_str().as_encoded_bytes())?;
            out.push((child, "symlink", hash));
        } else if meta.is_file() {
            let bytes =
                fs::read(&full).map_err(|e| format!("could not read {}: {e}", full.display()))?;
            out.push((child, "file", crate::util::digest(&bytes)?));
        } else if meta.is_dir() {
            collect(root, &child, out)?;
        }
        // Anything else (device, socket, fifo) is not spec content.
    }
    Ok(())
}

// ---------------------------------------------------------------- persistence

/// `plans` holds the milestone plan links added by M3; files written before it
/// have none, and `load` defaults it.
pub(crate) fn default_state(slug: &str) -> Value {
    json!({
        "version": 1,
        "slug": slug,
        "reviews": [],
        "approvals": [],
        "chat": [],
        "architect_session": Value::Null,
        "plans": [],
    })
}

pub(crate) fn state_path(ctx: &Ctx, slug: &str) -> PathBuf {
    ctx.forge_path(&format!("features/{slug}.json"))
}

/// The persisted state, or the default for a feature that has none yet.
/// A file written before M3 has no `plans`; it loads with `plans: []`, and
/// the file itself is never rewritten by a read.
/// An unreadable or structurally wrong file is an error: state is never
/// silently reset, because that would drop approval history.
pub(crate) fn load(ctx: &Ctx, slug: &str) -> Result<Value, String> {
    validate_slug(slug)?;
    let path = state_path(ctx, slug);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(default_state(slug)),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    let mut state: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("invalid feature state {}: {e}", path.display()))?;
    if !state.is_object() {
        return Err(format!("invalid feature state {}: not an object", path.display()));
    }
    if state["version"] != json!(1) {
        return Err(format!(
            "invalid feature state {}: unsupported version {}",
            path.display(),
            state["version"]
        ));
    }
    if state["slug"] != json!(slug) {
        return Err(format!(
            "invalid feature state {}: recorded slug {} does not match {slug}",
            path.display(),
            state["slug"]
        ));
    }
    for key in ["reviews", "approvals", "chat"] {
        if !state[key].is_array() {
            return Err(format!(
                "invalid feature state {}: {key} must be an array",
                path.display()
            ));
        }
    }
    match state.get("plans") {
        None => state["plans"] = json!([]),
        Some(plans) if !plans.is_array() => {
            return Err(format!(
                "invalid feature state {}: plans must be an array",
                path.display()
            ));
        },
        Some(_) => {},
    }
    Ok(state)
}

/// Durable publication (temp file, fsync, rename): an interrupted write never
/// leaves an invalid state file, and stray temp files are ignored on read.
///
/// `fail_save` is the failure seam the transaction tests use: it takes the
/// checked publisher's post-rename failure path, which restores the previous
/// state file before reporting the error, so a caller that has to undo its own
/// work sees exactly what a real publication failure gives it. Production
/// passes `false`.
fn save(ctx: &Ctx, slug: &str, state: &Value, fail_save: bool) -> Result<(), String> {
    crate::durable_json::publish_pretty_checked(&state_path(ctx, slug), state, fail_save)
        .map_err(|e| format!("could not save feature state for {slug}: {e}"))
}

/// Serialized load-modify-publish. Concurrent chat, review and approval
/// updates all go through this, so no record is ever lost to a lost update.
pub(crate) fn update<T>(
    ctx: &Ctx,
    slug: &str,
    change: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<T, String> {
    update_at(ctx, slug, false, change)
}

/// `update` with the publication failure seam, so a caller whose own work has
/// to be undone when the record cannot be persisted can be tested against a
/// genuinely failed save rather than a simulated one. Production calls
/// `update`; `fail_save` is documented on [`save`].
pub(crate) fn update_at<T>(
    ctx: &Ctx,
    slug: &str,
    fail_save: bool,
    change: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = ctx.session.feature_lock.lock().unwrap();
    update_locked_at(ctx, slug, fail_save, change)
}

/// Load-modify-publish for a caller that already holds `feature_lock`, so a
/// decision and the record it produces can share one critical section. An
/// approval uses this to read the folder, derive its record and publish it
/// without ever releasing the lock in between (D10).
///
/// The caller must hold `feature_lock`; `update` is the entry point for
/// everyone else.
pub(crate) fn update_locked<T>(
    ctx: &Ctx,
    slug: &str,
    change: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<T, String> {
    update_locked_at(ctx, slug, false, change)
}

/// `update_locked` with the publication failure seam; see [`update_at`].
fn update_locked_at<T>(
    ctx: &Ctx,
    slug: &str,
    fail_save: bool,
    change: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<T, String> {
    let mut state = load(ctx, slug)?;
    let result = change(&mut state)?;
    save(ctx, slug, &state, fail_save)?;
    Ok(result)
}

/// The activity of the most recent feature chat or review request, or null.
/// Served by `/api/features` and `/api/features/state`, deliberately never by
/// `/api/state`, whose shape the existing flow depends on (M1 S9).
pub(crate) fn activity(ctx: &Ctx) -> Value {
    ctx.session.state.lock().unwrap().feature_activity.clone()
}

// ---------------------------------------------------------------- derived status

/// Derived on every read from the persisted approvals and the folder's current
/// content hash (D10); status is never stored as authoritative state.
pub(crate) fn spec_status(state: &Value, current_hash: &str) -> &'static str {
    let latest = |kind: &str| {
        state["approvals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["kind"] == json!(kind))
            .last()
    };
    let is_current =
        |approval: Option<&Value>| approval.is_some_and(|a| a["content_hash"] == json!(current_hash));
    if !is_current(latest("spec")) {
        return "draft";
    }
    if is_current(latest("scenarios")) {
        "scenarios approved"
    } else {
        "spec approved"
    }
}

pub(crate) fn latest_review(state: &Value) -> Value {
    state["reviews"]
        .as_array()
        .and_then(|reviews| reviews.last())
        .cloned()
        .unwrap_or(Value::Null)
}

/// State plus everything derived from the current folder content, read under
/// the feature-state lock so the hash matches the state it is compared with.
pub(crate) fn snapshot(ctx: &Ctx, slug: &str) -> Result<Snapshot, String> {
    let _guard = ctx.session.feature_lock.lock().unwrap();
    let state = load(ctx, slug)?;
    let content_hash = content_hash(&feature_dir(ctx, slug))?;
    let latest_review = latest_review(&state);
    Ok(Snapshot {
        spec_status: spec_status(&state, &content_hash),
        review_current: latest_review["content_hash"] == json!(&content_hash),
        latest_review,
        content_hash,
        state,
    })
}

/// The spec-phase fields `/api/features` adds to every listed feature.
/// Defined here so the listing and `/api/features/state` share one shape.
///
/// A feature whose state file or folder cannot be read keeps its place in the
/// list, but is reported as `spec_status: "error"` with the reason in
/// `state_error` and no hash, review or currency claim; it is never silently
/// downgraded to `"draft"`, which would hide a corrupt file behind a status
/// that looks like ordinary unapproved work. `/api/features/state` answers
/// 500 with the same reason for that feature.
///
/// The second value is the state the fields were derived from, so the listing
/// also reports per-milestone plan links from the same read; it is null when
/// the state could not be read.
pub(crate) fn listing(ctx: &Ctx, slug: &str) -> (Value, Value) {
    match snapshot(ctx, slug) {
        Ok(snapshot) => (
            json!({
                "spec_status": snapshot.spec_status,
                "content_hash": snapshot.content_hash,
                "latest_review": snapshot.latest_review,
                "review_current": snapshot.review_current,
            }),
            snapshot.state,
        ),
        Err(error) => (
            json!({
                "spec_status": "error",
                "content_hash": Value::Null,
                "latest_review": Value::Null,
                "review_current": false,
                "state_error": error,
            }),
            Value::Null,
        ),
    }
}

// ---------------------------------------------------------------- milestone plan links

/// The scenario IDs of the latest `scenarios` approval, if it approved exactly
/// the current content; None otherwise.
pub(crate) fn approved_scenario_ids(state: &Value, current_hash: &str) -> Option<Vec<String>> {
    let approval = state["approvals"]
        .as_array()?
        .iter()
        .rfind(|a| a["kind"] == json!("scenarios"))?;
    if approval["content_hash"] != json!(current_hash) {
        return None;
    }
    Some(
        approval["scenario_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

/// The most recent plan link recorded for `milestone`, or null.
pub(crate) fn latest_plan_link(state: &Value, milestone: &str) -> Value {
    state["plans"]
        .as_array()
        .and_then(|plans| plans.iter().rfind(|link| link["milestone"] == json!(milestone)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The compact view of a milestone's latest plan link that `/api/features`
/// reports, or null when the milestone was never planned.
pub(crate) fn plan_link_view(state: &Value, milestone: &str) -> Value {
    let link = latest_plan_link(state, milestone);
    if link.is_null() {
        return Value::Null;
    }
    json!({
        "status": link["status"],
        "plan_id": link["plan_id"],
        "started_unix": link["started_unix"],
        "completed_unix": link["completed_unix"],
        "commit_range": link["commit_range"],
    })
}

fn plans_mut(state: &mut Value) -> &mut Vec<Value> {
    if !state["plans"].is_array() {
        state["plans"] = json!([]);
    }
    state["plans"].as_array_mut().unwrap()
}

/// Records that planning of `feature.milestone` started with `goal`.
/// `feature` is the plan mode's feature object ({slug, milestone, title,
/// scenario_ids, ...}).
pub(crate) fn append_plan_link(ctx: &Ctx, feature: &Value, goal: &str) -> Result<(), String> {
    let slug = feature["slug"].as_str().ok_or("feature has no slug")?;
    let link = json!({
        "milestone": feature["milestone"],
        "title": feature["title"],
        "goal": goal,
        "scenario_ids": feature["scenario_ids"],
        "started_unix": crate::util::unix_timestamp(),
        "status": "planning",
        "plan_id": Value::Null,
        "completed_unix": Value::Null,
        "commit_range": Value::Null,
    });
    update(ctx, slug, |state| {
        plans_mut(state).push(link);
        Ok(())
    })
}

/// Settles the latest `planning` link of `milestone`: `planned` with the
/// published plan ID, or `failed` with the error. A milestone with no link in
/// `planning` is an error, so a lost link is reported rather than invented.
pub(crate) fn set_plan_link_status(
    ctx: &Ctx,
    slug: &str,
    milestone: &str,
    status: &str,
    plan_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    update(ctx, slug, |state| {
        let link = plans_mut(state)
            .iter_mut()
            .rev()
            .find(|link| link["milestone"] == json!(milestone) && link["status"] == json!("planning"))
            .ok_or_else(|| format!("no planning link for {slug} {milestone}"))?;
        link["status"] = json!(status);
        if let Some(plan_id) = plan_id {
            link["plan_id"] = json!(plan_id);
        }
        match error {
            Some(error) => link["error"] = json!(error),
            None => {
                if let Some(link) = link.as_object_mut() {
                    link.remove("error");
                }
            },
        }
        Ok(())
    })
}

/// Marks a milestone plan's link `completed` with its commit range (S34).
/// The link is the latest one of `milestone` carrying `plan_id`, or else the
/// latest `planned` one. A link that is already completed keeps its first
/// completion, so a repeated completion changes nothing.
pub(crate) fn complete_plan_link(
    ctx: &Ctx,
    slug: &str,
    milestone: &str,
    plan_id: &str,
    base: &str,
    head: &str,
) -> Result<(), String> {
    update(ctx, slug, |state| {
        let plans = plans_mut(state);
        let index = plans
            .iter()
            .rposition(|link| link["milestone"] == json!(milestone) && link["plan_id"] == json!(plan_id))
            .or_else(|| {
                plans
                    .iter()
                    .rposition(|link| link["milestone"] == json!(milestone) && link["status"] == json!("planned"))
            })
            .ok_or_else(|| format!("no plan link for {slug} {milestone} plan {plan_id}"))?;
        let link = &mut plans[index];
        if link["status"] == json!("completed") {
            return Ok(());
        }
        link["status"] = json!("completed");
        link["plan_id"] = json!(plan_id);
        link["completed_unix"] = json!(crate::util::unix_timestamp());
        link["commit_range"] = json!({"base": base, "head": head});
        Ok(())
    })
}

// ---------------------------------------------------------------- creation

/// Creates `docs/features/<slug>/` from `docs/features/_template/` with the
/// title as the README heading, and initializes the feature's runtime state.
/// Nothing is staged or committed: approval owns the commit (S10, S15).
pub(crate) fn create(ctx: &Ctx, slug: &str, title: &str) -> Result<(), Refusal> {
    let refuse = |status: u32, message: String| Refusal { status, message };
    validate_slug(slug).map_err(|e| refuse(400, e))?;
    let title = validate_title(title).map_err(|e| refuse(400, e))?;

    let template = features_root(ctx).join("_template");
    match fs::symlink_metadata(&template) {
        Ok(meta) if meta.is_dir() => {},
        _ => return Err(refuse(409, "docs/features/_template/ is missing".into())),
    }
    let target = feature_dir(ctx, slug);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(refuse(409, format!("docs/features/{slug}/ already exists")));
    }

    // A partial copy is removed again, so a failure never leaves a half
    // feature behind for discovery to report as invalid.
    if let Err(error) = copy_template(&template, &target).and_then(|()| apply_title(&target, &title)) {
        let _ = fs::remove_dir_all(&target);
        return Err(refuse(500, error));
    }
    update(ctx, slug, |_state| Ok(())).map_err(|error| {
        let _ = fs::remove_dir_all(&target);
        refuse(500, error)
    })
}

/// Recursive copy that refuses symlinks, so a link in the template can never
/// make a created feature reach outside its own folder.
fn copy_template(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("could not create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("could not read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", src.display()))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let target = dst.join(entry.file_name());
        if meta.is_symlink() {
            return Err(format!("template contains a symlink: {}", path.display()));
        } else if meta.is_dir() {
            copy_template(&path, &target)?;
        } else if meta.is_file() {
            fs::copy(&path, &target)
                .map(|_| ())
                .map_err(|e| format!("could not copy {}: {e}", path.display()))?;
        } else {
            return Err(format!("unsupported template entry: {}", path.display()));
        }
    }
    Ok(())
}

/// Replaces the first `# ` heading outside fenced code with the given title.
fn apply_title(dir: &Path, title: &str) -> Result<(), String> {
    let path = dir.join("README.md");
    let Ok(readme) = fs::read_to_string(&path) else { return Ok(()) };
    let mut in_fence = false;
    let mut replaced = false;
    let lines: Vec<String> = readme
        .lines()
        .map(|line| {
            if line.starts_with("```") || line.starts_with("~~~") {
                in_fence = !in_fence;
            } else if !in_fence && !replaced && line.starts_with("# ") {
                replaced = true;
                return format!("# {title}");
            }
            line.to_string()
        })
        .collect();
    let mut text = lines.join("\n");
    if readme.ends_with('\n') {
        text.push('\n');
    }
    fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}
