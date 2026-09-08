use super::prepare_candidate_draft;
use crate::test_support::editable_stage;
use serde_json::{Value, json};

fn committed_plan() -> Value {
    let mut first = editable_stage(9);
    first.as_object_mut().unwrap().extend(json!({
        "status": "committed", "rounds": 3, "sha": "exact-sha",
        "started_unix": 10, "finished_unix": 20, "duration_secs": 10,
        "reviews": [{"approved": true}], "last_verdict": {"approved": true},
        "review_gate": {"status": "approved", "roles": {"architect": "approved", "reviewer": "approved"}},
        "model_constraint": {"provider": "codex", "model": "saved-model"},
        "model_agreement": {"valid": true}, "model_proposal": {"saved": true},
        "model_proposal_inputs": {"saved": true}, "model_proposer": {"saved": true},
        "usage": {"codex": {"total_tokens": 12}}, "unknown": {"keep": [null, 7]}
    }).as_object().unwrap().clone());
    let mut second = first.clone();
    second["id"] = json!(1);
    second["sha"] = Value::Null;
    second["depends_on"] = json!([9]);
    let mut pending = editable_stage(4);
    pending["status"] = json!("blocked");
    pending["rounds"] = json!(2);
    pending["unknown"] = json!("saved stage metadata");
    json!({"goal": "Goal", "status": "approved", "plan_id": "saved-plan", "revision": 7,
        "stage_id_high_water": 20, "architecture": {"checkpoint": "saved"},
        "contract_version": 1, "unknown": {"saved": true}, "stages": [first, second, pending]})
}

fn assert_committed_restoration(stages: Value) {
    let previous = committed_plan();
    let before = previous.clone();
    let candidate = json!({"stages": stages, "unknown": "injected", "plan_id": "injected"});
    let (draft, count) = prepare_candidate_draft(candidate, "Goal", Some(&previous)).unwrap();
    let mut expected = previous.clone();
    expected["status"] = json!("draft");
    expected["revision"] = json!(8);
    expected["planner_selection_actor"] = Value::Null;
    expected["stages"][2]["model_proposal"] = Value::Null;
    assert_eq!(draft, expected);
    assert_eq!(count, 3);
    assert_eq!(&draft["stages"].as_array().unwrap()[..2], &previous["stages"].as_array().unwrap()[..2]);
    assert_eq!(previous, before);
}

#[test]
fn dropped_committed_stages_are_restored_exactly() {
    assert_committed_restoration(json!([editable_stage(4)]));
}

#[test]
fn every_duplicate_committed_occurrence_is_removed_before_restoration() {
    assert_committed_restoration(json!([
        editable_stage(9), editable_stage(1), editable_stage(9), editable_stage(4),
        editable_stage(1), editable_stage(9)
    ]));
}

#[test]
fn reordered_committed_stages_regain_the_saved_prefix_order() {
    assert_committed_restoration(json!([editable_stage(4), editable_stage(1), editable_stage(9)]));
}

#[test]
fn modified_committed_objects_are_replaced_before_content_validation() {
    // These objects would fail content, constraint and dependency validation if retained.
    assert_committed_restoration(json!([
        {"id": 1, "title": false, "depends_on": "invalid", "sha": "forged"},
        editable_stage(4),
        {"id": 9, "instructions": null, "model_constraint": {}, "reviews": ["forged"]}
    ]));
}

fn inject_execution_records(stage: &mut Value) {
    for key in ["sha", "started_unix", "finished_unix", "duration_secs", "reviews", "review_count",
        "reviews_truncated", "last_verdict", "last_verdict_valid", "review_gate", "context_valid",
        "guidance", "model_agreement", "model_proposal_inputs", "model_proposer", "usage",
        "model_block", "reassessment", "unknown"] {
        stage[key] = json!({"forged": true});
    }
    stage["status"] = json!("committed");
    stage["rounds"] = json!(99);
    stage["model_constraint"] = json!({"model": "injected"});
}

fn new_draft_stage(id: i64, proposal: Value) -> Value {
    let mut stage = editable_stage(id);
    stage.as_object_mut().unwrap().extend(json!({
        "status": "pending", "rounds": 0, "context_valid": false,
        "last_verdict_valid": false, "review_gate": {"status": "obsolete"},
        "model_proposal": proposal
    }).as_object().unwrap().clone());
    stage
}

