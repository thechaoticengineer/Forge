use super::*;
use crate::app::PlanMode;
use crate::test_support::{QueueTest, editable_stage};
use serde_json::{Value, json};

#[test]
fn changing_validation_errors_share_three_corrections_and_can_succeed_on_the_last() {
    for succeed in [false, true] {
        let f = QueueTest::new(false);
        let mut calls = Vec::new();
        let result = f.app.repair_response("test operation", 0,
            |n| if succeed && *n == 3 { Ok(*n) } else { Err(format!("bad field {n}")) },
            |n, error| { calls.push(error.to_string()); Ok(n + 1) });
        assert_eq!(calls, ["bad field 0", "bad field 1", "bad field 2"]);
        if succeed { assert_eq!(result.unwrap(), (3, 3)); }
        else { assert_eq!(result.unwrap_err(), "bad field 3"); }
    }
}

#[test]
fn valid_responses_provider_failures_and_stop_never_trigger_extra_calls() {
    let f = QueueTest::new(false);
    let result = f.app.repair_response("valid", 7, |n| Ok(*n), |_, _| -> Result<i32, String> { panic!("unneeded call") });
    assert_eq!(result.unwrap(), (7, 7));
    let mut calls = 0;
    let result = f.app.repair_response("failed provider", 0, |_| Err::<(), _>("bad JSON".into()), |_, _| {
        calls += 1;
        Err("provider unavailable".into())
    });
    assert_eq!(result.unwrap_err(), "provider unavailable");
    assert_eq!(calls, 1);
    let result = f.app.repair_response("cancel", 0, |_| Err::<(), _>("bad JSON".into()), |_, _| {
        f.app.session.stop_requested.store(true, Ordering::SeqCst);
        Ok(1)
    });
    assert_eq!(result.unwrap_err(), "cancel stopped");
    f.app.repair_response("cancel", 0, |_| Ok(()), |_, _| -> Result<i32, String> { panic!("call after stop") }).unwrap_err();
}

fn candidate(f: &QueueTest) -> Value {
    let option = f.app.routing_options().unwrap().into_iter().find(|o| o["eligible"] == true).unwrap();
    let mut stage = editable_stage(3);
    stage["model_proposal"] = json!({"provider":option["provider"],"model":option["model"],
        "native_effort":option["effort"],"risk":"standard","complexity":"standard",
        "task":"functionality","rationale":"Adequate for the requested stage"});
    json!({"stages":[stage]})
}

#[test]
fn malformed_json_then_stage_then_nested_proposal_are_repaired_before_architect() {
    let f = QueueTest::new(false);
    let valid = candidate(&f);
    let mut bad_stage = valid.clone();
    bad_stage["stages"][0].as_object_mut().unwrap().remove("title");
    let mut bad_proposal = valid.clone();
    bad_proposal["stages"][0]["model_proposal"]["rounds"] = json!(0);
    bad_proposal["stages"][0]["model_proposal"]["status"] = json!("pending");
    {
        let mut settings = f.app.app.settings.lock().unwrap();
        settings["mock_plan_output"] = json!(["{broken",bad_stage,bad_proposal,valid]);
        settings["mock_usage"] = json!({"input":2,"output":1,"total":3,"model":"fixture"});
    }
    f.app.acquire_busy().unwrap();
    f.app.plan_worker("Goal", &PlanMode::Standard);
    let plan = f.app.load_plan().unwrap();
    assert_eq!(f.app.session.state.lock().unwrap().phase, "plan_ready");
    assert!(plan["stages"][0]["model_proposal"].get("rounds").is_none());
    assert_eq!(plan["planner_usage"]["mock"]["calls"], 4);
    assert_eq!(plan["planner_usage"]["mock"]["total_tokens"], 12);
    let settings = f.app.app.settings.lock().unwrap();
    assert_eq!(settings["mock_architect_requests"].as_array().unwrap().len(), 1);
    let calls: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
    assert_eq!(calls.len(), 4);
    assert!(calls[3]["prompt"].as_str().unwrap().contains("unknown field `rounds`"));
}

#[test]
fn exhausted_candidate_never_calls_architect_or_replaces_saved_plan() {
    let f = QueueTest::new(false);
    f.app.save_plan(&json!({"goal":"Saved","status":"draft","stages":[editable_stage(1)]})).unwrap();
    let saved = std::fs::read(f.app.forge_path("plan.json")).unwrap();
    let mut bad = candidate(&f);
    bad["stages"][0]["model_proposal"]["rounds"] = json!(0);
    f.app.app.settings.lock().unwrap()["mock_plan_output"] = bad;
    f.app.acquire_busy().unwrap();
    f.app.plan_worker("Replacement", &PlanMode::Standard);
    assert_eq!(std::fs::read(f.app.forge_path("plan.json")).unwrap(), saved);
    assert!(!f.app.session.busy.load(Ordering::SeqCst));
    let settings = f.app.app.settings.lock().unwrap();
    assert!(settings["mock_architect_requests"].is_null());
    assert_eq!(settings["mock_agent_requests"].as_array().unwrap().len(), 4);
}

