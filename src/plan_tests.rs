use crate::app::{PlanMode, WorkerGuard};
use crate::plan::edit_plan;
use crate::prompts::PLANNER_PROMPT;
use crate::test_support::{QueueTest, api_request, editable_stage, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[test]
fn plan_refactor_without_focus_replaces_plan_and_exposes_draft_in_state() {
    for body in [json!({"mode": "refactor"}), json!({"mode": "refactor", "goal": " \t\n\u{2003}"})] {
        let test = QueueTest::new(false);
        test.app.save_plan(&json!({"goal": "Old goal", "status": "approved",
            "stages": [editable_stage(9)]})).unwrap();
        test.app.app.settings.lock().unwrap()["mock_plan_output"] = json!({
            "goal": "Planner's goal", "status": "approved", "stages": [editable_stage(1)]});
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan", body),
            (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        assert_eq!(state["goal"], "Refactor the codebase");
        assert_eq!(state["phase"], "plan_ready");
        assert_eq!(state["plan"], test.app.architecture_store().state_plan(test.app.load_plan().unwrap()));
        assert_eq!(state["plan"]["goal"], "Refactor the codebase");
        assert_eq!(state["plan"]["status"], "draft");
        assert_eq!(state["plan"]["stages"][0]["id"], 1);
        assert_eq!(state["plan"]["stages"][0]["status"], "pending");
        assert_eq!(state["plan"]["stages"][0]["rounds"], 0);
        let settings = test.app.app.settings.lock().unwrap();
        assert_eq!(settings["mock_planner_phase"], "planning");
        assert_eq!(settings["mock_planner_had_plan"], false);
        let prompt = settings["mock_planner_prompt"].as_str().unwrap();
        assert!(prompt.contains("Explore this repository and read the code."));
        assert!(prompt.contains("duplication, dead code, overly long functions, unclear naming, and poor module structure."));
        assert!(prompt.contains("WITHOUT changing observable behavior."));
        assert!(prompt.contains("USER FOCUS (may be empty — if empty, choose the most valuable refactorings yourself):\n\n\nWrite"));
        assert!(prompt.contains("Rules: 2 to 8 stages, each independently committable, ordered by dependency."));
        assert!(prompt.contains("Every stage's acceptance criteria must require that observable behavior is preserved and builds/tests still pass."));
        assert!(prompt.contains("Do NOT implement anything, do not modify any other file. Only write .forge/plan-candidate.json."));
        let schema = PLANNER_PROMPT.split_once("with exactly this schema:\n").unwrap().1
            .split_once("\n\nRules:").unwrap().0;
        assert!(prompt.contains(schema));
        assert_eq!(test.app.read_history()[0]["text"],
            "planning started for goal: Refactor the codebase");
    }
}

#[test]
fn plan_refactor_trims_focus_and_keeps_user_placeholders_literal() {
    for focus in ["split main.rs into modules", "Keep {plan_path}, {focus}, {goal}, and {nested: {}} literal 界🙂"] {
        let test = QueueTest::new(false);
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan",
            json!({"mode": "refactor", "goal": format!(" \t{focus}\n")})),
            (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        let goal = format!("Refactor the codebase — focus: {focus}");
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        assert_eq!(state["goal"], goal);
        assert_eq!(state["plan"]["goal"], goal);
        assert_eq!(state["plan"]["status"], "draft");
        let settings = test.app.app.settings.lock().unwrap();
        let prompt = settings["mock_planner_prompt"].as_str().unwrap();
        assert!(prompt.contains(&format!("yourself):\n{focus}\n\nWrite")));
        assert!(prompt.contains("Only write .forge/plan-candidate.json."));
        assert_eq!(test.app.read_history()[0]["text"], format!("planning started for goal: {goal}"));
    }
}

#[test]
fn plan_api_rejects_unknown_modes_without_starting_planning() {
    let test = QueueTest::new(false);
    let original = json!({"goal": "Original goal", "status": "draft", "stages": [editable_stage(1)]});
    test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
    test.app.set_phase("plan_ready");
    for mode in [json!("nonsense"), json!("REFACTOR"), json!(" refactor "), json!(null), json!(42), json!(false)] {
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan", json!({"mode": mode})),
            (400, json!({"error": "unknown mode"})));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        assert_eq!(test.app.load_plan().unwrap(), original);
        assert_eq!(test.app.read_history(), json!([]));
        assert!(test.app.app.settings.lock().unwrap().get("mock_planner_prompt").is_none());
    }
}

#[test]
fn plan_refactor_respects_busy_and_queue_guards() {
    let test = QueueTest::new(false);
    for flag in [&test.app.session.busy, &test.app.session.queue_active] {
        flag.store(true, Ordering::SeqCst);
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan", json!({"mode": "refactor"})),
            (409, json!({"error": "busy"})));
        flag.store(false, Ordering::SeqCst);
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        assert!(test.app.load_plan().is_none());
        assert_eq!(test.app.read_history(), json!([]));
        assert!(test.app.app.settings.lock().unwrap().get("mock_planner_prompt").is_none());
    }
}