#[test]
fn initial_draft_strips_stage_execution_and_identity_but_keeps_top_level_handling() {
    let mut first = editable_stage(0);
    inject_execution_records(&mut first);
    first.as_object_mut().unwrap().remove("id");
    first["depends_on"] = json!([]);
    first["model_proposal"] = json!({"marker": "first"});
    let mut second = editable_stage(5);
    inject_execution_records(&mut second);
    let candidate = json!({"goal": "wrong", "status": "done", "stages": [first, second],
        "plan_id": "forged", "revision": 99, "architecture": {"forged": true}, "contract_version": 99,
        "stage_id_high_water": 999, "unknown": {"keep": true}, "reviews": ["top level preserved"],
        "planner_usage": {"codex": {"total_tokens": 5}}, "role_usage": {"planner": {"measured": true}},
        "planner_selection_actor": {"provider": "codex", "model": "actual", "native_effort": "high"}});
    let (draft, count) = prepare_candidate_draft(candidate, "Goal", None).unwrap();
    let mut first = new_draft_stage(1, json!({"marker": "first"}));
    first["depends_on"] = json!([]);
    assert_eq!(draft, json!({"goal": "Goal", "status": "draft", "stages": [first, new_draft_stage(5, Value::Null)],
        "stage_id_high_water": 5, "unknown": {"keep": true}, "reviews": ["top level preserved"],
        "planner_usage": {"codex": {"total_tokens": 5}}, "role_usage": {"planner": {"measured": true}},
        "planner_selection_actor": {"provider": "codex", "model": "actual", "native_effort": "high"}}));
    assert_eq!(count, 2);
}

#[test]
fn revised_draft_keeps_authoritative_records_constraints_and_positional_proposals() {
    let mut previous = committed_plan();
    previous["stages"][2]["model_constraint"] = json!({"provider": "codex", "model": "saved"});
    previous["stages"][2]["reviews"] = json!([{"saved": true}]);
    previous["stages"][2]["review_gate"] = json!({"status": "approved", "saved": true});
    previous["stages"][2]["sha"] = json!("saved-stage-sha");
    previous["planner_usage"] = json!({"codex": {"total_tokens": 10, "calls": 2, "models": {"old": 10}, "note": "keep"}});
    previous["role_usage"] = json!({"reviewer": {"saved": true}, "planner": "outdated"});
    let mut pending = editable_stage(4);
    inject_execution_records(&mut pending);
    pending["model_proposal"] = json!({"marker": "existing"});
    let mut new = editable_stage(2); // Retired ID below the saved high-water mark.
    inject_execution_records(&mut new);
    new["model_proposal"] = json!({"marker": "new"});
    new["depends_on"] = json!([4]);
    let candidate = json!({"stages": [pending, new], "role_usage": {"reviewer": "forged"},
        "planner_usage": {"codex": {"total_tokens": 5, "calls": 1, "models": {"new": 5}}},
        "planner_selection_actor": {"provider": "codex", "model": "actual"}, "unknown": "forged"});
    let (draft, count) = prepare_candidate_draft(candidate, "Goal", Some(&previous)).unwrap();
    let mut expected = previous.clone();
    expected["status"] = json!("draft");
    expected["revision"] = json!(8);
    expected["stage_id_high_water"] = json!(21);
    expected["stages"][2]["model_proposal"] = json!({"marker": "existing"});
    let mut new = new_draft_stage(21, json!({"marker": "new"}));
    new["depends_on"] = json!([4]);
    expected["stages"].as_array_mut().unwrap().push(new);
    expected["planner_usage"] = json!({"codex": {"total_tokens": 15, "calls": 3, "models": {"old": 10, "new": 5}, "note": "keep"}});
    expected["role_usage"]["planner"] = expected["planner_usage"].clone();
    expected["planner_selection_actor"] = json!({"provider": "codex", "model": "actual"});
    assert_eq!(draft, expected);
    assert_eq!(count, 4);
}

