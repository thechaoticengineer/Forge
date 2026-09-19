//! The bounded project synthesis: what holds across plans, stored as runtime
//! state in `.forge/architecture/synthesis.json`, plus the current work the
//! engine derives on every read and never stores.
//!
//! The document lives beside the per-plan directories, never inside one: its
//! file name contains a dot, which `durable_json::safe_id` rejects, so it can
//! never be a plan identity. Plan reset and archiving only touch plan
//! directories and `plan.json`, so the synthesis survives both. It is a
//! separate prompt field and is never merged into a checkpoint's constraints.
//!
//! Every entry ID is derived by the engine from its kind and text with the
//! shared content hash of `prompt_view` (`c-`, `i-` and `d-` prefixes). A
//! replacement keeps every previous entry byte for byte or retires its ID with
//! a reason; `retired` describes only the latest replacement.
use crate::app::Ctx;
use crate::durable_json::publish_pretty;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) const FILE: &str = "synthesis.json";
pub(crate) const VERSION: u64 = 1;
/// Bytes of the stored (pretty) serialization; the compact prompt form is smaller.
pub(crate) const MAX_BYTES: usize = 8192;
pub(crate) const MAX_CONSTRAINTS: usize = 24;
pub(crate) const MAX_INTERFACES: usize = 24;
pub(crate) const MAX_DECISIONS: usize = 16;
/// UTF-8 bytes of one entry text, retirement reason or source goal.
pub(crate) const MAX_ENTRY_BYTES: usize = 300;

/// Serialized bytes of the derived current work, whatever the plan, queue and
/// feature state hold.
pub(crate) const CURRENT_WORK_MAX_BYTES: usize = 4096;
pub(crate) const MAX_QUEUE_ITEMS: usize = 10;
pub(crate) const MAX_FEATURES: usize = 8;
const GOAL_CHARS: usize = 300;
const STAGE_TITLE_CHARS: usize = 120;
const QUEUE_GOAL_CHARS: usize = 200;
const FEATURE_TITLE_CHARS: usize = 120;
const LABEL_CHARS: usize = 40;

/// (document key, ID prefix, entry limit) of each kind of active entry.
const KINDS: [(&str, &str, usize); 3] = [
    ("constraints", "c", MAX_CONSTRAINTS),
    ("interfaces", "i", MAX_INTERFACES),
    ("decisions", "d", MAX_DECISIONS),
];
const DOCUMENT_KEYS: [&str; 6] = ["version", "source", "constraints", "interfaces", "decisions", "retired"];
const SOURCE_KEYS: [&str; 5] = ["plan_id", "checkpoint", "sha", "goal", "unix"];

pub(crate) fn path(root: &Path) -> PathBuf {
    root.join("architecture").join(FILE)
}

/// The engine-derived ID of one entry: its kind prefix plus the shared short
/// content hash, so a constraint's ID matches its prompt `constraint_ids`.
pub(crate) fn entry_id(prefix: &str, text: &str) -> String {
    crate::prompt_view::content_id(prefix, text)
}

fn only_keys(value: &Value, allowed: &[&str], what: &str) -> Result<(), String> {
    let object = value.as_object().ok_or_else(|| format!("{what} must be an object"))?;
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!("{what} has unknown field {key:?}")),
        None => Ok(()),
    }
}

fn bounded_text<'a>(value: &'a Value, what: &str) -> Result<&'a str, String> {
    let text = value.as_str().ok_or_else(|| format!("{what} must be a string"))?;
    if text.trim().is_empty() { return Err(format!("{what} is empty")); }
    if text.len() > MAX_ENTRY_BYTES {
        return Err(format!("{what} is {} bytes, over the {MAX_ENTRY_BYTES}-byte limit", text.len()));
    }
    Ok(text)
}

fn validate_source(source: &Value) -> Result<(), String> {
    only_keys(source, &SOURCE_KEYS, "synthesis source")?;
    if !source["plan_id"].as_str().is_some_and(crate::durable_json::safe_id) {
        return Err("synthesis source needs a valid plan_id".into());
    }
    if !(source["checkpoint"].is_null() || source["checkpoint"].as_str().is_some_and(crate::durable_json::safe_id)) {
        return Err("synthesis source checkpoint must be a checkpoint token or null".into());
    }
    if !source["sha"].as_str().is_some_and(|sha| !sha.is_empty() && sha.len() <= 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("synthesis source needs a commit sha".into());
    }
    if !source["goal"].as_str().is_some_and(|goal| goal.len() <= MAX_ENTRY_BYTES) {
        return Err(format!("synthesis source goal must be a string of at most {MAX_ENTRY_BYTES} bytes"));
    }
    if !source["unix"].is_u64() { return Err("synthesis source needs a unix time".into()); }
    Ok(())
}

