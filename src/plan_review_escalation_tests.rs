//! Plan-review constraint-conflict escalation to the planner: stall,
//! exhaustion and role-reported triggers, each planner decision, the one-pass
//! limit across approved corrections, planner failure and restart safety.
use super::*;
use super::test_support::*;
use crate::test_support::api_request;
use std::sync::atomic::Ordering;

const STATEMENT: &str = "Stage 2 may change only documentation, yet a test reads the documentation it changes";

fn planner_calls(ctx: &Ctx) -> Vec<Value> {
    ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().into_iter().flatten()
        .filter(|r| r["role"] == "planner").cloned().collect()
}

fn records(p: &Value) -> Vec<Value> {
    p["plan_review"]["constraint_escalations"].as_array().cloned().unwrap_or_default()
}

fn restarted(f: &Fixture) -> Ctx {
    let mut settings = f.ctx.app.settings.lock().unwrap().clone();
    settings["mock_scope_output"] = Value::Null;
    settings["mock_agent_requests"] = json!([]);
    settings["test_plan_conflict_crash"] = Value::Null;
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    app.context(f.root.to_str().unwrap())
}

fn append_answer() -> Value {
    json!({"analysis":"The requests need a test that reads a fixture; the committed stages cannot change.",
        "decision":{"revise":{"append":[{"title":"Move the documentation test onto fixtures",
            "instructions":"Make the documentation test read a fixture.","acceptance":"The documentation test reads a fixture.",
            "commit":"test: read documentation from a fixture"}]}}})
}

fn criteria_answer() -> Value {
    json!({"analysis":"The deferred criterion of stage 2 asked for more than the plan meant.",
        "decision":{"revise":{"criteria":[{"id":2,"acceptance":"Second feature works on fixtures."}]}}})
}

/// A plan review whose fix rounds commit nothing against the same requests,
/// so it stalls in round 3 of a budget of 3.
fn stalling(f: &Fixture) {
    f.two_deferred_stages();
    let same = json!({"first.rs":"fn first() {}\n"});
    let mut edits = vec![json!({"first.rs":"fn first() {}\n"}), json!({"second.rs":"fn second() {}\n"})];
    edits.extend(vec![same; 8]);
    f.setting("mock_edits", json!(edits));
    f.setting("mock_verdicts", json!(vec![reject("Resolve the saved constraint conflict"); 8]));
}

/// Committed stages keep every field, their order and their commit range.
fn assert_committed_unchanged(p: &Value, subject: &Value) {
    for (stage, captured) in p["stages"].as_array().unwrap().iter().zip(subject["stages"].as_array().unwrap()) {
        assert_eq!(stage["status"], "committed");
        for key in ["id", "title", "instructions", "acceptance", "commit", "sha", "attempt_head"] {
            assert_eq!(stage[key], captured[key], "{key}");
        }
    }
}

fn assert_awaits_approval(f: &Fixture, p: &Value, commits: &str) {
    assert_eq!(p["status"], "draft", "{p}");
    assert_eq!(p["plan_review"]["status"], "awaiting_approval");
    assert_eq!(p["plan_review"]["next_action"], "awaiting_approval");
    assert_eq!(p["plan_review"]["gate"]["status"], "awaiting_approval");
    assert_eq!(p["plan_review"]["pending_revision"], p["revision"]);
    assert_eq!(p["plan_review"]["gate"]["constraint_escalation"]["outcome"], "awaiting_approval");
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "plan_ready");
    // The engine made no commit and rewrote nothing.
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), commits);
    assert_eq!(f.ctx.git(&["rev-parse", "HEAD"]).unwrap(), p["plan_review"]["head"]);
    assert_committed_unchanged(p, &p["plan_review"]["subject"]);
    assert!(!f.ctx.forge_path("reports.jsonl").exists());
}

