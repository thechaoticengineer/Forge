use crate::plan::edit_plan;
use crate::test_support::{QueueTest, api_request, editable_stage, seed_mock_plan, wait_for_worker};
use serde_json::json;
use std::fs;

#[test]
fn architecture_reconciliation_retains_unaffected_agreements_and_history() {
    let test = QueueTest::new(false);
    let mut first = editable_stage(1); first["depends_on"] = json!([]);
    first["status"] = json!("committed"); first["custom"] = json!({"keep": true});
    let mut second = editable_stage(2); second["depends_on"] = json!([1]);
    second["reviews"] = json!([{"approved": true, "round": 1}]);
    second["last_verdict"] = json!({"approved": true}); second["last_verdict_valid"] = json!(true);
    second["usage"] = json!({"codex": {"total_tokens": 12}});
    let mut third = editable_stage(3); third["depends_on"] = json!([]);
    let mut fourth = editable_stage(4); fourth["depends_on"] = json!([2]);
    test.app.save_plan(&json!({"goal": "Goal", "status": "draft", "stages": [first, second, third, fourth]})).unwrap();
    let mut original = test.app.load_plan().unwrap();
    let store = test.app.architecture_store();
    let mut cp = store.checkpoint(&original).unwrap();
    cp["session"] = json!({"provider": "codex", "reference": "exact-session",
        "checkpoint_reference": "exact-turn", "resume_policy": "fork_from_checkpoint"});
    cp["context_status"] = json!("ready");
    for id in [1, 2, 3, 4] {
        cp["agreements"][id.to_string()] = crate::architecture::agreement_fixture(&original, id as usize - 1);
        cp["guidance"][id.to_string()] = json!({"version": 1, "id": format!("guidance-{id}"),
            "stage_id": id, "revision": 1, "relevant_inputs": crate::plan::stage_inputs(&original, id as usize - 1),
            "text": "Keep the contract", "unix": 10, "valid": true});
    }
    original = store.publish(original, cp, json!({"kind": "seed_context"})).unwrap();
    let mut body = original.clone(); body["stages"][2]["instructions"] = json!("Unrelated edit");
    let edited = edit_plan(&original, &json!({"plan": body})).unwrap();
    test.app.save_plan(&edited).unwrap();
    let published = test.app.load_plan().unwrap(); let cp = store.checkpoint(&published).unwrap();
    assert_eq!(published["plan_id"], original["plan_id"]); assert_eq!(published["revision"], 2);
    assert_eq!(published["stages"][0], original["stages"][0]);
    assert_eq!(published["stages"][1], original["stages"][1]);
    assert_eq!(cp["agreements"]["2"]["valid"], true); assert_eq!(cp["agreements"]["4"]["valid"], true);
    assert_eq!(cp["agreements"]["3"]["valid"], false);
    assert_eq!(cp["context_status"], "needs_recovery");
    assert_eq!(cp["session"]["checkpoint_reference"], "exact-turn");
    let mut body = published.clone(); body["stages"][1]["instructions"] = json!("Dependency edit");
    let edited = edit_plan(&published, &json!({"plan": body})).unwrap(); test.app.save_plan(&edited).unwrap();
    let changed = test.app.load_plan().unwrap(); let cp = store.checkpoint(&changed).unwrap();
    assert_eq!(cp["agreements"]["2"]["valid"], false); assert_eq!(cp["agreements"]["4"]["valid"], false);
    for key in ["reviews", "last_verdict", "usage"] { assert_eq!(changed["stages"][1][key], original["stages"][1][key]); }
    assert_eq!(changed["stages"][1]["last_verdict_valid"], false);
    let mut body = changed.clone(); body["goal"] = json!("New goal");
    let edited = edit_plan(&changed, &json!({"plan": body})).unwrap();
    assert_eq!(edited["stages"][0], original["stages"][0]);
    assert_eq!(crate::plan::affected_stages(&changed, &edited).unwrap(), json!([1, 2, 3, 4]).as_array().unwrap().clone());
}