#[test]
fn revised_draft_reconciles_transitive_dependencies_and_retains_history() {
    let mut stages = vec![editable_stage(1), editable_stage(2), editable_stage(3)];
    for stage in &mut stages {
        stage.as_object_mut().unwrap().extend(json!({"status": "blocked", "rounds": 2,
            "sha": "old", "started_unix": 10, "finished_unix": 20, "duration_secs": 10,
            "model_block": {}, "reassessment": {}, "guidance": {"valid": true, "text": "keep"},
            "model_agreement": {"valid": true}, "reviews": [{"saved": true}], "last_verdict": {"saved": true}
        }).as_object().unwrap().clone());
    }
    stages[0]["depends_on"] = json!([]);
    stages[1]["depends_on"] = json!([1]);
    stages[2]["depends_on"] = json!([]);
    let previous = json!({"goal": "Goal", "revision": 3, "stages": stages});
    let mut candidate = previous.clone();
    candidate["stages"][0]["instructions"] = json!("Changed dependency");
    let (draft, count) = prepare_candidate_draft(candidate, "Goal", Some(&previous)).unwrap();
    let mut expected = previous.clone();
    expected["revision"] = json!(4);
    expected["status"] = json!("draft");
    expected["stage_id_high_water"] = json!(3);
    expected["planner_selection_actor"] = Value::Null;
    expected["stages"][0]["instructions"] = json!("Changed dependency");
    for idx in 0..3 {
        let stage = &mut expected["stages"][idx];
        stage["model_proposal"] = Value::Null;
        if idx == 2 { continue; }
        stage["status"] = json!("pending");
        stage["rounds"] = json!(0);
        stage["context_valid"] = json!(false);
        stage["last_verdict_valid"] = json!(false);
        stage["review_gate"] = json!({"status": "obsolete"});
        stage["guidance"]["valid"] = json!(false);
        stage["model_agreement"]["valid"] = json!(false);
        for key in ["sha", "started_unix", "finished_unix", "duration_secs", "model_block", "reassessment"] {
            stage.as_object_mut().unwrap().remove(key);
        }
    }
    assert_eq!(draft, expected);
    assert_eq!(count, 3);
}

#[test]
fn malformed_candidates_keep_stage_object_and_reconciliation_error_order() {
    let mut invalid_constraint = json!({"stages": [editable_stage(1)]});
    invalid_constraint["stages"][0]["model_constraint"] = json!({});
    let mut limit = json!({"stages": [], "stage_id_high_water": i64::MAX});
    for (stages, previous, error) in [
        (json!([{"id": 9, "title": false}, null]), Some(committed_plan()), "planner produced a stage that is not an object"),
        (json!([{"title": false}, 42]), None, "planner produced a stage that is not an object"),
        (json!([]), None, "stages must be a non-empty array"),
        (json!([{"id": 1, "title": " "}, editable_stage(1)]), None, "stage title and instructions must not be blank"),
        (json!([editable_stage(1), {"id": 1}]), None, "stage content fields must be strings"),
        (json!([editable_stage(1), editable_stage(1)]), None, "duplicate stage ids"),
        (json!([editable_stage(1)]), Some(invalid_constraint), "invalid stage model constraint"),
        (json!([{"id": 1, "title": "T", "instructions": "I", "acceptance": "", "commit": "", "depends_on": false}]), None, "depends_on must be an array"),
        (json!([{"id": 1, "title": "T", "instructions": "I", "acceptance": "", "commit": "", "depends_on": [2]}]), None, "dependencies must refer to earlier stages"),
        (json!([editable_stage(0)]), Some(limit.clone()), "stage id limit reached"),
    ] {
        assert_eq!(prepare_candidate_draft(json!({"stages": stages}), "Goal", previous.as_ref()), Err(error.into()));
    }
    limit["revision"] = json!(u64::MAX);
    assert_eq!(prepare_candidate_draft(json!({"stages": [editable_stage(i64::MAX)]}), "Goal", Some(&limit)), Err("stage id limit reached".into()));
    limit["stages"] = json!([editable_stage(i64::MAX)]);
    assert_eq!(prepare_candidate_draft(json!({"stages": [editable_stage(i64::MAX)]}), "Goal", Some(&limit)), Err("revision limit reached".into()));
}