#[test]
fn plan_api_standard_modes_preserve_existing_goal_and_prompt_behavior() {
    for mode in [None, Some(""), Some("standard")] {
        for goal in [None, Some(" \tAdd a feature\n")] {
            let test = QueueTest::new(false);
            let mut body = json!({});
            if let Some(mode) = mode { body["mode"] = json!(mode); }
            if let Some(goal) = goal { body["goal"] = json!(goal); }
            assert_eq!(api_request(&test.app.app, "POST", "/api/plan", body),
                (200, json!({"ok": true})));
            wait_for_worker(&test.app);
            let goal = goal.unwrap_or("").trim();
            assert_eq!(test.app.load_plan().unwrap()["goal"], goal);
            assert_eq!(test.app.load_plan().unwrap()["status"], "draft");
            assert!(test.app.app.settings.lock().unwrap()["mock_planner_prompt"].as_str().unwrap().starts_with(
                &PLANNER_PROMPT.replace("{goal}", goal).replace("{plan_path}", ".forge/plan-candidate.json")));
        }
    }
}

#[test]
fn plan_chat_persists_transcript_and_preserves_plan_and_phase() {
    let test = QueueTest::new(false);
    let original = json!({"goal": "Explain {question} and {history}", "status": "draft",
        "stages": [editable_stage(1)]});
    test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
    test.app.set_phase("plan_ready");
    test.app.app.settings.lock().unwrap()["planner_model"] = json!("chat-model");
    let before = fs::read(test.app.forge_path("plan.json")).unwrap();
    let question = format!("Explain {{answer_path}} and {{current_plan}}: {}", "界🙂".repeat(200));
    for question in [question.as_str(), "What about {history} and {question}?"] {
        let prior = serde_json::to_string_pretty(&test.app.read_chat()).unwrap();
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/chat",
            json!({"question": format!(" \t{question}\n")})), (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
        let settings = test.app.app.settings.lock().unwrap();
        assert_eq!(settings["mock_chat_model"], "chat-model");
        assert_eq!(settings["mock_chat_phase"], "plan_ready");
        assert_eq!(settings["mock_chat_step"], "answering plan question");
        assert_eq!(settings["mock_chat_busy"], true);
        let prompt = settings["mock_chat_prompt"].as_str().unwrap();
        assert!(prompt.contains(&serde_json::to_string_pretty(&original).unwrap()));
        assert!(prompt.contains(&prior));
        assert!(prompt.contains(question));
        assert!(prompt.contains("{\"answer\": \"...\"}"));
        assert!(prompt.contains("Only write .forge/answer.json."));
    }
    let chat = test.app.read_chat();
    assert_eq!(chat.as_array().unwrap().len(), 4);
    assert_eq!(chat[0]["role"], "user");
    assert_eq!(chat[0]["text"], question);
    assert_eq!(chat[1]["role"], "assistant");
    assert_eq!(chat[1]["text"], "mock answer");
    assert_eq!(chat[2]["text"], "What about {history} and {question}?");
    assert_eq!(chat[3]["text"], "mock answer");
    assert!(chat.as_array().unwrap().iter().all(|entry| entry["unix"].as_i64().unwrap() > 0));
    let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["chat"], chat);
    assert_eq!(state["phase"], "plan_ready");
    assert_eq!(state["current_step"], "");
    assert_eq!(state["busy"], false);
    let history = test.app.read_history();
    assert_eq!(history[0]["kind"], "chat");
    assert_eq!(history[0]["text"], question.chars().take(300).collect::<String>());
}

