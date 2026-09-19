//! Stage-level constraint-conflict escalation to the planner: each trigger,
//! each planner decision, the one-pass limit and restart safety.
use super::*;
use crate::app::constraint_escalation::CONFLICT_BLOCKED;

const STATEMENT: &str = "The stage may change only docs, yet a test reads the changed milestones.md";

/// A fake implementer CLI. Each turn appends to `stage-<id>.txt`, so every
/// stage's work is distinguishable, and takes the next scripted action from
/// `.forge/test-cli.json`: null completes, `{kind, reason}` escalates.
fn install_scripted_cli(f: &Fixture, actions: Value) {
    use std::os::unix::fs::PermissionsExt;
    let cli = f.root.join(".forge/fake-codex");
    fs::write(&cli, r#"#!/usr/bin/env python3
import sys,json,pathlib
model=sys.argv[sys.argv.index('-m')+1]
prompt=sys.argv[-1]
outcome,_=json.JSONDecoder().raw_decode(prompt.split('Finish with ONLY JSON: ',1)[1])
with pathlib.Path('stage-%s.txt' % outcome['stage_id']).open('a') as output: output.write('work\n')
script=pathlib.Path('.forge/test-cli.json')
actions=json.loads(script.read_text()) if script.exists() else []
action=actions.pop(0) if actions else None
script.write_text(json.dumps(actions))
if action:
    outcome.update(status='escalation',evidence=['A test reads the real milestones.md'],request={'kind':action['kind'],'reason':action['reason'],'required_capability':'Planner correction of the stage constraints'})
print(json.dumps({'type':'thread.started','thread_id':'11111111-2222-4333-8444-555555555555','model':model}))
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':json.dumps(outcome)}}))
print(json.dumps({'type':'turn.completed','usage':{'input_tokens':10,'output_tokens':5}}))
"#).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(f.root.join(".forge/test-cli.json"), actions.to_string()).unwrap();
    f.set("test_cli_codex", json!(cli));
    f.set("test_real_implementation_cli", json!(true));
}

fn conflict_action() -> Value {
    json!({"kind":"constraint_conflict","reason":STATEMENT})
}

fn conflict_verdict() -> Value {
    json!({"approved":false,"issues":[],"constraint_conflict":STATEMENT})
}

fn requests(f: &Fixture, role: &str) -> Vec<Value> {
    f.ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().into_iter().flatten()
        .filter(|r| r["role"] == role).cloned().collect()
}

fn records(p: &Value, idx: usize) -> Vec<Value> {
    p["stages"][idx]["constraint_escalations"].as_array().cloned().unwrap_or_default()
}

fn restarted(f: &Fixture) -> Ctx {
    let mut settings = f.ctx.app.settings.lock().unwrap().clone();
    settings["mock_scope_output"] = Value::Null;
    settings["mock_agent_requests"] = json!([]);
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    app.context(f.root.to_str().unwrap())
}

fn revise_current(instructions: &str, acceptance: &str) -> Value {
    json!({"analysis":"The docs-only limit collides with a test that reads real docs.",
        "decision":{"revise":{"stages":[{"id":1,"instructions":instructions,"acceptance":acceptance}]}}})
}

#[test]
fn an_implementer_conflict_revising_the_current_stage_keeps_approval_and_restarts_it() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([conflict_action()]));
    f.set("mock_scope_output", revise_current("Implement greeting with a fixture test", "Greeting works on fixtures"));
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{p}");
    assert_eq!(s["acceptance"], "Greeting works on fixtures");
    // The correction confined to the approved stage kept the run's approval:
    // the run went on to completion without asking the user.
    assert_eq!(p["status"], "done");
    let records = records(&p, 0);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["trigger"]["source"], "implementer");
    assert_eq!(record["decision"], "revise");
    assert_eq!(record["outcome"], "applied");
    assert_eq!(record["plan_revision"], record["answer_revision"].as_u64().unwrap() + 1);
    assert!(record["correction_summary"].as_str().unwrap().contains("revised stage(s) 1"));
    // The corrected stage restarted as a fresh attempt with replenished rounds.
    assert_ne!(s["attempt_id"], record["attempt_id"]);
    assert_eq!(s["rounds"], 1);
    assert_eq!(requests(&f, "planner").len(), 1);
    let prompt = requests(&f, "planner")[0]["prompt"].as_str().unwrap().to_owned();
    for text in [STATEMENT, "FULL PLAN", "Implement greeting", "stage-1.txt", "\"source\": \"implementer\"", "\"budget\": 3"] {
        assert!(prompt.contains(text), "{text}: {prompt}");
    }
    let implementer = requests(&f, "implementer");
    assert!(implementer.last().unwrap()["prompt"].as_str().unwrap().contains("Greeting works on fixtures"));
}