#[test]
fn a_stall_gets_one_planner_pass_whose_appended_stage_returns_the_plan_for_approval() {
    let f = Fixture::new("Implement feature", 3);
    stalling(&f);
    f.setting("mock_scope_output", append_answer());
    let p = f.run();
    assert_awaits_approval(&f, &p, "3");
    let stages = p["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 3);
    assert_eq!(stages[2]["status"], "pending");
    assert_eq!(stages[2]["title"], "Move the documentation test onto fixtures");
    assert_eq!(f.count("fixer"), 2);
    let saved = records(&p);
    assert_eq!(saved.len(), 1);
    let record = &saved[0];
    assert_eq!(record["scope"], "plan");
    assert_eq!(record["trigger"]["source"], "engine");
    assert_eq!(record["trigger"]["kind"], "plan_review_stalled");
    assert_eq!(record["blocks_as"], "stalled");
    assert_eq!(record["round"], 3);
    assert_eq!(record["attempt_id"], p["plan_review"]["attempt_id"]);
    assert_eq!(record["decision"], "revise");
    assert_eq!(record["outcome"], "awaiting_approval");
    assert_eq!(record["plan_revision"], p["revision"]);
    assert!(record["analysis"].as_str().unwrap().contains("committed stages cannot change"));
    assert!(record["correction_summary"].as_str().unwrap().contains("appended 1 stage(s)"));
    let inputs = &record["inputs"];
    assert_eq!(inputs["requests"]["items"][0], json!({"role":"reviewer","text":"Resolve the saved constraint conflict"}));
    assert_eq!(inputs["rounds"], json!({"used":3,"budget":3}));
    assert_eq!(inputs["commit_range"]["base"], p["plan_review"]["base"]);
    assert_eq!(inputs["fixer_replies"]["total"], 2);
    assert!(inputs["changed_files"]["items"].as_array().unwrap().contains(&json!("second.rs")));
    let calls = planner_calls(&f.ctx);
    assert_eq!(calls.len(), 1);
    let prompt = calls[0]["prompt"].as_str().unwrap();
    for text in ["COMMIT RANGE UNDER REVIEW", "Resolve the saved constraint conflict", "FIX COMMITS ALREADY MADE",
        "committed history, cannot change", "\"budget\": 3", "plan_review_stalled"] {
        assert!(prompt.contains(text), "{text}");
    }

    // Nothing runs under the unapproved correction.
    let unchanged = f.run();
    assert_eq!(unchanged["plan_review"], p["plan_review"]);
    assert_eq!(f.count("fixer"), 2);
    assert_eq!(planner_calls(&f.ctx).len(), 1);

    // Approval runs the appended stage, then a fresh plan review attempt.
    f.setting("mock_verdicts", json!([]));
    f.setting("mock_edits", json!([{"fixture.rs":"fn fixture() {}\n"}]));
    assert_eq!(api_request(&f.ctx.app, "POST", "/api/approve", json!({})).0, 200);
    let done = f.run();
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["stages"][2]["status"], "committed");
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "4");
    assert_committed_unchanged(&done, &p["plan_review"]["subject"]);
    let review = &done["plan_review"];
    assert_ne!(review["attempt_id"], p["plan_review"]["attempt_id"]);
    assert_eq!(review["status"], "approved");
    assert_eq!(review["subject"]["stages"].as_array().unwrap().len(), 3);
    let carried = records(&done);
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0]["outcome"], "applied");
    assert_eq!(carried[0]["signature"], record["signature"]);
    assert_eq!(planner_calls(&f.ctx).len(), 1);
}

#[test]
fn an_exhausted_plan_review_gets_one_planner_pass_whose_appended_stage_returns_the_plan_for_approval() {
    let f = Fixture::new("Implement feature", 0);
    f.two_deferred_stages();
    f.setting("mock_verdicts", json!([reject("Fix behavior")]));
    f.setting("mock_scope_output", append_answer());
    let p = f.run();
    assert_awaits_approval(&f, &p, "3");
    assert_eq!(p["stages"][2]["status"], "pending");
    assert_eq!(f.count("fixer"), 0);
    let record = &records(&p)[0];
    assert_eq!(record["trigger"]["source"], "engine");
    assert_eq!(record["trigger"]["kind"], "plan_review_exhausted");
    assert_eq!(record["blocks_as"], "exhausted");
    assert_eq!(record["outcome"], "awaiting_approval");
    assert_eq!(record["inputs"]["rounds"], json!({"used":1,"budget":0}));
    assert_eq!(planner_calls(&f.ctx).len(), 1);
    // The panel polls the plan JSON: a bounded preview of the record is there.
    let polled = f.ctx.architecture_store().state_plan(p.clone());
    let preview = &polled["plan_review"]["constraint_escalations"][0];
    for key in ["trigger", "signature", "analysis", "decision", "outcome", "plan_revision"] {
        assert_eq!(preview[key], record[key], "{key}");
    }
    assert!(preview.get("inputs").is_none());
    assert_eq!(polled["plan_review"]["constraint_escalation_count"], 1);
    assert_eq!(polled["plan_review"]["gate"]["constraint_escalation"]["outcome"], "awaiting_approval");
    let mut twice = polled.clone();
    crate::review_history::bounded(&mut twice);
    assert_eq!(twice, polled);
}