#[test]
fn plan_chat_validates_question_before_busy_and_no_plan() {
    let test = QueueTest::new(false);
    let engine = &test.app.app;
    let body = json!({"question": "Why this plan?"});
    for busy in [false, true] {
        test.app.session.busy.store(busy, Ordering::SeqCst);
        for question in [Value::Null, json!(42), json!(""), json!(" \t\n\u{2003}")] {
            assert_eq!(api_request(engine, "POST", "/api/plan/chat", json!({"question": question})),
                (400, json!({"error": "question required"})));
        }
    }
    test.app.session.busy.store(false, Ordering::SeqCst);
    assert_eq!(api_request(engine, "POST", "/api/plan/chat", body.clone()),
        (400, json!({"error": "no plan"})));
    assert!(!test.app.session.busy.load(Ordering::SeqCst));
    for has_plan in [false, true] {
        if has_plan {
            test.app.save_plan(&json!({"goal": "Goal", "stages": [editable_stage(1)]})).unwrap();
        }
        for flag in [&test.app.session.busy, &test.app.session.queue_active] {
            flag.store(true, Ordering::SeqCst);
            assert_eq!(api_request(engine, "POST", "/api/plan/chat", body.clone()),
                (409, json!({"error": "busy"})));
            assert!(flag.load(Ordering::SeqCst));
            flag.store(false, Ordering::SeqCst);
            assert!(!test.app.session.busy.load(Ordering::SeqCst));
        }
    }
    assert_eq!(test.app.read_history(), json!([]));
    assert_eq!(test.app.read_chat(), json!([]));
}

#[test]
fn plan_chat_targets_project_and_leaves_other_session_alone() {
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
    second.app.save_plan(&json!({"goal": "Second", "stages": [editable_stage(1)]})).unwrap();
    second.app.set_phase("blocked");
    first.app.acquire_busy().unwrap();
    let _worker = WorkerGuard(&first.app.session);
    assert_eq!(api_request(engine, "POST", "/api/plan/chat",
        json!({"project": second.app.project(), "question": "Why blocked?"})),
        (200, json!({"ok": true})));
    wait_for_worker(&second.app);
    let (status, state) = api_request(engine, "GET",
        &format!("/api/state?project={}", second.app.project()), json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["chat"][0]["text"], "Why blocked?");
    assert_eq!(state["phase"], "blocked");
    assert_eq!(first.app.read_chat(), json!([]));
    assert_eq!(first.app.read_history(), json!([]));
    assert!(first.app.session.busy.load(Ordering::SeqCst));
}

#[test]
fn plan_chat_bad_output_logs_error_without_appending_or_changing_plan() {
    for (tool, output) in [
        ("unknown-planner", Value::Null), ("mock", Value::Null),
        ("mock", json!("not JSON")), ("mock", json!({})),
        ("mock", json!({"answer": 42})), ("mock", json!({"answer": " \n"})),
    ] {
        let test = QueueTest::new(false);
        test.app.save_plan(&json!({"goal": "Goal", "stages": [editable_stage(1)]})).unwrap();
        test.app.set_phase("plan_ready");
        let before = fs::read(test.app.forge_path("plan.json")).unwrap();
        fs::write(test.app.forge_path("answer.json"), r#"{"answer":"stale answer"}"#).unwrap();
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["planner"] = json!(tool);
            settings["mock_chat_output"] = output;
        }
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/chat",
            json!({"question": "Why?"})), (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        assert!(!test.app.forge_path("chat.jsonl").exists());
        assert_eq!(test.app.read_chat(), json!([]));
        assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
        let state = test.app.session.state.lock().unwrap();
        assert_eq!(state.phase, "plan_ready");
        assert_eq!(state.current_step, "");
        drop(state);
        assert!(test.app.read_history().as_array().unwrap().iter().any(|event|
            event["kind"] == "error" && event["text"].as_str().unwrap().starts_with("chat failed: ")));
    }
}

#[test]
fn plan_chat_state_keeps_last_100_valid_entries() {
    let test = QueueTest::new(false);
    test.app.ensure_forge_dir();
    let entries: Vec<Value> = (0..105).map(|i|
        json!({"role": "user", "text": format!("Question {i}"), "unix": i})).collect();
    let lines = entries.iter().map(Value::to_string).collect::<Vec<_>>().join("\ninvalid\n");
    fs::write(test.app.forge_path("chat.jsonl"), lines).unwrap();
    let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["chat"], json!(&entries[5..]));
}

