use crate::app::WorkerGuard;
use crate::test_support::{QueueTest, api_request, editable_stage, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const PRIOR_TURN: &[u8] = b"{\"role\":\"user\",\"text\":\"keep this\",\"unix\":1}\n{\"role\":\"assistant\",\"text\":\"kept\",\"unix\":1}\n";

fn agent_roles(test: &QueueTest) -> Vec<String> {
    test.app.app.settings.lock().unwrap()["mock_agent_requests"].as_array().into_iter().flatten()
        .map(|request| request["role"].as_str().unwrap().to_string()).collect()
}

#[test]
fn discussion_message_validates_before_admission_and_rejects_busy_or_queued() {
    let test = QueueTest::new(false);
    let previous = json!({"status":"ready","request_id":7,"unix":1});
    {
        let mut state = test.app.session.state.lock().unwrap();
        state.discussion_serial = 7;
        state.discussion_activity = previous.clone();
    }
    fs::write(test.app.forge_path("discussion.jsonl"), PRIOR_TURN).unwrap();
    for busy in [false, true] {
        for queued in [false, true] {
            test.app.session.busy.store(busy, Ordering::SeqCst);
            test.app.session.queue_active.store(queued, Ordering::SeqCst);
            test.app.session.stop_requested.store(true, Ordering::SeqCst);
            let mut cases = vec![(json!({}), "message required")];
            for message in [Value::Null, json!(42), json!(["hi"]), json!(""), json!(" \t\n\u{2003}")] {
                cases.push((json!({"message":message}), "message required"));
            }
            cases.push((json!({"message":"界".repeat(20001)}), "message too long"));
            cases.push((json!({"message":format!(" {} ", "🙂".repeat(20001))}), "message too long"));
            if busy || queued { cases.push((json!({"message":"hello"}), "busy")); }
            for (body, error) in cases {
                assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message", body),
                    (if error == "busy" { 409 } else { 400 }, json!({"error":error})));
                assert_eq!(test.app.session.busy.load(Ordering::SeqCst), busy);
                assert_eq!(test.app.session.queue_active.load(Ordering::SeqCst), queued);
                assert!(test.app.session.stop_requested.load(Ordering::SeqCst));
                let state = test.app.session.state.lock().unwrap();
                assert_eq!(state.discussion_serial, 7);
                assert_eq!(state.discussion_activity, previous);
            }
        }
    }
    assert!(test.app.app.settings.lock().unwrap()["mock_agent_requests"].is_null());
    assert_eq!(test.app.read_history(), json!([]));
    assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    assert!(!test.app.forge_path("answer.json").exists());
    // The limit counts characters, not bytes.
    test.app.session.busy.store(false, Ordering::SeqCst);
    test.app.session.queue_active.store(false, Ordering::SeqCst);
    let longest = "界".repeat(20000);
    assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
        json!({"message":format!("\n{longest}\t")})), (200, json!({"ok":true,"request_id":8})));
    wait_for_worker(&test.app);
    assert_eq!(test.app.read_discussion()[2]["text"], longest);
}

