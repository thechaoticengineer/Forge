//! Forge — a minimal wrapper around AI coding agents.
//!
//! Loop: plan (codex/claude) -> human approves -> per stage:
//! implement (tool A) -> independent review (tool B, fresh session) ->
//! bounded fix loop -> commit proposed message -> next stage -> push.
//!
//! Persistent goal queue: process goals sequentially with optional automatic approval.
//!
//! Serves a JSON API on 127.0.0.1:8734 for the Quickshell panel.

mod catalogue;
mod catalogue_process;
mod metadata;
mod agent;
mod architecture;
mod review_history;
mod contracts;
mod app;
mod http;
mod plan;
mod prompts;
mod util;

use crate::app::App;
use crate::http::{PORT, handle};
use crate::plan::default_settings;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    let project = std::env::args()
        .nth(1)
        .unwrap_or_else(|| std::env::current_dir().unwrap().display().to_string());
    let project = fs::canonicalize(&project)
        .map(|p| p.display().to_string())
        .unwrap_or(project);
    let projects_root = PathBuf::from(&project)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(&project))
        .display()
        .to_string();
    let mut settings = default_settings();
    settings["projects_root"] = json!(projects_root);
    let policy_path = crate::catalogue::policy_path();
    let loaded = crate::catalogue::load_policy(&policy_path);
    if let Ok(Some(policy)) = &loaded { settings["model_catalogue"] = json!(policy); }
    let mut app = App::new(&project, settings);
    app.model_policy_path = Some(policy_path);
    *app.model_policy_error.lock().unwrap() = loaded.err();
    let app = Arc::new(app);
    app.refresh_catalogue();
    {
        // Engine-side periodic discovery/metadata refresh; no panel polling.
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                app.scheduler_tick(crate::util::unix_timestamp());
            }
        });
    }
    let port: u16 = std::env::var("FORGE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(PORT);
    let server = tiny_http::Server::http(("127.0.0.1", port)).expect("bind server");
    println!(
        "Forge engine on http://127.0.0.1:{port}  (project: {})",
        app.active_project.lock().unwrap()
    );
    for req in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || handle(&app, req));
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::{claude_activity, stream_agent_output};
    use crate::app::{App, Ctx, FORGE_DIR, PlanMode, State, WorkerGuard};
    use crate::http::handle;
    use crate::plan::{default_settings, edit_plan, mutate_queue};
    use crate::prompts::PLANNER_PROMPT;
    use crate::util::{fmt_duration, unix_timestamp};
    use serde_json::{Value, json};
    use std::fs;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn seed_mock_plan(ctx: &Ctx) {
        ctx.ensure_forge_dir();
        ctx.mock_agent("planner").unwrap();
        let plan: Value = serde_json::from_slice(&fs::read(ctx.forge_path("plan-candidate.json")).unwrap()).unwrap();
        ctx.publish_plan(&plan, true).unwrap();
    }

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
            settings["mock_usage"] = json!({"total": 5, "model": "model"});
        }
        let previous = test.app.load_plan().unwrap();
        test.app.revise_worker(&previous, "Keep the stage");
        let updated = test.app.load_plan().unwrap();
        assert_eq!(updated["planner_usage"]["mock"]["total_tokens"], 15);
        assert_eq!(updated["planner_usage"]["mock"]["calls"], 2);
        assert_eq!(updated["planner_usage"]["mock"]["models"]["model"], 15);
    }

    fn editable_stage(id: i64) -> Value {
        json!({"id": id, "title": "Repaired title", "instructions": "Repaired instructions",
            "acceptance": "", "commit": "feat: repair stage"})
    }

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
            assert_eq!(state["plan"], test.app.load_plan().unwrap());
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
            assert!(prompt.ends_with("Do NOT implement anything, do not modify any other file. Only write .forge/plan-candidate.json."));
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
            assert!(prompt.ends_with("Only write .forge/plan-candidate.json."));
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
                assert_eq!(test.app.app.settings.lock().unwrap()["mock_planner_prompt"],
                    PLANNER_PROMPT.replace("{goal}", goal).replace("{plan_path}", ".forge/plan-candidate.json"));
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
        assert_eq!(history[1]["text"], "plan revised with 2 stages");
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
        assert_eq!(state["plan"], expected);
        assert_eq!(first.app.load_plan().unwrap(), first_plan);
        let history = second.app.read_history();
        assert_eq!(history[0]["kind"], "plan");
        assert_eq!(history[0]["text"], "plan edited by user");
        assert_eq!(history[0]["goal"], "Edited goal");
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

    struct QueueTest {
        app: Ctx,
        path: PathBuf,
    }

    impl QueueTest {
        fn new(auto_approve: bool) -> Self {
            Self::with_engine(auto_approve, None)
        }

        fn with_engine(auto_approve: bool, engine: Option<Arc<App>>) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "forge-queue-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst),
            ));
            fs::create_dir(&path).unwrap();
            let mut settings = default_settings();
            settings["planner"] = json!("mock");
            settings["implementer"] = json!("mock");
            settings["reviewer"] = json!("mock");
            settings["auto_push"] = json!(false);
            settings["queue_auto_approve"] = json!(auto_approve);
            let engine = engine.unwrap_or_else(|| Arc::new(App::new(&path.display().to_string(), settings)));
            let app = engine.context(&path.display().to_string());
            app.git(&["init", "-q"]).unwrap();
            for (key, value) in [
                ("user.name", "Forge Test"), ("user.email", "test@example.invalid"),
                ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null"),
            ] {
                app.git(&["config", key, value]).unwrap();
            }
            app.git(&["commit", "--allow-empty", "-qm", "initial"]).unwrap();
            let mut queue = json!({"items": []});
            for goal in ["first goal", "second goal"] {
                mutate_queue(&mut queue, "add", &json!({"goal": goal})).unwrap();
            }
            app.save_queue(&queue);
            Self { app, path }
        }

        fn start(&self) {
            self.app.acquire_busy().unwrap();
            self.app.session.queue_active.store(true, Ordering::SeqCst);
            self.app.queue_worker(None);
        }

        fn statuses(&self) -> Vec<String> {
            self.app.load_queue()["items"].as_array().unwrap().iter()
                .map(|item| item["status"].as_str().unwrap().to_string()).collect()
        }
    }

    impl Drop for QueueTest {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
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
        assert_eq!(plan["stages"][0]["usage"], expected(4));
        assert_eq!(plan["stages"][1]["status"], "committed");
        assert_eq!(plan["stages"][1]["usage"], expected(2));
        assert_eq!(plan["usage"], expected(6));
        let history = test.app.read_history();
        for role in ["planner", "implementer", "fixer", "reviewer"] {
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
    fn resumed_run_keeps_usage_and_tracks_each_invocations_model() {
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
            "input_tokens": 0, "output_tokens": 0, "total_tokens": 20, "calls": 2,
            "models": {"configured-model": 10},
        }));
        assert_eq!(blocked["stages"][0]["usage"], blocked["usage"]);
        assert!(blocked["stages"][1].get("usage").is_none());
        test.app.app.settings.lock().unwrap()["mock_usage"]["model"] = json!("reported-model");
        test.app.run_worker();
        let plan = test.app.load_plan().unwrap();
        assert_eq!(plan["status"], "done");
        assert_eq!(plan["stages"][0]["usage"]["mock"], json!({
            "input_tokens": 0, "output_tokens": 0, "total_tokens": 40, "calls": 4,
            "models": {"configured-model": 10, "reported-model": 20},
        }));
        assert_eq!(plan["usage"]["mock"], json!({
            "input_tokens": 0, "output_tokens": 0, "total_tokens": 60, "calls": 6,
            "models": {"configured-model": 10, "reported-model": 40},
        }));
    }

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
                    assert_eq!(report["usage"], expected_usage(4));
                    assert_eq!(report["planner_usage"], expected_usage(1));
                    completion.push_str(" — 2 commits — mock: 500 tokens");
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
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0]["round"], 1);
            assert_eq!(reviews[0]["approved"], true);
            assert_eq!(reviews[0]["issues"], json!([]));
            assert!(reviews[0]["unix"].as_i64().unwrap() > 0);
            let summary = reviews[0]["summary"].as_str().unwrap();
            assert!(!summary.is_empty());
            assert_eq!(stage["last_verdict"], json!({
                "approved": true, "summary": summary, "issues": [], "notes": [], "checks": [],
            }));
            assert!(entries.iter().any(|entry| entry["kind"] == "review"
                && entry["text"] == format!("stage {} approved: {summary}", stage["id"])));
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
    fn session_aliases_and_selection_preserve_existing_work() {
        let test = QueueTest::new(true);
        let engine = &test.app.app;
        let alias = test.path.join("alias");
        std::os::unix::fs::symlink(&test.path, &alias).unwrap();
        let session = engine.session(&alias.display().to_string());
        assert!(Arc::ptr_eq(&session, &test.app.session));
        test.app.acquire_busy().unwrap();
        session.state.lock().unwrap().goal = "in flight".into();
        test.app.set_phase("running");
        test.app.set_step(Some(2), "reviewing");
        engine.set_project(&alias.display().to_string()).unwrap();
        assert!(engine.set_project(&test.path.join(FORGE_DIR).display().to_string()).is_err());
        assert_eq!(*engine.active_project.lock().unwrap(), test.app.project());
        assert_eq!(engine.open_sessions().len(), 1);
        assert_eq!(session.state.lock().unwrap().phase, "running");
        let summary = engine.session_summaries(test.app.project());
        assert_eq!(summary[0]["goal"], "in flight");
        assert_eq!(summary[0]["current_step"], "reviewing");
        assert_eq!(summary[0]["name"], test.path.file_name().unwrap().to_string_lossy().as_ref());
        assert!(session.busy.load(Ordering::SeqCst));
    }

    fn api_request(engine: &Arc<App>, method: &str, path: &str, body: Value) -> (u16, Value) {
        use std::io::Read as _;
        let server = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
        let address = server.server_addr().to_ip().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| handle(engine, server.recv().unwrap()));
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let body = body.to_string();
            write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, serde_json::from_str(body).unwrap())
        })
    }

    #[test]
    fn catalogue_api_is_responsive_shared_and_settings_are_atomic() {
        use crate::catalogue::{Catalogue, Discovery, Provider, Policy, Probe, Failure, FailureKind};
        use crate::catalogue_process::Budget;
        use std::sync::atomic::AtomicUsize;
        struct Slow(AtomicUsize);
        impl Discovery for Slow {
            fn probe(&self, _: Provider, _: &Policy, budget: Budget) -> Probe {
                self.0.fetch_add(1, Ordering::SeqCst);
                loop {
                    if let Err(error) = budget.check() { return Probe { cli_version: Some("fixture 1".into()), result: Err(error) }; }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        let first = QueueTest::new(false);
        let slow = Arc::new(Slow(AtomicUsize::new(0)));
        let mut app = App::new(first.app.project(), default_settings());
        app.catalogue = Catalogue::new(slow.clone(), first.path.join("cache"), Duration::from_secs(10));
        app.model_policy_path = Some(first.path.join("model-policy.json"));
        let app = Arc::new(app);
        let (code, state) = api_request(&app, "GET", "/api/state", json!({}));
        assert_eq!(code, 200); assert_eq!(state["model_catalogue"]["providers"][0]["status"], "pending");
        assert_eq!(slow.0.load(Ordering::SeqCst), 0); // App::new does not run processes.
        let mut policy = json!(Policy::default()); policy["policy_revision"] = json!("2");
        policy["entries"] = json!([{"provider":"codex","model":"exact-id","tier":"basic","relative_cost_preference":1}]);
        assert_eq!(api_request(&app,"POST","/api/settings",json!({"model_catalogue":policy})).0,200);
        assert_eq!(crate::catalogue::load_policy(app.model_policy_path.as_ref().unwrap()).unwrap().unwrap().policy_revision,"2");
        assert_eq!(api_request(&app,"POST","/api/models/refresh",json!({})).0,202);
        let second = QueueTest::with_engine(false, Some(app.clone()));
        let start = Instant::now();
        for project in [first.app.project(), second.app.project()] {
            let (code,state)=api_request(&app,"GET",&format!("/api/state?project={project}"),json!({}));
            assert_eq!(code,200); assert_eq!(state["model_catalogue"]["refreshing"],true);
            assert!(state["model_catalogue"].to_string().len()<3000);
            assert_eq!(state["settings"]["model_catalogue"]["entries"],json!([]));
        }
        assert!(start.elapsed()<Duration::from_secs(1));
        assert_eq!(api_request(&app,"POST","/api/models/refresh",json!({})).1["started"],false);
        let (_,details)=api_request(&app,"GET","/api/models",json!({}));
        assert_eq!(details["options"][0]["availability"],"configured_unverified");
        assert_eq!(details["options"][0]["provenance"],"configured"); assert!(details["options"][0]["pricing"].is_null());
        let (_,option)=api_request(&app,"GET","/api/models?provider=codex&model=exact-id&effort=high",json!({}));
        assert_eq!(option["eligible"],false);
        assert_eq!(api_request(&app,"GET","/api/models?provider=bogus&model=x",json!({})).0,400);
        assert_eq!(api_request(&app,"POST","/api/models",json!({})).0,404);
        let mut changed=policy.clone(); changed["policy_revision"]=json!("3");
        assert_eq!(api_request(&app,"POST","/api/settings",json!({"model_catalogue":changed})).0,409);
        assert_eq!(api_request(&app,"POST","/api/models/cancel",json!({})).0,202);
        let deadline=Instant::now()+Duration::from_secs(3);
        while app.catalogue.running() { assert!(Instant::now()<deadline); std::thread::sleep(Duration::from_millis(5)); }
        assert_eq!(slow.0.load(Ordering::SeqCst),2);
        let mut invalid=policy.clone(); invalid["entries"][0]["tier"]=json!("magic");
        assert_eq!(api_request(&app,"POST","/api/settings",json!({"model_catalogue":invalid,"auto_push":false})).0,400);
        assert_eq!(app.settings.lock().unwrap()["auto_push"],true);
        let mut invalid=changed.clone(); invalid["entries"][0]["effort"]=json!("high");
        assert_eq!(api_request(&app,"POST","/api/settings",json!({"model_catalogue":invalid})).0,400);
        app.catalogue.observe(&Policy::from_settings(&app.settings.lock().unwrap()).unwrap(),Provider::Codex,"exact-id",Err(Failure::new(FailureKind::Auth)));
        let (_,option)=api_request(&app,"GET","/api/models?provider=codex&model=exact-id",json!({}));
        assert_eq!(option["availability"],"unavailable"); assert_eq!(option["eligible"],false);
    }

    #[test]
    fn metadata_api_reports_freshness_and_never_blocks_configured_routing() {
        use crate::catalogue::{Catalogue, Discovery, Policy, Probe, Provider, Failure, FailureKind};
        use crate::catalogue_process::Budget;
        use crate::metadata::{Clock, Fetch, FetchRequest, FetchResponse, Service};
        struct Offline;
        impl Fetch for Offline {
            fn fetch(&self, _: &FetchRequest) -> Result<FetchResponse, String> { Err("offline".into()) }
        }
        struct FixedClock;
        impl Clock for FixedClock {
            fn now_unix(&self) -> i64 { 1_000 }
        }
        struct NoDiscovery;
        impl Discovery for NoDiscovery {
            fn probe(&self, _: Provider, _: &Policy, _: Budget) -> Probe {
                Probe { cli_version: None, result: Err(Failure::new(FailureKind::Unsupported)) }
            }
        }
        let test = QueueTest::new(false);
        let mut app = App::new(test.app.project(), default_settings());
        app.catalogue = Catalogue::new(Arc::new(NoDiscovery), test.path.join("cache"), Duration::from_secs(2));
        app.metadata = Service::new(Arc::new(Offline), Arc::new(FixedClock), test.path.join("metadata.json"));
        let app = Arc::new(app);
        let mut policy = json!(Policy::default());
        policy["policy_revision"] = json!("2");
        policy["entries"] = json!([{"provider":"codex","model":"exact-id","tier":"strong","relative_cost_preference":1}]);
        assert_eq!(api_request(&app, "POST", "/api/settings", json!({"model_catalogue": policy})).0, 200);
        // A configured-only model triggers research; the offline source fails
        // into backoff state without blocking anything.
        assert_eq!(api_request(&app, "POST", "/api/models/metadata/refresh", json!({})).1["started"], true);
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.metadata.running() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let (_, state) = api_request(&app, "GET", "/api/state", json!({}));
        let meta = &state["model_catalogue"]["metadata"];
        assert_eq!(meta["records"], 0);
        assert_eq!(meta["source_errors"], 1);
        assert_eq!(meta["last_requests"], 1);
        assert_eq!(meta["provenance"], "official");
        let (_, details) = api_request(&app, "GET", "/api/models", json!({}));
        assert_eq!(details["metadata"]["sources"][0]["error"], "offline");
        assert!(details["metadata"]["sources"][0]["next_attempt_unix"].as_i64().unwrap() > 1_000);
        // Deferred/failed research never prevents configured tiers from
        // routing, and configured cost preference is never shown as a price.
        assert_eq!(details["options"][0]["eligible"], true);
        assert_eq!(details["options"][0]["availability"], "configured_unverified");
        assert!(details["options"][0]["pricing"].is_null());
        // Research off means cache-only; routing is still unaffected.
        policy["policy_revision"] = json!("3");
        policy["metadata_research"] = json!(false);
        assert_eq!(api_request(&app, "POST", "/api/settings", json!({"model_catalogue": policy})).0, 200);
        assert_eq!(api_request(&app, "POST", "/api/models/metadata/refresh", json!({})).1["started"], false);
        let (_, details) = api_request(&app, "GET", "/api/models", json!({}));
        assert_eq!(details["options"][0]["eligible"], true);
    }

    fn wait_for_worker(ctx: &Ctx) {
        let started = Instant::now();
        while ctx.session.busy.load(Ordering::SeqCst) {
            assert!(started.elapsed() < Duration::from_secs(5), "worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
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
    fn api_routes_projects_while_another_session_is_busy() {
        let first = QueueTest::new(true);
        let engine = &first.app.app;
        let second = QueueTest::with_engine(true, Some(Arc::clone(engine)));
        first.app.acquire_busy().unwrap();
        let _worker = WorkerGuard(&first.app.session);
        first.app.set_phase("running");
        first.app.set_step(Some(1), "reviewing");
        first.app.session.state.lock().unwrap().goal = "first project's work".into();
        first.app.session.queue_active.store(true, Ordering::SeqCst);
        first.app.save_plan(&json!({"goal": "first project's work", "stages": []})).unwrap();
        let post = |path: &str, body: Value| api_request(engine, "POST", path, body).0;
        for route in ["/api/project", "/api/project/select"] {
            assert_eq!(post(route, json!({"path": second.app.project()})), 200);
            assert_eq!(post(route, json!({"path": first.app.project()})), 200);
        }
        assert_eq!(post("/api/project/select", json!({"path": second.app.project()})), 200);
        assert_eq!(post("/api/plan", json!({"goal": "default target"})), 200);
        wait_for_worker(&second.app);
        assert_eq!(second.app.load_plan().unwrap()["goal"], "default target");
        assert_eq!(post("/api/plan", json!({"project": first.app.project(), "goal": "busy target"})), 409);

        // Explicit writes to B keep working when the active project is busy A.
        assert_eq!(post("/api/project", json!({"path": first.app.project()})), 200);
        let target = json!({"project": second.app.project()});
        assert_eq!(post("/api/plan", json!({"project": second.app.project(), "goal": "explicit target"})), 200);
        wait_for_worker(&second.app);
        assert_eq!(post("/api/approve", target.clone()), 200);
        assert_eq!(post("/api/run", target.clone()), 200);
        wait_for_worker(&second.app);
        assert_eq!(second.app.load_plan().unwrap()["goal"], "explicit target");
        assert_eq!(post("/api/queue/start", target.clone()), 200);
        wait_for_worker(&second.app);
        assert!(second.statuses().is_empty());
        assert_eq!(first.statuses(), ["queued", "queued"]);

        // Decode project paths independently of other query parameters.
        let alias = second.path.join("project + space");
        std::os::unix::fs::symlink(&second.path, &alias).unwrap();
        let encoded = alias.display().to_string().replace('/', "%2F").replace('+', "%2B").replace(' ', "%20");
        let get = |route: &str| api_request(engine, "GET", &format!("{route}?offset=2&project={encoded}"), json!({}));
        let (status, state) = get("/api/state");
        assert_eq!(status, 200);
        assert_eq!(state["project"], second.app.project());
        assert_eq!(state["active_project"], first.app.project());
        assert_eq!(state["phase"], "done");
        assert_eq!(state["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(state["sessions"][0]["project"], first.app.project());
        assert_eq!(state["sessions"][0]["busy"], true);
        assert_eq!(state["sessions"][0]["queued"], 2);
        assert_eq!(state["sessions"][0]["goal"], "first project's work");
        assert_eq!(state["sessions"][0]["current_step"], "reviewing");
        assert_eq!(state["sessions"][1]["busy"], false);
        assert_eq!(state["sessions"][1]["queued"], 0);
        fs::write(second.app.forge_path("agent.log"), "B log").unwrap();
        assert_eq!(get("/api/agent_log").1["log"], "log");
        fs::write(second.path.join("mock.txt"), "B diff\n").unwrap();
        assert!(get("/api/diff").1["diff"].as_str().unwrap().contains("+B diff"));
        assert_eq!(post("/api/stop", target.clone()), 200);
        assert!(!first.app.session.stop_requested.load(Ordering::SeqCst));
        assert!(first.app.session.queue_active.load(Ordering::SeqCst));
        assert_eq!(post("/api/reset_plan", target), 200);
        assert!(second.app.load_plan().is_none());
        assert_eq!(first.app.load_plan().unwrap()["goal"], "first project's work");
        assert_eq!(post("/api/project/select", json!({"path": second.app.forge_path("")})), 400);
        assert_eq!(post("/api/stop", json!({"project": 42})), 400);
        assert_eq!(api_request(engine, "GET", "/api/state?project=%ZZ", json!({})).0, 400);
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
            assert_eq!(test.statuses(), ["failed", "queued"], "{role}");
            assert_eq!(test.app.session.state.lock().unwrap().phase, "failed");
            assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
            assert!(!test.app.session.busy.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn queue_blocked_stage_halts_and_releases_busy() {
        let test = QueueTest::new(true);
        // An exhausted review budget exercises the blocked-stage exit.
        test.app.app.settings.lock().unwrap()["max_fix_rounds"] = json!(-1);
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

    #[test]
    fn contradictory_review_runs_fixer_without_committing() {
        let test = QueueTest::new(true);
        let initial_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
        let issues = json!(["Add the missing line."]);
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["max_fix_rounds"] = json!(1);
            settings["mock_verdicts"] = json!([
                {"approved": true, "summary": "Looks good.", "issues": issues},
                {"approved": false, "issues": issues},
            ]);
        }
        seed_mock_plan(&test.app);
        test.app.run_worker();
        let plan = test.app.load_plan().unwrap();
        let stage = &plan["stages"][0];
        assert_eq!(stage["status"], "blocked");
        assert_eq!(stage["rounds"], 2);
        assert_eq!(stage["last_verdict"]["approved"], false);
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 2);
        for review in reviews {
            assert_eq!(review["approved"], false);
            assert_eq!(review["issues"], issues);
            assert_eq!(review["notes"], json!([]));
            assert_eq!(review["checks"], json!([]));
        }
        assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), initial_head);
        assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
            "work by implementer\nwork by fixer\n");
        let history = test.app.read_history();
        let events = history.as_array().unwrap();
        assert!(!events.iter().any(|entry| entry["kind"] == "review"
            && entry["text"].as_str().unwrap().contains("approved")));
        assert!(events.iter().any(|entry| entry["kind"] == "review"
            && entry["text"] == "stage 1 changes requested: Add the missing line."));
    }

    #[test]
    fn clean_initial_review_commits_without_a_fixer() {
        assert!(default_settings().get("apply_review_notes").is_none());
        for legacy_setting in [None, Some(true), Some(false)] {
            for max_rounds in [0, 2] {
                for summary in ["Verified the implementation.", ""] {
                    let test = QueueTest::new(true);
                    let checks = json!(["Inspected mock.txt.", "Verified the appended line."]);
                    {
                        let mut settings = test.app.app.settings.lock().unwrap();
                        if let Some(value) = legacy_setting {
                            settings["apply_review_notes"] = json!(value);
                        }
                        settings["max_fix_rounds"] = json!(max_rounds);
                        settings["mock_verdicts"] = json!([
                            {"approved": true, "summary": summary, "issues": [], "notes": [],
                             "checks": ["Inspected mock.txt.", null, "Verified the appended line."]},
                        ]);
                    }
                    seed_mock_plan(&test.app);
                    test.app.run_worker();
                    let plan = test.app.load_plan().unwrap();
                    let stage = &plan["stages"][0];
                    assert_eq!(stage["status"], "committed");
                    assert_eq!(stage["rounds"], 1);
                    assert_eq!(stage["last_verdict"], json!({
                        "approved": true, "summary": summary, "issues": [],
                        "notes": [], "checks": checks,
                    }));
                    let reviews = stage["reviews"].as_array().unwrap();
                    assert_eq!(reviews.len(), 1);
                    assert_eq!(reviews[0], json!({
                        "round": 1, "approved": true, "summary": summary, "issues": [],
                        "notes": [], "checks": checks, "unix": reviews[0]["unix"],
                        "role": "reviewer", "revision": plan["revision"], "attempt_id": stage["attempt_id"],
                    }));
                    assert!(reviews[0]["unix"].as_i64().unwrap() > 0);
                    let message = if summary.is_empty() {
                        "stage 1 approved by reviewer".to_string()
                    } else {
                        format!("stage 1 approved: {summary}")
                    };
                    let history = test.app.read_history();
                    let events: Vec<_> = history.as_array().unwrap().iter().filter(|entry|
                        entry["kind"] == "review" && entry["stage"] == 1).collect();
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0]["text"], message);
                    let committed_file = format!("{}:mock.txt", stage["sha"].as_str().unwrap());
                    assert_eq!(test.app.git(&["show", &committed_file]).unwrap(), "work by implementer");
                    assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
                        "work by implementer\nwork by implementer\n");
                    assert!(test.app.app.settings.lock().unwrap().get("mock_fixer_prompts").is_none());
                }
            }
        }
    }

    #[test]
    fn legacy_notes_preserve_feedback_and_checks_as_changes_requested() {
        for approved in [false, true] {
            let test = QueueTest::new(true);
            let notes = json!(["Clarify the output.", "Simplify the helper."]);
            let checks = json!(["Inspected mock.txt."]);
            {
                let mut settings = test.app.app.settings.lock().unwrap();
                settings["max_fix_rounds"] = json!(0);
                settings["mock_verdicts"] = json!([
                    {"approved": approved, "summary": "Inspected the implementation.", "issues": [],
                     "notes": ["Clarify the output.", 42, "Simplify the helper."],
                     "checks": ["Inspected mock.txt.", null]},
                ]);
            }
            seed_mock_plan(&test.app);
            test.app.run_worker();
            let plan = test.app.load_plan().unwrap();
            let stage = &plan["stages"][0];
            assert_eq!(stage["status"], "blocked");
            let expected = json!({
                "approved": false, "summary": "Inspected the implementation.",
                "issues": [], "notes": notes, "checks": checks,
            });
            assert_eq!(stage["last_verdict"], expected);
            let mut review = stage["reviews"][0].clone();
            assert_eq!(review.as_object_mut().unwrap().remove("round"), Some(json!(1)));
            assert!(review.as_object_mut().unwrap().remove("unix").unwrap().as_i64().unwrap() > 0);
            assert_eq!(review.as_object_mut().unwrap().remove("role"), Some(json!("reviewer")));
            assert_eq!(review.as_object_mut().unwrap().remove("revision"), Some(plan["revision"].clone()));
            assert_eq!(review.as_object_mut().unwrap().remove("attempt_id"), Some(stage["attempt_id"].clone()));
            assert_eq!(review, expected);
            let history = test.app.read_history();
            let events: Vec<_> = history.as_array().unwrap().iter().filter(|entry|
                entry["kind"] == "review" && entry["stage"] == 1).collect();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["text"],
                "stage 1 changes requested: Clarify the output.; Simplify the helper.");
        }
    }

    #[test]
    fn requested_changes_repeat_fix_and_review_until_clean() {
        for legacy_setting in [None, Some(true), Some(false)] {
            for field in ["issues", "notes", "both"] {
                for requested_rounds in [1, 2] {
                    let test = QueueTest::new(true);
                    let mut request = json!({"approved": true, "issues": [], "notes": []});
                    if field != "notes" {
                        request["issues"] = json!(["Clarify the output.", "Clarify the output."]);
                    }
                    if field != "issues" {
                        request["notes"] = json!(["Clarify the output.", "Simplify the helper."]);
                    }
                    let mut verdicts = vec![request.clone(); requested_rounds];
                    verdicts.push(json!({"approved": true, "issues": [], "notes": []}));
                    {
                        let mut settings = test.app.app.settings.lock().unwrap();
                        if let Some(value) = legacy_setting {
                            settings["apply_review_notes"] = json!(value);
                        }
                        settings["max_fix_rounds"] = json!(requested_rounds);
                        settings["mock_verdicts"] = json!(verdicts);
                    }
                    seed_mock_plan(&test.app);
                    test.app.run_worker();
                    let plan = test.app.load_plan().unwrap();
                    let stage = &plan["stages"][0];
                    assert_eq!(stage["status"], "committed");
                    assert_eq!(stage["rounds"], requested_rounds + 1);
                    let reviews = stage["reviews"].as_array().unwrap();
                    assert_eq!(reviews.len(), requested_rounds + 1);
                    for (i, review) in reviews.iter().enumerate() {
                        assert_eq!(review["round"], i + 1);
                        assert_eq!(review["approved"], i == requested_rounds);
                        if i < requested_rounds {
                            assert_eq!(review["issues"], request["issues"]);
                            assert_eq!(review["notes"], request["notes"]);
                        } else {
                            assert_eq!(review["issues"], json!([]));
                            assert_eq!(review["notes"], json!([]));
                        }
                    }
                    assert_eq!(stage["last_verdict"], json!({
                        "approved": true, "summary": "", "issues": [], "notes": [], "checks": [],
                    }));
                    let fixed_content = format!("work by implementer\n{}", "work by fixer\n".repeat(requested_rounds));
                    let committed_file = format!("{}:mock.txt", stage["sha"].as_str().unwrap());
                    assert_eq!(test.app.git(&["show", &committed_file]).unwrap(), fixed_content.trim_end());
                    assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
                        format!("{fixed_content}work by implementer\n"));
                    let history = test.app.read_history();
                    let events: Vec<_> = history.as_array().unwrap().iter().filter(|entry|
                        entry["kind"] == "review" && entry["stage"] == 1).collect();
                    assert_eq!(events.len(), reviews.len());
                    for (i, event) in events.iter().enumerate() {
                        let text = event["text"].as_str().unwrap();
                        if i < requested_rounds {
                            assert!(text.starts_with("stage 1 changes requested:"));
                            assert!(!text.contains("approved"));
                        } else {
                            assert_eq!(text, "stage 1 approved by reviewer");
                        }
                    }
                    let settings = test.app.app.settings.lock().unwrap();
                    let prompts = settings["mock_fixer_prompts"].as_array().unwrap();
                    assert_eq!(prompts.len(), requested_rounds);
                    for prompt in prompts {
                        let prompt = prompt.as_str().unwrap();
                        assert_eq!(prompt.matches("- Clarify the output.\n").count(), 1);
                        assert_eq!(prompt.matches("- Simplify the helper.\n").count(), usize::from(field != "issues"));
                        assert!(!prompt.contains("reviewer rejected without details"));
                    }
                }
            }
        }
    }

    #[test]
    fn follow_up_prompts_use_only_the_latest_stage_feedback_verbatim() {
        for new_second_round_request in [false, true] {
            let test = QueueTest::new(true);
            let markers = "{goal} {plan_overview} {sid} {title} {instructions} {acceptance} {forge_dir} {verdict_path} {issues} {review_context}";
            let issue = format!("Fix Unicode 界🙂 output.\nKeep these literals: {markers}");
            let note = format!("Legacy request: preserve indentation.\n  Zażółć gęślą {markers}");
            let summary = format!("Reviewed the output.\nRésumé 界🙂 {markers}");
            let check = format!("cargo test: failed\nDetails 界🙂 {markers}");
            let latest = format!("New defect: handle empty input.\nEvidence 界🙂 {markers}");
            let latest_summary = "Previous output requests resolved; empty input still fails.";
            let latest_check = "cargo test empty_input: failed";
            let foreign = "OTHER STAGE FEEDBACK MUST STAY LOCAL";
            let clean = json!({"approved": true, "summary": "Verified all criteria and fixes.",
                "issues": [], "notes": [], "checks": ["cargo build: passed", "cargo test: passed"]});
            let mut verdicts = vec![json!({
                // Contradictory approval and duplicate legacy notes must be normalized.
                "approved": true, "summary": summary, "issues": [issue, null, issue],
                "notes": [issue, note, 42], "checks": [check, false],
            })];
            if new_second_round_request {
                verdicts.push(json!({"approved": false, "summary": latest_summary,
                    "issues": [latest], "checks": [latest_check]}));
            }
            verdicts.push(clean.clone());
            verdicts.push(clean);
            let fixes = if new_second_round_request { 2 } else { 1 };
            {
                let mut settings = test.app.app.settings.lock().unwrap();
                settings["max_fix_rounds"] = json!(fixes);
                settings["mock_verdicts"] = json!(verdicts);
                settings["mock_reviewer_prompts"] = json!([]);
            }
            seed_mock_plan(&test.app);
            let mut plan = test.app.load_plan().unwrap();
            plan["goal"] = json!(format!("Goal with literal {markers}"));
            plan["stages"][0]["title"] = json!("Title with literal {instructions}");
            plan["stages"][0]["instructions"] = json!("Instructions with literal {acceptance}");
            plan["stages"][0]["acceptance"] = json!("Criteria with literal {verdict_path}");
            let other_verdict = json!({"approved": false, "summary": foreign,
                "issues": [foreign], "notes": [], "checks": [foreign]});
            plan["stages"][1]["last_verdict"] = other_verdict.clone();
            plan["stages"][1]["reviews"] = json!([other_verdict]);
            test.app.save_plan(&plan).unwrap();
            test.app.run_worker();

            let plan = test.app.load_plan().unwrap();
            let stage = &plan["stages"][0];
            assert_eq!(stage["status"], "committed");
            assert_eq!(stage["rounds"], fixes + 1);
            assert_eq!(stage["reviews"].as_array().unwrap().len(), fixes + 1);
            assert_eq!(test.app.git(&["show", &format!("{}:mock.txt", stage["sha"].as_str().unwrap())]).unwrap(),
                format!("work by implementer\n{}", "work by fixer\n".repeat(fixes)).trim_end());
            assert_eq!(plan["stages"][1]["reviews"][0], other_verdict);

            let settings = test.app.app.settings.lock().unwrap();
            let reviews = settings["mock_reviewer_prompts"].as_array().unwrap();
            let fixers = settings["mock_fixer_prompts"].as_array().unwrap();
            assert_eq!(reviews.len(), fixes + 2); // Includes the next stage's initial review.
            assert_eq!(fixers.len(), fixes);
            let initial = reviews[0].as_str().unwrap();
            assert!(initial.contains("REVIEW ROUND: 1 (current attempt) — Initial review."));
            assert!(initial.contains("No previous review findings for this stage."));
            assert!(!initial.contains("BEGIN PREVIOUS REVIEW CONTEXT"));
            for prompt in reviews[..fixes + 1].iter().chain(fixers) {
                let prompt = prompt.as_str().unwrap();
                assert!(!prompt.contains(foreign));
                assert!(prompt.contains("Title with literal {instructions}"));
                assert!(prompt.contains("Instructions with literal {acceptance}"));
                assert!(prompt.contains("Criteria with literal {verdict_path}"));
            }
            let preceding = format!(
                "BEGIN PREVIOUS REVIEW CONTEXT\nEffective decision: changes requested\nSummary:\n{summary}\nOutstanding change requests (including legacy notes):\n- {issue}\n- {note}\nPrevious checks (must be verified again):\n- {check}\nEND PREVIOUS REVIEW CONTEXT\n"
            );
            for prompt in [&reviews[1], &fixers[0]] {
                let prompt = prompt.as_str().unwrap();
                assert!(prompt.contains("REVIEW ROUND: 2 (current attempt) — Re-review."));
                assert!(prompt.contains("immediately preceding review in this attempt"));
                assert!(prompt.contains(&preceding));
                assert_eq!(prompt.matches(&issue).count(), 1);
                assert_eq!(prompt.matches(&note).count(), 1);
            }
            assert!(fixers[0].as_str().unwrap().contains(&format!("Goal with literal {markers}")));
            if new_second_round_request {
                let preceding = format!(
                    "BEGIN PREVIOUS REVIEW CONTEXT\nEffective decision: changes requested\nSummary:\n{latest_summary}\nOutstanding change requests (including legacy notes):\n- {latest}\nPrevious checks (must be verified again):\n- {latest_check}\nEND PREVIOUS REVIEW CONTEXT\n"
                );
                for prompt in [&reviews[2], &fixers[1]] {
                    let prompt = prompt.as_str().unwrap();
                    assert!(prompt.contains("REVIEW ROUND: 3 (current attempt) — Re-review."));
                    assert!(prompt.contains(&preceding));
                    for old_feedback in [&issue, &note, &summary, &check] {
                        assert!(!prompt.contains(old_feedback));
                    }
                }
                assert_eq!(stage["reviews"][1]["approved"], false);
            }
            let next_stage = reviews.last().unwrap().as_str().unwrap();
            assert!(next_stage.contains("REVIEW ROUND: 1 (current attempt) — Initial review."));
            assert!(next_stage.contains(foreign));
            for feedback in [&issue, &note, &summary, &check, &latest] {
                assert!(!next_stage.contains(feedback));
            }
        }
    }

    #[test]
    fn unresolved_requests_exhaust_budget_without_commit_or_later_stage() {
        for legacy_setting in [None, Some(true), Some(false)] {
            for field in ["issues", "notes"] {
                for max_rounds in [0, 1, 2] {
                    let test = QueueTest::new(true);
                    let initial_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
                    let mut request = json!({"approved": true, "issues": [], "notes": []});
                    request[field] = json!(["Clarify the output."]);
                    {
                        let mut settings = test.app.app.settings.lock().unwrap();
                        if let Some(value) = legacy_setting {
                            settings["apply_review_notes"] = json!(value);
                        }
                        settings["max_fix_rounds"] = json!(max_rounds);
                        settings["mock_verdicts"] = json!(vec![request.clone(); max_rounds + 1]);
                    }
                    seed_mock_plan(&test.app);
                    test.app.run_worker();
                    let plan = test.app.load_plan().unwrap();
                    let stage = &plan["stages"][0];
                    assert_eq!(stage["status"], "blocked");
                    assert_eq!(stage["rounds"], max_rounds + 1);
                    assert!(stage.get("sha").is_none());
                    assert_eq!(stage["last_verdict"]["approved"], false);
                    assert_eq!(stage["last_verdict"][field], request[field]);
                    let reviews = stage["reviews"].as_array().unwrap();
                    assert_eq!(reviews.len(), max_rounds + 1);
                    for review in reviews {
                        assert_eq!(review["approved"], false);
                        assert_eq!(review[field], request[field]);
                    }
                    assert_eq!(test.app.session.state.lock().unwrap().phase, "blocked");
                    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), initial_head);
                    assert_eq!(plan["stages"][1]["status"], "pending");
                    assert_eq!(plan["stages"][1]["rounds"], 0);
                    assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
                        format!("work by implementer\n{}", "work by fixer\n".repeat(max_rounds)));
                    let history = test.app.read_history();
                    let events: Vec<_> = history.as_array().unwrap().iter().filter(|entry|
                        entry["kind"] == "review").collect();
                    assert_eq!(events.len(), max_rounds + 1);
                    assert!(events.iter().all(|entry| entry["text"]
                        .as_str().unwrap().starts_with("stage 1 changes requested:")));
                }
            }
        }
    }

    #[test]
    fn review_notes_and_checks_default_to_empty_arrays() {
        for verdict in [
            json!({"approved": true}),
            json!({"approved": true, "notes": "not an array", "checks": {"invalid": true}}),
            json!({"approved": true, "notes": [42, null], "checks": [false, {}]}),
        ] {
            let test = QueueTest::new(true);
            test.app.app.settings.lock().unwrap()["mock_verdicts"] = json!([verdict]);
            seed_mock_plan(&test.app);
            test.app.run_worker();
            let plan = test.app.load_plan().unwrap();
            let stage = &plan["stages"][0];
            assert_eq!(stage["status"], "committed");
            assert_eq!(stage["reviews"].as_array().unwrap().len(), 1);
            for recorded in [&stage["last_verdict"], &stage["reviews"][0]] {
                assert_eq!(recorded["notes"], json!([]));
                assert_eq!(recorded["checks"], json!([]));
            }
        }
    }

    #[test]
    fn review_history_preserves_rejections_after_fix_and_approval() {
        let test = QueueTest::new(true);
        let summary = "界🙂".repeat(200);
        test.app.app.settings.lock().unwrap()["mock_verdicts"] = json!([
            {"approved": false, "summary": "Inspected output; a line is missing.",
             "issues": ["Add the missing line.", 42]},
            {"approved": true, "summary": summary, "issues": []},
        ]);
        seed_mock_plan(&test.app);
        test.app.run_worker();
        let plan = test.app.load_plan().unwrap();
        let stage = &plan["stages"][0];
        assert_eq!(stage["status"], "committed");
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 2);
        for (i, review) in reviews.iter().enumerate() {
            assert_eq!(review["round"], i + 1);
            assert_eq!(review["approved"], i == 1);
            assert!(review["unix"].as_i64().unwrap() > 0);
        }
        assert_eq!(reviews[0]["summary"], "Inspected output; a line is missing.");
        assert_eq!(reviews[0]["issues"], json!(["Add the missing line."]));
        assert_eq!(reviews[1]["summary"], summary);
        assert_eq!(reviews[1]["issues"], json!([]));
        assert_eq!(stage["last_verdict"], json!({
            "approved": true, "summary": summary, "issues": [], "notes": [], "checks": [],
        }));
        let history = test.app.read_history();
        let events: Vec<_> = history.as_array().unwrap().iter()
            .filter(|entry| entry["kind"] == "review" && entry["stage"] == 1).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["text"], "stage 1 changes requested: Add the missing line.");
        assert_eq!(events[1]["text"], format!("stage 1 approved: {}", "界🙂".repeat(150)));
        assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
            "work by implementer\nwork by fixer\nwork by implementer\n");
    }

    #[test]
    fn review_history_accumulates_after_exhaustion_and_resume() {
        let test = QueueTest::new(true);
        let resumed_rejection = json!({
            "approved": false,
            "summary": "The resumed implementation still misses an output line.",
            "issues": ["Add the missing output line."],
            "notes": [],
            "checks": ["Compared the resumed output with the acceptance criteria."],
        });
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["max_fix_rounds"] = json!(1);
            settings["mock_reviewer_prompts"] = json!([]);
            settings["mock_verdicts"] = json!([
                {"approved": false, "issues": []},
                "not readable JSON",
                resumed_rejection,
                {"approved": true, "issues": []},
            ]);
        }
        seed_mock_plan(&test.app);
        test.app.run_worker();
        let blocked = test.app.load_plan().unwrap();
        assert_eq!(blocked["stages"][0]["status"], "blocked");
        let previous = blocked["stages"][0]["reviews"].as_array().unwrap();
        assert_eq!(previous.len(), 2);
        for (i, review) in previous.iter().enumerate() {
            assert_eq!(review["round"], i + 1);
            assert_eq!(review["approved"], false);
            assert_eq!(review["summary"], "");
            assert!(review["unix"].as_i64().unwrap() > 0);
        }
        assert_eq!(previous[0]["issues"], json!(["reviewer rejected without details"]));
        let missing_verdict_issues = json!([
            "The previous review session failed to produce a verdict. Re-verify the implementation end to end."
        ]);
        assert_eq!(previous[1]["issues"], missing_verdict_issues);
        assert_eq!(blocked["stages"][0]["last_verdict"], json!({
            "approved": false, "summary": "", "issues": missing_verdict_issues, "notes": [], "checks": [],
        }));

        test.app.run_worker();
        let resumed = test.app.load_plan().unwrap();
        let stage = &resumed["stages"][0];
        assert_eq!(stage["status"], "committed");
        assert_eq!(stage["rounds"], 2);
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 4);
        assert_eq!(&reviews[..2], previous.as_slice());
        // Round numbers are local to the attempt; earlier attempts stay intact.
        assert_eq!(reviews[2]["round"], 1);
        for field in ["approved", "summary", "issues", "notes", "checks"] {
            assert_eq!(reviews[2][field], resumed_rejection[field]);
        }
        assert!(reviews[2]["unix"].as_i64().unwrap() > 0);
        assert_eq!(reviews[3]["round"], 2);
        assert_eq!(reviews[3]["approved"], true);
        assert_eq!(reviews[3]["summary"], "");
        assert_eq!(reviews[3]["issues"], json!([]));
        assert!(reviews[3]["unix"].as_i64().unwrap() > 0);
        assert_eq!(stage["last_verdict"], json!({
            "approved": true, "summary": "", "issues": [], "notes": [], "checks": [],
        }));
        assert!(test.app.read_history().as_array().unwrap().iter().any(|entry|
            entry["kind"] == "review" && entry["text"] == "stage 1 approved by reviewer"));
        let settings = test.app.app.settings.lock().unwrap();
        let prompts = settings["mock_reviewer_prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 5); // Four reviews of this stage, then the next stage.
        assert!(prompts[0].as_str().unwrap().contains("No previous review findings for this stage."));
        assert!(prompts[1].as_str().unwrap().contains("- reviewer rejected without details\n"));
        let resumed_prompt = prompts[2].as_str().unwrap();
        assert!(resumed_prompt.contains("REVIEW ROUND: 1 (current attempt) — Initial review."));
        assert!(resumed_prompt.contains("from this stage's previous attempt"));
        assert!(resumed_prompt.contains(missing_verdict_issues[0].as_str().unwrap()));
        assert!(!resumed_prompt.contains("reviewer rejected without details"));
        let fixers = settings["mock_fixer_prompts"].as_array().unwrap();
        assert_eq!(fixers.len(), 2); // One fix per attempt.
        let preceding = format!(
            "BEGIN PREVIOUS REVIEW CONTEXT\nEffective decision: changes requested\nSummary:\n{}\nOutstanding change requests (including legacy notes):\n- {}\nPrevious checks (must be verified again):\n- {}\nEND PREVIOUS REVIEW CONTEXT\n",
            resumed_rejection["summary"].as_str().unwrap(),
            resumed_rejection["issues"][0].as_str().unwrap(),
            resumed_rejection["checks"][0].as_str().unwrap(),
        );
        for prompt in [&prompts[3], &fixers[1]] {
            let prompt = prompt.as_str().unwrap();
            assert!(prompt.contains("REVIEW ROUND: 2 (current attempt) — Re-review."));
            assert!(prompt.contains("immediately preceding review in this attempt"));
            assert!(prompt.contains(&preceding));
            assert!(!prompt.contains("from this stage's previous attempt"));
            assert!(!prompt.contains(missing_verdict_issues[0].as_str().unwrap()));
            assert!(!prompt.contains("reviewer rejected without details"));
        }
        let next_stage_prompt = prompts[4].as_str().unwrap();
        assert!(next_stage_prompt.contains("REVIEW ROUND: 1 (current attempt) — Initial review."));
        assert!(next_stage_prompt.contains("No previous review findings for this stage."));
        assert!(!next_stage_prompt.contains(missing_verdict_issues[0].as_str().unwrap()));
        assert!(!next_stage_prompt.contains(resumed_rejection["issues"][0].as_str().unwrap()));
    }

    #[test]
    fn claude_assistant_text() {
        let event = json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "Inspecting the code."},
                {"type": "tool_result", "content": "ignored"},
                {"type": "text", "text": "界".repeat(401)}
            ]}
        });
        let activity = claude_activity(&event.to_string());
        assert_eq!(
            activity.lines,
            ["Inspecting the code.".to_string(), "界".repeat(400)]
        );
        assert!(activity.result.is_none());
    }

    #[test]
    fn claude_tool_use_summary_priority_and_single_line() {
        let event = json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "name": "Read", "input": {
                    "file_path": "src/main.rs", "command": "ignored"
                }},
                {"type": "tool_use", "name": "Bash", "input": {
                    "command": "cargo build\n  cargo test"
                }},
                {"type": "tool_use", "name": "Custom", "input": {"value": 42}}
            ]}
        });
        assert_eq!(
            claude_activity(&event.to_string()).lines,
            [
                "» Read: src/main.rs",
                "» Bash: cargo build cargo test",
                "» Custom: {\"value\":42}",
            ]
        );
    }

    #[test]
    fn claude_tool_summary_fields_and_unicode_limit() {
        for field in ["file_path", "command", "pattern", "description", "url"] {
            let event = json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use", "name": "Tool", "input": {field: "界".repeat(161)}
                }]}
            });
            assert_eq!(
                claude_activity(&event.to_string()).lines,
                [format!("» Tool: {}", "界".repeat(160))]
            );
        }
    }

    #[test]
    fn claude_result_keeps_full_text_for_history() {
        let result = "界".repeat(701);
        let event = json!({"type": "result", "subtype": "success", "result": result});
        let activity = claude_activity(&event.to_string());
        assert_eq!(activity.lines, [format!("✔ result: {}", "界".repeat(400))]);
        assert_eq!(activity.result.as_deref(), Some(result.as_str()));
    }

    #[test]
    fn claude_error_result_uses_subtype() {
        let event = r#"{"type":"result","is_error":true,"subtype":"error_during_execution","result":"failed"}"#;
        assert_eq!(
            claude_activity(event).lines,
            ["✔ result: error_during_execution"]
        );
        let event = r#"{"type":"result","subtype":"error_max_turns"}"#;
        assert_eq!(claude_activity(event).lines, ["✔ result: error_max_turns"]);
    }

    #[test]
    fn claude_init_and_ignored_events() {
        let event = r#"{"type":"system","subtype":"init","model":"claude-test"}"#;
        assert_eq!(
            claude_activity(event).lines,
            ["session started (model claude-test)"]
        );
        for event in [
            r#"{"type":"system","subtype":"other"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ignored"}]}}"#,
            r#"{"type":"assistant","message":{}}"#,
            r#"{"type":"unknown"}"#,
            "null",
        ] {
            assert!(claude_activity(event).lines.is_empty());
        }
    }

    #[test]
    fn claude_non_json_passes_through() {
        for line in ["plain output", "{invalid json", "", "  keep spacing  "] {
            assert_eq!(claude_activity(line).lines, [line]);
        }
    }

    fn capture_stream(input: &str, claude: bool) -> (String, String, State) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = Mutex::new(State {
            phase: String::new(),
            goal: String::new(),
            current_stage: None,
            current_step: String::new(),
            run_started_unix: 0,
            agent_role: String::new(),
            agent_tool: String::new(),
            agent_model: String::new(),
            model_selection: Value::Null,
            agent_started_unix: 0,
            agent_lines: 0,
            agent_last_line: String::new(),
        });
        let (tail, _) = stream_agent_output(input.as_bytes(), &log, &state, 600, claude).unwrap();
        let output = String::from_utf8(log.lock().unwrap().clone()).unwrap();
        (tail, output, state.into_inner().unwrap())
    }

    #[test]
    fn claude_stream_updates_activity_and_prefers_result_for_history() {
        let input = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Working\"}]}}\n",
            "{\"type\":\"result\",\"result\":\"Done\"}\n",
            "{\"type\":\"unknown\"}\n",
        );
        let (tail, output, state) = capture_stream(input, true);
        assert_eq!(tail, "Done");
        assert_eq!(output, "Working\n✔ result: Done\n");
        assert_eq!(state.agent_lines, 2);
        assert_eq!(state.agent_last_line, "✔ result: Done");
    }

    #[test]
    fn claude_stream_history_falls_back_to_raw_output() {
        let input = "{\"type\":\"unknown\"}\nplain output";
        let (tail, output, state) = capture_stream(input, true);
        assert_eq!(tail, input);
        assert_eq!(output, "plain output\n");
        assert_eq!(state.agent_lines, 1);
        assert_eq!(state.agent_last_line, "plain output");
    }

    #[test]
    fn raw_stream_preserves_codex_and_stderr_behavior() {
        let input = "  Working  \r\n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone";
        let (tail, output, state) = capture_stream(input, false);
        let expected = "  Working  \n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone\n";
        assert_eq!(tail, expected.trim());
        assert_eq!(output, expected);
        assert_eq!(state.agent_lines, 4);
        assert_eq!(state.agent_last_line, "Done");
    }
}
