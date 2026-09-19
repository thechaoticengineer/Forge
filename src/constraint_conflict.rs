//! Constraint-conflict escalation protocol shared by stage and plan review.
//!
//! A constraint conflict is the stage (or plan) text contradicting itself: two
//! of its own constraints cannot both be met. It goes to the planner, which owns
//! that text. An architectural context gap is two roles disagreeing and goes to
//! the architect instead.
//!
//! Everything here is side-effect free: verdict-field validation, statement
//! normalisation and the conflict signature, bounded planner inputs, the planner
//! prompt, the planner answer validator and the persisted escalation record.
//! Orchestration collects repository evidence and saves the plan elsewhere.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Implementer/fixer outcome request kind and review verdict field name.
pub(crate) const KIND: &str = "constraint_conflict";
/// Stage field (and later `plan_review` field) holding escalation records.
pub(crate) const RECORDS: &str = "constraint_escalations";
pub(crate) const RECORD_VERSION: u64 = 1;
/// Who can trigger an escalation: an editing role, a reviewing role or the engine.
pub(crate) const SOURCES: [&str; 5] = ["implementer", "fixer", "reviewer", "architect", "engine"];
pub(crate) const OUTCOMES: [&str; 6] = ["pending", "applied", "awaiting_approval", "refused", "blocked", "failed"];
/// Display bounds for stored inputs. Signatures and fingerprints always use the
/// complete normalised inputs, so truncation never changes identity.
pub(crate) const MAX_TEXT: usize = 2000;
pub(crate) const MAX_ITEMS: usize = 16;
/// Upper bound on any one planner answer text.
const MAX_ANSWER_TEXT: usize = 32 * 1024;
/// Fixer replies kept on a stage for later escalation input.
pub(crate) const OUTCOME_HISTORY_KEEP: usize = 8;

/// Validate an optional `constraint_conflict` verdict field: absent or null, or
/// a non-empty explanation. Anything else invalidates the verdict.
pub(crate) fn verdict_field(verdict: &Value) -> Result<Option<String>, String> {
    match verdict.get(KIND) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_str().filter(|text| !text.trim().is_empty())
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| "constraint_conflict must be a non-empty explanation or null".into()),
    }
}

/// Collapse whitespace and case so formatting never changes identity.
pub(crate) fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Whitespace-canonical text for comparing a correction with the current text.
fn canonical(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn request_text(request: &Value) -> String {
    request["text"].as_str().or_else(|| request.as_str()).map(str::to_owned)
        .unwrap_or_else(|| request.to_string())
}

fn normalized_set(texts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut set: Vec<String> = texts.into_iter().map(|t| normalize(&t)).filter(|t| !t.is_empty()).collect();
    set.sort();
    set.dedup();
    set
}

/// The same conflict always yields the same signature, independent of order,
/// formatting, roles or display truncation; different requests yield another.
pub(crate) fn signature(statements: &[String], requests: &[Value]) -> String {
    let statements = normalized_set(statements.iter().cloned());
    let requests = normalized_set(requests.iter().map(request_text));
    crate::app::reassessment::signature(KIND, &json!({"statements":statements,"requests":requests}))
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT { return text.to_owned(); }
    let mut short: String = text.chars().take(MAX_TEXT).collect();
    short.push('…');
    short
}

fn truncate_strings(value: &Value) -> Value {
    match value {
        Value::String(text) => json!(truncate(text)),
        Value::Array(items) => Value::Array(items.iter().map(truncate_strings).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), truncate_strings(v))).collect()),
        other => other.clone(),
    }
}

/// Keep the first `MAX_ITEMS` items with every text truncated, and a fingerprint
/// of the complete input so a truncated copy still identifies what it came from.
pub(crate) fn bounded(items: &[Value]) -> Value {
    let kept: Vec<Value> = items.iter().take(MAX_ITEMS).map(truncate_strings).collect();
    let truncated = items.len() > MAX_ITEMS || kept.iter().zip(items).any(|(k, i)| k != i);
    json!({
        "items": kept,
        "total": items.len(),
        "truncated": truncated,
        "fingerprint": crate::metadata::fingerprint(Value::Array(items.to_vec()).to_string().as_bytes()),
    })
}