#[test]
fn a_reviewer_conflict_inserting_a_stage_returns_the_plan_for_approval_and_keeps_the_work_apart() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    f.set("mock_verdicts", json!([conflict_verdict()]));
    f.set("mock_scope_output", json!({"analysis":"A test reads the real milestones.md.",
        "decision":{"revise":{"insert_before":[{"title":"Move the milestone test onto fixtures",
            "instructions":"Make the milestone test read a fixture","acceptance":"The test reads a fixture",
            "commit":"test: read milestones from a fixture"}]}}}));
    let p = f.run();
    // The run stops cleanly: nothing committed, the plan waits for the user.
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
    assert_eq!(p["status"], "draft");
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "plan_ready");
    let ids: Vec<_> = p["stages"].as_array().unwrap().iter().map(|s| s["id"].clone()).collect();
    assert_eq!(ids, vec![json!(2), json!(1)]);
    assert_eq!(p["stages"][0]["title"], "Move the milestone test onto fixtures");
    let s = &p["stages"][1];
    assert_eq!(s["status"], "pending");
    assert_eq!(s["suspended_work"]["status"], "held");
    let record = &records(&p, 1)[0];
    assert_eq!(record["trigger"]["source"], "reviewer");
    assert_eq!(record["outcome"], "awaiting_approval");
    assert_eq!(record["plan_revision"], p["revision"]);
    assert_eq!(fs::read_to_string(f.root.join("stage-1.txt")).unwrap(), "work\n");
    assert_eq!(requests(&f, "planner").len(), 1);
    // Running again without approval does not continue.
    f.ctx.run_worker();
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
    assert_eq!(requests(&f, "implementer").len(), 1);

    // After approval the inserted stage runs first, without the suspended work.
    let p = f.ctx.load_plan().unwrap();
    let mut p = f.ctx.architect_publish(p.clone(), Some(&p), "approval").unwrap();
    p["status"] = json!("approved");
    f.ctx.save_plan(&p).unwrap();
    let p = f.run();
    assert!(p["stages"].as_array().unwrap().iter().all(|s| s["status"] == "committed"), "{p}");
    let inserted = f.ctx.git(&["show", "--name-only", "--format=%s", "HEAD~1"]).unwrap();
    assert_eq!(inserted, "test: read milestones from a fixture\n\nstage-2.txt");
    let current = f.ctx.git(&["show", "--name-only", "--format=%s", "HEAD"]).unwrap();
    assert_eq!(current, "feat: greeting\n\nstage-1.txt");
    // The first attempt's work came back and the corrected stage built on it.
    assert_eq!(fs::read_to_string(f.root.join("stage-1.txt")).unwrap(), "work\nwork\n");
    assert!(p["stages"][1].get("suspended_work").is_none());
    assert_eq!(records(&p, 1).len(), 1);
    assert_eq!(requests(&f, "planner").len(), 1);
}

