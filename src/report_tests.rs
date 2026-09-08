use crate::app::{App, FORGE_DIR};
use crate::plan::default_settings;
use crate::reports::{completed_run_report, completion_message};
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use crate::util::{fmt_duration, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;

#[test]
fn history_entries_include_timestamp_and_only_available_context() {
    let test = QueueTest::new(true);
    let before = unix_timestamp();
    test.app.log_event("run", "idle event");
    test.app.session.state.lock().unwrap().goal = "short goal".into();
    test.app.log_event("plan", "goal event");
    {
        let mut s = test.app.session.state.lock().unwrap();
        s.goal = "界🙂".repeat(61);
        s.current_stage = Some(3);
    }
    test.app.log_event("stage", "active event");
    test.app.session.state.lock().unwrap().goal.clear();
    test.app.log_event("stage", "stage event");

    let text = fs::read_to_string(test.path.join(FORGE_DIR).join("history.jsonl")).unwrap();
    let entries: Vec<Value> = text.lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(entries.len(), 4);
    for entry in &entries {
        assert!((before..=unix_timestamp()).contains(&entry["unix"].as_i64().unwrap()));
        assert_eq!(entry["t"].as_str().unwrap().split(':').count(), 3);
    }
    assert!(entries[0].get("goal").is_none());
    assert!(entries[0].get("stage").is_none());
    assert_eq!(entries[1]["goal"], "short goal");
    assert!(entries[1].get("stage").is_none());
    assert_eq!(entries[2]["goal"], "界🙂".repeat(60));
    assert_eq!(entries[2]["stage"], 3);
    assert_eq!(entries[2]["kind"], "stage");
    assert_eq!(entries[2]["text"], "active event");
    assert!(entries[3].get("goal").is_none());
    assert_eq!(entries[3]["stage"], 3);
}

#[test]
fn update_pending_marker_logs_finished_event_once_on_next_engine_start() {
    let test = QueueTest::new(false);
    let project = test.path.display().to_string();
    let marker = test.path.join(FORGE_DIR).join("update-pending");
    let update_events = |engine: &Arc<App>| {
        engine.context(&project).read_history().as_array().unwrap().iter()
            .filter(|e| e["kind"] == "update").cloned().collect::<Vec<_>>()
    };

    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, unix_timestamp().to_string()).unwrap();
    // A fresh App is a restarted engine: its first session for the project
    // consumes the marker and records the completion.
    let engine = Arc::new(App::new(&project, default_settings()));
    let events = update_events(&engine);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["text"], "self-update finished; engine restarted on the new build");
    assert!(!marker.exists());

    // A stale marker (the update never restarted the engine) is discarded
    // without logging a bogus completion.
    fs::write(&marker, (unix_timestamp() - 3600).to_string()).unwrap();
    let engine = Arc::new(App::new(&project, default_settings()));
    assert_eq!(update_events(&engine).len(), 1);
    assert!(!marker.exists());
}

#[test]
fn history_keeps_last_400_parsed_entries_and_preserves_legacy_shape() {
    let test = QueueTest::new(true);
    let legacy: Vec<Value> = (0..402).map(|i| {
        json!({"t": "12:34:56", "kind": "stage", "text": format!("old event {i}")})
    }).collect();
    let mut text = legacy.iter().map(|entry| format!("{entry}\n")).collect::<String>();
    text.push_str("not json\n\n");
    let path = test.app.forge_path("history.jsonl");
    fs::write(&path, &text).unwrap();
    test.app.log_event("run", "new event");

    let history = test.app.read_history();
    let entries = history.as_array().unwrap();
    assert_eq!(entries.len(), 400);
    assert_eq!(&entries[..399], &legacy[3..]);
    assert!(entries[399]["unix"].is_i64());
    assert_eq!(entries[399]["text"], "new event");
    assert!(fs::read_to_string(path).unwrap().starts_with(&text));
}

#[test]
fn run_completion_uses_run_start_time_before_cleanup() {
    let test = QueueTest::new(true);
    test.app.save_plan(&json!({"stages": [], "status": "approved"})).unwrap();
    let started = unix_timestamp() - 252;
    {
        let mut s = test.app.session.state.lock().unwrap();
        s.goal = "timed goal".into();
        s.run_started_unix = started;
    }
    test.app.run_worker();
    let elapsed = unix_timestamp() - started;
    let history = test.app.read_history();
    let entry = history.as_array().unwrap().last().unwrap();
    assert_eq!(entry["kind"], "run");
    assert_eq!(entry["goal"], "timed goal");
    assert!((252..=elapsed).any(|secs| {
        entry["text"] == format!("all stages committed — run complete in {}", fmt_duration(secs))
    }));
    let reports = test.app.read_reports();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    let report = &reports[0];
    assert_eq!(report["duration_secs"], report["unix"].as_i64().unwrap() - started);
    assert!((252..=elapsed).contains(&report["duration_secs"].as_i64().unwrap()));
    assert_eq!(report["commits"], json!([]));
    assert_eq!(test.app.session.state.lock().unwrap().run_started_unix, 0);
}

