//! Durable, self-contained run summaries, without copying dialogue or diff inputs.
use crate::util::fmt_duration;
use serde_json::{Value, json};

pub(crate) fn stage_outcome(stage: &Value) -> Value {
    let a = &stage["model_agreement"];
    let p = &a["policy_inputs"];
    let calls = stage["model_invocations"].as_array();
    json!({"id":stage["id"], "title":stage["title"], "status":stage["status"],
        "attempt_id":stage["attempt_id"], "rounds":stage["rounds"], "sha":stage["sha"],
        "model_agreement": if a.is_object() { json!({
            "id":a["id"], "valid":a["valid"], "validated_proposal":a["validated_proposal"], "effective":a["effective"],
            "availability":a["availability"], "verification_state":a["verification_state"],
            "planner_reason":a["planner_reason"], "architect_reason":a["architect_reason"],
            "trigger":a["trigger"], "superseded_agreement":a["superseded_agreement"],
            "policy_inputs": {"policy":p["policy"], "tier":p["tier"], "tier_provenance":p["tier_provenance"],
                "relative_cost_preference":p["relative_cost_preference"], "pricing":p["pricing"],
                "billing_basis":p["billing_basis"], "constraint":p["constraint"]}
        }) } else { Value::Null },
        "last_invocation":calls.and_then(|c| c.last()), "invocation_count":calls.map_or(0, Vec::len),
        "review_policy":stage["review_policy"],
        "review_gate":{"status":stage["review_gate"]["status"], "roles":stage["review_gate"]["roles"],
            "identity":stage["review_gate"]["identity"]},
        "reassessment":{"count":stage["reassessment"]["count"],
            "operational_retries":stage["reassessment"]["operational_retries"]}, "usage":stage["usage"]})
}

/// Assemble presentation from the completed plan and already-loaded authoritative summary.
pub(crate) fn completed_run_report(
    plan: &Value,
    project: &str,
    count: usize,
    started: i64,
    now: i64,
    architecture: Value,
) -> Value {
    let duration_secs = if started > 0 { (now - started).max(0) } else { 0 };
    let commits: Vec<Value> = plan["stages"].as_array().unwrap().iter()
        .filter_map(|stage| stage.get("sha").map(|sha| json!({
            "sha": sha, "message": stage["commit"].as_str().unwrap_or("forge: stage"),
            "title": stage["title"],
        })))
        .collect();
    let mut report = json!({
        "unix": now, "goal": plan["goal"], "duration_secs": duration_secs,
        "stages": count, "commits": commits,
    });
    report["version"] = json!(1);
    report["project"] = json!(project);
    report["plan_id"] = plan["plan_id"].clone();
    report["revision"] = plan["revision"].clone();
    report["architecture"] = architecture;
    report["stage_outcomes"] = json!(plan["stages"].as_array().unwrap().iter()
        .map(stage_outcome).collect::<Vec<_>>());
    for key in ["usage", "planner_usage", "role_usage"] {
        if let Some(usage) = plan.get(key) {
            report[key] = usage.clone();
        }
    }
    report
}

/// Format the completion event for an assembled run report.
pub(crate) fn completion_message(report: &Value) -> String {
    let commits = report["commits"].as_array().unwrap();
    let duration = fmt_duration(report["duration_secs"].as_i64().unwrap());
    let mut text = format!("all stages committed — run complete in {duration}");
    if let Some(usage) = report["usage"].as_object().filter(|usage| !usage.is_empty()) {
        let tokens = usage.iter().map(|(tool, usage)| {
            format!("{tool}: {} tokens", usage["total_tokens"].as_i64().unwrap_or(0))
        }).collect::<Vec<_>>().join(", ");
        let noun = if commits.len() == 1 { "commit" } else { "commits" };
        text.push_str(&format!(" — {} {noun} — {tokens}", commits.len()));
    }
    text
}
