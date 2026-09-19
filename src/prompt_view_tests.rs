use super::*;
use serde_json::json;

fn inputs() -> Value { json!({"instructions": "copied stage text", "depends_on": [1]}) }

fn checkpoint(cp: &Value) -> Value { checkpoint_for(cp, &Value::Null) }

fn model_agreement() -> Value {
    json!({"id": "a1", "kind": "agreement", "stage_id": 1, "valid": true, "agreed": true,
        "dialogue": ["keep"], "architectural_constraints": ["kept", "gone"], "relevant_inputs": inputs(),
        "policy_inputs": {"policy": "p"}, "provenance": {"x": 1}, "validated_proposal": {"model": "m"},
        "planner_bootstrap": {"b": 1}, "architect_bootstrap": {"b": 2}, "reviewer": {"status": "deferred"},
        "effective": {"provider": "claude", "model": "opus", "native_effort": "high", "extra": 1},
        "planner_reason": "planner", "architect_reason": "architect", "trigger": "joint_assignment"})
}

fn fixture() -> Value {
    json!({"summary": "keep", "constraints": ["kept", "new"], "context_status": "ready",
        "guidance": {"1": {"id": "g1", "text": "guide one", "valid": true, "relevant_inputs": inputs()},
                     "2": {"id": "g2", "text": "guide two", "valid": false, "relevant_inputs": inputs()}},
        "agreements": {"1": model_agreement()},
        "recent_decisions": [{"id": "d", "relevant_inputs": inputs()}]})
}