#[test]
fn a_reported_constraint_conflict_escalates_and_constraint_wrong_corrects_only_the_review_criteria() {
    let f = Fixture::new("Implement feature", 2);
    f.two_deferred_stages();
    f.setting("mock_verdicts", json!([{"approved":false,"issues":["Keep stage 2 documentation-only"],"constraint_conflict":STATEMENT}]));
    f.setting("mock_scope_output", json!({"analysis":"Documentation-only was never required of stage 2.",
        "decision":{"constraint_wrong":{"stage":2,"constraint":"Change only documentation",
            "instructions":"Integrate the second feature and keep its tests passing.",
            "acceptance":"Second feature works and every test passes.",
            "justification":"The delivered integration and its tests meet this."}}}));
    let p = f.run();
    assert_awaits_approval(&f, &p, "3");
    // No stage was added; the stage text stays as committed.
    assert_eq!(p["stages"].as_array().unwrap().len(), 2);
    assert_eq!(p["stages"][1]["acceptance"], "Second feature works.\n\n Both features integrate. ");
    let corrected = &p["plan_review_criteria"]["2"];
    assert_eq!(corrected["acceptance"], "Second feature works and every test passes.");
    assert_eq!(corrected["constraint"], "Change only documentation");
    assert_eq!(corrected["revision"], p["revision"]);
    let record = &records(&p)[0];
    assert_eq!(record["trigger"]["source"], "reviewer");
    assert_eq!(record["trigger"]["kind"], "constraint_conflict");
    assert_eq!(record["inputs"]["statements"]["items"], json!([STATEMENT]));
    assert_eq!(record["decision"], "constraint_wrong");
    assert_eq!(f.count("fixer"), 0);
    assert_eq!(planner_calls(&f.ctx).len(), 1);

    // The fresh attempt holds stage 2 to the corrected criteria.
    assert_eq!(api_request(&f.ctx.app, "POST", "/api/approve", json!({})).0, 200);
    let done = f.run();
    assert_eq!(done["status"], "done", "{done}");
    let acceptance = done["plan_review"]["acceptance"].as_str().unwrap();
    assert!(acceptance.contains("stage 2 (Integrate feature): Second feature works and every test passes."), "{acceptance}");
    assert!(!acceptance.contains("Both features integrate"));
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "3");
    assert_committed_unchanged(&done, &p["plan_review"]["subject"]);
    let reviewer = f.ctx.app.settings.lock().unwrap()["test_review_sessions"].as_array().unwrap().iter()
        .filter(|r| r["prompt"].as_str().unwrap().contains("\"scope\":\"plan\"")).last().unwrap().clone();
    assert!(reviewer["prompt"].as_str().unwrap().contains("planner_corrected_criteria"));
}

#[test]
fn a_fixer_reported_conflict_escalates_before_the_review() {
    let f = Fixture::new("Implement feature", 2);
    f.pending_plan_review();
    f.setting("mock_verdicts", json!([reject("Fix integration")]));
    f.setting("mock_implementation_outputs", json!([format!("I checked the tests.\n{}", json!({"constraint_conflict":STATEMENT}))]));
    f.setting("mock_scope_output", json!({"analysis":"The documentation test can use a fixture copy.",
        "decision":{"refused":"Point the documentation test at a fixture copy in the same fix round."}}));
    let p = f.run();
    assert_eq!(p["status"], "done", "{p}");
    let record = &records(&p)[0];
    assert_eq!(record["trigger"]["source"], "fixer");
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["resume_action"], "review_pending");
    assert_eq!(record["inputs"]["fixer_replies"]["items"][0]["constraint_conflict"], STATEMENT);
    assert_eq!(planner_calls(&f.ctx).len(), 1);
    assert_eq!(p["plan_review"]["rounds"], 2);
}

