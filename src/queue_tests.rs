use crate::app::{Ctx, PlanMode};
use crate::plan::{default_settings, mutate_queue};
use crate::test_support::{QueueTest, api_request, seed_mock_plan, wait_for_worker};
use crate::util::{fmt_duration, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[test]
fn approval_and_queue_approval_reuse_actual_selection_calls() {
    let count = |ctx: &Ctx| {
        let s = ctx.app.settings.lock().unwrap();
        (s["mock_routing_planner_requests"].as_array().map_or(0, Vec::len), s["mock_routing_architect_requests"].as_array().map_or(0, Vec::len))
    };
    let test = QueueTest::new(false);
    test.start();
    assert_eq!(count(&test.app), (1,1));
    // User approval starts the first queue goal, then drafts the second goal.
    assert_eq!(api_request(&test.app.app,"POST","/api/approve",json!({})).0,200);
    wait_for_worker(&test.app);
    assert_eq!(count(&test.app), (2,2));
    let p = test.app.load_plan().unwrap();
    assert_eq!(p["status"],"draft");
    let solo = QueueTest::new(false);
    solo.app.plan_worker("one plan", &PlanMode::Standard);
    let before = count(&solo.app);
    assert_eq!(api_request(&solo.app.app,"POST","/api/approve",json!({})).0,200);
    assert_eq!(count(&solo.app),before);
    let p = solo.app.load_plan().unwrap();
    assert_eq!(p["status"],"approved");
    assert!(p["stages"].as_array().unwrap().iter().all(|s| s["model_agreement"]["agreed"] == true));
}

#[test]
fn queue_add_uses_max_id_and_records_goal_metadata() {
    let mut queue = json!({"items": []});
    let started = unix_timestamp();
    mutate_queue(&mut queue, "add", &json!({"goal": "  First goal\n"})).unwrap();
    mutate_queue(&mut queue, "add", &json!({"goal": "Second goal"})).unwrap();
    assert_eq!(queue["items"][0]["id"], 1);
    assert_eq!(queue["items"][1]["id"], 2);
    assert_eq!(queue["items"][0]["goal"], "First goal");
    assert_eq!(queue["items"][0]["status"], "queued");
    let added = queue["items"][0]["added_unix"].as_i64().unwrap();
    assert!((started..=unix_timestamp()).contains(&added));

    queue["items"][0]["id"] = json!(10);
    queue["items"][0]["status"] = json!("done");
    mutate_queue(&mut queue, "add", &json!({"goal": "Third goal"})).unwrap();
    assert_eq!(queue["items"][2]["id"], 11);
}

#[test]
fn queue_invalid_mutations_leave_items_unchanged() {
    let original = json!({"items": [
        {"id": 1, "status": "queued"},
        {"id": 2, "status": "running"},
        {"id": 3, "status": "done"},
    ]});
    for (action, body) in [
        ("add", json!({})),
        ("add", json!({"goal": ""})),
        ("add", json!({"goal": " \t\n\u{2003}"})),
        ("add", json!({"goal": 123})),
        ("remove", json!({})),
        ("remove", json!({"id": "1"})),
        ("remove", json!({"id": -1})),
        ("remove", json!({"id": 1.5})),
        ("remove", json!({"id": 99})),
        ("remove", json!({"id": 2})),
        ("remove", json!({"id": 3})),
        ("move", json!({"id": 1})),
        ("move", json!({"id": 1, "dir": "left"})),
        ("move", json!({"id": 2, "dir": "up"})),
        ("move", json!({"id": 3, "dir": "down"})),
        ("move", json!({"id": 99, "dir": "down"})),
    ] {
        let mut queue = original.clone();
        assert!(mutate_queue(&mut queue, action, &body).is_err());
        assert_eq!(queue, original);
    }
}

#[test]
fn queue_moves_only_swap_queued_neighbors_and_allow_edges() {
    let original = json!({"items": [
        {"id": 1, "status": "running"},
        {"id": 2, "status": "queued"},
        {"id": 3, "status": "done"},
        {"id": 4, "status": "queued"},
        {"id": 5, "status": "failed"},
    ]});
    let mut queue = original.clone();
    mutate_queue(&mut queue, "move", &json!({"id": 2, "dir": "up"})).unwrap();
    mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "down"})).unwrap();
    assert_eq!(queue, original);
    mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "up"})).unwrap();
    let mut swapped = original.clone();
    swapped["items"].as_array_mut().unwrap().swap(1, 3);
    assert_eq!(queue, swapped);
    mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "down"})).unwrap();
    assert_eq!(queue, original);
}

