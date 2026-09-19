//! Verdict parsing, evidence validation, policy partitioning, and gate aggregation.
use super::Ctx;
use serde_json::{Value, json};

pub(super) fn acceptance_criteria_items(acceptance: &str) -> Vec<&str> {
    acceptance
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}
pub(super) fn normalize_review_verdict(output: &str, identity: &Value, acceptance: &str) -> Result<Value, String> {
    if output.len() > 128 * 1024 {
        return Err("oversized verdict".into());
    }
    // Accept a prose preamble and an optional Markdown fence, but never search
    // past an earlier JSON candidate or ignore content after the verdict.
    // The shared response parser rejects duplicate keys at every nesting level.
    let output = output.trim();
    let start = output.find(['{', '[']).into_iter()
        .chain(output.find("```"))
        .min().unwrap_or(0);
    let output = &output[start..];
    let output = if let Some(fenced) = output.strip_prefix("```json\n")
        .or_else(|| output.strip_prefix("```\n"))
        .or_else(|| output.strip_prefix("```json\r\n"))
        .or_else(|| output.strip_prefix("```\r\n"))
    {
        fenced.trim_end().strip_suffix("```")
            .ok_or("malformed verdict: unclosed JSON fence")?.trim()
    } else {
        output
    };
    let mut v: Value =
        crate::response::parse_json(output).map_err(|e| format!("malformed verdict: {e}"))?;
    if v["identity"] != *identity {
        return Err("wrong or missing review identity".into());
    }
    if !v["approved"].is_boolean()
        || v["summary"].as_str().is_none_or(|s| s.trim().is_empty())
        || !v["requires_dual"].is_boolean()
    {
        return Err("missing verdict fields".into());
    }
    for field in ["issues", "checks"] {
        if v[field].as_array().is_none_or(|a| {
            a.iter()
                .any(|s| s.as_str().is_none_or(|s| s.trim().is_empty()))
        }) {
            return Err(format!("malformed {field}"));
        }
    }
    if v["notes"].is_null() {
        v["notes"] = json!([]);
    }
    if v["notes"].as_array().is_none_or(|a| {
        a.iter()
            .any(|s| s.as_str().is_none_or(|s| s.trim().is_empty()))
    }) {
        return Err("malformed notes".into());
    }
    if !v["notes"].as_array().unwrap().is_empty() {
        v["approved"] = json!(false);
    }
    if v["approved"] == true && !v["issues"].as_array().unwrap().is_empty() {
        return Err("contradictory approval with requests".into());
    }
    if let Some(gap) = v.get("architecture_context_gap").filter(|gap| !gap.is_null()) {
        gap.as_str().filter(|text| !text.trim().is_empty())
            .ok_or("architecture_context_gap must be a non-empty explanation or null")?;
    }
    // A context gap is two roles disagreeing and goes to the architect; a
    // constraint conflict is the stage (or plan) text contradicting itself and
    // goes to the planner. Both are rejections that keep their own statement.
    let conflict = crate::constraint_conflict::verdict_field(&v)?;
    if let Some(gap) = v["architecture_context_gap"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
    {
        v["approved"] = json!(false);
        v["issues"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("Resolve architectural context gap: {gap}")));
    }
    if let Some(conflict) = conflict {
        v["approved"] = json!(false);
        v["issues"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("Resolve constraint conflict (the planner owns the stage text): {conflict}")));
    }
    if v["approved"] == true {
        let required = acceptance_criteria_items(acceptance);
        if v["criteria"].as_array().is_none_or(|items| {
            items.len() != required.len()
                || required.iter().any(|criterion| {
                    items
                        .iter()
                        .filter(|item| {
                            item["criterion"] == *criterion
                                && item["status"] == "passed"
                                && item["evidence"]
                                    .as_str()
                                    .is_some_and(|s| !s.trim().is_empty())
                        })
                        .count()
                        != 1
                })
        }) {
            return Err("approval lacks individual criterion evidence".into());
        }
        if v["checks"].as_array().unwrap().is_empty()
            || v["acceptance_evidence"]["acceptance"] != acceptance
            || v["acceptance_evidence"]["verified"] != true
            || v["acceptance_evidence"]["evidence"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            || v["project_checks"].as_array().is_none_or(|a| {
                a.is_empty()
                    || a.iter().any(|c| {
                        !matches!(c["status"].as_str(), Some("passed" | "unavailable"))
                            || c["command"].as_str().is_none_or(|s| s.trim().is_empty())
                            || c["evidence"].as_str().is_none_or(|s| s.trim().is_empty())
                    })
            })
        {
            return Err(
                "approval lacks evidenced acceptance criteria or required project checks".into(),
            );
        }
    } else if Ctx::review_requests(&v).is_empty() {
        return Err("review rejection requires actionable issues, notes, an architecture context gap or a constraint conflict; preserve the rejection and explain what needs correction".into());
    }
    Ok(v)
}
pub(super) fn partition_review_policy_by_cadence(mut policy: Value, stage: &Value) -> Value {
    let (required, deferred): (Vec<_>, Vec<_>) = policy["required_roles"].as_array().unwrap()
        .iter().cloned().partition(|role| crate::plan::review_cadence(stage, role.as_str().unwrap()) == "per_stage");
    policy["stage_required_roles"] = json!(required);
    policy["deferred_roles"] = json!(deferred);
    policy
}