#[test]
fn constraint_ids_are_stable_short_hashes_of_the_text() {
    assert_eq!(constraint_id("keep it small"), constraint_id("keep it small"));
    assert_ne!(constraint_id("keep it small"), constraint_id("keep it smal"));
    let id = constraint_id("keep it small");
    assert!(id.starts_with("c-") && id.len() == 10 && id[2..].bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
}

#[test]
fn checkpoint_drops_the_fingerprint_and_names_constraints_in_order() {
    let original = fixture();
    let view = checkpoint(&original);
    assert_eq!(view["guidance"]["1"], json!({"id": "g1", "text": "guide one", "valid": true}));
    assert_eq!(view["guidance"]["2"], json!({"id": "g2", "text": "guide two", "valid": false}));
    assert_eq!(view["constraints"], original["constraints"]);
    assert_eq!(view["constraint_ids"], json!([constraint_id("kept"), constraint_id("new")]));
    for field in ["summary", "context_status", "recent_decisions"] {
        assert_eq!(view[field], original[field], "{field}");
    }
    // The projection works on a clone.
    assert_eq!(original, fixture());
    assert!(original["agreements"]["1"]["relevant_inputs"].is_object());
    assert!(original.get("constraint_ids").is_none());
}

#[test]
fn checkpoint_tolerates_missing_groups() {
    assert_eq!(checkpoint(&json!({"summary": "only"})), json!({"summary": "only"}));
    assert_eq!(checkpoint(&Value::Null), Value::Null);
    assert_eq!(checkpoint(&json!({"guidance": null, "agreements": {"1": "text"}})),
        json!({"guidance": null, "agreements": {"1": "text"}}));
}

#[test]
fn agreements_keep_only_compact_fields_and_split_constraint_refs() {
    let view = checkpoint(&fixture())["agreements"]["1"].clone();
    assert_eq!(view, json!({"id": "a1", "stage_id": 1, "valid": true,
        "effective": {"provider": "claude", "model": "opus", "native_effort": "high"},
        "planner_reason": "planner", "architect_reason": "architect", "trigger": "joint_assignment",
        "constraint_refs": [constraint_id("kept")], "retired_constraint_refs": [constraint_id("gone")]}));
    for dropped in ["dialogue", "policy_inputs", "provenance", "validated_proposal", "planner_bootstrap",
        "architect_bootstrap", "architectural_constraints", "relevant_inputs"] {
        assert!(view.get(dropped).is_none(), "{dropped}");
    }
}

#[test]
fn invalidated_and_tier_agreements_are_projected() {
    let mut record = model_agreement();
    record["valid"] = json!(false);
    record["invalidation_trigger"] = json!("stage_edit");
    record["effective"] = Value::Null;
    record["binding"] = json!("at_implementation_start");
    record["policy_inputs"] = json!({"tier": "standard", "minimum_tier": 2, "constraint": {"big": "payload"}});
    let view = agreement_view(&record, &json!(["kept"]));
    assert_eq!(view["invalidation_trigger"], "stage_edit");
    assert_eq!(view["valid"], false);
    assert_eq!(view["effective"], Value::Null);
    assert_eq!((&view["tier"], &view["minimum_tier"], &view["binding"]), (&json!("standard"), &json!(2), &json!("at_implementation_start")));
    assert!(view.get("policy_inputs").is_none());
    assert_eq!(agreement_view(&json!("text"), &json!([])), json!("text"));
}

#[test]
fn outcomes_replace_the_review_gate_with_a_summary() {
    let gate = json!({"status": "deferred", "requests": ["a", "b"], "roles": {"architect": "deferred"},
        "identity": {"snapshot": "big"}, "policy": {"rationale": "long"}});
    let cp = json!({"execution_outcomes": [{"stage_id": 1, "revision": 2, "status": "committed", "sha": "abc",
        "acceptance": "long acceptance", "review_policy": {"x": 1}, "review_gate": gate, "unix": 5}]});
    let plain = checkpoint(&cp)["execution_outcomes"][0].clone();
    assert_eq!(plain, json!({"stage_id": 1, "revision": 2, "status": "committed", "sha": "abc", "unix": 5,
        "review": {"status": "deferred", "requests": 2, "roles": {"architect": "deferred"}, "full_record": ".forge/plan.json"}}));
    let plan = json!({"plan_id": "p1", "architecture": {"review_history": {"1": {"file": "f1"}}}});
    let indexed = checkpoint_for(&cp, &plan)["execution_outcomes"][0]["review"]["full_record"].clone();
    assert_eq!(indexed, ".forge/architecture/p1/reviews/f1.jsonl");
    assert!(!checkpoint(&cp).to_string().contains("review_gate"));
    assert!(cp["execution_outcomes"][0]["review_gate"].is_object());
}

#[test]
fn committed_stages_keep_acceptance_only_for_direct_dependencies() {
    let plan = json!({"stages": [
        {"id": 1, "title": "one", "status": "committed", "sha": "s1", "acceptance": "ACCEPT-ONE"},
        {"id": 2, "title": "two", "status": "committed", "sha": "s2", "acceptance": "ACCEPT-TWO",
         "review_gate": {"status": "passed"}},
        {"id": 3, "title": "three", "status": "in_progress", "depends_on": [1]},
        {"id": 4, "title": "four", "status": "pending"}]});
    let cp = json!({"execution_outcomes": [{"stage_id": 1, "status": "committed", "review_gate": {"status": "deferred"}}]});
    let listed = committed_stages(&plan, &cp, &plan["stages"][2]);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["acceptance"], "ACCEPT-ONE");
    assert_eq!(listed[0]["outcome"]["review"]["status"], "deferred");
    assert_eq!(listed[1], json!({"id": 2, "title": "two", "sha": "s2",
        "outcome": {"status": "committed", "review": {"status": "passed", "full_record": ".forge/plan.json"}}}));
    // Without depends_on every earlier stage is a dependency.
    let all = committed_stages(&plan, &cp, &plan["stages"][3]);
    assert!(all.iter().all(|s| s.get("acceptance").is_some()));
}

#[test]
fn guidance_accepts_a_map_or_a_single_record() {
    let original = fixture();
    let map = guidance(&original["guidance"]);
    assert_eq!(map, checkpoint(&original)["guidance"]);
    let record = guidance(&original["guidance"]["1"]);
    assert_eq!(record, json!({"id": "g1", "text": "guide one", "valid": true}));
    for view in [&map, &record] { assert_eq!(guidance(view), *view); }
    assert_eq!(guidance(&json!({})), json!({}));
    assert_eq!(guidance(&Value::Null), Value::Null);
    assert!(original["guidance"]["1"]["relevant_inputs"].is_object());
}

#[test]
fn agreement_drops_the_fingerprint_of_one_record() {
    let record = json!({"id": "a", "relevant_inputs": inputs(), "valid": true});
    assert_eq!(agreement(&record), json!({"id": "a", "valid": true}));
    assert_eq!(agreement(&agreement(&record)), agreement(&record));
}