#[test]
fn queue_remove_and_clear_preserve_nonqueued_items() {
    let mut queue = json!({"items": [
        {"id": 1, "status": "running"},
        {"id": 2, "status": "queued"},
        {"id": 3, "status": "done"},
        {"id": 4, "status": "queued"},
        {"id": 5, "status": "failed"},
    ]});
    mutate_queue(&mut queue, "remove", &json!({"id": 2})).unwrap();
    assert_eq!(queue["items"].as_array().unwrap().len(), 4);
    assert_eq!(queue["items"][1]["id"], 3);
    mutate_queue(&mut queue, "clear", &json!({})).unwrap();
    assert_eq!(queue, json!({"items": [
        {"id": 1, "status": "running"},
        {"id": 3, "status": "done"},
        {"id": 5, "status": "failed"},
    ]}));
    let preserved = queue.clone();
    mutate_queue(&mut queue, "clear", &json!({})).unwrap();
    assert_eq!(queue, preserved);
    let mut empty = json!({"items": []});
    mutate_queue(&mut empty, "clear", &json!({})).unwrap();
    assert_eq!(empty, json!({"items": []}));
}

#[test]
fn queue_id_overflow_is_rejected_without_mutation() {
    let mut queue = json!({"items": [{"id": u64::MAX, "status": "done"}]});
    let original = queue.clone();
    assert!(mutate_queue(&mut queue, "add", &json!({"goal": "Next"})).is_err());
    assert_eq!(queue, original);
}

#[test]
fn queue_run_accumulates_planner_stage_and_run_usage() {
    let test = QueueTest::new(true);
    let mut queue = test.app.load_queue();
    queue["items"].as_array_mut().unwrap().truncate(1);
    test.app.save_queue(&queue);
    {
        let mut settings = test.app.app.settings.lock().unwrap();
        settings["mock_usage"] = json!({
            "input": 100, "output": 25, "total": 125, "model": "test-model",
        });
        settings["mock_verdicts"] = json!([
            {"approved": false, "issues": ["Fix the first stage."]},
            {"approved": true, "issues": []},
        ]);
    }
    test.start();
    assert!(test.statuses().is_empty());
    assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
    let plan = test.app.load_plan().unwrap();
    let expected = |calls: i64| json!({"mock": {
        "input_tokens": 100 * calls, "output_tokens": 25 * calls,
        "total_tokens": 125 * calls, "calls": calls,
        "models": {"test-model": 125 * calls},
    }});
    assert_eq!(plan["planner_usage"], expected(1));
    assert_eq!(plan["stages"][0]["status"], "committed");
    assert_eq!(plan["stages"][0]["rounds"], 2);
    assert_eq!(plan["stages"][0]["usage"], expected(6));
    assert_eq!(plan["stages"][1]["status"], "committed");
    assert_eq!(plan["stages"][1]["usage"], expected(3));
    assert_eq!(plan["usage"], expected(9));
    let history = test.app.read_history();
    for role in ["planner", "implementer", "fixer", "reviewer", "architect"] {
        assert!(history.as_array().unwrap().iter().any(|event|
            event["kind"] == "agent"
            && event["text"] == format!("[{role}] mock finished (125 tokens)")));
    }
    // Running a completed plan again must not count the calls twice.
    test.app.run_worker();
    assert_eq!(test.app.load_plan().unwrap(), plan);
}