#[test]
fn plan_generation_and_reset_clear_chat() {
    for reset in [false, true] {
        let test = QueueTest::new(false);
        test.app.save_plan(&json!({"goal": "Old goal", "stages": [editable_stage(1)]})).unwrap();
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/chat",
            json!({"question": "Why?"})), (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        assert!(test.app.forge_path("chat.jsonl").exists());
        if reset {
            assert_eq!(api_request(&test.app.app, "POST", "/api/reset_plan", json!({})),
                (200, json!({"ok": true})));
            assert!(test.app.load_plan().is_none());
        } else {
            test.app.acquire_busy().unwrap();
            test.app.plan_worker("New goal", &PlanMode::Standard);
            assert_eq!(test.app.load_plan().unwrap()["goal"], "New goal");
        }
        assert!(!test.app.forge_path("chat.jsonl").exists());
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        assert_eq!(state["chat"], json!([]));
    }
}

#[test]
fn plan_revise_api_preserves_committed_work_and_passes_feedback_to_planner() {
    let test = QueueTest::new(false);
    let mut committed = editable_stage(1);
    committed["status"] = json!("committed");
    committed["rounds"] = json!(3);
    committed["sha"] = json!("abc123");
    committed["reviews"] = json!([{"approved": true}]);
    let original = json!({"goal": "Original {feedback} goal", "status": "draft",
        "stages": [committed, {"id": 2, "status": "pending"}]});
    test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
    let feedback = format!("Improve {{goal}} and {{plan_path}}: {}", "界🙂".repeat(200));
    assert_eq!(api_request(&test.app.app, "POST", "/api/plan/revise",
        json!({"feedback": format!(" \t{feedback}\n")})), (200, json!({"ok": true})));
    wait_for_worker(&test.app);
    let plan = test.app.load_plan().unwrap();
    assert_eq!(plan["stages"][0], original["stages"][0]);
    assert_eq!(plan["stages"].as_array().unwrap().len(), 2);
    assert_eq!(plan["goal"], original["goal"]);
    assert_eq!(plan["status"], "draft");
    assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
    let settings = test.app.app.settings.lock().unwrap();
    assert_eq!(settings["mock_planner_phase"], "planning");
    assert_eq!(settings["mock_planner_had_plan"], false);
    let prompt = settings["mock_planner_prompt"].as_str().unwrap();
    assert!(prompt.contains(&serde_json::to_string_pretty(&original).unwrap()));
    assert!(prompt.contains(&feedback));
    assert!(prompt.contains("Keep the same overall goal:\nOriginal {feedback} goal"));
    assert!(prompt.contains("Only write .forge/plan-candidate.json."));
    let schema = PLANNER_PROMPT.split_once("with exactly this schema:\n").unwrap().1
        .split_once("\n\nRules:").unwrap().0;
    assert!(prompt.contains(schema));
    let history = test.app.read_history();
    assert_eq!(history[0]["kind"], "plan");
    assert_eq!(history[0]["text"], format!("revision started: {}",
        feedback.chars().take(300).collect::<String>()));
    assert!(history.as_array().unwrap().iter().any(|e| e["text"] == "plan revised with 2 stages"));
}

#[test]
fn plan_revise_restores_dropped_and_changed_committed_stages_in_original_order() {
    let test = QueueTest::new(false);
    let mut first = editable_stage(9);
    first["status"] = json!("committed");
    let mut second = editable_stage(1);
    second["status"] = json!("committed");
    second["rounds"] = json!(2);
    let original = json!({"goal": "Goal", "status": "approved",
        "stages": [first, second, editable_stage(2)]});
    test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
    test.app.acquire_busy().unwrap();
    test.app.revise_worker(&original, "Split the remaining work");
    let plan = test.app.load_plan().unwrap();
    assert_eq!(&plan["stages"].as_array().unwrap()[..2],
        &original["stages"].as_array().unwrap()[..2]);
    assert_eq!(plan["stages"].as_array().unwrap().len(), 3);
    assert_eq!(plan["stages"][2]["id"], 2);
    assert_eq!(plan["status"], "draft");
    assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
    assert!(!test.app.session.busy.load(Ordering::SeqCst));
}