fn bounded_text(text: &str) -> Value {
    json!({"text": truncate(text), "truncated": text.chars().count() > MAX_TEXT,
        "fingerprint": crate::metadata::fingerprint(text.as_bytes())})
}

/// The planner inputs for one stage, from the saved plan and the files changed
/// since the attempt began. Statements are the reported conflicts.
pub(crate) fn stage_inputs(plan: &Value, idx: usize, statements: &[String], changed_files: &[String]) -> Value {
    let stage = &plan["stages"][idx];
    let attempt = &stage["attempt_id"];
    let requests: Vec<Value> = stage["review_gate"]["requests"].as_array().cloned().unwrap_or_default();
    let replies: Vec<Value> = stage["outcome_history"].as_array().into_iter().flatten()
        .filter(|entry| entry["attempt_id"] == *attempt).cloned().collect();
    let checks: Vec<Value> = stage["reviews"].as_array().into_iter().flatten()
        .filter(|review| review["identity"]["attempt_id"] == *attempt)
        .flat_map(|review| review["project_checks"].as_array().into_iter().flatten().map(|check| json!({
            "role": review["identity"]["role"], "round": review["identity"]["round"],
            "command": check["command"], "status": check["status"], "evidence": check["evidence"],
        })).collect::<Vec<_>>())
        .collect();
    let statements: Vec<Value> = statements.iter().map(|s| json!(s)).collect();
    let files: Vec<Value> = changed_files.iter().map(|f| json!(f)).collect();
    json!({
        "goal": bounded_text(plan["goal"].as_str().unwrap_or("")),
        "plan_revision": plan["revision"],
        "plan": plan["stages"].as_array().into_iter().flatten().map(|s| json!({
            "id": s["id"], "title": s["title"], "status": s["status"], "sha": s["sha"],
        })).collect::<Vec<_>>(),
        "stage": {
            "id": stage["id"], "title": stage["title"], "status": stage["status"],
            "instructions": bounded_text(stage["instructions"].as_str().unwrap_or("")),
            "acceptance": bounded_text(stage["acceptance"].as_str().unwrap_or("")),
        },
        "statements": bounded(&statements),
        "requests": bounded(&requests),
        "fixer_replies": bounded(&replies),
        "checks": bounded(&checks),
        "changed_files": bounded(&files),
        "rounds": {"used": stage["rounds"].as_u64().unwrap_or(0), "budget": stage["review_budget"].as_u64().unwrap_or(0)},
    })
}

/// A persisted, self-describing escalation record in the `pending` state.
pub(crate) fn new_record(source: &str, kind: &str, reason: &str, signature: &str, inputs: Value, unix: i64)
    -> Result<Value, String>
{
    if !SOURCES.contains(&source) {
        return Err(format!("unknown constraint escalation source `{source}`"));
    }
    if kind.trim().is_empty() || reason.trim().is_empty() || signature.is_empty() {
        return Err("constraint escalation requires a trigger kind, reason and signature".into());
    }
    Ok(json!({
        "version": RECORD_VERSION,
        "trigger": {"source": source, "kind": kind, "reason": truncate(reason)},
        "signature": signature,
        "inputs": inputs,
        "analysis": null,
        "decision": null,
        "correction": null,
        "plan_revision": null,
        "outcome": "pending",
        "detail": null,
        "unix": unix,
        "updated_unix": unix,
    }))
}

/// Store the planner's validated answer on the record.
pub(crate) fn record_answer(record: &mut Value, answer: &PlannerAnswer) {
    record["analysis"] = json!(answer.analysis);
    record["decision"] = json!(answer.decision.name());
    record["correction"] = json!(answer.decision);
}

