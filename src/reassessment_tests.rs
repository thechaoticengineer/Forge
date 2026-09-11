use super::*;
use crate::app::App;
use std::sync::Arc;
use std::fs;
use std::path::PathBuf;
struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}
impl Fixture {
    fn new() -> Self {
        Self::with_effort("provider_default")
    }
    fn with_effort(effort: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "forge-reassessment-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        settings["planner"] = json!("mock");
        settings["architect"] = json!("mock");
        settings["test_fake_providers"] = json!(true);
        settings["auto_push"] = json!(false);
        settings["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"small","tier":"standard","relative_cost_preference":1},
            {"provider":"codex","model":"large","tier":"strong","relative_cost_preference":5},
            {"provider":"claude","model":"other","tier":"strong"}]);
        settings["model_catalogue"]["entries"][0]["effort"] = json!(effort);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        ctx.git(&["init", "-q"]).unwrap();
        ctx.git(&["config", "user.name", "Test"]).unwrap();
        ctx.git(&["config", "user.email", "test@example.invalid"])
            .unwrap();
        ctx.git(&["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(root.join("README.md"), "Initial\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        ctx.architect_publish(json!({"goal":"Greeting","status":"ready","stages":[{"id":1,"title":"Greeting feature","instructions":"Implement greeting","acceptance":"Greeting works","commit":"feat: greeting","status":"pending","rounds":0}]}),None,"fixture").unwrap();
        Self { root, ctx }
    }
    fn set(&self, key: &str, v: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = v;
    }
    fn choose(&self, provider: &str, model: &str, effort: &str) {
        self.set("mock_routing_planner_outputs",json!([{"proposals":[{"stage_id":1,"proposal":{"risk":"standard","complexity":"standard","task":"functionality","provider":provider,"model":model,"native_effort":effort,"rationale":"Concrete failures justify this adequate assignment."}}]}]));
    }
    fn run(&self) -> Value {
        self.ctx.run_worker();
        self.ctx.load_plan().unwrap()
    }
    fn counts(&self) -> (usize, usize) {
        let s = self.ctx.app.settings.lock().unwrap();
        (
            s["mock_routing_planner_requests"].as_array().unwrap().len(),
            s["mock_routing_architect_requests"]
                .as_array()
                .unwrap()
                .len(),
        )
    }
    fn attempt(&self) -> Value {
        let mut p = self.ctx.load_plan().unwrap();
        p["stages"][0]["attempt_id"] = json!("attempt");
        p["stages"][0]["attempt_revision"] = p["revision"].clone();
        p["stages"][0]["review_budget"] = json!(3);
        self.ctx.save_plan(&p).unwrap();
        p
    }
    fn failures(&self) {
        self.set("mock_verdicts",json!([{"approved":false,"issues":["Greeting test still fails"]},{"approved":false,"issues":["Greeting test still fails"]}]));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn repeated_concrete_failures_escalate_cheap_adequate_implementation_without_resetting_fixes() {
    let f = Fixture::new();
    f.failures();
    f.choose("codex", "large", "provider_default");
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed");
    assert_eq!(s["rounds"], 3);
    assert_eq!(s["review_budget"], 3);
    assert_eq!(s["reassessment"]["count"], 1);
    assert_eq!(f.counts(), (2, 2));
    assert_eq!(s["model_invocations"][0]["requested"]["model"], "small");
    assert_eq!(s["model_invocations"][2]["requested"]["model"], "large");
    let h = &s["reassessment"]["history"][0];
    assert_eq!(h["kind"], "repeated_reasoning_failure");
    assert!(h["planner_reason"].is_string());
    assert!(h["architect_reason"].is_string());
}
#[test]
fn supported_effort_is_preferred_and_no_selection_runs_on_unchanged_boundaries() {
    let f = Fixture::with_effort("low");
    let mut options = f.ctx.routing_options().unwrap();
    let mut high = options[0].clone();
    high["effort"] = json!("high");
    options.push(high);
    f.set("mock_routing_options", json!(options));
    f.failures();
    f.choose("codex", "small", "high");
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(
        p["stages"][0]["model_invocations"][2]["requested"]["native_effort"],
        "high"
    );
    assert_eq!(f.counts(), (2, 2));
    let g = Fixture::new();
    let mut p = g.attempt();
    let count = g.counts();
    for _ in 0..5 {
        g.ctx.assignment_boundary(&mut p, 0).unwrap();
    }
    let mut options = g.ctx.routing_options().unwrap();
    options[0]["pricing"] = json!({"input":999});
    options[0]["policy_revision"] = json!("cosmetic");
    options[0]["provenance"] = json!("refreshed");
    g.set("mock_routing_options", json!(options));
    g.ctx.assignment_boundary(&mut p, 0).unwrap();
    assert_eq!(g.counts(), count);
}
#[test]
fn provider_switch_handoff_preserves_unfinished_work_and_selects_fresh_other_reviewer() {
    let f = Fixture::new();
    f.set(
        "mock_verdicts",
        json!([{"approved":false,"issues":["Preserve the unfinished greeting invariant"]}]),
    );
    f.set(
        "mock_implementation_errors",
        json!([null, "rate limit 429", "rate limit 429", "rate limit 429"]),
    );
    f.choose("claude", "other", "provider_default");
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed");
    assert_eq!(s["rounds"], 2);
    assert_eq!(s["reassessment"]["operational_retries"], 2);
    assert_eq!(s["reassessment"]["count"], 1);
    assert_eq!(s["model_agreement"]["reviewer"]["provider"], "codex");
    let settings = f.ctx.app.settings.lock().unwrap();
    let request = settings["mock_agent_requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["provider"] == "claude" && r["role"] == "fixer")
        .unwrap();
    let prompt = request["prompt"].as_str().unwrap();
    for text in [
        "Greeting works",
        "ARCHITECT GUIDANCE",
        "SAVED ARCHITECTURAL SUMMARY",
        "Relevant decisions",
        "Completed stage interfaces",
        "mock.txt",
        "Inspect and preserve",
        "[reviewer] Preserve the unfinished greeting invariant",
    ] {
        assert!(prompt.contains(text), "{text}");
    }
    let reviews = settings["test_review_sessions"].as_array().unwrap();
    let independent: Vec<_> = reviews.iter().filter(|r| r["role"] == "reviewer").collect();
    assert_eq!(independent.len(), 2);
    assert_eq!(independent[0]["provider"], "claude");
    assert_eq!(independent[1]["provider"], "codex");
    assert!(independent.iter().all(|r| r["session"].is_null()));
    let selection = settings["mock_routing_planner_requests"][1]["prompt"]
        .as_str()
        .unwrap();
    assert!(selection.contains("Architecture checkpoint") && selection.contains("mock.txt"));
}
#[test]
fn auth_has_no_paid_retry_and_constraints_prevent_switch() {
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["model_constraint"] = json!({"provider":"codex"});
    f.ctx.save_plan(&p).unwrap();
    // Reconcile the explicit pin through both roles before exercising auth.
    let p = f.ctx.load_plan().unwrap();
    f.ctx.architect_publish(p.clone(), Some(&p), "pin").unwrap();
    f.set(
        "mock_implementation_errors",
        json!(["authentication failed: login required"]),
    );
    f.choose("claude", "other", "provider_default");
    let p = f.run();
    assert_ne!(p["stages"][0]["status"], "committed");
    assert_eq!(p["stages"][0]["reassessment"]["operational_retries"], 0);
    assert_eq!(
        p["stages"][0]["model_invocations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(p["stages"][0]["reassessment"]["status"], "blocked");
    assert!(f.root.join("mock.txt").exists());
}
#[test]
fn alias_and_removal_trigger_bounded_agreement_and_critical_never_falls_back() {
    let f = Fixture::new();
    let mut p = f.attempt();
    let mut options = f.ctx.routing_options().unwrap();
    options[0]["resolved_id"] = json!("small-new");
    f.set("mock_routing_options", json!(options.clone()));
    f.ctx.assignment_boundary(&mut p, 0).unwrap();
    assert_eq!(f.counts(), (2, 2));
    assert_eq!(
        p["stages"][0]["model_agreement"]["effective"]["model"],
        "small-new"
    );
    options[0]["eligible"] = json!(false);
    options[0]["error"] = json!("removed");
    f.set("mock_routing_options", json!(options));
    f.choose("codex", "large", "provider_default");
    f.ctx.assignment_boundary(&mut p, 0).unwrap();
    assert_eq!(f.counts(), (3, 3));
    f.choose("codex", "small", "provider_default");
    assert!(
        f.ctx
            .reassess(
                &mut p,
                0,
                "implementer_escalation",
                json!({"need":"stronger"})
            )
            .is_err()
    );
    assert_eq!(
        p["stages"][0]["model_agreement"]["effective"]["model"],
        "large"
    );
}
#[test]
fn persisted_reservations_stop_loops_and_do_not_cross_projects() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.choose("codex", "large", "provider_default");
    f.ctx
        .reassess(
            &mut p,
            0,
            "repeated_reasoning_failure",
            json!(["[reviewer] test failed"]),
        )
        .unwrap();
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    let mut restored = ctx.load_plan().unwrap();
    assert!(
        ctx.reassess(
            &mut restored,
            0,
            "repeated_reasoning_failure",
            json!(["[reviewer] test failed"])
        )
        .unwrap_err()
        .contains("signature")
    );
    let other = Fixture::new();
    let mut foreign = other.attempt();
    assert!(ctx.assignment_boundary(&mut foreign, 0).is_err());
    assert_eq!(other.counts(), (1, 1));
    ctx.session.stop_requested.store(true, Ordering::SeqCst);
    let before = ctx.architecture_store().checkpoint(&restored).unwrap();
    assert!(
        ctx.reassess(
            &mut restored,
            0,
            "material_scope_change",
            json!(["new constraint"])
        )
        .is_err()
    );
    assert_eq!(
        ctx.architecture_store()
            .checkpoint(&ctx.load_plan().unwrap())
            .unwrap(),
        before
    );
}

#[test]
fn restored_material_assignment_recovers_reservation_without_refunding_or_selecting() {
    for kind in ["material_assignment_change", "repeated_reasoning_failure"] {
        let f = Fixture::new();
        let mut p = f.attempt();
        p["stages"][0]["rounds"] = json!(1);
        f.ctx.reassessment_init(&mut p, 0);
        let old = p["stages"][0]["model_agreement"].clone();
        p["stages"][0]["reassessment"]["count"] = json!(1);
        p["stages"][0]["reassessment"]["signatures"] = json!(["reserved"]);
        p["stages"][0]["reassessment"]["pending"] = json!({
            "kind":kind,"old_agreement":old,"signature":"reserved",
            "evidence":{"local_validity":"legacy reviewer setting changed"}
        });
        f.ctx.save_plan(&p).unwrap();
        let settings = f.ctx.app.settings.lock().unwrap().clone();
        let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
        let ctx = app.context(f.root.to_str().unwrap());
        let mut restored = ctx.load_plan().unwrap();
        // A real capability change must still block and retain its reservation.
        let options = ctx.routing_options().unwrap();
        let mut changed = options.clone();
        changed[0]["resolved_id"] = json!("different-model");
        ctx.app.settings.lock().unwrap()["mock_routing_options"] = json!(changed);
        assert!(ctx.assignment_boundary(&mut restored, 0).is_err());
        assert!(ctx.load_plan().unwrap()["stages"][0]["reassessment"]["pending"].is_object());
        ctx.app.settings.lock().unwrap()["mock_routing_options"] = json!(options);
        let result = ctx.assignment_boundary(&mut restored, 0);
        if kind == "material_assignment_change" {
            assert_eq!(result.unwrap(), old);
            assert!(restored["stages"][0]["reassessment"]["pending"].is_null());
            assert_eq!(restored["stages"][0]["reassessment"]["history"][0]["kind"], "material_assignment_restored");
            assert_eq!(ctx.assignment_boundary(&mut restored, 0).unwrap(), old);
        } else {
            assert!(result.is_err());
            assert!(restored["stages"][0]["reassessment"]["pending"].is_object());
        }
        assert_eq!(restored["stages"][0]["rounds"], 1);
        assert_eq!(restored["stages"][0]["review_budget"], 3);
        assert_eq!(restored["stages"][0]["reassessment"]["count"], 1);
        assert_eq!(restored["stages"][0]["reassessment"]["signatures"], json!(["reserved"]));
        let settings = ctx.app.settings.lock().unwrap();
        assert_eq!(settings["mock_routing_planner_requests"].as_array().unwrap().len(), 1);
        assert_eq!(settings["mock_routing_architect_requests"].as_array().unwrap().len(), 1);
    }
}
#[test]
fn outcome_channel_rejects_forged_identity_and_validates_escalation_evidence() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    let valid = json!({"version":1,"plan_id":p["plan_id"],"stage_id":1,"attempt_id":"attempt","turn_id":"turn","status":"escalation","evidence":["test greeting fails on empty input"],"request":{"kind":"reasoning","reason":"Repeated attempt cannot establish invariant","required_capability":"stronger reasoning"}});
    for key in ["plan_id", "stage_id", "attempt_id", "turn_id", "extra"] {
        let mut v = valid.clone();
        v[key] = json!("forged");
        assert!(
            f.ctx
                .implementer_outcome(
                    &mut p,
                    0,
                    "turn",
                    &crate::agent::AgentResult {
                        output: v.to_string(),
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    assert!(
        f.ctx
            .implementer_outcome(
                &mut p,
                0,
                "turn",
                &crate::agent::AgentResult {
                    output: valid.to_string(),
                    ..Default::default()
                }
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn constraint_scope_and_context_changes_are_evidenced_and_bounded() {
    let f = Fixture::new();
    f.attempt();
    let mut cp = f
        .ctx
        .architecture_store()
        .checkpoint(&f.ctx.load_plan().unwrap())
        .unwrap();
    cp["constraints"] = json!(["Greeting must preserve the public interface"]);
    let mut p = f
        .ctx
        .architecture_store()
        .publish(
            f.ctx.load_plan().unwrap(),
            cp,
            json!({"kind":"test_constraint"}),
        )
        .unwrap();
    f.ctx.assignment_boundary(&mut p, 0).unwrap();
    assert_eq!(f.counts(), (2, 2));
    assert_eq!(
        p["stages"][0]["model_agreement"]["trigger"],
        "material_assignment_change"
    );
    f.ctx
        .reassess(
            &mut p,
            0,
            "material_scope_change",
            json!({"role":"implementer","evidence":"new public interface constraint"}),
        )
        .unwrap();
    assert_eq!(f.counts(), (3, 3));
    assert_eq!(
        p["stages"][0]["model_agreement"]["trigger"],
        "material_scope_change"
    );
    let g = Fixture::new();
    let mut p = g.attempt();
    let mut options = g.ctx.routing_options().unwrap();
    options[0]["limits"] = json!({"context_window":100});
    options[1]["limits"] = json!({"context_window":1000});
    g.set("mock_routing_options", json!(options));
    g.ctx.assignment_boundary(&mut p, 0).unwrap();
    g.set("mock_usage", json!({"input":90,"output":1,"total":91}));
    g.choose("codex", "large", "provider_default");
    let p = g.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(
        p["stages"][0]["model_agreement"]["trigger"],
        "context_pressure"
    );
}
#[test]
fn native_effort_cannot_replace_required_capability_and_retired_models_cannot_return() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.choose("codex", "large", "provider_default");
    f.ctx
        .reassess(
            &mut p,
            0,
            "implementer_escalation",
            json!({"need":"strong"}),
        )
        .unwrap();
    let mut options = f.ctx.routing_options().unwrap();
    options[1]["eligible"] = json!(false);
    options[1]["error"] = json!("critical model removed");
    f.set("mock_routing_options", json!(options));
    f.choose("codex", "small", "provider_default");
    assert!(f.ctx.assignment_boundary(&mut p, 0).is_err());
    assert_eq!(
        p["stages"][0]["model_agreement"]["effective"]["model"],
        "large"
    );
    let before = f.counts();
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    let mut p = ctx.load_plan().unwrap();
    assert!(ctx.assignment_boundary(&mut p, 0).is_err());
    let settings = ctx.app.settings.lock().unwrap();
    assert_eq!(
        settings["mock_routing_planner_requests"]
            .as_array()
            .unwrap()
            .len(),
        before.0
    );
}
#[test]
fn operational_budget_is_restored_and_review_retry_uses_fresh_sessions() {
    let f = Fixture::new();
    let mut p = f.attempt();
    assert!(
        f.ctx
            .operational_retry(&mut p, 0, "429 rate limit", "implementer")
            .unwrap()
    );
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    let mut p = ctx.load_plan().unwrap();
    assert!(
        ctx.operational_retry(&mut p, 0, "Selected model is at capacity. Please try a different model.", "implementer")
            .unwrap()
    );
    assert!(
        !ctx.operational_retry(&mut p, 0, "429 rate limit", "implementer")
            .unwrap()
    );
    assert!(
        !ctx.operational_retry(&mut p, 0, "unauthorized: login required", "implementer")
            .unwrap()
    );
    let g = Fixture::new();
    g.set("mock_reviewer_actions", json!([{"error":"429 rate limit"}]));
    let p = g.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(g.counts(), (1, 1));
    assert_eq!(p["stages"][0]["reassessment"]["operational_retries"], 1);
    let settings = g.ctx.app.settings.lock().unwrap();
    assert!(
        settings["test_review_sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["role"] == "reviewer")
            .all(|r| r["session"].is_null())
    );
}

#[test]
fn structured_test_failures_require_repetition_and_strict_identity() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    for (turn, expected) in [("first", false), ("second", true)] {
        let outcome = json!({"version":1,"plan_id":p["plan_id"],"stage_id":1,"attempt_id":"attempt","turn_id":turn,"status":"test_failure","evidence":["cargo test greeting_empty fails with expected greeting"],"request":null});
        let trigger = f
            .ctx
            .implementer_outcome(
                &mut p,
                0,
                turn,
                &crate::agent::AgentResult {
                    output: outcome.to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(trigger.is_some(), expected);
    }
    assert_eq!(f.counts(), (1, 1));
}
#[test]
fn critical_removed_model_blocks_without_weaker_fallback_or_fix_reset() {
    let f = Fixture::new();
    let old = f.ctx.load_plan().unwrap();
    let mut candidate = old.clone();
    candidate["revision"] = json!(old["revision"].as_u64().unwrap() + 1);
    candidate["stages"][0]["instructions"] = json!("Implement atomic persistence safely");
    candidate["stages"][0]["model_constraint"] = json!({"provider":"codex"});
    f.choose("codex", "large", "provider_default");
    f.ctx
        .architect_publish(candidate, Some(&old), "critical scope")
        .unwrap();
    let mut p = f.attempt();
    assert_eq!(
        p["stages"][0]["model_agreement"]["policy_inputs"]["minimum_tier"],
        3
    );
    p["stages"][0]["rounds"] = json!(2);
    f.ctx.save_plan(&p).unwrap();
    let mut options = f.ctx.routing_options().unwrap();
    options[1]["eligible"] = json!(false);
    options[1]["error"] = json!("removed");
    f.set("mock_routing_options", json!(options));
    f.choose("codex", "small", "provider_default");
    assert!(f.ctx.assignment_boundary(&mut p, 0).is_err());
    assert_eq!(p["stages"][0]["rounds"], 2);
    assert_eq!(
        p["stages"][0]["model_agreement"]["effective"]["model"],
        "large"
    );
}

#[test]
fn fake_cli_structured_test_outcomes_escalate_and_preserve_real_partial_work() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let cli = f.root.join("fake-codex");
    fs::write(&cli,r#"#!/usr/bin/env python3
import sys,json,pathlib
model=sys.argv[sys.argv.index('-m')+1]
prompt=sys.argv[-1]
outcome,_=json.JSONDecoder().raw_decode(prompt.split('Finish with ONLY JSON: ',1)[1])
work=pathlib.Path('unfinished.txt')
with work.open('a') as output: output.write(model+' preserved previous work\n')
outcome['status']='test_failure' if model=='small' else 'completed'
outcome['evidence']=['greeting test fails on empty input'] if model=='small' else ['greeting test passes']
print(json.dumps({'type':'thread.started','thread_id':'11111111-2222-4333-8444-555555555555','model':model}))
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':json.dumps(outcome)}}))
print(json.dumps({'type':'turn.completed','usage':{'input_tokens':10,'output_tokens':5}}))
"#).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    f.set("test_cli_codex", json!(cli));
    f.set("test_real_implementation_cli", json!(true));
    f.set(
        "mock_verdicts",
        json!([{"approved":false,"issues":["Verify the failing greeting test"]}]),
    );
    f.choose("codex", "large", "provider_default");
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed");
    assert_eq!(s["rounds"], 3);
    assert_eq!(f.counts(), (2, 2));
    assert_eq!(
        s["model_agreement"]["trigger_evidence"]["role"],
        "implementer"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("unfinished.txt")).unwrap(),
        "small preserved previous work\nsmall preserved previous work\nlarge preserved previous work\n"
    );
    assert!(
        s["model_invocations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["model_reported"] == true)
    );
}

#[test]
fn stopped_architect_review_recovers_checkpoint_without_reselecting_or_resetting_budget() {
    let f = Fixture::new();
    let original = f.ctx.load_plan().unwrap();
    let checkpoint = f.ctx.architecture_store().checkpoint(&original).unwrap();
    f.set("mock_architect_actions", json!([{"stop":true}]));
    let stopped = f.run();
    assert!(f.root.join("mock.txt").exists());
    assert_eq!(stopped["stages"][0]["rounds"], 1);
    assert_eq!(stopped["stages"][0]["review_gate"]["status"], "interrupted");
    let saved = f.ctx.architecture_store().checkpoint(&stopped).unwrap();
    assert_eq!(saved["summary"], checkpoint["summary"]);
    assert_eq!(
        saved["session"]["reference"],
        checkpoint["session"]["reference"]
    );
    f.ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let resumed = f.run();
    assert_eq!(resumed["stages"][0]["status"], "committed");
    assert_eq!(resumed["stages"][0]["rounds"], 2);
    assert_eq!(
        resumed["stages"][0]["attempt_id"],
        stopped["stages"][0]["attempt_id"]
    );
    assert_ne!(
        f.ctx.architecture_store().checkpoint(&resumed).unwrap()["session"]["reference"],
        checkpoint["session"]["reference"]
    );
    assert_eq!(f.counts(), (1, 1));
}

#[test]
fn a_scope_escalation_revises_the_stage_once_and_then_belongs_to_a_human() {
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["rounds"] = json!(2);
    f.ctx.save_plan(&p).unwrap();
    let outcome = json!({"status":"escalation","evidence":["Qt normalises CRLF to LF on assignment"],
        "request":{"kind":"scope","reason":"Acceptance demands byte equality Qt cannot provide",
                   "required_capability":"Reconcile the acceptance criterion"}});
    let revision = json!({"revised":{
        "instructions":"Implement greeting","acceptance":"Greeting works on normalised text"},
        "removed":"Dropped byte equality; Qt normalises newlines"});
    f.set("mock_scope_output", json!(format!(
        "I inspected settings[\"reviewer\"]. The escalation is correct.\n\n{revision}"
    )));
    let (revised, message) = f.ctx.renegotiate_scope(&mut p, 0, &outcome).unwrap();
    assert!(revised);
    assert_eq!(message, "Dropped byte equality; Qt normalises newlines");
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["acceptance"], "Greeting works on normalised text");
    // Reconciliation replenishes the rounds the unbuildable text consumed, and
    // the run keeps the approval the user already gave this plan.
    assert_eq!(saved["stages"][0]["rounds"], 0);
    assert_eq!(saved["status"], "ready");
    assert_eq!(saved["stages"][0]["scope_renegotiations"], 1);
    assert_eq!(saved["stages"][0]["scope_history"][0]["reason"],
        "Acceptance demands byte equality Qt cannot provide");
    // A second escalation on the same stage stops instead of looping.
    let mut again = saved;
    let (revised, message) = f.ctx.renegotiate_scope(&mut again, 0, &outcome).unwrap();
    assert!(!revised);
    assert!(message.contains("already revised once"), "{message}");
    assert_eq!(f.ctx.load_plan().unwrap()["stages"][0]["scope_renegotiations"], 1);
}

#[test]
fn a_refused_or_unchanged_scope_revision_leaves_the_stage_exactly_as_written() {
    let f = Fixture::new();
    let outcome = json!({"status":"escalation","evidence":["cannot be done"],
        "request":{"kind":"scope","reason":"too hard","required_capability":"more"}});
    for (answer, expected) in [
        (json!({"refused":"The acceptance is met by trimming the input first"}), "holds the stage buildable"),
        (json!({"revised":{"instructions":"Implement greeting","acceptance":"Greeting works"}}), "unchanged"),
    ] {
        let mut p = f.attempt();
        f.set("mock_scope_output", answer);
        let (revised, message) = f.ctx.renegotiate_scope(&mut p, 0, &outcome).unwrap();
        assert!(!revised);
        assert!(message.contains(expected), "{message}");
        let saved = f.ctx.load_plan().unwrap();
        assert_eq!(saved["stages"][0]["acceptance"], "Greeting works");
        assert!(saved["stages"][0]["scope_renegotiations"].is_null());
    }
}