#[test]
fn plan_revise_failure_restores_original_file_and_reports_error() {
    for (tool, output) in [
        ("unknown-planner", Value::Null),
        ("mock", Value::Null),
        ("mock", json!("not valid JSON")),
        ("mock", json!({"goal": "Unusable", "stages": {}})),
        ("mock", json!({"stages": [42]})),
    ] {
        let test = QueueTest::new(false);
        let original = json!({"goal": "Keep this goal", "status": "draft",
            "stages": [editable_stage(1), editable_stage(2)]});
        test.app.save_plan(&original).unwrap();
        let before = fs::read(test.app.forge_path("plan.json")).unwrap();
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["planner"] = json!(tool);
            settings["mock_plan_output"] = output;
        }
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/revise",
            json!({"feedback": "Improve the plan"})), (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        assert!(test.app.read_history().as_array().unwrap().iter().any(|event|
            event["kind"] == "error" && event["text"].as_str().unwrap().starts_with("revision failed: ")));
    }
}

#[test]
fn plan_revise_api_validates_input_busy_flags_and_project_routing() {
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
    for feedback in [Value::Null, json!(42), json!(""), json!(" \t\n\u{2003}")] {
        assert_eq!(api_request(engine, "POST", "/api/plan/revise", json!({"feedback": feedback})),
            (400, json!({"error": "feedback required"})));
    }
    let body = json!({"feedback": "Improve the plan"});
    assert_eq!(api_request(engine, "POST", "/api/plan/revise", body.clone()),
        (400, json!({"error": "no plan"})));
    assert!(!first.app.session.busy.load(Ordering::SeqCst));
    let original = json!({"goal": "Goal", "status": "draft", "stages": [editable_stage(1)]});
    first.app.save_plan(&original).unwrap();
    let original = first.app.load_plan().unwrap();
    for flag in [&first.app.session.busy, &first.app.session.queue_active] {
        flag.store(true, Ordering::SeqCst);
        assert_eq!(api_request(engine, "POST", "/api/plan/revise", body.clone()),
            (409, json!({"error": "busy"})));
        assert_eq!(first.app.load_plan().unwrap(), original);
        flag.store(false, Ordering::SeqCst);
    }
    assert_eq!(first.app.read_history(), json!([]));
    first.app.acquire_busy().unwrap();
    let _worker = WorkerGuard(&first.app.session);
    second.app.save_plan(&original).unwrap();
    let mut targeted = body;
    targeted["project"] = json!(second.app.project());
    assert_eq!(api_request(engine, "POST", "/api/plan/revise", targeted),
        (200, json!({"ok": true})));
    wait_for_worker(&second.app);
    assert_eq!(second.app.session.state.lock().unwrap().phase, "plan_ready");
    assert_eq!(first.app.load_plan().unwrap(), original);
}

#[test]
fn planning_and_revision_share_normalization() {
    for revision in [false, true] {
        let test = QueueTest::new(false);
        let original = json!({"goal": "Original goal", "stages": [editable_stage(1)]});
        test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
        test.app.app.settings.lock().unwrap()["mock_plan_output"] = json!({
            "goal": "Wrong goal", "status": "approved", "stages": [editable_stage(2), editable_stage(3)]});
        test.app.acquire_busy().unwrap();
        if revision {
            test.app.revise_worker(&original, "Improve the plan");
        } else {
            test.app.plan_worker("Original goal", &PlanMode::Standard);
        }
        let plan = test.app.load_plan().unwrap();
        assert_eq!(plan["goal"], "Original goal");
        assert_eq!(plan["status"], "draft");
        for stage in plan["stages"].as_array().unwrap() {
            assert_eq!(stage["status"], "pending");
            assert_eq!(stage["rounds"], 0);
        }
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
    }
}