pub(super) fn stage_required_roles(policy: &Value) -> Option<&Vec<Value>> {
    policy.get("stage_required_roles").unwrap_or(&policy["required_roles"]).as_array()
}

pub(super) fn aggregate_review_gate(identity: &Value, records: &[Value]) -> Value {
    let required = stage_required_roles(&identity["policy"]).unwrap();
    let mut roles = json!({"architect":"not_required","reviewer":if identity["scope"] == "plan" {"not_required"} else {"pending"}});
    let mut requests = vec![];
    let mut conflicts = vec![];
    for role in required {
        let name = role.as_str().unwrap();
        let record = records.iter().find(|r| {
            r["identity"]["role"] == *role
                && (identity["scope"] != "plan" || r["identity"]["scope"] == "plan")
                && r["identity"]["snapshot"] == identity["snapshot"]
                && r["identity"]["policy"] == identity["policy"]
                && r["identity"]["plan_id"] == identity["plan_id"]
                && r["identity"]["revision"] == identity["revision"]
                && r["identity"]["stage_id"] == identity["stage_id"]
                && r["identity"]["attempt_id"] == identity["attempt_id"]
                && r["identity"]["round"] == identity["round"]
        });
        roles[name] = json!(match record {
            Some(r) if r["approved"] == true && Ctx::review_requests(r).is_empty() => "approved",
            Some(_) => "changes_requested",
            None => "pending",
        });
        if let Some(r) = record {
            for text in Ctx::review_requests(r) {
                requests.push(json!({"role":name,"text":text}));
            }
            if let Ok(Some(text)) = crate::constraint_conflict::verdict_field(r) {
                conflicts.push(json!({"role":name,"text":text}));
            }
        }
    }
    for role in identity["policy"]["deferred_roles"].as_array().into_iter().flatten() {
        roles[role.as_str().unwrap()] = json!("deferred");
    }
    let approved = required
        .iter()
        .all(|r| roles[r.as_str().unwrap()] == "approved");
    let mut gate = json!({"identity":identity,"policy":identity["policy"],"roles":roles,"status":if required.is_empty() {"deferred"} else if approved {"approved"} else {"blocked"},"requests":requests});
    // Reported constraint conflicts stay on the gate for the planner hand-back.
    if !conflicts.is_empty() {
        gate["constraint_conflicts"] = json!(conflicts);
    }
    gate
}