#[test]
fn discussion_message_without_plan_appends_reply_and_never_plans() {
    let test = QueueTest::new(false);
    test.app.session.state.lock().unwrap().goal = "unchanged goal".into();
    test.app.app.settings.lock().unwrap()["mock_chat_output"] =
        json!({"answer":"Great, start planning now: create the plan and split it into stages."});
    let (status, initial) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(status, 200);
    assert_eq!(initial["discussion"], json!([]));
    assert_eq!(initial.get("discussion_activity"), Some(&Value::Null));
    for (index, message) in ["How is the engine structured?", "Let's plan it now."].iter().enumerate() {
        let request_id = index as i64 + 1;
        test.app.session.stop_requested.store(true, Ordering::SeqCst);
        // Hold settings so the worker cannot finish before admission is inspected.
        let settings = test.app.app.settings.lock().unwrap();
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
            json!({"message":format!(" \t{message}\n")})), (200, json!({"ok":true,"request_id":request_id})));
        {
            let state = test.app.session.state.lock().unwrap();
            assert_eq!(state.discussion_serial, request_id);
            assert_eq!(state.discussion_activity["status"], "running");
            assert_eq!(state.discussion_activity["request_id"], request_id);
            assert_eq!(state.discussion_activity["message"], *message);
            assert!(state.discussion_activity["unix"].as_i64().unwrap() > 0);
        }
        assert!(test.app.session.busy.load(Ordering::SeqCst));
        assert!(!test.app.session.stop_requested.load(Ordering::SeqCst));
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
            json!({"message":"overlapping"})), (409, json!({"error":"busy"})));
        drop(settings);
        wait_for_worker(&test.app);
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        let discussion = state["discussion"].as_array().unwrap();
        assert_eq!(discussion.len(), 2 * (index + 1));
        assert_eq!(discussion[2 * index]["role"], "user");
        assert_eq!(discussion[2 * index]["text"], *message);
        assert_eq!(discussion[2 * index + 1]["role"], "assistant");
        assert_eq!(discussion[2 * index + 1]["text"],
            "Great, start planning now: create the plan and split it into stages.");
        assert!(discussion.iter().all(|entry| entry["unix"].as_i64().unwrap() > 0));
        assert_eq!(state["discussion_activity"]["status"], "ready");
        assert_eq!(state["discussion_activity"]["request_id"], request_id);
        assert!(state["discussion_activity"]["unix"].as_i64().unwrap() > 0);
        assert_eq!(state["phase"], initial["phase"]);
        assert_eq!(state["goal"], "unchanged goal");
        assert_eq!(state["plan"], initial["plan"]);
        assert_eq!(state["chat"], json!([]));
        assert_eq!(state["busy"], false);
        assert_eq!(state["current_step"], "");
        let settings = test.app.app.settings.lock().unwrap();
        assert_eq!(settings["mock_chat_step"], "discussing before planning");
        assert_eq!(settings["mock_chat_busy"], true);
        drop(settings);
        assert_eq!(agent_roles(&test), vec!["chat".to_string(); index + 1]);
        let history = test.app.read_history();
        let events = history.as_array().unwrap();
        assert!(events.iter().any(|event| event["kind"] == "discussion" && event["text"] == *message));
        assert_eq!(events.iter().filter(|event|
            event["kind"] == "discussion" && event["text"] == "discussion reply ready").count(), index + 1);
    }
    for artifact in ["plan.json", "plan-candidate.json", "chat.jsonl"] {
        assert!(!test.app.forge_path(artifact).exists(), "{artifact} was created");
    }
    assert!(test.app.load_plan().is_none());
    assert!(test.app.app.settings.lock().unwrap()["mock_planner_prompt"].is_null());
}

#[test]
fn discussion_prompt_builds_on_history_and_keeps_placeholders_literal() {
    let test = QueueTest::new(false);
    test.app.app.settings.lock().unwrap()["mock_chat_output"] =
        json!([{"answer":"First reply about {message}"}, {"answer":"Second reply"}]);
    let messages = ["Should we add a cache? {answer_path}", "What about {history} and {message}?"];
    for (index, message) in messages.iter().enumerate() {
        let prior = serde_json::to_string_pretty(&test.app.read_discussion()).unwrap();
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
            json!({"message":message})), (200, json!({"ok":true,"request_id":index as i64 + 1})));
        wait_for_worker(&test.app);
        let prompt = test.app.app.settings.lock().unwrap()["mock_chat_prompt"].as_str().unwrap().to_string();
        assert!(prompt.contains(&prior));
        assert!(prompt.contains(&format!("latest message:\n{message}\n")));
        assert!(prompt.contains("{\"answer\": \"...\"}"));
        assert!(prompt.contains("Only write .forge/answer.json."));
        assert!(prompt.contains("OUTPUT CONTRACT OVERRIDE: read-only discussion in a fresh conversation."));
        if index == 1 {
            assert!(prompt.contains(messages[0]));
            assert!(prompt.contains("First reply about {message}"));
        }
    }
    let discussion = test.app.read_discussion();
    assert_eq!(discussion.as_array().unwrap().len(), 4);
    assert_eq!(discussion[0]["text"], messages[0]);
    assert_eq!(discussion[1]["text"], "First reply about {message}");
    assert_eq!(discussion[2]["text"], messages[1]);
    assert_eq!(discussion[3]["text"], "Second reply");
    assert_eq!(agent_roles(&test), vec!["chat", "chat"]);
}