#[test]
fn refused_with_rounds_left_continues_fixing_with_the_planner_explanation() {
    let f = Fixture::new("Implement feature", 2);
    f.two_deferred_stages();
    let explanation = "Point the documentation test at a fixture copy; stage 2 stays documentation-only.";
    f.setting("mock_verdicts", json!([{"approved":false,"issues":["Keep stage 2 documentation-only"],"constraint_conflict":STATEMENT}]));
    f.setting("mock_scope_output", json!({"analysis":"The requests can be met as written.","decision":{"refused":explanation}}));
    let p = f.run();
    assert_eq!(p["status"], "done", "{p}");
    assert_eq!(p["stages"].as_array().unwrap().len(), 2);
    let record = &records(&p)[0];
    assert_eq!(record["decision"], "refused");
    assert_eq!(record["outcome"], "refused");
    assert_eq!(p["plan_review"]["conflict_clarification"]["message"], explanation);
    assert_eq!(p["plan_review"]["rounds"], 2);
    assert_eq!(f.count("fixer"), 1);
    let prompt = f.ctx.app.settings.lock().unwrap()["mock_fixer_prompts"][0].as_str().unwrap().to_owned();
    assert!(prompt.contains("PLANNER ANSWER TO THE REPORTED CONSTRAINT CONFLICT"));
    assert!(prompt.contains(explanation));
    assert_eq!(planner_calls(&f.ctx).len(), 1);

    // Without a fix round left the refusal blocks with the exhausted gate.
    let g = Fixture::new("Implement feature", 0);
    g.two_deferred_stages();
    g.setting("mock_verdicts", json!([{"approved":false,"issues":["Keep stage 2 documentation-only"],"constraint_conflict":STATEMENT}]));
    g.setting("mock_scope_output", json!({"analysis":"a","decision":{"refused":explanation}}));
    let p = g.run();
    let review = &p["plan_review"];
    assert_eq!(review["gate"]["status"], "exhausted", "{review}");
    assert_eq!(review["status"], "blocked");
    assert_eq!(review["next_action"], "exhausted");
    assert_eq!(review["gate"]["constraint_escalation"]["outcome"], "blocked");
    assert_eq!(records(&p).len(), 1);
    assert_eq!(g.count("fixer"), 0);
    // The round already had its escalation: no fallback pass on top of it.
    assert_eq!(planner_calls(&g.ctx).len(), 1);
    assert_eq!(g.ctx.session.state.lock().unwrap().phase, "blocked");
}

#[test]
fn the_same_conflict_after_an_approved_correction_blocks_with_the_record_attached() {
    let f = Fixture::new("Implement feature", 3);
    stalling(&f);
    f.setting("mock_scope_output", json!([criteria_answer(), {"analysis":"never asked","decision":{"refused":"never asked"}}]));
    let first = f.run();
    assert_awaits_approval(&f, &first, "3");
    assert_eq!(first["plan_review_criteria"]["2"]["acceptance"], "Second feature works on fixtures.");
    assert_eq!(api_request(&f.ctx.app, "POST", "/api/approve", json!({})).0, 200);
    let p = f.run();
    let review = &p["plan_review"];
    assert_eq!(p["status"], "approved");
    assert_ne!(review["attempt_id"], first["plan_review"]["attempt_id"]);
    assert!(review["acceptance"].as_str().unwrap().contains("Second feature works on fixtures."));
    assert_eq!(review["gate"]["status"], "stalled", "{review}");
    assert_eq!(review["status"], "blocked");
    assert_eq!(review["next_action"], "stalled");
    let records = records(&p);
    let outcomes: Vec<_> = records.iter().map(|r| r["outcome"].clone()).collect();
    assert_eq!(outcomes, vec![json!("applied"), json!("blocked")]);
    assert_eq!(records[0]["signature"], records[1]["signature"]);
    assert_ne!(records[0]["attempt_id"], records[1]["attempt_id"]);
    assert_eq!(records[1]["repeats"], 0);
    assert!(records[1]["decision"].is_null());
    assert_eq!(review["gate"]["constraint_escalation"]["outcome"], "blocked");
    assert!(review["gate"]["reason"].as_str().unwrap().contains("already had its planner pass"));
    assert_eq!(planner_calls(&f.ctx).len(), 1);
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "3");
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "blocked");
    // Resuming a blocked review neither reserves a round nor asks the planner.
    let resumed = f.run();
    assert_eq!(resumed["plan_review"], p["plan_review"]);
    assert_eq!(planner_calls(&f.ctx).len(), 1);
}

