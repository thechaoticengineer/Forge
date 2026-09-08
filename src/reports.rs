//! Durable, self-contained run summaries, without copying dialogue or diff inputs.
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
