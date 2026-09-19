use super::*;
use serde_json::json;

fn inputs() -> Value { json!({"instructions": "copied stage text", "depends_on": [1]}) }

fn fixture() -> Value {
    json!({"summary": "keep", "constraints": ["c"], "context_status": "ready",
        "guidance": {"1": {"id": "g1", "text": "guide one", "valid": true, "relevant_inputs": inputs()},
                     "2": {"id": "g2", "text": "guide two", "valid": false, "relevant_inputs": inputs()}},
        "agreements": {"1": {"id": "a1", "dialogue": ["keep"], "architectural_constraints": ["c"], "relevant_inputs": inputs()}},
        "recent_decisions": [{"id": "d", "relevant_inputs": inputs()}]})
}

#[test]
fn checkpoint_drops_only_the_fingerprint_of_guidance_and_agreements() {
    let original = fixture();
    let view = checkpoint(&original);
    assert_eq!(view["guidance"]["1"], json!({"id": "g1", "text": "guide one", "valid": true}));
    assert_eq!(view["guidance"]["2"], json!({"id": "g2", "text": "guide two", "valid": false}));
    assert_eq!(view["agreements"]["1"], json!({"id": "a1", "dialogue": ["keep"], "architectural_constraints": ["c"]}));
    for field in ["summary", "constraints", "context_status", "recent_decisions"] {
        assert_eq!(view[field], original[field], "{field}");
    }
    // The projection works on a clone.
    assert!(original["guidance"]["1"]["relevant_inputs"].is_object());
    assert!(original["agreements"]["1"]["relevant_inputs"].is_object());
}

#[test]
fn checkpoint_is_idempotent_and_tolerates_missing_groups() {
    let once = checkpoint(&fixture());
    assert_eq!(checkpoint(&once), once);
    assert_eq!(checkpoint(&json!({"summary": "only"})), json!({"summary": "only"}));
    assert_eq!(checkpoint(&Value::Null), Value::Null);
    assert_eq!(checkpoint(&json!({"guidance": null, "agreements": {"1": "text"}})),
        json!({"guidance": null, "agreements": {"1": "text"}}));
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