#[test]
fn constraint_wrong_reviews_the_delivered_work_again_under_the_corrected_text() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    f.set("mock_verdicts", json!([conflict_verdict()]));
    f.set("mock_scope_output", json!({"analysis":"Docs-only was never required.",
        "decision":{"constraint_wrong":{"constraint":"Change only documentation",
            "instructions":"Implement greeting and keep its tests passing","acceptance":"Greeting works and every test passes",
            "justification":"The delivered greeting and its test already meet this"}}}));
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{p}");
    assert_eq!(s["acceptance"], "Greeting works and every test passes");
    assert_eq!(p["status"], "done");
    assert_eq!(records(&p, 0)[0]["outcome"], "applied");
    assert_eq!(records(&p, 0)[0]["decision"], "constraint_wrong");
    // No new implementer turn: the unchanged work passed the normal review gate.
    assert_eq!(requests(&f, "implementer").len() + requests(&f, "fixer").len(), 1);
    assert_eq!(fs::read_to_string(f.root.join("stage-1.txt")).unwrap(), "work\n");
    assert_eq!(s["rounds"], 1);
    assert_eq!(s["review_gate"]["status"], "approved");
    let settings = f.ctx.app.settings.lock().unwrap();
    let reviews: Vec<_> = settings["test_review_sessions"].as_array().unwrap().iter()
        .filter(|r| r["role"] == "reviewer").collect();
    assert_eq!(reviews.len(), 2);
    assert!(reviews[1]["prompt"].as_str().unwrap().contains("Greeting works and every test passes"));
    assert!(s.get("conflict_rereview").is_none());
}

#[test]
fn a_refused_conflict_feeds_the_explanation_to_the_next_round() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    f.set("mock_verdicts", json!([conflict_verdict()]));
    let explanation = "Update the milestone test fixture in the same stage; it is not documentation-only.";
    f.set("mock_scope_output", json!({"analysis":"The stage can be done as written.","decision":{"refused":explanation}}));
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{p}");
    assert_eq!(records(&p, 0)[0]["outcome"], "refused");
    assert_eq!(s["scope_clarification"]["message"], explanation);
    assert_eq!(s["rounds"], 2);
    let implementer = requests(&f, "implementer");
    assert_eq!(implementer.len(), 2);
    assert!(implementer[1]["prompt"].as_str().unwrap().contains(explanation));
    let settings = f.ctx.app.settings.lock().unwrap();
    let reviews: Vec<_> = settings["test_review_sessions"].as_array().unwrap().iter()
        .filter(|r| r["role"] == "reviewer").collect();
    assert!(reviews[1]["prompt"].as_str().unwrap().contains(explanation));
    drop(settings);

    // Refused in the last round: no round remains, so the stage blocks.
    let g = Fixture::new();
    g.set("max_fix_rounds", json!(0));
    install_scripted_cli(&g, json!([]));
    g.set("mock_verdicts", json!([conflict_verdict()]));
    g.set("mock_scope_output", json!({"analysis":"a","decision":{"refused":explanation}}));
    let p = g.run();
    assert_eq!(p["stages"][0]["status"], "blocked");
    assert_eq!(records(&p, 0).len(), 1);
    assert_eq!(records(&p, 0)[0]["decision"], "refused");
    assert_eq!(records(&p, 0)[0]["outcome"], "blocked");
    assert_eq!(p["stages"][0]["review_gate"]["constraint_escalation"]["outcome"], "blocked");
    // The exhausted round had its escalation: no fallback pass on top of it.
    assert_eq!(requests(&g, "planner").len(), 1);
}

#[test]
fn an_exhausted_review_without_a_reported_conflict_gets_exactly_one_planner_pass() {
    let f = Fixture::new();
    f.set("max_fix_rounds", json!(1));
    f.set("reassessment_limits", json!({"max_reassessments":2,"max_operational_retries":2,"repeat_threshold":10,"context_percent":85}));
    install_scripted_cli(&f, json!([]));
    f.set("mock_verdicts", json!([{"approved":false,"issues":["Greeting lacks a test"]},
        {"approved":false,"issues":["Greeting lacks a test"]}]));
    f.set("mock_scope_output", json!({"analysis":"The stage is buildable.","decision":{"refused":"Add the greeting test."}}));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "blocked");
    let records = records(&p, 0);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["trigger"]["source"], "engine");
    assert_eq!(records[0]["trigger"]["kind"], "review_exhausted");
    assert_eq!(records[0]["outcome"], "blocked");
    assert_eq!(records[0]["inputs"]["rounds"], json!({"used":2,"budget":1}));
    assert_eq!(records[0]["inputs"]["requests"]["items"][0]["text"], "Greeting lacks a test");
    assert_eq!(p["stages"][0]["review_gate"]["constraint_escalation"]["signature"], records[0]["signature"]);
    // The record stays visible in the plan JSON the panel polls.
    let polled = f.ctx.architecture_store().state_plan(p.clone());
    assert_eq!(polled["stages"][0]["constraint_escalations"], p["stages"][0]["constraint_escalations"]);
    assert_eq!(requests(&f, "planner").len(), 1);
    // A restart neither replenishes the rounds nor spends another planner pass.
    let ctx = restarted(&f);
    ctx.run_worker();
    let p = ctx.load_plan().unwrap();
    assert_eq!(p["stages"][0]["status"], "blocked");
    assert_eq!(p["stages"][0]["rounds"], 2);
    assert_eq!(p["stages"][0]["constraint_escalations"].as_array().unwrap().len(), 1);
    assert!(!ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().unwrap()
        .iter().any(|r| r["role"] == "planner"));
}

