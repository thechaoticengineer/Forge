//! Pure preparation of planner candidates for architectural publication.
use crate::usage::merge_planner_revision_totals;
use serde_json::{Value, json};

/// The caller has checked that candidate stages are an array. Return the draft
/// and the stage count sampled after committed restoration, before reconciliation.
pub(crate) fn prepare_candidate_draft(mut plan: Value, goal: &str, previous: Option<&Value>)
    -> Result<(Value, usize), String>
{
    let stages = plan["stages"].as_array_mut().unwrap();
    sanitize_candidate_stages(stages, previous)?;
    restore_committed_stages(stages, previous);
    let n = stages.len();
    plan["goal"] = json!(goal);
    plan["status"] = json!("draft");
    let draft = if let Some(previous) = previous {
        prepare_revised_draft(plan, previous)?
    } else {
        prepare_initial_draft(plan, goal)?
    };
    Ok((draft, n))
}

fn sanitize_candidate_stages(stages: &mut [Value], previous: Option<&Value>) -> Result<(), String> {
    for stage in stages.iter_mut() {
        if !stage.is_object() {
            return Err("planner produced a stage that is not an object".into());
        }
        // Planner output cannot forge execution records or relax user constraints.
        let saved_constraint = previous.and_then(|p| p["stages"].as_array()).and_then(|stages| stages.iter().find(|s| s["id"] == stage["id"]))
            .map(|s| s["model_constraint"].clone()).unwrap_or(Value::Null);
        stage.as_object_mut().unwrap().retain(|key, _| ["id", "title", "instructions", "acceptance", "commit", "depends_on", "model_proposal"].contains(&key.as_str()));
        // An omitted acceptance or commit is a planner slip, not a defective
        // plan: both are already accepted empty from the panel's own edits.
        // Title and instructions stay required, so real gaps are still refused.
        for field in ["acceptance", "commit"] {
            if stage[field].is_null() { stage[field] = json!(""); }
        }
        if !saved_constraint.is_null() { stage["model_constraint"] = saved_constraint; }
        stage["status"] = json!("pending");
        stage["rounds"] = json!(0);
    }
    Ok(())
}

fn restore_committed_stages(stages: &mut Vec<Value>, previous: Option<&Value>) {
    let committed: Vec<Value> = previous.into_iter().flat_map(|p| p["stages"].as_array().unwrap())
        .filter(|s| s["status"] == "committed").cloned().collect();
    // Restore originals even if the agent edited, reordered, duplicated or dropped them.
    stages.retain(|stage| !committed.iter().any(|original| original["id"] == stage["id"]));
    stages.splice(0..0, committed.iter().cloned());
}

fn prepare_revised_draft(plan: Value, previous: &Value) -> Result<Value, String> {
    let mut edited = crate::plan::edit_plan(previous, &json!({"plan": plan})).map_err(str::to_owned)?;
    for (idx, stage) in edited["stages"].as_array_mut().unwrap().iter_mut().enumerate() {
        if stage["status"] != "committed" { stage["model_proposal"] = plan["stages"][idx]["model_proposal"].clone(); }
    }
    if let Some(usage) = plan.get("planner_usage") {
        merge_planner_revision_totals(&mut edited, usage);
    }
    if !edited["planner_usage"].is_null() {
        if !edited["role_usage"].is_object() { edited["role_usage"] = json!({}); }
        edited["role_usage"]["planner"] = edited["planner_usage"].clone();
    }
    edited["planner_selection_actor"] = plan["planner_selection_actor"].clone();
    Ok(edited)
}

fn prepare_initial_draft(mut plan: Value, goal: &str) -> Result<Value, String> {
    let base = json!({"stages": [], "goal": goal});
    let normalized = crate::plan::edit_plan(&base, &json!({"plan": plan})).map_err(str::to_owned)?;
    let proposals: Vec<Value> = plan["stages"].as_array().unwrap().iter().map(|s| s["model_proposal"].clone()).collect();
    plan["stages"] = normalized["stages"].clone();
    for (idx, proposal) in proposals.into_iter().enumerate() { plan["stages"][idx]["model_proposal"] = proposal; }
    plan["stage_id_high_water"] = normalized["stage_id_high_water"].clone();
    for key in ["plan_id", "revision", "architecture", "contract_version"] { plan.as_object_mut().unwrap().remove(key); }
    Ok(plan)
}

#[cfg(test)]
#[path = "candidate_draft_tests.rs"]
mod tests;