#[test]
fn candidate_publication_preserves_selection_counts_and_failure_effects() {
    for revision in [false, true] {
        for supplied_proposal in [false, true] {
            for outcome in ["success", "malformed candidate", "failed publication"] {
                let test = QueueTest::new(false);
                let previous = if revision {
                    test.app.save_plan(&json!({"goal": "Goal", "status": "draft", "stages": [editable_stage(1)]})).unwrap();
                    test.app.load_plan()
                } else { None };
                let before = fs::read(test.app.forge_path("plan.json")).ok();
                let checkpoint = previous.as_ref().map(|p| test.app.architecture_store().checkpoint(p).unwrap());
                let mut stage = editable_stage(2);
                if supplied_proposal {
                    let option = test.app.routing_options().unwrap().into_iter().find(|o| o["eligible"] == true).unwrap();
                    stage["model_proposal"] = json!({"provider": option["provider"], "model": option["model"],
                        "native_effort": option["effort"], "risk": "standard", "complexity": "standard",
                        "task": "functionality", "rationale": "Planner proposal from candidate"});
                    stage["model_proposal_inputs"] = json!({"forged": true});
                    stage["model_proposer"] = json!({"forged": true});
                }
                {
                    let mut settings = test.app.app.settings.lock().unwrap();
                    settings["mock_plan_output"] = json!({"stages": [stage]});
                    if outcome == "malformed candidate" { settings["mock_plan_output"]["stages"] = json!([42]); }
                    if outcome == "failed publication" { settings["mock_architect_output"] = json!("not JSON"); }
                }
                test.app.acquire_busy().unwrap();
                if let Some(previous) = &previous {
                    test.app.revise_worker(previous, "Improve the plan");
                } else {
                    test.app.plan_worker("Goal", &PlanMode::Standard);
                }
                let expected_calls = usize::from(outcome != "malformed candidate");
                let settings = test.app.app.settings.lock().unwrap();
                for key in ["mock_architect_requests", "mock_routing_architect_requests"] {
                    assert_eq!(settings[key].as_array().map_or(0, Vec::len), expected_calls, "{revision}/{supplied_proposal}/{outcome}/{key}");
                }
                assert_eq!(settings["mock_routing_planner_requests"].as_array().map_or(0, Vec::len),
                    if supplied_proposal { 0 } else { expected_calls });
                drop(settings);
                let history = test.app.read_history();
                let success_message = format!("plan {} with 1 stages", if revision { "revised" } else { "ready" });
                if outcome == "success" {
                    assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
                    assert_eq!(history.as_array().unwrap().last().unwrap()["text"], success_message);
                    let plan = test.app.load_plan().unwrap();
                    assert_eq!(plan["stages"][0]["model_agreement"]["agreed"], true);
                    if supplied_proposal {
                        assert_eq!(plan["stages"][0]["model_proposal"], stage["model_proposal"]);
                        assert_eq!(plan["stages"][0]["model_proposal_inputs"], test.app.proposal_inputs(&plan, 0));
                        assert_eq!(plan["stages"][0]["model_proposer"], plan["planner_selection_actor"]);
                    }
                } else {
                    assert_eq!(fs::read(test.app.forge_path("plan.json")).ok(), before);
                    if let Some(previous) = &previous {
                        assert_eq!(test.app.architecture_store().checkpoint(previous).unwrap(), checkpoint.unwrap());
                    }
                    assert_eq!(test.app.session.state.lock().unwrap().phase,
                        if outcome == "failed publication" { "blocked" } else if revision { "plan_ready" } else { "failed" });
                    assert!(!history.as_array().unwrap().iter().any(|e| e["text"] == success_message));
                    let error = history.as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
                    assert!(error.starts_with(if revision { "revision failed: " } else { "planning failed: " }));
                    assert!(error.contains(if outcome == "malformed candidate" {
                        "planner produced a stage that is not an object"
                    } else { "invalid architect output" }));
                }
            }
        }
    }
}