#[test]
fn the_same_conflict_after_one_correction_blocks_with_the_record_attached() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    f.set("mock_verdicts", json!([conflict_verdict(), conflict_verdict()]));
    f.set("mock_scope_output", json!([revise_current("Implement greeting with a fixture test", "Greeting works on fixtures"),
        {"analysis":"never asked","decision":{"refused":"never asked"}}]));
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "blocked", "{p}");
    let records = records(&p, 0);
    let outcomes: Vec<_> = records.iter().map(|r| r["outcome"].clone()).collect();
    assert_eq!(outcomes, vec![json!("applied"), json!("blocked")]);
    assert_eq!(records[0]["signature"], records[1]["signature"]);
    assert_ne!(records[0]["attempt_id"], records[1]["attempt_id"]);
    assert_eq!(records[1]["repeats"], 0);
    assert!(records[1]["decision"].is_null());
    assert_eq!(s["review_gate"]["constraint_escalation"]["outcome"], "blocked");
    assert!(s["review_gate"]["reason"].as_str().unwrap().contains("already had its planner pass"));
    assert_eq!(requests(&f, "planner").len(), 1);
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
}

#[test]
fn a_reported_conflict_skips_the_repeated_findings_model_reassessment() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    let mut repeated = conflict_verdict();
    repeated["issues"] = json!(["Greeting test still fails"]);
    f.set("mock_verdicts", json!([{"approved":false,"issues":["Greeting test still fails"]}, repeated]));
    f.set("mock_scope_output", json!({"analysis":"a","decision":{"refused":"Fix the greeting test in this stage."}}));
    let before = f.counts();
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{p}");
    assert_eq!(s["rounds"], 3);
    // The repeated finding would have switched models; the conflict went to the planner.
    assert!(s["reassessment"]["count"].as_u64().unwrap_or(0) == 0, "{}", s["reassessment"]);
    assert_eq!(f.counts(), before);
    assert_eq!(records(&p, 0)[0]["outcome"], "refused");
    assert_eq!(requests(&f, "planner").len(), 1);
    let invocations: Vec<_> = s["model_invocations"].as_array().unwrap().iter()
        .map(|i| i["requested"]["model"].clone()).collect();
    assert!(invocations.iter().all(|m| *m == invocations[0]), "{invocations:?}");
}

/// A stage whose planner answered a conflict but whose correction was not yet
/// applied: the state a crash after the answer was saved leaves behind.
fn answered_escalation(f: &Fixture, answer: Value) -> (Value, usize) {
    let mut p = f.attempt();
    p["stages"][0]["rounds"] = json!(1);
    p["stages"][0]["attempt_head"] = json!(f.ctx.git(&["rev-parse", "HEAD"]).unwrap());
    f.ctx.save_plan(&p).unwrap();
    fs::write(f.root.join("stage-1.txt"), "work\n").unwrap();
    let statement = STATEMENT.to_string();
    let at = f.ctx.begin_constraint_escalation(&mut p, 0, "fixer", "constraint_conflict", &statement,
        std::slice::from_ref(&statement)).unwrap();
    f.set("mock_scope_output", answer);
    f.ctx.consult_conflict_planner(&mut p, 0, at).unwrap();
    (p, at)
}