#[test]
fn discussion_bad_output_reports_failure_without_touching_transcript() {
    for output in [Value::Null, json!("not JSON"), json!({}), json!({"answer":42}), json!({"answer":" \n"})] {
        let test = QueueTest::new(false);
        let plan = b"{\"goal\":\"Existing goal\",\"stages\":[]}\n";
        fs::write(test.app.forge_path("plan-candidate.json"), plan).unwrap();
        fs::write(test.app.forge_path("chat.jsonl"), PRIOR_TURN).unwrap();
        fs::write(test.app.forge_path("discussion.jsonl"), PRIOR_TURN).unwrap();
        fs::write(test.app.forge_path("answer.json"), r#"{"answer":"stale answer"}"#).unwrap();
        test.app.set_phase("blocked");
        test.app.app.settings.lock().unwrap()["mock_chat_output"] = output.clone();
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
            json!({"message":" Why? "})), (200, json!({"ok":true,"request_id":1})));
        wait_for_worker(&test.app);
        assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN, "{output}");
        assert_eq!(fs::read(test.app.forge_path("chat.jsonl")).unwrap(), PRIOR_TURN);
        assert_eq!(fs::read(test.app.forge_path("plan-candidate.json")).unwrap(), plan);
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        let activity = &state["discussion_activity"];
        assert_eq!(activity["status"], "failed");
        assert_eq!(activity["request_id"], 1);
        assert_eq!(activity["message"], "Why?");
        assert!(activity["unix"].as_i64().unwrap() > 0);
        let error = activity["error"].as_str().unwrap();
        assert!(!error.is_empty());
        assert!(state["history"].as_array().unwrap().iter().any(|event|
            event["kind"] == "error" && event["text"] == format!("discussion failed: {error}")));
        assert_eq!(state["phase"], "blocked");
        assert_eq!(state["busy"], false);
        assert_eq!(state["current_step"], "");
        assert!(agent_roles(&test).iter().all(|role| role == "chat"));
    }
}

#[test]
fn discussion_append_after_truncated_line_keeps_whole_turns() {
    let test = QueueTest::new(false);
    let truncated = [PRIOR_TURN, b"{\"role\":\"user\",\"te".as_slice()].concat();
    fs::write(test.app.forge_path("discussion.jsonl"), &truncated).unwrap();
    assert_eq!(test.app.read_discussion().as_array().unwrap().len(), 2);
    assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
        json!({"message":"next"})), (200, json!({"ok":true,"request_id":1})));
    wait_for_worker(&test.app);
    let discussion = test.app.read_discussion();
    let texts: Vec<_> = discussion.as_array().unwrap().iter().map(|entry| entry["text"].clone()).collect();
    assert_eq!(texts, [json!("keep this"), json!("kept"), json!("next"), json!("mock answer")]);
    assert!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap().starts_with(&truncated));
}

#[test]
fn discussion_fails_without_agent_when_stale_answer_cannot_be_removed() {
    let test = QueueTest::new(false);
    fs::create_dir_all(test.app.forge_path("answer.json").join("blocker")).unwrap();
    assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
        json!({"message":"hello"})), (200, json!({"ok":true,"request_id":1})));
    wait_for_worker(&test.app);
    let state = test.app.session.state.lock().unwrap();
    assert_eq!(state.discussion_activity["status"], "failed");
    assert!(state.discussion_activity["error"].as_str().unwrap().starts_with("could not remove previous answer: "));
    drop(state);
    assert!(test.app.app.settings.lock().unwrap()["mock_agent_requests"].is_null());
    assert!(!test.app.forge_path("discussion.jsonl").exists());
}

#[test]
fn discussion_stale_request_does_not_overwrite_newer_activity() {
    for output in [json!({"answer":"late reply"}), json!({})] {
        let test = QueueTest::new(false);
        let newer = json!({"status":"running","request_id":2,"message":"newer","unix":1});
        {
            let mut state = test.app.session.state.lock().unwrap();
            state.discussion_serial = 2;
            state.discussion_activity = newer.clone();
        }
        test.app.app.settings.lock().unwrap()["mock_chat_output"] = output.clone();
        test.app.acquire_busy().unwrap();
        test.app.discuss_worker("older", 1);
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        let state = test.app.session.state.lock().unwrap();
        assert_eq!(state.discussion_activity, newer);
        assert_eq!(state.discussion_serial, 2);
        drop(state);
        let appended = test.app.read_discussion().as_array().unwrap().len();
        assert_eq!(appended, if output["answer"].is_string() { 2 } else { 0 });
        assert!(!test.app.read_history().as_array().unwrap().iter().any(|event|
            event["text"] == "discussion reply ready"
                || event["text"].as_str().unwrap().starts_with("discussion failed: ")));
    }
}