#[test]
fn architecture_stage_ids_survive_reorder_removal_and_do_not_recycle() {
    let mut stages = vec![editable_stage(1), editable_stage(2), editable_stage(3)];
    for s in &mut stages { s["depends_on"] = json!([]); s["model_agreement"] = json!({"valid": true}); }
    let old = json!({"goal": "Goal", "revision": 1, "stages": stages});
    let reordered = edit_plan(&old, &json!({"plan": {"stages": [old["stages"][2], old["stages"][0]]}})).unwrap();
    assert_eq!(reordered["stages"][0], old["stages"][2]);
    assert_eq!(crate::plan::affected_stages(&old, &reordered).unwrap(), vec![json!(2)]);
    let added = edit_plan(&reordered, &json!({"plan": {"stages": [reordered["stages"][0], editable_stage(2)]}})).unwrap();
    assert_eq!(added["stages"][1]["id"], 4);
    let mut implicit = old.clone();
    for stage in implicit["stages"].as_array_mut().unwrap() { stage.as_object_mut().unwrap().remove("depends_on"); }
    let reordered = edit_plan(&implicit, &json!({"plan": {"stages": [implicit["stages"][2], implicit["stages"][0], implicit["stages"][1]]}})).unwrap();
    for stage in reordered["stages"].as_array().unwrap() { assert_eq!(stage["model_agreement"]["valid"], false); }
}