#[test]
fn plan_edit_resets_execution_and_preserves_or_replaces_goal() {
    let original = json!({"goal": "Old goal", "status": "blocked", "metadata": "keep",
        "stages": [{"id": 3, "title": "Old title", "instructions": "Old instructions",
            "acceptance": "Old acceptance", "commit": "old", "status": "blocked",
            "rounds": 2, "started_unix": 10, "finished_unix": 20, "duration_secs": 10,
            "last_verdict": {"approved": false}, "reviews": [{"round": 2}], "sha": "old"}]});
    let mut stage = original["stages"][0].clone();
    for field in ["title", "instructions", "acceptance", "commit"] {
        stage[field] = editable_stage(3)[field].clone();
    }
    let edited = edit_plan(&original, &json!({"plan": {
        "goal": "New goal", "stages": [stage, editable_stage(9)]}})).unwrap();
    assert_eq!(edited["goal"], "New goal");
    assert_eq!(edited["status"], "draft");
    assert_eq!(edited["metadata"], "keep");
    assert_eq!(edited["stages"][0]["reviews"], original["stages"][0]["reviews"]);
    assert_eq!(edited["stages"][0]["last_verdict"], original["stages"][0]["last_verdict"]);
    assert_eq!(edited["stages"][0]["last_verdict_valid"], false);
    for (stage, id) in edited["stages"].as_array().unwrap().iter().zip([3, 9]) {
        assert_eq!(stage["id"], id);
        assert_eq!(stage["status"], "pending");
        assert_eq!(stage["rounds"], 0);
        assert!(stage.get("sha").is_none());
    }
    for goal in [Value::Null, json!(42), json!(" \t\n\u{2003}")] {
        let edited = edit_plan(&original, &json!({"plan": {
            "goal": goal, "stages": [editable_stage(3)]}})).unwrap();
        assert_eq!(edited["goal"], "Old goal");
    }
    let edited = edit_plan(&original, &json!({"plan": {"stages": [editable_stage(3)]}})).unwrap();
    assert_eq!(edited["goal"], "Old goal");
    assert_eq!(original["status"], "blocked");
}

#[test]
fn plan_edit_preserves_committed_stages_verbatim_and_in_order() {
    let mut first = editable_stage(2);
    first["status"] = json!("committed");
    first["rounds"] = json!(3);
    first["sha"] = json!("abc123");
    first["reviews"] = json!([{"approved": true}]);
    first["started_unix"] = json!(10);
    first["finished_unix"] = json!(20);
    first["duration_secs"] = json!(10);
    first["last_verdict"] = json!({"approved": true});
    first["custom"] = json!({"preserve": true});
    let mut second = first.clone();
    second["id"] = json!(5);
    let original = json!({"goal": "Goal", "status": "approved",
        "stages": [first, second, editable_stage(8)]});
    let mut submitted = editable_stage(2);
    submitted["title"] = json!("Ignored edit to committed work");
    let edited = edit_plan(&original, &json!({"plan": {"stages": [
        submitted, editable_stage(5), editable_stage(8)]}})).unwrap();
    assert_eq!(edited["stages"][0], original["stages"][0]);
    assert_eq!(edited["stages"][1], original["stages"][1]);
    assert_eq!(edit_plan(&original, &json!({"plan": {"stages": [editable_stage(8)]}})),
        Err("cannot remove a committed stage"));
    for ids in [[5, 2, 8], [2, 8, 5], [8, 2, 5]] {
        assert_eq!(edit_plan(&original, &json!({"plan": {
            "stages": ids.map(editable_stage)}})), Err("cannot reorder committed stages"));
    }
}