#[test]
fn chat_and_enhancement_use_the_same_parser_and_field_correction_budget() {
    let f = QueueTest::new(false);
    f.app.app.settings.lock().unwrap()["mock_chat_output"] = json!(["{broken",{"answer":false},{"answer":"Corrected answer"}]);
    f.app.acquire_busy().unwrap();
    f.app.chat_worker(&json!({"stages":[]}), "Question");
    assert_eq!(f.app.read_chat()[1]["text"], "Corrected answer");
    f.app.app.settings.lock().unwrap()["mock_enhance_output"] = json!(["{broken",{"goal":""},{"goal":"Corrected goal"}]);
    f.app.session.state.lock().unwrap().goal_enhancement_serial = 1;
    f.app.acquire_busy().unwrap();
    f.app.enhance_goal_worker("Goal", 1);
    assert_eq!(f.app.session.state.lock().unwrap().goal_enhancement["goal"], "Corrected goal");
}

#[test]
fn duplicate_response_fields_are_never_silently_resolved() {
    for response in [r#"{"approved":false,"approved":true}"#,
        r#"{"stages":[{"model_proposal":{"task":"persistence","task":"documentation"}}]}"#,
        r#"{"answer":"first","\u0061nswer":"second"}"#] {
        assert!(parse_json::<Value>(response).unwrap_err().contains("duplicate response key"));
    }
}

#[test]
fn duplicate_planner_fields_are_corrected_before_any_architect_turn() {
    let f = QueueTest::new(false);
    let valid = candidate(&f);
    let duplicate = valid.to_string().replacen("\"title\":", "\"title\":\"conflicting stage\",\"title\":", 1);
    f.app.app.settings.lock().unwrap()["mock_plan_output"] = json!([duplicate, valid]);
    f.app.acquire_busy().unwrap();
    f.app.plan_worker("Goal", &PlanMode::Standard);
    assert!(f.app.load_plan().is_some());
    let settings = f.app.app.settings.lock().unwrap();
    assert_eq!(settings["mock_architect_requests"].as_array().unwrap().len(), 1);
    let calls: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
    assert_eq!(calls.len(), 2);
    assert!(calls[1]["prompt"].as_str().unwrap().contains("duplicate response key `title`"));
}

#[test]
fn planning_tier_errors_use_all_three_corrections_before_architecture() {
    for repaired in [false, true] {
        let f = QueueTest::new(false);
        let mut valid = candidate(&f);
        valid["stages"][0]["instructions"] = json!("Persist drafts with atomic writes");
        valid["stages"][0]["model_proposal"] = json!({"risk":"standard","complexity":"standard",
            "task":"persistence","tier":"strong","rationale":"Durable drafts need the strong tier."});
        let mut invalid = valid.clone();
        invalid["stages"][0]["model_proposal"]["tier"] = json!("standard");
        f.app.app.settings.lock().unwrap()["mock_plan_output"] = json!([
            invalid, invalid, invalid, if repaired { valid } else { invalid },
        ]);
        f.app.acquire_busy().unwrap();
        f.app.plan_worker("Goal", &PlanMode::Standard);
        assert_eq!(f.app.load_plan().is_some(), repaired);
        let settings = f.app.app.settings.lock().unwrap();
        let calls: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
        assert_eq!(calls.len(), 4);
        assert!(calls[3]["prompt"].as_str().unwrap().contains("capability floor 3"));
        assert_eq!(settings["mock_architect_requests"].as_array().map_or(0, Vec::len), usize::from(repaired));
    }
}

#[test]
fn chat_and_enhancement_repair_duplicates_and_unknown_fields_before_publishing() {
    let f = QueueTest::new(false);
    f.app.app.settings.lock().unwrap()["mock_chat_output"] = json!([
        r#"{"answer":"first","answer":"second"}"#, {"answer":"answer","approved":true}, {"answer":"Corrected answer"},
    ]);
    f.app.acquire_busy().unwrap();
    f.app.chat_worker(&json!({"stages":[]}), "Question");
    assert_eq!(f.app.read_chat()[1]["text"], "Corrected answer");
    f.app.app.settings.lock().unwrap()["mock_enhance_output"] = json!([
        r#"{"goal":"first","goal":"second"}"#, {"goal":"goal","status":"done"}, {"goal":"Corrected goal"},
    ]);
    f.app.session.state.lock().unwrap().goal_enhancement_serial = 1;
    f.app.acquire_busy().unwrap();
    f.app.enhance_goal_worker("Goal", 1);
    assert_eq!(f.app.session.state.lock().unwrap().goal_enhancement["goal"], "Corrected goal");
    let settings = f.app.app.settings.lock().unwrap();
    for role in ["chat", "enhance"] {
        assert_eq!(settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == role).count(), 3);
    }
}