#[test]
fn discussion_reset_clears_transcript_and_rejects_busy_work() {
    let test = QueueTest::new(false);
    assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/message",
        json!({"message":"hello"})), (200, json!({"ok":true,"request_id":1})));
    wait_for_worker(&test.app);
    let before = fs::read(test.app.forge_path("discussion.jsonl")).unwrap();
    let activity = test.app.session.state.lock().unwrap().discussion_activity.clone();
    assert_eq!(activity["status"], "ready");
    for flag in [&test.app.session.busy, &test.app.session.queue_active] {
        flag.store(true, Ordering::SeqCst);
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/reset", json!({})),
            (409, json!({"error":"busy"})));
        flag.store(false, Ordering::SeqCst);
        assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), before);
        assert_eq!(test.app.session.state.lock().unwrap().discussion_activity, activity);
    }
    test.app.save_plan(&json!({"goal":"Goal","stages":[editable_stage(1)]})).unwrap();
    let plan = fs::read(test.app.forge_path("plan.json")).unwrap();
    for _ in 0..2 {
        assert_eq!(api_request(&test.app.app, "POST", "/api/discussion/reset", json!({})),
            (200, json!({"ok":true})));
        assert!(!test.app.forge_path("discussion.jsonl").exists());
        let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(status, 200);
        assert_eq!(state["discussion"], json!([]));
        assert_eq!(state["discussion_activity"], Value::Null);
    }
    assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), plan);
    assert_eq!(test.app.read_history().as_array().unwrap().iter().filter(|event|
        event["kind"] == "discussion" && event["text"] == "discussion cleared").count(), 2);
}

#[test]
fn plan_reset_generation_and_plan_chat_keep_discussion_transcript() {
    let test = QueueTest::new(false);
    fs::write(test.app.forge_path("discussion.jsonl"), PRIOR_TURN).unwrap();
    assert_eq!(api_request(&test.app.app, "POST", "/api/plan", json!({"goal":"Build it"})),
        (200, json!({"ok":true})));
    wait_for_worker(&test.app);
    assert!(test.app.load_plan().is_some());
    assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    assert_eq!(api_request(&test.app.app, "POST", "/api/plan/chat", json!({"question":"Why?"})),
        (200, json!({"ok":true})));
    wait_for_worker(&test.app);
    assert_eq!(test.app.read_chat().as_array().unwrap().len(), 2);
    assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    assert_eq!(api_request(&test.app.app, "POST", "/api/reset_plan", json!({})),
        (200, json!({"ok":true})));
    assert!(!test.app.forge_path("chat.jsonl").exists());
    assert_eq!(fs::read(test.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    let (status, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["discussion"].as_array().unwrap().len(), 2);
}

#[test]
fn discussion_targets_project_and_leaves_other_session_alone() {
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
    let previous = json!({"status":"ready","request_id":7,"unix":1});
    {
        let mut state = first.app.session.state.lock().unwrap();
        state.discussion_serial = 7;
        state.discussion_activity = previous.clone();
        state.phase = "running".into();
        state.current_step = "other work".into();
    }
    fs::write(first.app.forge_path("discussion.jsonl"), PRIOR_TURN).unwrap();
    first.app.acquire_busy().unwrap();
    let _worker = WorkerGuard(&first.app.session);
    second.app.set_phase("blocked");
    assert_eq!(api_request(engine, "POST", "/api/discussion/message",
        json!({"project":second.app.project(),"message":"second project idea"})),
        (200, json!({"ok":true,"request_id":1})));
    wait_for_worker(&second.app);
    let (status, state) = api_request(engine, "GET",
        &format!("/api/state?project={}", second.app.project()), json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["discussion"][0]["text"], "second project idea");
    assert_eq!(state["discussion"].as_array().unwrap().len(), 2);
    assert_eq!(state["discussion_activity"]["status"], "ready");
    assert_eq!(state["discussion_activity"]["request_id"], 1);
    assert_eq!(state["phase"], "blocked");
    assert_eq!(state["busy"], false);
    let (status, state) = api_request(engine, "GET", "/api/state", json!({}));
    assert_eq!(status, 200);
    assert_eq!(state["discussion_activity"], previous);
    assert_eq!(state["discussion"].as_array().unwrap().len(), 2);
    assert_eq!(state["busy"], true);
    assert_eq!(state["phase"], "running");
    assert_eq!(state["current_step"], "other work");
    assert_eq!(first.app.session.state.lock().unwrap().discussion_serial, 7);
    assert_eq!(fs::read(first.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    assert_eq!(first.app.read_history(), json!([]));
    // The reset endpoint follows the same routing.
    assert_eq!(api_request(engine, "POST", "/api/discussion/reset",
        json!({"project":second.app.project()})), (200, json!({"ok":true})));
    assert!(!second.app.forge_path("discussion.jsonl").exists());
    assert_eq!(fs::read(first.app.forge_path("discussion.jsonl")).unwrap(), PRIOR_TURN);
    assert_eq!(api_request(engine, "POST", "/api/discussion/reset", json!({})),
        (409, json!({"error":"busy"})));
    assert_eq!(*engine.active_project.lock().unwrap(), first.app.project());
}