/// Shape, limits and ID rules of one stored document, without comparing it
/// to the document it replaced. `retired` may name entries of that earlier
/// document, which is no longer available here.
fn validate_document(doc: &Value) -> Result<(), String> {
    only_keys(doc, &DOCUMENT_KEYS, "synthesis")?;
    if doc["version"] != VERSION { return Err("unsupported synthesis version".into()); }
    validate_source(&doc["source"])?;
    let mut ids = HashSet::new();
    for (kind, prefix, max) in KINDS {
        let entries = doc[kind].as_array().ok_or_else(|| format!("synthesis {kind} must be an array"))?;
        if entries.len() > max {
            return Err(format!("synthesis has {} {kind}, over the limit of {max}", entries.len()));
        }
        for entry in entries {
            only_keys(entry, &["id", "text"], &format!("synthesis {kind} entry"))?;
            let text = bounded_text(&entry["text"], &format!("synthesis {kind} text"))?;
            let id = entry["id"].as_str().unwrap_or("");
            if id != entry_id(prefix, text) {
                return Err(format!("synthesis {kind} entry {id:?} does not carry its engine-derived ID"));
            }
            if !ids.insert(id) { return Err(format!("duplicate synthesis ID {id}")); }
        }
    }
    let retired = doc["retired"].as_array().ok_or("synthesis retired must be an array")?;
    for record in retired {
        only_keys(record, &["id", "text", "reason"], "synthesis retired entry")?;
        let text = bounded_text(&record["text"], "retired synthesis text")?;
        bounded_text(&record["reason"], "retirement reason")?;
        let id = record["id"].as_str().unwrap_or("");
        if !KINDS.iter().any(|(_, prefix, _)| id == entry_id(prefix, text)) {
            return Err(format!("retired synthesis ID {id:?} does not match its text"));
        }
        if !ids.insert(id) { return Err(format!("synthesis ID {id} is both active and retired, or retired twice")); }
    }
    let bytes = serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?.len();
    if bytes > MAX_BYTES {
        return Err(format!("synthesis is {bytes} bytes, over the {MAX_BYTES}-byte limit"));
    }
    Ok(())
}

/// Every active entry of a document as (id, text).
fn active_entries(doc: &Value) -> Vec<(&str, &str)> {
    KINDS.iter().flat_map(|(kind, _, _)| doc[*kind].as_array().into_iter().flatten())
        .filter_map(|e| Some((e["id"].as_str()?, e["text"].as_str()?))).collect()
}

/// Validates `new` as the replacement of `old` (None when there is no stored
/// synthesis). Each previously active entry must stay active with identical
/// text, or be retired by ID with a non-empty reason; a changed text is a new
/// entry and retires the old one. Retirements name only entries of `old`.
pub(crate) fn validate(new: &Value, old: Option<&Value>) -> Result<(), String> {
    validate_document(new)?;
    let previous = old.map(active_entries).unwrap_or_default();
    let current = active_entries(new);
    let retired = new["retired"].as_array().unwrap();
    for (id, text) in &previous {
        let kept = current.iter().any(|(i, t)| i == id && t == text);
        if !kept && !retired.iter().any(|r| r["id"] == *id) {
            return Err(format!("previous synthesis entry {id} was dropped without being retired"));
        }
    }
    for record in retired {
        if !previous.iter().any(|(id, text)| record["id"] == *id && record["text"] == *text) {
            return Err(format!("retired synthesis ID {} is not an entry of the previous synthesis", record["id"]));
        }
    }
    Ok(())
}

fn truncate_bytes(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) { end -= 1; }
    text[..end].to_string()
}

/// Builds and validates a replacement document from architect-style output,
/// `{constraints:[string], interfaces:[string], decisions:[string],
/// retired:[{id, reason}]}`: the engine derives every ID, and a retirement
/// copies its text from `old`. `source` is {plan_id, checkpoint, sha, goal,
/// unix}; its goal is cut to the entry byte limit.
#[cfg_attr(not(test), allow(dead_code))] // The architect synthesis turn is its production caller.
pub(crate) fn build(output: &Value, old: Option<&Value>, mut source: Value) -> Result<Value, String> {
    only_keys(output, &["constraints", "interfaces", "decisions", "retired"], "synthesis output")?;
    let mut doc = json!({"version": VERSION});
    for (kind, prefix, _) in KINDS {
        let texts = output[kind].as_array().ok_or_else(|| format!("synthesis output {kind} must be an array of strings"))?;
        let entries = texts.iter().map(|text| {
            let text = text.as_str().ok_or_else(|| format!("synthesis output {kind} must be an array of strings"))?.trim();
            Ok(json!({"id": entry_id(prefix, text), "text": text}))
        }).collect::<Result<Vec<_>, String>>()?;
        doc[kind] = json!(entries);
    }
    let previous = old.map(active_entries).unwrap_or_default();
    let none = Vec::new();
    let retired = output.get("retired").map_or(Ok(&none), |r| r.as_array().ok_or("synthesis output retired must be an array"))?;
    doc["retired"] = json!(retired.iter().map(|record| {
        only_keys(record, &["id", "reason"], "synthesis output retirement")?;
        let (id, text) = previous.iter().find(|(id, _)| record["id"] == *id)
            .ok_or_else(|| format!("retired synthesis ID {} is not an entry of the previous synthesis", record["id"]))?;
        Ok(json!({"id": id, "text": text, "reason": record["reason"]}))
    }).collect::<Result<Vec<_>, String>>()?);
    if let Some(goal) = source["goal"].as_str() { source["goal"] = json!(truncate_bytes(goal, MAX_ENTRY_BYTES)); }
    doc["source"] = source;
    validate(&doc, old)?;
    Ok(doc)
}