#[test]
fn queue_run_omits_usage_when_tokens_are_absent_or_zero() {
    for usage in [None, Some(json!({
        "input": 0, "output": 0, "total": 0, "model": "test-model",
    }))] {
        let test = QueueTest::new(true);
        if let Some(usage) = usage {
            test.app.app.settings.lock().unwrap()["mock_usage"] = usage;
        }
        test.start();
        assert!(test.statuses().is_empty());
        assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
        let plan = test.app.load_plan().unwrap();
        assert!(plan.get("usage").is_none());
        assert!(plan.get("planner_usage").is_none());
        for stage in plan["stages"].as_array().unwrap() {
            assert_eq!(stage["status"], "committed");
            assert!(stage.get("usage").is_none());
        }
    }
}

#[test]
fn exhausted_run_keeps_usage_without_replenishing_budget() {
    let test = QueueTest::new(true);
    seed_mock_plan(&test.app);
    {
        let mut settings = test.app.app.settings.lock().unwrap();
        settings["mock_usage"] = json!({"total": 10});
        settings["max_fix_rounds"] = json!(0);
        settings["implementer_model"] = json!("configured-model");
        settings["reviewer_model"] = json!("");
        settings["mock_verdicts"] = json!([
            {"approved": false, "issues": ["Try again."]},
        ]);
    }
    test.app.run_worker();
    let blocked = test.app.load_plan().unwrap();
    assert_eq!(blocked["stages"][0]["status"], "blocked");
    assert_eq!(blocked["usage"]["mock"], json!({
        "input_tokens": 0, "output_tokens": 0, "total_tokens": 30, "calls": 3,
        "models": {"configured-model": 10},
    }));
    assert_eq!(blocked["stages"][0]["usage"], blocked["usage"]);
    assert!(blocked["stages"][1].get("usage").is_none());
    test.app.app.settings.lock().unwrap()["mock_usage"]["model"] = json!("reported-model");
    test.app.run_worker();
    let plan = test.app.load_plan().unwrap();
    assert_eq!(plan["stages"][0]["status"], "blocked");
    assert_eq!(plan["stages"][0]["usage"], blocked["stages"][0]["usage"]);
    assert_eq!(plan["usage"], blocked["usage"]);
}

#[test]
fn queue_auto_approval_commits_both_goals_and_releases_busy() {
    let test = QueueTest::new(true);
    assert_eq!(api_request(&test.app.app, "POST", "/api/queue/start", json!({})),
        (200, json!({"ok": true})));
    wait_for_worker(&test.app);
    let queue: Value = serde_json::from_str(
        &fs::read_to_string(test.app.forge_path("queue.json")).unwrap(),
    ).unwrap();
    assert_eq!(queue["items"], json!([]));
    assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
    assert!(!test.app.session.busy.load(Ordering::SeqCst));
    assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
    assert_eq!(test.app.session.state.lock().unwrap().goal, "second goal");
    assert_eq!(test.app.git(&["log", "--format=%s", "-4"]).unwrap(),
        "feat: line two\nfeat: line one\nfeat: line two\nfeat: line one");

    let history = test.app.read_history();
    let entries = history.as_array().unwrap();
    assert!(entries.iter().all(|entry| entry["unix"].is_i64()));
    for (id, goal) in [(1, "first goal"), (2, "second goal")] {
        let events: Vec<_> = entries.iter().filter(|entry| entry["goal"] == goal).collect();
        assert!(events.iter().any(|entry| entry["text"] == format!("goal {id}: planning")));
        assert!(events.iter().any(|entry| entry["kind"] == "queue"
            && entry["text"] == format!("goal {id}: done — removed from queue")));
        for sid in [1, 2] {
            for prefix in [format!("stage {sid} started:"), format!("stage {sid} committed in ")] {
                let event = events.iter().find(|entry| entry["text"].as_str().unwrap().starts_with(&prefix))
                    .expect("stage lifecycle event");
                assert_eq!(event["stage"], sid);
            }
        }
        assert!(events.iter().any(|entry| entry["text"].as_str().unwrap()
            .starts_with("all stages committed — run complete in ")));
    }
    for stage in test.app.load_plan().unwrap()["stages"].as_array().unwrap() {
        let duration = fmt_duration(stage["duration_secs"].as_i64().unwrap());
        let text = format!("stage {} committed in {duration}", stage["id"]);
        assert!(entries.iter().any(|entry| entry["goal"] == "second goal" && entry["text"] == text));
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0]["round"], 1);
        assert_eq!(reviews[1]["role"], "architect");
        assert_eq!(reviews[0]["approved"], true);
        assert_eq!(reviews[0]["issues"], json!([]));
        assert!(reviews[0]["unix"].as_i64().unwrap() > 0);
        let summary = reviews[0]["summary"].as_str().unwrap();
        assert!(!summary.is_empty());
        assert_eq!(stage["last_verdict"], reviews[0]);
        assert_eq!(stage["review_gate"]["status"], "approved");
        assert!(entries.iter().any(|entry| entry["kind"] == "review"
            && entry["text"].as_str().unwrap().starts_with(&format!("stage {} gate", stage["id"]))));
    }
}