#[test]
fn a_restart_during_a_pending_correction_applies_it_without_asking_the_planner_again() {
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    let (mut p, at) = answered_escalation(&f, revise_current("Implement greeting with a fixture test", "Greeting works on fixtures"));
    // Publication fails before the correction lands: nothing changes.
    f.set("mock_routing_planner_outputs", json!(vec![json!({"proposals":[]}); 4]));
    let error = f.ctx.apply_conflict_answer(&mut p, 0, at, true).unwrap_err();
    assert!(error.contains("proposal count mismatch"), "{error}");
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["acceptance"], "Greeting works");
    assert_eq!(saved["stages"][0]["constraint_escalations"][at]["outcome"], "pending");
    assert_eq!(saved["stages"][0]["constraint_escalations"][at]["decision"], "revise");

    let ctx = restarted(&f);
    ctx.run_worker();
    let done = ctx.load_plan().unwrap();
    let s = &done["stages"][0];
    assert_eq!(s["status"], "committed", "{done}");
    assert_eq!(s["acceptance"], "Greeting works on fixtures");
    assert_eq!(s["constraint_escalations"].as_array().unwrap().len(), 1);
    assert_eq!(s["constraint_escalations"][at]["outcome"], "applied");
    assert_eq!(done["status"], "done");
    let settings = ctx.app.settings.lock().unwrap();
    assert!(!settings["mock_agent_requests"].as_array().unwrap().iter().any(|r| r["role"] == "planner"));
}