#[test]
fn a_planner_failure_blocks_the_exhausted_review_as_before() {
    let f = Fixture::new("Implement feature", 0);
    f.two_deferred_stages();
    f.setting("mock_verdicts", json!([reject("Fix behavior")]));
    f.setting("mock_scope_output", json!("not an answer"));
    let p = f.run();
    let review = &p["plan_review"];
    assert_eq!(p["status"], "ready");
    assert_eq!(review["gate"]["status"], "exhausted");
    assert_eq!(review["status"], "blocked");
    assert_eq!(review["next_action"], "exhausted");
    let record = &review["gate"]["constraint_escalation"];
    assert_eq!(record["outcome"], "failed");
    assert!(record["decision"].is_null());
    assert!(record["detail"].as_str().unwrap().contains("the planner pass failed"));
    assert_eq!(records(&p).len(), 1);
    assert_eq!(planner_calls(&f.ctx).len(), 1 + crate::response::MAX_CORRECTIONS);
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "blocked");
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "3");
}

#[test]
fn a_restart_never_repeats_the_planner_call() {
    // Crash after the record was saved, before the planner answered: the
    // restart fails the record and blocks without a planner pass.
    let f = Fixture::new("Implement feature", 0);
    f.two_deferred_stages();
    f.setting("mock_verdicts", json!([reject("Fix behavior")]));
    f.setting("test_plan_conflict_crash", json!("pending"));
    let p = f.run();
    assert_eq!(p["plan_review"]["next_action"], "conflict_pending");
    assert_eq!(records(&p)[0]["outcome"], "pending");
    assert!(planner_calls(&f.ctx).is_empty());
    let ctx = restarted(&f);
    ctx.set_phase("plan_ready");
    ctx.run_worker();
    let p = ctx.load_plan().unwrap();
    assert_eq!(p["plan_review"]["gate"]["status"], "exhausted", "{}", p["plan_review"]);
    assert_eq!(p["plan_review"]["next_action"], "exhausted");
    assert_eq!(records(&p).len(), 1);
    assert_eq!(records(&p)[0]["outcome"], "failed");
    assert!(planner_calls(&ctx).is_empty());

    // Crash after the answer was saved: the restart applies it without a new pass.
    let f = Fixture::new("Implement feature", 0);
    f.two_deferred_stages();
    f.setting("mock_verdicts", json!([reject("Fix behavior")]));
    f.setting("mock_scope_output", append_answer());
    f.setting("test_plan_conflict_crash", json!("answered"));
    let p = f.run();
    assert_eq!(p["status"], "ready");
    assert_eq!(p["plan_review"]["next_action"], "conflict_pending");
    assert_eq!(records(&p)[0]["decision"], "revise");
    assert_eq!(records(&p)[0]["outcome"], "pending");
    assert_eq!(planner_calls(&f.ctx).len(), 1);
    let ctx = restarted(&f);
    ctx.run_worker();
    let p = ctx.load_plan().unwrap();
    assert_eq!(p["status"], "draft", "{}", p["plan_review"]);
    assert_eq!(p["plan_review"]["next_action"], "awaiting_approval");
    assert_eq!(p["stages"].as_array().unwrap().len(), 3);
    assert_eq!(records(&p).len(), 1);
    assert_eq!(records(&p)[0]["outcome"], "awaiting_approval");
    assert!(planner_calls(&ctx).is_empty());
    assert_committed_unchanged(&p, &p["plan_review"]["subject"]);
    // A further restart finds the draft awaiting approval and does nothing.
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    ctx.run_worker();
    assert_eq!(ctx.load_plan().unwrap()["plan_review"], p["plan_review"]);
    assert!(planner_calls(&ctx).is_empty());
}