#[test]
fn two_sessions_run_mock_queues_concurrently() {
    let first = QueueTest::new(true);
    let second = QueueTest::with_engine(true, Some(Arc::clone(&first.app.app)));
    let engine = &first.app.app;
    let barrier = std::sync::Barrier::new(2);
    // Distinct goals make crossed plan/history writes observable too.
    let mut queue = second.app.load_queue();
    for item in queue["items"].as_array_mut().unwrap() {
        item["goal"] = json!(format!("other project goal {}", item["id"]));
    }
    second.app.save_queue(&queue);
    let goals: Vec<Vec<Value>> = [&first, &second].iter().map(|test| {
        test.app.load_queue()["items"].as_array().unwrap().iter()
            .map(|item| item["goal"].clone()).collect()
    }).collect();
    for test in [&first, &second] {
        test.app.acquire_busy().unwrap();
        test.app.session.queue_active.store(true, Ordering::SeqCst);
    }
    engine.set_project(second.app.project()).unwrap();
    assert!(Arc::ptr_eq(&engine.active_session(), &second.app.session));
    let summaries = engine.session_summaries(second.app.project());
    assert_eq!(summaries[0]["project"], second.app.project());
    assert_eq!(summaries[1]["project"], first.app.project());
    for summary in summaries.as_array().unwrap() {
        assert_eq!(summary["busy"], true);
        assert_eq!(summary["queue_active"], true);
        assert_eq!(summary["queued"], 2);
    }
    std::thread::scope(|scope| {
        for test in [&first, &second] {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                test.app.queue_worker(None);
            });
        }
    });
    for (test, goals) in [&first, &second].iter().zip(&goals) {
        assert!(test.statuses().is_empty());
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
        assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
        assert_eq!(test.app.git(&["rev-list", "--count", "HEAD"]).unwrap(), "5");
        assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap().lines().count(), 4);
        assert_eq!(test.app.load_plan().unwrap()["goal"], goals[1]);
        assert!(test.app.read_history().as_array().unwrap().iter().all(|entry| {
            goals.contains(&entry["goal"])
        }));
    }
    assert!(!engine.any_busy());
}

#[test]
fn queue_remove_api_allows_queued_failed_and_blocked_items_only() {
    let test = QueueTest::new(false);
    for status in ["queued", "failed", "blocked", "planning", "awaiting_approval", "running"] {
        let queue = json!({"items": [
            {"id": 1, "goal": "first goal", "status": status},
            {"id": 2, "goal": "second goal", "status": "queued"},
        ]});
        test.app.save_queue(&queue);
        let response = api_request(&test.app.app, "POST", "/api/queue/remove", json!({"id": 1}));
        if matches!(status, "queued" | "failed" | "blocked") {
            assert_eq!(response, (200, json!({"ok": true})), "{status}");
            assert_eq!(test.app.load_queue(), json!({"items": [queue["items"][1]]}), "{status}");
        } else {
            assert_eq!(response, (400, json!({"error": "item is not queued"})), "{status}");
            assert_eq!(test.app.load_queue(), queue, "{status}");
        }
    }
}