/// The stored synthesis: None when there is none, an error when the file is
/// unreadable or invalid. The caller holds the persistence lock.
pub(crate) fn load(root: &Path) -> Result<Option<Value>, String> {
    let path = path(root);
    let doc = match crate::durable_json::read_json(&path,
        |path, e| format!("{}: {e}", path.display()), |path, e| format!("{}: {e}", path.display())) {
        Ok(doc) => doc,
        Err(_) if !path.exists() => return Ok(None),
        Err(error) => return Err(error),
    };
    validate_document(&doc).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(doc))
}

/// Replaces the stored synthesis atomically after validating `doc` against
/// it. The caller holds the persistence lock.
#[cfg_attr(not(test), allow(dead_code))] // The architect synthesis turn is its production caller.
pub(crate) fn save(root: &Path, doc: &Value) -> Result<(), String> {
    let old = load(root)?;
    validate(doc, old.as_ref())?;
    publish_pretty(&path(root), doc)
}

/// The stored part of the synthesis for prompts: everything but `retired`.
pub(crate) fn what_holds(doc: &Value) -> Value {
    let mut view = doc.clone();
    if let Some(object) = view.as_object_mut() { object.remove("retired"); }
    view
}

/// The advisory sentence every receiving role reads with the synthesis.
pub(crate) fn note(doc: Option<&Value>) -> String {
    match doc.and_then(|d| d["source"]["sha"].as_str()) {
        Some(sha) => format!("This project synthesis is advisory: what_holds was recorded at commit {sha} and may be stale, so check every entry against the repository code before relying on it."),
        None => "This project synthesis is advisory: no what_holds has been recorded yet, and current work must be checked against the repository code before relying on it.".into(),
    }
}

// ---------------------------------------------------------------- current work

fn chars(value: &Value, limit: usize) -> Value {
    match value.as_str() {
        Some(text) => json!(text.chars().take(limit).collect::<String>()),
        None if value.is_number() => value.clone(),
        None => Value::Null,
    }
}