#[test]
fn queue_reports_preserve_each_goals_commits_and_usage_across_restart() {
    for with_usage in [false, true] {
        let test = QueueTest::new(true);
        assert_eq!(api_request(&test.app.app, "GET", "/api/state", json!({})).1["reports"], json!([]));
        if with_usage {
            test.app.app.settings.lock().unwrap()["mock_usage"] = json!({
                "input": 100, "output": 25, "total": 125, "model": "test-model",
            });
        }
        let before = unix_timestamp();
        assert_eq!(api_request(&test.app.app, "POST", "/api/queue/start", json!({})),
            (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        let after = unix_timestamp();
        let (code, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
        assert_eq!(code, 200);
        assert_eq!(state["phase"], "done");
        let reports = state["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 2);
        let git_log = test.app.git(&["log", "--reverse", "--format=%h %s", "-4"]).unwrap();
        let git_commits: Vec<_> = git_log.lines().collect();
        assert_eq!(git_commits.len(), 4);
        let expected_usage = |calls: i64| json!({"mock": {
            "input_tokens": 100 * calls, "output_tokens": 25 * calls,
            "total_tokens": 125 * calls, "calls": calls,
            "models": {"test-model": 125 * calls},
        }});
        for (idx, goal) in ["first goal", "second goal"].iter().enumerate() {
            let report = &reports[idx];
            assert_eq!(report["goal"], *goal);
            assert_eq!(report["stages"], 2);
            assert!((before..=after).contains(&report["unix"].as_i64().unwrap()));
            let duration = report["duration_secs"].as_i64().unwrap();
            assert!((0..=after - before).contains(&duration));
            let commits = report["commits"].as_array().unwrap();
            assert_eq!(commits.len(), 2);
            for (stage_idx, (message, title)) in [
                ("feat: line one", "first"), ("feat: line two", "second"),
            ].iter().enumerate() {
                let commit = &commits[stage_idx];
                assert_eq!(commit["message"], *message);
                assert_eq!(commit["title"], *title);
                assert_eq!(format!("{} {message}", commit["sha"].as_str().unwrap()),
                    git_commits[idx * 2 + stage_idx]);
            }
            let mut completion = format!(
                "all stages committed — run complete in {}", fmt_duration(duration));
            if with_usage {
                assert_eq!(report["usage"], expected_usage(6));
                assert_eq!(report["planner_usage"], expected_usage(1));
                completion.push_str(" — 2 commits — mock: 750 tokens");
            } else {
                assert!(report.get("usage").is_none());
                assert!(report.get("planner_usage").is_none());
            }
            assert!(state["history"].as_array().unwrap().iter().any(|event|
                event["kind"] == "run" && event["goal"] == *goal && event["text"] == completion));
        }
        let persisted: Vec<Value> = fs::read_to_string(test.app.forge_path("reports.jsonl"))
            .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(&persisted, reports);
        let restarted = Arc::new(App::new(test.path.to_str().unwrap(), default_settings()));
        assert_eq!(restarted.context(test.path.to_str().unwrap()).read_reports(), state["reports"]);
        assert_eq!(api_request(&restarted, "GET", "/api/state", json!({})).1["reports"], state["reports"]);
    }
}

#[test]
fn run_reports_include_only_commits_with_shas_and_summarize_each_tool() {
    let test = QueueTest::new(true);
    let sha = test.app.git(&["rev-parse", "--short", "HEAD"]).unwrap();
    let usage = json!({
        "codex": {"total_tokens": 100000, "models": {"codex-model": 100000}},
        "claude": {"total_tokens": 200000, "models": {"claude-model": 200000}},
    });
    test.app.save_plan(&json!({
        "goal": "completed goal", "status": "approved", "usage": usage,
        "stages": [
            {"status": "committed", "sha": sha, "commit": "initial", "title": "Initial stage"},
            {"status": "committed", "commit": "feat: no changes", "title": "No changes"},
        ],
    })).unwrap();
    test.app.run_worker();
    let reports = test.app.read_reports();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    assert_eq!(reports[0]["duration_secs"], 0);
    assert_eq!(reports[0]["stages"], 2);
    assert_eq!(reports[0]["commits"], json!([
        {"sha": sha, "message": "initial", "title": "Initial stage"},
    ]));
    assert_eq!(reports[0]["usage"], usage);
    assert!(reports[0].get("planner_usage").is_none());
    assert_eq!(test.app.read_history().as_array().unwrap().last().unwrap()["text"],
        "all stages committed — run complete in 0s — 1 commit — claude: 200000 tokens, codex: 100000 tokens");
}

#[test]
fn run_reports_match_default_commit_message_and_handle_missing_token_totals() {
    let test = QueueTest::new(true);
    test.app.save_plan(&json!({
        "goal": "default commit message", "status": "approved",
        "usage": {"codex": {}},
        "stages": [{
            "id": 1, "title": "Default message", "status": "pending",
            "instructions": "append a line", "acceptance": "file has a line",
        }],
    })).unwrap();
    test.app.run_worker();
    assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
    let reports = test.app.read_reports();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    let commits = reports[0]["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["message"], "forge: stage");
    assert_eq!(test.app.git(&["log", "-1", "--format=%h %s"]).unwrap(),
        format!("{} {}", commits[0]["sha"].as_str().unwrap(),
            commits[0]["message"].as_str().unwrap()));
    assert_eq!(test.app.read_history().as_array().unwrap().last().unwrap()["text"],
        "all stages committed — run complete in 0s — 1 commit — codex: 0 tokens");
}

#[test]
fn api_state_returns_the_last_100_reports() {
    let test = QueueTest::new(true);
    let reports: Vec<Value> = (0..105).map(|idx| json!({"goal": format!("goal {idx}")})).collect();
    let lines = reports.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    fs::write(test.app.forge_path("reports.jsonl"), format!("{lines}\ninvalid JSON\n")).unwrap();
    let (code, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(code, 200);
    assert_eq!(state["reports"], json!(reports[5..]));
}

#[test]
fn duration_formats_seconds_and_minutes() {
    for (secs, expected) in [
        (-1, "0s"), (0, "0s"), (58, "58s"), (60, "1m 0s"),
        (61, "1m 1s"), (252, "4m 12s"), (3600, "60m 0s"),
    ] {
        assert_eq!(fmt_duration(secs), expected);
    }
}

#[test]
fn completed_report_preserves_optional_presence_and_clamps_duration() {
    for (started, now, duration) in [(100, 352, 252), (400, 352, 0), (0, 352, 0), (-1, 352, 0)] {
        let mut plan = json!({"stages": [], "architecture": {"stale": true}});
        let mut expected = json!({
            "version": 1, "project": "/project", "unix": now, "goal": null,
            "plan_id": null, "revision": null, "duration_secs": duration,
            "stages": 7, "commits": [], "stage_outcomes": [],
            "architecture": {"authoritative": true},
        });
        assert_eq!(completed_run_report(&plan, "/project", 7, started, now,
            json!({"authoritative": true})), expected);
        for value in [Value::Null, json!({}), json!([]), json!(false), json!({"codex": {"total_tokens": 5}})] {
            for key in ["usage", "planner_usage", "role_usage"] {
                plan[key] = value.clone();
                expected[key] = value.clone();
            }
            assert_eq!(completed_run_report(&plan, "/project", 7, started, now,
                json!({"authoritative": true})), expected);
        }
    }
}

#[test]
fn completed_report_includes_present_shas_and_preserves_commit_fallbacks() {
    let plan = json!({"stages": [
        {"title": "No SHA", "commit": "ignored"},
        {"sha": null, "title": "Null SHA"},
        {"sha": "", "commit": null, "title": "Empty SHA"},
        {"sha": 42, "commit": false},
        {"sha": "abc", "commit": "", "title": "Empty message"},
        {"sha": "def", "commit": "feat: done", "title": "Named message"},
    ]});
    let report = completed_run_report(&plan, "/project", 6, 0, 100, Value::Null);
    assert_eq!(report["commits"], json!([
        {"sha": null, "message": "forge: stage", "title": "Null SHA"},
        {"sha": "", "message": "forge: stage", "title": "Empty SHA"},
        {"sha": 42, "message": "forge: stage", "title": null},
        {"sha": "abc", "message": "", "title": "Empty message"},
        {"sha": "def", "message": "feat: done", "title": "Named message"},
    ]));
    assert_eq!(completion_message(&report), "all stages committed — run complete in 0s");
}

#[test]
fn completed_report_keeps_exact_stage_presentation() {
    let plan = json!({"goal": "Ship", "plan_id": "plan-1", "revision": 2, "stages": [{
        "id": 3, "title": "Finish", "status": "committed", "attempt_id": "attempt-1",
        "rounds": 2, "sha": "abc", "commit": "feat: finish", "instructions": "omitted",
        "model_agreement": {
            "id": "agreement-1", "valid": true, "validated_proposal": {"model": "small"},
            "effective": {"model": "large"}, "availability": "verified",
            "verification_state": "execution_verified", "planner_reason": "plan reason",
            "architect_reason": "architect reason", "trigger": "review",
            "superseded_agreement": "agreement-0", "dialogue": ["omitted"],
            "policy_inputs": {"policy": "routing-1", "tier": 3, "tier_provenance": "configured",
                "relative_cost_preference": null, "pricing": {"input": 1}, "billing_basis": "tokens",
                "constraint": {"provider": "codex"}, "omitted": true},
        },
        "model_invocations": [{"id": "first"}, {"id": "last", "tokens": 10}],
        "review_policy": {"required_roles": ["architect", "reviewer"]},
        "review_gate": {"status": "approved", "roles": {"reviewer": "approved"},
            "identity": {"attempt_id": "attempt-1"}, "omitted": true},
        "reassessment": {"count": 1, "operational_retries": 2, "omitted": true},
        "usage": {"codex": {"total_tokens": 10}},
    }, {}]});
    let report = completed_run_report(&plan, "/project", 2, 100, 161, json!({"summary": "loaded"}));
    assert_eq!(report, json!({
        "version": 1, "project": "/project", "unix": 161, "duration_secs": 61,
        "goal": "Ship", "plan_id": "plan-1", "revision": 2, "stages": 2,
        "commits": [{"sha": "abc", "message": "feat: finish", "title": "Finish"}],
        "architecture": {"summary": "loaded"}, "stage_outcomes": [{
            "id": 3, "title": "Finish", "status": "committed", "attempt_id": "attempt-1",
            "rounds": 2, "sha": "abc", "model_agreement": {
                "id": "agreement-1", "valid": true, "validated_proposal": {"model": "small"},
                "effective": {"model": "large"}, "availability": "verified",
                "verification_state": "execution_verified", "planner_reason": "plan reason",
                "architect_reason": "architect reason", "trigger": "review",
                "superseded_agreement": "agreement-0", "policy_inputs": {
                    "policy": "routing-1", "tier": 3, "tier_provenance": "configured",
                    "relative_cost_preference": null, "pricing": {"input": 1}, "billing_basis": "tokens",
                    "constraint": {"provider": "codex"}},
            }, "last_invocation": {"id": "last", "tokens": 10}, "invocation_count": 2,
            "review_policy": {"required_roles": ["architect", "reviewer"]},
            "review_gate": {"status": "approved", "roles": {"reviewer": "approved"},
                "identity": {"attempt_id": "attempt-1"}},
            "reassessment": {"count": 1, "operational_retries": 2},
            "usage": {"codex": {"total_tokens": 10}},
        }, {
            "id": null, "title": null, "status": null, "attempt_id": null, "rounds": null,
            "sha": null, "model_agreement": null, "last_invocation": null, "invocation_count": 0,
            "review_policy": null, "review_gate": {"status": null, "roles": null, "identity": null},
            "reassessment": {"count": null, "operational_retries": null}, "usage": null,
        }],
    }));
}

#[test]
fn completion_suffix_requires_nonempty_usage_and_formats_zero_one_many_commits() {
    let mut plan = json!({"stages": [{"sha": null}], "planner_usage": {"codex": {"total_tokens": 7}},
        "role_usage": {"planner": {"codex": {"total_tokens": 7}}}});
    for usage in [None, Some(Value::Null), Some(json!({})), Some(json!([])), Some(json!(false)), Some(json!(7))] {
        if let Some(usage) = usage { plan["usage"] = usage; }
        let report = completed_run_report(&plan, "/project", 1, 100, 352, Value::Null);
        assert_eq!(completion_message(&report), "all stages committed — run complete in 4m 12s");
    }
    plan["usage"] = json!({"zeta": {"total_tokens": "9"}, "codex": {"total_tokens": 1.5},
        "claude": {"total_tokens": 200}, "alpha": null, "empty": {}, "large": {"total_tokens": u64::MAX}});
    for (stages, suffix) in [
        (json!([{}]), "0 commits"),
        (json!([{"sha": null}, {}]), "1 commit"),
        (json!([{"sha": "abc"}, {"sha": "def"}]), "2 commits"),
    ] {
        plan["stages"] = stages;
        let report = completed_run_report(&plan, "/project", 2, 100, 352, Value::Null);
        assert_eq!(completion_message(&report), format!(
            "all stages committed — run complete in 4m 12s — {suffix} — alpha: 0 tokens, claude: 200 tokens, codex: 0 tokens, empty: 0 tokens, large: 0 tokens, zeta: 0 tokens"));
    }
}