#[test]
fn queue_move_api_rejects_all_nonqueued_states() {
    let test = QueueTest::new(false);
    for status in ["failed", "blocked", "planning", "awaiting_approval", "running", "done"] {
        let queue = json!({"items": [
            {"id": 1, "goal": "first goal", "status": status},
            {"id": 2, "goal": "second goal", "status": "queued"},
        ]});
        test.app.save_queue(&queue);
        assert_eq!(api_request(&test.app.app, "POST", "/api/queue/move", json!({"id": 1, "dir": "down"})),
            (400, json!({"error": "item is not queued"})), "{status}");
        assert_eq!(test.app.load_queue(), queue, "{status}");
    }
}

#[test]
fn queue_manual_approval_pauses_again_after_each_goal() {
    assert_eq!(default_settings()["queue_auto_approve"], false);
    let test = QueueTest::new(false);
    test.start();
    assert_eq!(test.statuses(), ["awaiting_approval", "queued"]);
    assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
    assert!(test.app.session.queue_active.load(Ordering::SeqCst));
    assert!(!test.app.session.busy.load(Ordering::SeqCst));
    for id in [1, 2] {
        test.app.acquire_busy().unwrap();
        {
            let _queue_guard = test.app.session.queue_lock.lock().unwrap();
            test.app.start_queue_run(
                &mut test.app.load_queue(), id, &mut test.app.load_plan().unwrap(),
            ).unwrap();
        }
        test.app.queue_worker(Some(id));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        if id == 1 {
            assert_eq!(test.statuses(), ["awaiting_approval"]);
            assert_eq!(test.app.load_queue()["items"][0]["id"], 2);
            assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        }
    }
    assert!(test.statuses().is_empty());
    assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
}

#[test]
fn queue_agent_failures_halt_and_leave_remaining_goals_queued() {
    for role in ["planner", "implementer", "reviewer"] {
        let test = QueueTest::new(true);
        test.app.app.settings.lock().unwrap()[role] = json!("invalid-agent");
        test.start();
        let expected = if role == "planner" { "failed" } else { "blocked" };
        assert_eq!(test.statuses(), [expected, "queued"], "{role}");
        assert_eq!(test.app.session.state.lock().unwrap().phase, expected);
        assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
    }
}

#[test]
fn queue_blocked_stage_halts_and_releases_busy() {
    let test = QueueTest::new(true);
    // An exhausted review budget exercises the blocked-stage exit.
    test.app.app.settings.lock().unwrap()["max_fix_rounds"] = json!(0);
    test.app.app.settings.lock().unwrap()["mock_verdicts"] = json!([{ "approved": false, "issues": ["Unresolved defect"] }]);
    test.start();
    assert_eq!(test.statuses(), ["blocked", "queued"]);
    assert_eq!(test.app.session.state.lock().unwrap().phase, "blocked");
    assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
    assert!(!test.app.session.busy.load(Ordering::SeqCst));
    let plan = test.app.load_plan().unwrap();
    let duration = fmt_duration(plan["stages"][0]["duration_secs"].as_i64().unwrap());
    let history = test.app.read_history();
    let event = history.as_array().unwrap().iter().find(|entry| {
        entry["text"].as_str().unwrap().starts_with(&format!("stage 1 blocked after {duration}:"))
    }).expect("blocked stage duration");
    assert_eq!(event["goal"], "first goal");
    assert_eq!(event["stage"], 1);
}

// Scope, paired verdicts, evidence, budgets and review-history regressions
// are exercised in app::review::tests (src/review_tests.rs).
