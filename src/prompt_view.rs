//! Compact projections of architecture records for provider prompts.
//!
//! Every function clones its input: storage, publication and validity checks
//! keep the complete records, and only the text a provider reads is reduced.

use serde_json::{Value, json};

/// Guidance and agreement records carry a `relevant_inputs` fingerprint that
/// copies the stage text a prompt already contains and grows with every
/// dependency. No provider prompt needs it.
const FINGERPRINT: &str = "relevant_inputs";

fn strip_record(record: &mut Value) {
    if let Some(record) = record.as_object_mut() { record.remove(FINGERPRINT); }
}

/// One guidance or agreement record for a prompt.
pub(crate) fn agreement(record: &Value) -> Value {
    let mut view = record.clone();
    strip_record(&mut view);
    view
}

/// Guidance for a prompt: accepts a map of guidance records keyed by stage or
/// one guidance record, and returns it without the input fingerprint.
pub(crate) fn guidance(map_or_record: &Value) -> Value {
    if map_or_record.get(FINGERPRINT).is_some() { return agreement(map_or_record); }
    let mut view = map_or_record.clone();
    if let Some(records) = view.as_object_mut() { records.values_mut().for_each(strip_record); }
    view
}

/// A short, stable content ID: `prefix`, a hyphen and the first eight hex
/// characters of the text's content hash. Equal texts always share an ID.
pub(crate) fn content_id(prefix: &str, text: &str) -> String {
    format!("{prefix}-{}", &crate::metadata::fingerprint(text.as_bytes())[..8])
}

/// A short, stable ID for one constraint text: `c-` plus the first eight hex
/// characters of its content hash. Equal texts always share an ID.
pub(crate) fn constraint_id(text: &str) -> String {
    content_id("c", text)
}

fn text_id(value: &Value) -> String {
    constraint_id(&value.as_str().map_or_else(|| value.to_string(), str::to_string))
}

/// A compact agreement for a prompt. Dialogue, policy inputs, provenance, the
/// validated proposal, bootstraps, the stored constraint copy and the input
/// fingerprint stay in storage and the architecture events. Constraints are
/// referenced by ID: `constraint_refs` for those the checkpoint still holds
/// and `retired_constraint_refs` for those it no longer holds.
pub(crate) fn agreement_view(record: &Value, constraints: &Value) -> Value {
    if !record.is_object() { return record.clone(); }
    let mut view = serde_json::Map::new();
    for key in ["id", "stage_id", "valid"] { view.insert(key.into(), record[key].clone()); }
    if !record["invalidation_trigger"].is_null() {
        view.insert("invalidation_trigger".into(), record["invalidation_trigger"].clone());
    }
    if record["effective"].is_object() {
        let effective = &record["effective"];
        view.insert("effective".into(), json!({"provider": effective["provider"],
            "model": effective["model"], "native_effort": effective["native_effort"]}));
    } else {
        view.insert("effective".into(), Value::Null);
        for (key, value) in [("tier", &record["policy_inputs"]["tier"]),
            ("minimum_tier", &record["policy_inputs"]["minimum_tier"]), ("binding", &record["binding"])] {
            if !value.is_null() { view.insert(key.into(), value.clone()); }
        }
    }
    for key in ["planner_reason", "architect_reason", "trigger"] {
        view.insert(key.into(), record[key].clone());
    }
    let current: Vec<&Value> = constraints.as_array().into_iter().flatten().collect();
    let (mut kept, mut retired) = (Vec::new(), Vec::new());
    for text in record["architectural_constraints"].as_array().into_iter().flatten() {
        if current.contains(&text) { kept.push(text_id(text)) } else { retired.push(text_id(text)) }
    }
    view.insert("constraint_refs".into(), json!(kept));
    view.insert("retired_constraint_refs".into(), json!(retired));
    Value::Object(view)
}

/// Where the full review records of a stage are: its indexed review file when
/// the plan names one, otherwise the plan document.
fn review_reference(plan: &Value, stage_id: &Value) -> String {
    let dir = crate::app::FORGE_DIR;
    match (plan["plan_id"].as_str(), plan["architecture"]["review_history"][stage_id.to_string()]["file"].as_str()) {
        (Some(plan_id), Some(file)) => format!("{dir}/architecture/{plan_id}/reviews/{file}.jsonl"),
        _ => format!("{dir}/plan.json"),
    }
}