/// Move the record to a final or waiting outcome.
pub(crate) fn set_outcome(record: &mut Value, outcome: &str, detail: Option<&str>, plan_revision: Option<&Value>, unix: i64)
    -> Result<(), String>
{
    if !OUTCOMES.contains(&outcome) {
        return Err(format!("unknown constraint escalation outcome `{outcome}`"));
    }
    record["outcome"] = json!(outcome);
    record["detail"] = detail.map(|d| json!(truncate(d))).unwrap_or(Value::Null);
    if let Some(revision) = plan_revision { record["plan_revision"] = revision.clone(); }
    record["updated_unix"] = json!(unix);
    Ok(())
}

/// Append a record to a stage or plan-review object; returns its index.
pub(crate) fn push_record(container: &mut Value, record: Value) -> usize {
    if !container[RECORDS].is_array() { container[RECORDS] = json!([]); }
    let list = container[RECORDS].as_array_mut().unwrap();
    list.push(record);
    list.len() - 1
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StageEdit {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub instructions: String,
    pub acceptance: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewStage {
    pub title: String,
    pub instructions: String,
    pub acceptance: String,
    pub commit: String,
}

/// Exactly one decision is representable: serde's external tagging accepts a
/// single-key object only.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Decision {
    Revise {
        #[serde(default)]
        stages: Vec<StageEdit>,
        #[serde(default)]
        insert_before: Vec<NewStage>,
    },
    ConstraintWrong {
        constraint: String,
        instructions: String,
        acceptance: String,
        justification: String,
    },
    Refused(String),
}

impl Decision {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Decision::Revise { .. } => "revise",
            Decision::ConstraintWrong { .. } => "constraint_wrong",
            Decision::Refused(_) => "refused",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlannerAnswer {
    pub analysis: String,
    pub decision: Decision,
}

fn required(field: &str, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(format!("{field} must be a non-empty text"));
    }
    if text.len() > MAX_ANSWER_TEXT {
        return Err(format!("{field} exceeds {MAX_ANSWER_TEXT} bytes"));
    }
    Ok(())
}

/// Validate the planner's answer against the plan it was asked about. Pure, so
/// it can run inside `repair_response`'s correction loop.
pub(crate) fn validate_answer(text: &str, plan: &Value, idx: usize) -> Result<PlannerAnswer, String> {
    let payload = crate::util::json_payload_with_keys(text, &["analysis", "decision"]);
    let answer: PlannerAnswer = crate::response::parse_json(payload).map_err(|e| format!(
        "invalid constraint conflict answer: {e}. Return ONLY JSON with a non-empty analysis and a decision object holding exactly one of revise, constraint_wrong or refused; preserve your actual decision"))?;
    required("analysis", &answer.analysis)?;
    let stages = plan["stages"].as_array().ok_or("plan has no stages")?;
    let current = stages.get(idx).ok_or("current stage is missing from the plan")?;
    if current["status"] == "committed" {
        return Err("the current stage is committed history and cannot be corrected".into());
    }
    let unchanged = |stage: &Value, instructions: &str, acceptance: &str| {
        canonical(stage["instructions"].as_str().unwrap_or("")) == canonical(instructions)
            && canonical(stage["acceptance"].as_str().unwrap_or("")) == canonical(acceptance)
    };
    match &answer.decision {
        Decision::Refused(how) => required("refused", how)?,
        Decision::ConstraintWrong { constraint, instructions, acceptance, justification } => {
            required("constraint_wrong.constraint", constraint)?;
            required("constraint_wrong.instructions", instructions)?;
            required("constraint_wrong.acceptance", acceptance)?;
            required("constraint_wrong.justification", justification)?;
            if unchanged(current, instructions, acceptance) {
                return Err("constraint_wrong returned the current stage text unchanged; correct the wrong constraint or return refused".into());
            }
        }
        Decision::Revise { stages: edits, insert_before } => {
            if edits.is_empty() && insert_before.is_empty() {
                return Err("revise must change at least one stage or insert a stage".into());
            }
            let mut seen = vec![];
            for edit in edits {
                let position = stages.iter().position(|s| s["id"] == edit.id)
                    .ok_or_else(|| format!("revise names unknown stage {}", edit.id))?;
                if seen.contains(&edit.id) {
                    return Err(format!("revise changes stage {} twice", edit.id));
                }
                seen.push(edit.id);
                if stages[position]["status"] == "committed" {
                    return Err(format!("stage {} is committed history and cannot change", edit.id));
                }
                if position < idx {
                    return Err(format!("stage {} precedes the current stage; only the current and later pending stages may change", edit.id));
                }
                if let Some(title) = &edit.title { required("revise.stages.title", title)?; }
                required("revise.stages.instructions", &edit.instructions)?;
                required("revise.stages.acceptance", &edit.acceptance)?;
                let same_title = edit.title.as_ref()
                    .is_none_or(|t| canonical(t) == canonical(stages[position]["title"].as_str().unwrap_or("")));
                if same_title && unchanged(&stages[position], &edit.instructions, &edit.acceptance) {
                    return Err(format!("revise returned stage {} unchanged; change it or leave it out", edit.id));
                }
            }
            for stage in insert_before {
                required("revise.insert_before.title", &stage.title)?;
                required("revise.insert_before.instructions", &stage.instructions)?;
                required("revise.insert_before.acceptance", &stage.acceptance)?;
                required("revise.insert_before.commit", &stage.commit)?;
            }
        }
    }
    Ok(answer)
}

/// The plan as the planner sees it: every stage with its status, committed
/// stages included, marking the current one.
pub(crate) fn plan_overview(plan: &Value, idx: usize) -> String {
    plan["stages"].as_array().into_iter().flatten().enumerate().map(|(i, s)| {
        let marker = if i == idx { " (CURRENT)" } else { "" };
        let fixed = if s["status"] == "committed" { " — committed history, cannot change" } else { "" };
        format!("STAGE {}{marker} [{}{fixed}]: {}\nINSTRUCTIONS:\n{}\nACCEPTANCE:\n{}\nCOMMIT: {}\n\n",
            s["id"], s["status"].as_str().unwrap_or("pending"), s["title"].as_str().unwrap_or(""),
            s["instructions"].as_str().unwrap_or(""), s["acceptance"].as_str().unwrap_or(""),
            s["commit"].as_str().unwrap_or(""))
    }).collect()
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Render the planner conflict prompt from the plan and a pending record.
pub(crate) fn prompt(plan: &Value, idx: usize, record: &Value) -> String {
    let stage = &plan["stages"][idx];
    let inputs = &record["inputs"];
    crate::util::fill_template(crate::prompts::CONFLICT_PROMPT, &[
        ("{goal}", plan["goal"].as_str().unwrap_or("")),
        ("{plan}", &plan_overview(plan, idx)),
        ("{sid}", &stage["id"].to_string()),
        ("{title}", stage["title"].as_str().unwrap_or("")),
        ("{instructions}", stage["instructions"].as_str().unwrap_or("")),
        ("{acceptance}", stage["acceptance"].as_str().unwrap_or("")),
        ("{trigger}", &pretty(&record["trigger"])),
        ("{statements}", &pretty(&inputs["statements"])),
        ("{requests}", &pretty(&inputs["requests"])),
        ("{replies}", &pretty(&inputs["fixer_replies"])),
        ("{checks}", &pretty(&inputs["checks"])),
        ("{changed_files}", &pretty(&inputs["changed_files"])),
        ("{rounds}", &pretty(&inputs["rounds"])),
    ])
}

#[cfg(test)]
#[path = "constraint_conflict_tests.rs"]
mod tests;