/// Features and milestones in progress: the plan's own feature milestone, then
/// every unimplemented milestone whose latest plan link is still `planning` or
/// `planned`, then each partially implemented feature with none of those.
/// Milestones come from feature discovery and plan links from the feature
/// state, the readers `/api/features` uses; each (slug, milestone) appears once.
fn features_in_progress(ctx: &Ctx, plan: Option<&Value>) -> Vec<Value> {
    let entry = |slug: &Value, title: &Value, milestone: &Value, status: &str|
        json!({"slug": chars(slug, crate::feature_state::MAX_SLUG_BYTES), "title": chars(title, FEATURE_TITLE_CHARS),
            "milestone": chars(milestone, LABEL_CHARS), "status": status});
    let mut out: Vec<Value> = Vec::new();
    let add = |item: Value, out: &mut Vec<Value>| {
        if !out.iter().any(|e| e["slug"] == item["slug"] && e["milestone"] == item["milestone"]) { out.push(item); }
    };
    if let Some(feature) = plan.map(|p| &p["feature"]).filter(|f| f.is_object()) {
        add(entry(&feature["slug"], &feature["title"], &feature["milestone"], "current plan"), &mut out);
    }
    for feature in crate::features::discover(Path::new(ctx.project())) {
        let state = crate::feature_state::load(ctx, &feature.slug).unwrap_or(Value::Null);
        let (slug, title) = (json!(feature.slug), json!(feature.title));
        let mut active = false;
        for milestone in feature.milestones.iter().filter(|m| !m.implemented) {
            let link = crate::feature_state::plan_link_view(&state, &milestone.id);
            if let Some(status @ ("planning" | "planned")) = link["status"].as_str() {
                add(entry(&slug, &title, &json!(milestone.id), status), &mut out);
                active = true;
            }
        }
        let current = out.iter().any(|e| e["slug"] == slug);
        if !active && !current && feature.progress() == "in progress" {
            let next = feature.milestones.iter().find(|m| !m.implemented).map(|m| json!(m.id)).unwrap_or(Value::Null);
            add(entry(&slug, &title, &next, "in progress"), &mut out);
        }
    }
    out
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// Current work for the architect and planner chat, derived from `plan` (the
/// saved or candidate plan), the goal queue and feature state on every call
/// and never stored. Collections keep their first items within fixed limits
/// and the whole value within `CURRENT_WORK_MAX_BYTES`; `omitted` counts what
/// each collection left out.
pub(crate) fn current_work(ctx: &Ctx, plan: Option<&Value>) -> Value {
    let mut stages: Vec<Value> = plan.and_then(|p| p["stages"].as_array()).into_iter().flatten()
        .map(|s| json!({"id": chars(&s["id"], LABEL_CHARS), "title": chars(&s["title"], STAGE_TITLE_CHARS),
            "status": chars(&s["status"], LABEL_CHARS)}))
        .collect();
    let mut queue: Vec<Value> = ctx.load_queue()["items"].as_array().into_iter().flatten()
        .map(|i| json!({"id": chars(&i["id"], LABEL_CHARS), "goal": chars(&i["goal"], QUEUE_GOAL_CHARS),
            "status": chars(&i["status"], LABEL_CHARS)}))
        .collect();
    let mut features = features_in_progress(ctx, plan);
    let mut omitted = [0, queue.len().saturating_sub(MAX_QUEUE_ITEMS), features.len().saturating_sub(MAX_FEATURES)];
    queue.truncate(MAX_QUEUE_ITEMS);
    features.truncate(MAX_FEATURES);
    let summary = plan.map(|p| json!({"plan_id": chars(&p["plan_id"], 128), "goal": chars(&p["goal"], GOAL_CHARS),
        "status": chars(&p["status"], LABEL_CHARS)}));
    loop {
        let mut plan_view = summary.clone().unwrap_or(Value::Null);
        if plan_view.is_object() { plan_view["stages"] = json!(stages); }
        let view = json!({"plan": plan_view, "queue": queue, "features": features,
            "omitted": {"stages": omitted[0], "queue": omitted[1], "features": omitted[2]}});
        // Drop the last item of the longest collection until the value fits.
        let longest = [stages.len(), queue.len(), features.len()].into_iter().enumerate()
            .filter(|(_, len)| *len > 0).max_by_key(|(_, len)| *len).map(|(index, _)| index);
        match longest {
            Some(index) if serialized_len(&view) > CURRENT_WORK_MAX_BYTES => {
                [&mut stages, &mut queue, &mut features][index].pop();
                omitted[index] += 1;
            }
            _ => return view,
        }
    }
}

// ---------------------------------------------------------------- role views

impl Ctx {
    /// The stored synthesis, read under the persistence lock.
    pub(crate) fn load_project_synthesis(&self) -> Result<Option<Value>, String> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        load(&self.forge_path(""))
    }

    /// Validates and replaces the stored synthesis under the persistence lock.
    #[cfg_attr(not(test), allow(dead_code))] // The architect synthesis turn is its production caller.
    pub(crate) fn save_project_synthesis(&self, doc: &Value) -> Result<(), String> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        save(&self.forge_path(""), doc)
    }

    /// A prompt never fails because of the advisory synthesis: an unreadable
    /// or invalid file is logged and left out.
    fn prompt_synthesis(&self) -> Option<Value> {
        self.load_project_synthesis().unwrap_or_else(|error| {
            self.log_event("error", &format!("project synthesis left out of the prompt: {error}"));
            None
        })
    }

    /// The `project_synthesis` field of the architect publish context and the
    /// planner chat: derived current work, what holds (null when nothing is
    /// stored) and the advisory note.
    pub(crate) fn synthesis_context(&self, plan: Option<&Value>) -> Value {
        let doc = self.prompt_synthesis();
        json!({"current_work": current_work(self, plan), "what_holds": doc.as_ref().map(what_holds),
            "note": note(doc.as_ref())})
    }

    /// The compact what-holds section of implementer, fixer, stage review,
    /// plan review and PLAN_FIX prompts; None when nothing is stored. Routing
    /// prompts receive neither this nor `synthesis_context`.
    pub(crate) fn synthesis_section(&self) -> Option<String> {
        let doc = self.prompt_synthesis()?;
        Some(format!("\nPROJECT SYNTHESIS (what holds across plans): {}\n{}\n", what_holds(&doc), note(Some(&doc))))
    }
}

#[cfg(test)]
#[path = "architecture_synthesis_tests.rs"]
mod tests;