#[test]
fn architecture_state_is_legacy_safe_bounded_and_reports_corruption() {
    let test = QueueTest::new(false);
    test.app.ensure_forge_dir();
    fs::write(test.app.forge_path("plan.json"), json!({"goal": "legacy", "stages": [editable_stage(1)]}).to_string()).unwrap();
    let (_, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(state["architecture"]["context_status"], "legacy");
    assert!(!test.app.forge_path("architecture").exists());
    let p = test.app.load_plan().unwrap(); test.app.save_plan(&p).unwrap();
    let (_, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(state["architecture"]["revision"], 1);
    assert_eq!(state["architecture"]["context_status"], "inactive");
    assert!(state["architecture"].to_string().len() < 1000);
    let (code, history) = api_request(&test.app.app, "GET", "/api/architecture/history?limit=1", json!({}));
    assert_eq!(code, 200); assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert_eq!(api_request(&test.app.app, "GET", "/api/architecture/history?cursor=bad", json!({})).0, 400);
    assert_eq!(api_request(&test.app.app, "GET", "/api/architecture/history?plan_id=../escape", json!({})).0, 400);
    fs::write(test.app.forge_path("plan.json"), b"{").unwrap();
    let (_, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert!(state["plan"].is_null()); assert_eq!(state["architecture"]["context_status"], "error");
}

#[test]
fn architecture_history_and_reviews_route_current_and_archived_projects() {
    let first = QueueTest::new(false); let second = QueueTest::new(false);
    let engine = &first.app.app;
    for ctx in [&first.app, &second.app] {
        ctx.save_plan(&json!({"goal": ctx.project(), "stages": [{"id": 1, "title": "one",
            "reviews": [{"summary": ctx.project()}]}]})).unwrap();
    }
    let first_plan = first.app.load_plan().unwrap();
    let second_plan = second.app.load_plan().unwrap();
    for ctx in [&first.app, &second.app] {
        let id = ctx.load_plan().unwrap()["plan_id"].as_str().unwrap().to_owned();
        for archived in [false, true] {
            if archived { ctx.architecture_store().reset().unwrap(); }
            let path = format!("/api/architecture/history?project={}&plan_id={id}&limit=1", ctx.project());
            let (code, page) = api_request(engine, "GET", &path, json!({}));
            assert_eq!(code, 200, "{page}"); assert_eq!(page["items"][0]["plan_id"], id);
            let path = format!("/api/architecture/reviews?project={}&plan_id={id}&stage_id=1&limit=1", ctx.project());
            let (code, page) = api_request(engine, "GET", &path, json!({}));
            assert_eq!(code, 200, "{page}"); assert_eq!(page["items"][0]["summary"], ctx.project());
            assert_eq!(page["plan_id"], id);
        }
    }
    assert_ne!(first_plan["plan_id"], second_plan["plan_id"]);
    assert_eq!(*engine.active_project.lock().unwrap(), first.app.project());
    for endpoint in ["history", "reviews"] {
        assert_eq!(api_request(engine, "GET", &format!("/api/architecture/{endpoint}?project=%XX"), json!({})).0, 400);
        assert_eq!(api_request(engine, "GET", &format!("/api/architecture/{endpoint}?project=/not-a-forge-test-repo"), json!({})).0, 400);
        let path = format!("/api/architecture/{endpoint}?project={}&plan_id={}&stage_id=1", second.app.project(), first_plan["plan_id"].as_str().unwrap());
        assert_eq!(api_request(engine, "GET", &path, json!({})).0, 400);
    }
}

#[test]
fn state_bounds_legacy_and_indexed_reviews_and_pages_the_full_records() {
    let test = QueueTest::new(false); test.app.ensure_forge_dir();
    let reviews: Vec<_> = (0..10_000).map(|i| json!({"round": i, "summary": "retained legacy review", "extra": i})).collect();
    let legacy = json!({"goal": "legacy", "stages": [{"id": 1, "reviews": reviews}]});
    fs::write(test.app.forge_path("plan.json"), legacy.to_string()).unwrap();
    for indexed in [false, true] {
        if indexed { test.app.save_plan(&legacy).unwrap(); }
        for _ in 0..2 {
            let (code, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
            assert_eq!(code, 200); assert!(state.to_string().len() < 15_000);
            assert_eq!(state["plan"]["stages"][0]["reviews"].as_array().unwrap().len(), 8);
            assert_eq!(state["plan"]["stages"][0]["review_count"], 10_000);
            assert_eq!(state["plan"]["stages"][0]["reviews_truncated"], true);
        }
        assert_eq!(test.app.session.legacy_state_cache.lock().unwrap().is_some(), !indexed);
        let (code, page) = api_request(&test.app.app, "GET", "/api/architecture/reviews?stage_id=1&cursor=9998&limit=1", json!({}));
        assert_eq!(code, 200, "{page}"); assert_eq!(page["items"], json!([reviews[9998]]));
        assert_eq!(page["next_cursor"], 9999);
        assert_eq!(test.app.forge_path("architecture").exists(), indexed);
    }
    assert_eq!(test.app.load_plan().unwrap()["stages"][0]["reviews"], json!(reviews));
}

#[test]
fn reviewer_prompts_exclude_obsolete_inputs_but_keep_fresh_round_feedback() {
    for field in ["context_valid", "last_verdict_valid"] {
        let test = QueueTest::new(false); seed_mock_plan(&test.app);
        let mut original = test.app.load_plan().unwrap();
        let old = json!({"approved": false, "summary": "OAuth design", "issues": ["Add OAuth middleware for the old goal"]});
        original["stages"][0]["last_verdict"] = old.clone();
        original["stages"][0]["reviews"] = json!([old]);
        test.app.save_plan(&original).unwrap();
        let original = test.app.load_plan().unwrap();
        let mut body = original.clone(); body["stages"][0]["instructions"] = json!("Remove OAuth; use local login");
        let mut edited = edit_plan(&original, &json!({"plan": body})).unwrap();
        // Either explicit invalidation flag is sufficient, including legacy mixed records.
        edited["stages"][0].as_object_mut().unwrap().remove(if field == "context_valid" { "last_verdict_valid" } else { "context_valid" });
        test.app.save_plan(&edited).unwrap();
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["max_fix_rounds"] = json!(1);
            settings["mock_reviewer_prompts"] = json!([]);
            settings["mock_verdicts"] = json!([
                {"approved": false, "issues": ["Validate the new local login"]},
                {"approved": true, "issues": []}, {"approved": true, "issues": []}]);
        }
        test.app.run_worker();
        let settings = test.app.app.settings.lock().unwrap();
        let prompts = settings["mock_reviewer_prompts"].as_array().unwrap();
        assert!(prompts.len() >= 2);
        for prompt in prompts { assert!(!prompt.as_str().unwrap().contains("Add OAuth middleware for the old goal")); }
        assert!(prompts[0].as_str().unwrap().contains("No previous review findings"));
        assert!(prompts[1].as_str().unwrap().contains("Validate the new local login"));
        let saved = test.app.load_plan().unwrap();
        assert_eq!(saved["stages"][0]["reviews"][0], old);
        assert_eq!(saved["stages"][0]["context_valid"], true);
    }
}

#[test]
fn architecture_serializes_project_writes_and_queue_goals_get_new_identities() {
    let test = QueueTest::new(false); test.start();
    let first = test.app.load_plan().unwrap();
    let mut threads = Vec::new();
    for _ in 0..8 {
        let ctx = test.app.clone(); let mut p = first.clone(); p["status"] = json!("approved");
        threads.push(std::thread::spawn(move || ctx.save_plan(&p).unwrap()));
    }
    for thread in threads { thread.join().unwrap(); }
    assert_eq!(test.app.load_plan().unwrap()["plan_id"], first["plan_id"]);
    assert_eq!(api_request(&test.app.app, "POST", "/api/approve", json!({})).0, 200);
    wait_for_worker(&test.app);
    let second = test.app.load_plan().unwrap();
    assert_ne!(first["plan_id"], second["plan_id"]); assert_eq!(second["revision"], 1);
    let store = test.app.architecture_store();
    let history = store.history(first["plan_id"].as_str(), 0, 100).unwrap();
    assert!(history["items"].as_array().unwrap().iter().all(|e| e["plan_id"] == first["plan_id"]));
    assert!(history["items"].as_array().unwrap().iter().any(|e| e["payload"]["reviews"].as_array().is_some_and(|r| !r.is_empty())));
    let other = QueueTest::new(false); other.app.save_plan(&second).unwrap();
    assert_ne!(other.app.load_plan().unwrap()["plan_id"], second["plan_id"]);
    assert!(other.app.architecture_store().history(first["plan_id"].as_str(), 0, 10).is_err());
}

#[test]
fn legacy_execution_imports_identity_before_recording_attempt_reviews() {
    let test = QueueTest::new(false);
    test.app.ensure_forge_dir();
    test.app.mock_agent("planner").unwrap();
    let legacy = fs::read(test.app.forge_path("plan-candidate.json")).unwrap();
    fs::write(test.app.forge_path("plan.json"), legacy).unwrap();
    test.app.run_worker();
    let plan = test.app.load_plan().unwrap();
    assert_eq!(plan["revision"], 1);
    for stage in plan["stages"].as_array().unwrap() {
        assert!(stage["attempt_id"].is_string());
        for review in stage["reviews"].as_array().unwrap() { assert_eq!(review["revision"], 1); }
    }
}

#[test]
fn revision_usage_adds_only_new_invocation_and_ignores_echoed_totals() {
    let test = QueueTest::new(false);
    let original = json!({"goal": "Goal", "stages": [editable_stage(1)],
        "planner_usage": {"mock": {"total_tokens": 10, "calls": 1, "models": {"model": 10}}}});
    test.app.save_plan(&original).unwrap();
    {
        let mut settings = test.app.app.settings.lock().unwrap();
        settings["mock_plan_output"] = original;
        settings["mock_plan_output"]["role_usage"] = json!({
            "planner": {"mock": {"total_tokens": 1000, "calls": 100}},
            "echoed": {"mock": {"total_tokens": 1000}},
        });
        settings["mock_usage"] = json!({"total": 5, "model": "model"});
    }
    let previous = test.app.load_plan().unwrap();
    test.app.revise_worker(&previous, "Keep the stage");
    let updated = test.app.load_plan().unwrap();
    assert_eq!(updated["planner_usage"]["mock"]["total_tokens"], 15);
    assert_eq!(updated["planner_usage"]["mock"]["calls"], 2);
    assert_eq!(updated["planner_usage"]["mock"]["models"]["model"], 15);
    let expected = json!({"mock": {"input_tokens": 0, "output_tokens": 0,
        "total_tokens": 15, "calls": 2, "models": {"model": 15}}});
    assert_eq!(updated["planner_usage"], expected);
    assert_eq!(updated["role_usage"]["planner"], expected);
    assert!(updated["role_usage"].get("echoed").is_none());
    // A subsequent empty invocation must not re-add the echoed previous totals.
    {
        let mut settings = test.app.app.settings.lock().unwrap();
        settings["mock_plan_output"] = updated.clone();
        settings["mock_usage"] = json!({"total": 0, "model": "model"});
    }
    test.app.revise_worker(&updated, "Keep the stage again");
    let revised = test.app.load_plan().unwrap();
    assert_eq!(revised["planner_usage"], expected);
    assert_eq!(revised["role_usage"]["planner"], expected);
}

#[test]
fn explicit_reviewer_context_gap_refreshes_only_affected_guidance_before_fix() {
    let test = QueueTest::new(true);
    seed_mock_plan(&test.app);
    test.app.app.settings.lock().unwrap()["mock_verdicts"] = json!([
        {"approved":false,"issues":["Clarify serialization compatibility"],
         "architecture_context_gap":"Which identifiers must remain stable?"},
        {"approved":true,"issues":[]},{"approved":true,"issues":[]}]);
    test.app.run_worker();
    assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
    let plan = test.app.load_plan().unwrap();
    let cp = test.app.architecture_store().checkpoint(&plan).unwrap();
    let settings = test.app.app.settings.lock().unwrap();
    let turns = settings["mock_architect_requests"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1]["session"], cp["session"]["reference"]);
    assert!(turns[1]["prompt"].as_str().unwrap().contains("Which identifiers must remain stable?"));
    assert!(turns[1]["prompt"].as_str().unwrap().contains("\"required_stage_ids\":[1]"));
    assert!(cp["context_gap"].is_null());
    assert!(settings["mock_fixer_prompts"][0].as_str().unwrap().contains("ARCHITECT GUIDANCE"));
}

#[test]
fn architect_guidance_spans_real_stage_loop_without_boundary_summary_turns() {
    let test = QueueTest::new(true);
    seed_mock_plan(&test.app);
    test.app.app.settings.lock().unwrap()["mock_reviewer_prompts"] = json!([]);
    test.app.run_worker();
    assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
    let plan = test.app.load_plan().unwrap();
    let cp = test.app.architecture_store().checkpoint(&plan).unwrap();
    let settings = test.app.app.settings.lock().unwrap();
    let turns = settings["mock_architect_requests"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert!(turns[0]["session"].is_null());
    assert_eq!(cp["execution_outcomes"].as_array().unwrap().len(), 2);
    assert_eq!(cp["guidance"].as_object().unwrap().len(), 2);
    assert!(settings["mock_reviewer_prompts"].as_array().unwrap().iter().all(|p|
        !p.as_str().unwrap().contains("ARCHITECT GUIDANCE")));
    let reference = cp["session"]["reference"].clone();
    drop(settings);
    test.app.run_worker();
    let completed = test.app.load_plan().unwrap();
    assert_eq!(test.app.architecture_store().checkpoint(&completed).unwrap()["session"]["reference"], reference);
    assert_eq!(test.app.app.settings.lock().unwrap()["mock_architect_requests"].as_array().unwrap().len(), 1);
}

#[test]
fn review_detail_snapshots_retrieve_large_feedback_and_pin_same_revision_publications() {
    let test = QueueTest::new(false);
    let records: Vec<_> = (0..13).map(|i| json!({
        "round": 1, "role": if i % 2 == 0 { "architect" } else { "reviewer" },
        "approved": false, "summary": format!("\r\n identical preview\n{}\nsummary END  \t", "界🙂".repeat(3000)),
        "issues": [format!("\nrequest {i}\r\n{}\nrequest END  ", "x".repeat(5000)), "second request\nlast  "],
        "notes": ["legacy note\r\nlast  "], "checks": ["check one\nlast  ", "check two"], "legacy_position": i
    })).collect();
    test.app.architecture_store().publish(json!({"goal":"complete reviews", "revision":1,
        "stages":[{"id":1,"title":"one","reviews":records}]}),
        crate::architecture::checkpoint_default(), json!({"kind":"legacy_import"})).unwrap();
    let published = test.app.load_plan().unwrap();
    let (_, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    let stage = &state["plan"]["stages"][0];
    assert_eq!(stage["review_count"],13);
    assert_eq!(stage["reviews"].as_array().unwrap().len(),8);
    assert!(stage["reviews"].as_array().unwrap().iter().all(|r| r["truncated"] == true && r.to_string().len() <= 4096));
    let checkpoint = published["architecture"]["checkpoint"].as_str().unwrap();
    let id = published["plan_id"].as_str().unwrap();
    let mut revised = published.clone();
    revised["stages"][0]["reviews"][12]["issues"][1] = json!("a different publication at the same revision");
    test.app.architecture_store().publish(revised.clone(),
        test.app.architecture_store().checkpoint(&published).unwrap(), json!({"kind":"fixture_review"})).unwrap();
    let newer = test.app.load_plan().unwrap();
    assert_eq!(published["revision"],newer["revision"]);
    assert_ne!(published["architecture"]["checkpoint"],newer["architecture"]["checkpoint"]);
    let before = fs::read(test.app.forge_path("plan.json")).unwrap();
    let mut cursor=0; let mut collected=Vec::new();
    loop {
        let path=format!("/api/architecture/reviews?project={}&plan_id={id}&checkpoint={checkpoint}&stage_id=1&cursor={cursor}&limit=3",test.app.project());
        let (code,page)=api_request(&test.app.app,"GET",&path,json!({}));
        assert_eq!(code,200,"{page}");
        assert_eq!(page["project"],test.app.project());
        assert_eq!(page["checkpoint"],checkpoint);
        assert_eq!(page["snapshot"],stage["review_snapshot"]);
        assert_eq!(page["revision"],published["revision"]);
        collected.extend(page["items"].as_array().unwrap().iter().cloned());
        if let Some(next)=page["next_cursor"].as_u64() { cursor=next; } else { break; }
    }
    assert_eq!(collected,records);
    assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(),before);
    let (_,current)=api_request(&test.app.app,"GET","/api/architecture/reviews?stage_id=1&cursor=12&limit=1",json!({}));
    assert_ne!(current["snapshot"],stage["review_snapshot"]);
    assert_eq!(current["items"][0]["issues"][1],revised["stages"][0]["reviews"][12]["issues"][1]);
}

#[test]
fn legacy_review_snapshots_detect_hidden_changes_and_last_verdict_only_is_readable() {
    let test=QueueTest::new(false); test.app.ensure_forge_dir();
    let record=json!({"summary":format!("same first line\n{}", "x".repeat(5000)),
        "issues":["same request", "hidden original"],"checks":["check\nlast  "]});
    let mut legacy=json!({"revision":3,"goal":"legacy","stages":[{"id":1,"reviews":vec![record.clone();10]}]});
    let path=test.app.forge_path("plan.json");
    fs::write(&path,legacy.to_string()).unwrap();
    let (_,first)=api_request(&test.app.app,"GET","/api/state",json!({}));
    let (_,cached)=api_request(&test.app.app,"GET","/api/state",json!({}));
    assert_eq!(cached["plan"]["stages"][0]["review_snapshot"],first["plan"]["stages"][0]["review_snapshot"]);
    let (_,page)=api_request(&test.app.app,"GET","/api/architecture/reviews?stage_id=1&cursor=2&limit=1",json!({}));
    assert!(page["checkpoint"].is_null());
    assert_eq!(page["snapshot"],first["plan"]["stages"][0]["review_snapshot"]);
    legacy["stages"][0]["reviews"][2]["issues"][1]=json!("hidden replacement");
    fs::write(&path,legacy.to_string()).unwrap();
    let (_,changed)=api_request(&test.app.app,"GET","/api/architecture/reviews?stage_id=1&cursor=3&limit=1",json!({}));
    assert_eq!(changed["count"],page["count"]);
    assert_eq!(changed["revision"],page["revision"]);
    assert_ne!(changed["snapshot"],page["snapshot"]);
    let (_,state)=api_request(&test.app.app,"GET","/api/state",json!({}));
    assert_eq!(state["plan"]["stages"][0]["review_snapshot"],changed["snapshot"]);
    assert_eq!(state["plan"]["stages"][0]["reviews"],first["plan"]["stages"][0]["reviews"]);
    legacy["stages"][0].as_object_mut().unwrap().remove("reviews");
    legacy["stages"][0]["last_verdict"]=record.clone();
    fs::write(&path,legacy.to_string()).unwrap();
    let before=fs::read(&path).unwrap();
    let (code,page)=api_request(&test.app.app,"GET","/api/architecture/reviews?stage_id=1&cursor=0&limit=8",json!({}));
    assert_eq!(code,200,"{page}"); assert_eq!(page["items"],json!([record]));
    assert_eq!(page["count"],1); assert!(page["next_cursor"].is_null());
    assert_eq!(fs::read(&path).unwrap(),before);
    assert!(!test.app.forge_path("architecture").exists());
}