/// The compact review of a stage: gate status, role states, the number of
/// requests and where the full records are.
fn review_summary(gate: &Value, plan: &Value, stage_id: &Value) -> Value {
    let mut review = json!({"status": gate["status"], "full_record": review_reference(plan, stage_id)});
    if let Some(requests) = gate["requests"].as_array() { review["requests"] = json!(requests.len()); }
    if let Some(roles) = gate["roles"].as_object().filter(|r| r.values().all(Value::is_string)) {
        review["roles"] = json!(roles);
    }
    review
}

/// One execution outcome for a prompt: identity and result fields plus a
/// small review summary instead of the full gate.
pub(crate) fn outcome_view(outcome: &Value, plan: &Value) -> Value {
    if !outcome.is_object() { return outcome.clone(); }
    let mut view = serde_json::Map::new();
    for key in ["stage_id", "revision", "status", "sha", "rounds", "unix"] {
        if let Some(value) = outcome.get(key) { view.insert(key.into(), value.clone()); }
    }
    view.insert("review".into(), review_summary(&outcome["review_gate"], plan, &outcome["stage_id"]));
    Value::Object(view)
}

fn outcomes(list: &Value, plan: &Value) -> Value {
    match list.as_array() {
        Some(list) => json!(list.iter().map(|o| outcome_view(o, plan)).collect::<Vec<_>>()),
        None => list.clone(),
    }
}

/// A checkpoint for a prompt. Guidance loses its input fingerprint,
/// agreements become compact records, execution outcomes lose their review
/// gates, and `constraint_ids` names each constraint in order. Everything else
/// is kept as is. `plan` only supplies review-file references.
pub(crate) fn checkpoint_for(cp: &Value, plan: &Value) -> Value {
    let mut view = cp.clone();
    if let Some(records) = view.get_mut("guidance").and_then(Value::as_object_mut) {
        records.values_mut().for_each(strip_record);
    }
    if let Some(records) = view.get_mut("agreements").and_then(Value::as_object_mut) {
        for record in records.values_mut() { *record = agreement_view(record, &cp["constraints"]); }
    }
    if cp["execution_outcomes"].is_array() { view["execution_outcomes"] = outcomes(&cp["execution_outcomes"], plan); }
    if let Some(constraints) = cp["constraints"].as_array() {
        view["constraint_ids"] = json!(constraints.iter().map(text_id).collect::<Vec<_>>());
    }
    view
}

/// Committed stages for an implementer or fixer prompt: id, title, sha and a
/// compact outcome. Only the current stage's direct dependencies keep their
/// acceptance; the rest is in git and `.forge/plan.json`. A stage without
/// `depends_on` depends on every earlier stage.
pub(crate) fn committed_stages(plan: &Value, cp: &Value, current: &Value) -> Vec<Value> {
    let stages = plan["stages"].as_array().cloned().unwrap_or_default();
    let position = stages.iter().position(|s| s["id"] == current["id"]).unwrap_or(stages.len());
    let dependencies: Vec<Value> = match current.get("depends_on").and_then(Value::as_array) {
        Some(ids) => ids.clone(),
        None => stages[..position].iter().map(|s| s["id"].clone()).collect(),
    };
    let recorded = cp["execution_outcomes"].as_array().cloned().unwrap_or_default();
    stages.iter().filter(|s| s["status"] == "committed").map(|stage| {
        let outcome = recorded.iter().rev().find(|o| o["stage_id"] == stage["id"]).map_or_else(
            || json!({"status": stage["status"], "review": review_summary(&stage["review_gate"], plan, &stage["id"])}),
            |o| { let v = outcome_view(o, plan); json!({"status": v["status"], "review": v["review"]}) });
        let mut entry = json!({"id": stage["id"], "title": stage["title"], "sha": stage["sha"], "outcome": outcome});
        if dependencies.contains(&stage["id"]) { entry["acceptance"] = stage["acceptance"].clone(); }
        entry
    }).collect()
}

#[cfg(test)]
#[path = "prompt_view_tests.rs"]
mod tests;