#[test]
fn restart_boundaries_never_spend_a_second_planner_pass() {
    // Crash after the record was saved, before the planner's answer: the
    // record fails on recovery and the stage continues without a new pass.
    let f = Fixture::new();
    install_scripted_cli(&f, json!([]));
    let mut p = f.attempt();
    let statement = STATEMENT.to_string();
    f.ctx.begin_constraint_escalation(&mut p, 0, "fixer", "constraint_conflict", &statement,
        std::slice::from_ref(&statement)).unwrap();
    let ctx = restarted(&f);
    ctx.run_worker();
    let done = ctx.load_plan().unwrap();
    assert_eq!(done["stages"][0]["status"], "committed", "{done}");
    assert_eq!(done["stages"][0]["constraint_escalations"][0]["outcome"], "failed");
    assert!(!ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().unwrap().iter().any(|r| r["role"] == "planner"));

    // Crash after the answer, for each decision: recovery applies the saved answer once.
    for (answer, outcome) in [
        (json!({"analysis":"a","decision":{"refused":"Keep the stage and fix the test."}}), "refused"),
        (json!({"analysis":"a","decision":{"constraint_wrong":{"constraint":"docs only","instructions":"Implement greeting and its test",
            "acceptance":"Greeting and its test pass","justification":"The delivered work meets it"}}}), "applied"),
        (json!({"analysis":"a","decision":{"revise":{"insert_before":[{"title":"Fixture first","instructions":"Move the test",
            "acceptance":"Test uses a fixture","commit":"test: fixture"}]}}}), "awaiting_approval"),
    ] {
        let f = Fixture::new();
        install_scripted_cli(&f, json!([]));
        let (_, at) = answered_escalation(&f, answer);
        let ctx = restarted(&f);
        ctx.run_worker();
        let done = ctx.load_plan().unwrap();
        let stage = done["stages"].as_array().unwrap().iter().find(|s| s["id"] == 1).unwrap();
        assert_eq!(stage["constraint_escalations"][at]["outcome"], outcome, "{done}");
        assert_eq!(stage["constraint_escalations"].as_array().unwrap().len(), 1);
        assert!(!ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().unwrap().iter().any(|r| r["role"] == "planner"));
        match outcome {
            "awaiting_approval" => {
                assert_eq!(done["status"], "draft");
                assert_eq!(done["stages"][0]["title"], "Fixture first");
                assert_eq!(ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
                assert_eq!(fs::read_to_string(f.root.join("stage-1.txt")).unwrap(), "work\n");
            }
            "applied" => {
                // constraint_wrong re-reviews the delivered work: no implementer turn.
                assert_eq!(stage["status"], "committed", "{done}");
                assert!(!ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().unwrap().iter()
                    .any(|r| r["role"] == "implementer" || r["role"] == "fixer"));
            }
            _ => assert_eq!(stage["status"], "committed", "{done}"),
        }
    }
}

#[test]
fn suspended_stage_work_survives_crashes_while_suspending_and_restoring() {
    let f = Fixture::new();
    let mut p = f.ctx.load_plan().unwrap();
    let mut later = p["stages"][0].clone();
    later["id"] = json!(2);
    later["title"] = json!("Corrected stage");
    p["stages"].as_array_mut().unwrap().push(later);
    f.ctx.save_plan(&p).unwrap();
    let mut p = f.ctx.load_plan().unwrap();
    fs::write(f.root.join("README.md"), "Edited\n").unwrap();
    fs::write(f.root.join("new.txt"), "untracked work\n").unwrap();
    p["stages"][1]["suspended_work"] = json!({"status":"held"});
    f.ctx.save_plan(&p).unwrap();
    let clean = |ctx: &Ctx| ctx.git(&["status", "--porcelain", "--", ".", ":(exclude).forge"]).unwrap().is_empty();

    f.ctx.settle_suspended_work(&mut p, 0).unwrap();
    let marker = f.ctx.load_plan().unwrap()["stages"][1]["suspended_work"].clone();
    assert_eq!(marker["status"], "suspended");
    assert!(clean(&f.ctx));
    let patch = f.root.join(marker["patch"].as_str().unwrap());
    assert!(patch.exists());

    // Crash after the patch was saved, before the tree was cleaned.
    f.ctx.git(&["apply", "--binary", patch.to_str().unwrap()]).unwrap();
    p = f.ctx.load_plan().unwrap();
    p["stages"][1]["suspended_work"]["status"] = json!("suspending");
    f.ctx.save_plan(&p).unwrap();
    let ctx = restarted(&f);
    let mut p = ctx.load_plan().unwrap();
    ctx.settle_suspended_work(&mut p, 0).unwrap();
    assert_eq!(ctx.load_plan().unwrap()["stages"][1]["suspended_work"]["status"], "suspended");
    assert!(clean(&ctx));

    // The predecessor commits; the suspended work is restored onto it.
    fs::write(f.root.join("fixture.txt"), "fixture\n").unwrap();
    ctx.git(&["add", "fixture.txt"]).unwrap();
    ctx.git(&["commit", "-qm", "inserted stage"]).unwrap();
    ctx.settle_suspended_work(&mut p, 1).unwrap();
    assert_eq!(fs::read_to_string(f.root.join("README.md")).unwrap(), "Edited\n");
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "untracked work\n");
    assert!(ctx.load_plan().unwrap()["stages"][1].get("suspended_work").is_none());

    // Crash after the patch was applied, before the marker was cleared.
    p = ctx.load_plan().unwrap();
    p["stages"][1]["suspended_work"] = marker.clone();
    p["stages"][1]["suspended_work"]["status"] = json!("restoring");
    ctx.save_plan(&p).unwrap();
    ctx.settle_suspended_work(&mut p, 1).unwrap();
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "untracked work\n");
    assert_eq!(fs::read_to_string(f.root.join("README.md")).unwrap(), "Edited\n");
    assert!(ctx.load_plan().unwrap()["stages"][1].get("suspended_work").is_none());
}

#[test]
fn a_blocked_escalation_is_not_followed_by_a_fallback_pass() {
    assert_eq!(CONFLICT_BLOCKED, "conflict_blocked");
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["rounds"] = json!(4);
    p["previous_requests"] = Value::Null;
    f.ctx.save_plan(&p).unwrap();
    // A failed planner pass in this round blocks; the fallback then finds it.
    f.set("mock_scope_output", Value::Null);
    let statement = STATEMENT.to_string();
    let trigger = crate::app::constraint_escalation::Trigger { source: "reviewer", kind: "constraint_conflict",
        reason: statement.clone(), statements: vec![statement] };
    let resolution = f.ctx.escalate_stage_conflict(&mut p, 0, trigger, false).unwrap();
    assert_eq!(resolution, crate::app::constraint_escalation::ConflictResolution::Blocked);
    let planner = requests(&f, "planner").len();
    assert_eq!(f.ctx.exhaustion_fallback(&mut p, 0).unwrap(), "exhausted");
    assert_eq!(requests(&f, "planner").len(), planner);
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(records(&saved, 0).len(), 1);
    assert_eq!(records(&saved, 0)[0]["outcome"], "failed");
    assert!(saved["stages"][0]["review_gate"]["constraint_escalation"].is_object());
}