#[test]
fn plan_edit_assigns_unique_ids_and_allows_editable_reordering_and_removal() {
    let original = json!({"goal": "Goal", "stages": [
        editable_stage(1), editable_stage(4), editable_stage(7)]});
    let mut new = editable_stage(0);
    new.as_object_mut().unwrap().remove("id");
    let mut invalid_id = editable_stage(0);
    invalid_id["id"] = json!("invalid");
    let edited = edit_plan(&original, &json!({"plan": {"stages": [
        editable_stage(7), new, editable_stage(8), editable_stage(1), invalid_id]}})).unwrap();
    let ids: Vec<i64> = edited["stages"].as_array().unwrap().iter()
        .map(|stage| stage["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [7, 9, 8, 1, 10]);
    let full = json!({"stages": [editable_stage(i64::MAX)]});
    assert_eq!(edit_plan(&full, &json!({"plan": {"stages": [editable_stage(0)]}})),
        Err("stage id limit reached"));
}

#[test]
fn plan_edit_invalid_bodies_leave_saved_plan_unchanged() {
    let test = QueueTest::new(false);
    let mut committed = editable_stage(1);
    committed["status"] = json!("committed");
    let original = json!({"goal": "Goal", "status": "approved", "stages": [committed]});
    test.app.save_plan(&original).unwrap();
    let original = test.app.load_plan().unwrap();
    test.app.session.state.lock().unwrap().goal = "Goal".into();
    let before = fs::read(test.app.forge_path("plan.json")).unwrap();
    let mut bodies = vec![json!({}), json!(null), json!({"plan": null}),
        json!({"plan": []}), json!({"plan": {}}), json!({"plan": {"stages": null}}),
        json!({"plan": {"stages": {}}}), json!({"plan": {"stages": []}}),
        json!({"plan": {"stages": [null]}}),
        json!({"plan": {"stages": [editable_stage(1), editable_stage(1)]}}),
        json!({"plan": {"stages": [editable_stage(2)]}})];
    for field in ["title", "instructions", "acceptance", "commit"] {
        let mut stage = editable_stage(1);
        stage.as_object_mut().unwrap().remove(field);
        bodies.push(json!({"plan": {"stages": [stage]}}));
        for value in [Value::Null, json!(123), json!(false), json!([]), json!({})] {
            let mut stage = editable_stage(1);
            stage[field] = value;
            bodies.push(json!({"plan": {"stages": [stage]}}));
        }
    }
    for field in ["title", "instructions"] {
        let mut stage = editable_stage(1);
        stage[field] = json!(" \t\n\u{2003}");
        bodies.push(json!({"plan": {"stages": [stage]}}));
    }
    for body in bodies {
        assert!(edit_plan(&original, &body).is_err(), "accepted {body}");
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/edit", body.clone()).0,
            400, "accepted {body}");
        assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
        assert_eq!(test.app.session.state.lock().unwrap().goal, "Goal");
    }
    assert_eq!(test.app.read_history(), json!([]));
}

#[test]
fn plan_edit_api_routes_projects_updates_state_and_rejects_busy_sessions() {
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
    let body = json!({"plan": {"goal": "Edited goal", "stages": [editable_stage(1)]}});
    assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
        (400, json!({"error": "no plan"})));
    assert!(!first.app.forge_path("plan.json").exists());
    let original = json!({"goal": "Old goal", "status": "approved", "stages": [editable_stage(1)]});
    first.app.save_plan(&original).unwrap();
    let original = first.app.load_plan().unwrap();
    let before = fs::read(first.app.forge_path("plan.json")).unwrap();
    for flag in [&first.app.session.busy, &first.app.session.queue_active] {
        flag.store(true, Ordering::SeqCst);
        assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
            (409, json!({"error": "busy"})));
        assert_eq!(fs::read(first.app.forge_path("plan.json")).unwrap(), before);
        flag.store(false, Ordering::SeqCst);
    }
    assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
        (200, json!({"ok": true})));
    let first_plan = first.app.load_plan().unwrap();
    assert_eq!(first_plan["status"], "draft");
    first.app.session.busy.store(true, Ordering::SeqCst);
    second.app.save_plan(&original).unwrap();
    let second_original = second.app.load_plan().unwrap();
    second.app.set_phase("blocked");
    let mut targeted = body;
    targeted["project"] = json!(second.app.project());
    assert_eq!(api_request(engine, "POST", "/api/plan/edit", targeted),
        (200, json!({"ok": true})));
    let (status, state) = api_request(engine, "GET",
        &format!("/api/state?project={}", second.app.project()), json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["phase"], "plan_ready");
    assert_eq!(state["goal"], "Edited goal");
    let mut expected = edit_plan(&second_original, &json!({"plan": {
        "goal": "Edited goal", "stages": [editable_stage(1)]}})).unwrap();
    expected["architecture"] = state["plan"]["architecture"].clone();
    for key in ["model_agreement", "model_proposal", "model_proposal_inputs", "model_proposer"] {
        expected["stages"][0][key] = state["plan"]["stages"][0][key].clone();
    }
    assert_eq!(state["plan"]["stages"][0]["model_agreement"]["agreed"], true);
    assert_eq!(state["plan"], second.app.architecture_store().state_plan(expected));
    assert_eq!(first.app.load_plan().unwrap(), first_plan);
    let history: Vec<_> = second.app.read_history().as_array().unwrap().iter().filter(|h| h["kind"] == "plan").cloned().collect();
    assert_eq!(history[0]["kind"], "plan");
    assert_eq!(history[0]["text"], "plan edited by user");
    assert_eq!(history[0]["goal"], "Edited goal");
}

