use super::*;
use crate::app::{App, planning::ScopeResolution};
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
        self.choose_task(provider, model, effort, "functionality");
    }
    fn choose_task(&self, provider: &str, model: &str, effort: &str, task: &str) {
        self.set("mock_routing_planner_outputs",json!([{"proposals":[{"stage_id":1,"proposal":{"risk":"standard","complexity":"standard","task":task,"provider":provider,"model":model,"native_effort":effort,"rationale":"Concrete failures justify this adequate assignment."}}]}]));
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
    for kind in ["material_assignment_change", "provider_operational_failure", "repeated_reasoning_failure"] {
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
        if kind != "repeated_reasoning_failure" {
            assert_eq!(result.unwrap(), old);
            assert!(restored["stages"][0]["reassessment"]["pending"].is_null());
            assert_eq!(restored["stages"][0]["reassessment"]["history"][0]["kind"],
                if kind == "provider_operational_failure" {"provider_operation_resumed"} else {"material_assignment_restored"});
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
    let before = p.clone();
    for output in [format!("{valid}\n{{\"status\":\"completed\"}}"),
        format!("```json\n{valid}"),
        valid.to_string().replacen("\"status\":", "\"status\":\"completed\",\"status\":", 1)] {
        assert!(f.ctx.validate_implementer_response(&p, 0, "turn", &output).is_err());
    }
    let fenced = format!("```json\n{valid}\n```");
    f.ctx.validate_implementer_response(&p, 0, "turn", &fenced).unwrap();
    assert_eq!(parse_outcome(&p, 0, "turn", &fenced).unwrap().unwrap().status, "escalation");
    assert_eq!(p, before);
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
    f.choose_task("codex", "large", "provider_default", "persistence");
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
    f.choose_task("codex", "small", "provider_default", "persistence");
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
fn fake_cli_repairs_outcome_with_same_model_readonly_and_preserves_single_implementation() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let cli = f.root.join("fake-codex");
    fs::write(&cli, r#"#!/usr/bin/env python3
import sys,json,pathlib
if '--help' in sys.argv:
    print('--sandbox read-only --ignore-user-config --ignore-rules --json resume')
    sys.exit(0)
model=sys.argv[sys.argv.index('-m')+1]
prompt=sys.stdin.read() if sys.argv[-1]=='-' else sys.argv[-1]
outcome,_=json.JSONDecoder().raw_decode(prompt.split('Finish with ONLY JSON: ',1)[1])
if 'RESPONSE CORRECTION:' in prompt:
    assert sys.argv[sys.argv.index('--sandbox')+1]=='read-only'
else:
    with pathlib.Path('unfinished.txt').open('a') as work: work.write('implemented once\n')
    outcome['unexpected_field']='repair this report without repeating work'
print(json.dumps({'type':'thread.started','thread_id':'11111111-2222-4333-8444-555555555555','model':model}))
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':json.dumps(outcome)}}))
print(json.dumps({'type':'turn.completed','usage':{'input_tokens':10,'output_tokens':5}}))
"#).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    f.set("test_cli_codex", json!(cli));
    f.set("test_real_implementation_cli", json!(true));
    let p = f.run();
    let stage = &p["stages"][0];
    assert_eq!(stage["status"], "committed", "{stage}");
    assert_eq!(stage["rounds"], 1);
    assert_eq!(stage["implementer_outcome"]["status"], "completed");
    assert_eq!(fs::read_to_string(f.root.join("unfinished.txt")).unwrap(), "implemented once\n");
    let settings = f.ctx.app.settings.lock().unwrap();
    let requests = settings["mock_agent_requests"].as_array().unwrap();
    let implementer = requests.iter().find(|r| r["role"] == "implementer").unwrap();
    let corrections: Vec<_> = requests.iter().filter(|r| r["role"] == "response_correction").collect();
    assert_eq!(corrections.len(), 1);
    assert_eq!(corrections[0]["provider"], implementer["provider"]);
    assert_eq!(corrections[0]["model"], implementer["model"]);
    assert_eq!(corrections[0]["effort"], implementer["effort"]);
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
    let resolution = f.ctx.renegotiate_scope(&mut p, 0, &outcome).unwrap();
    assert_eq!(resolution, ScopeResolution::Revised("Dropped byte equality; Qt normalises newlines".into()));
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
    let ScopeResolution::Blocked(message) = f.ctx.renegotiate_scope(&mut again, 0, &outcome).unwrap() else { panic!("expected bounded negotiation") };
    assert!(message.contains("already revised once"), "{message}");
    assert_eq!(f.ctx.load_plan().unwrap()["stages"][0]["scope_renegotiations"], 1);
}

#[test]
fn an_unchanged_scope_response_without_explanation_does_not_authorize_continuation() {
    let f = Fixture::new();
    let mut p = f.attempt();
    let outcome = scope_escalation();
    f.set("mock_scope_output", json!({"revised":{"instructions":"Implement greeting","acceptance":"Greeting works"}}));
    let message = f.ctx.renegotiate_scope(&mut p, 0, &outcome).unwrap_err();
    assert!(message.contains("unchanged without clarification"));
    assert_eq!(f.ctx.load_plan().unwrap()["stages"][0]["acceptance"], "Greeting works");
    assert_eq!(f.ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().unwrap()
        .iter().filter(|r| r["role"] == "planner").count(), 4);
}

#[test]
fn scope_answer_json_and_fields_are_corrected_before_saving_clarification() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.set("mock_scope_output", json!(["{broken", {"revised":{"instructions":42}},
        {"refused":"Implement the existing requirements using the documented boundary."}]));
    let resolution = f.ctx.renegotiate_scope(&mut p, 0, &scope_escalation()).unwrap();
    assert!(matches!(resolution, ScopeResolution::Clarified(_)));
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["acceptance"], "Greeting works");
    assert_eq!(saved["stages"][0]["scope_clarification"]["pending"], true);
    let settings = f.ctx.app.settings.lock().unwrap();
    let calls = settings["mock_agent_requests"].as_array().unwrap();
    assert_eq!(calls.iter().filter(|call| call["role"] == "planner").count(), 3);
}

#[test]
fn contradictory_scope_responses_share_one_budget_and_preserve_the_saved_stage() {
    for clarified in [false, true] {
        let f = Fixture::new();
        let mut p = f.attempt();
        let saved = f.ctx.load_plan().unwrap();
        let contradictory = json!({"refused":"The existing task is buildable", "revised":{
            "instructions":"Different instructions", "acceptance":"Different acceptance"}});
        f.set("mock_scope_output", json!([
            contradictory,
            r#"{"refused":"Keep it","refused":"Change it"}"#,
            {"revised":{"instructions":"Change it","acceptance":42}},
            if clarified { json!({"refused":"Keep the existing scope; use the documented boundary."}) } else { contradictory },
        ]));
        let result = f.ctx.renegotiate_scope(&mut p, 0, &scope_escalation());
        assert_eq!(result.is_ok(), clarified, "{result:?}");
        let current = f.ctx.load_plan().unwrap();
        if clarified {
            assert!(matches!(result.unwrap(), ScopeResolution::Clarified(_)));
            for field in ["instructions", "acceptance", "rounds", "review_budget", "scope_renegotiations"] {
                assert_eq!(current["stages"][0][field], saved["stages"][0][field]);
            }
        } else { assert_eq!(current, saved); }
        let settings = f.ctx.app.settings.lock().unwrap();
        let calls: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
        assert_eq!(calls.len(), 4);
        assert!(calls[1]["prompt"].as_str().unwrap().contains("exactly one of revised or refused"));
    }
}

fn scope_escalation() -> Value {
    json!({"status":"escalation","evidence":["The worker has no production caller until the next stage"],
        "request":{"kind":"scope","reason":"Interim dead-code warnings","required_capability":"Clarify stage boundaries"}})
}

#[test]
fn planner_clarification_survives_restart_without_changing_scope_or_resetting_budget() {
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["rounds"] = json!(1);
    p["stages"][0]["review_budget"] = json!(3);
    p["stages"][0]["previous_requests"] = json!({"approved":false,"issues":["Preserve the existing empty-input check"]});
    f.ctx.save_plan(&p).unwrap();
    let explanation = "Interim warnings are expected; wire the caller in the next stage.";
    f.set("mock_scope_output", json!({"refused":explanation}));
    let before = p.clone();
    let resolution = f.ctx.renegotiate_scope(&mut p, 0, &scope_escalation()).unwrap();
    assert_eq!(resolution, ScopeResolution::Clarified(explanation.into()));
    let saved = f.ctx.load_plan().unwrap();
    for key in ["instructions", "acceptance", "rounds", "review_budget", "attempt_id", "previous_requests"] {
        assert_eq!(saved["stages"][0][key], before["stages"][0][key], "{key}");
    }
    assert_eq!(saved["revision"], p["revision"]);
    assert!(saved["stages"][0]["scope_renegotiations"].is_null());
    let mut settings = f.ctx.app.settings.lock().unwrap().clone();
    settings["mock_scope_output"] = Value::Null;
    settings["mock_agent_requests"] = json!([]);
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    ctx.run_worker();
    let completed = ctx.load_plan().unwrap();
    assert_eq!(completed["stages"][0]["status"], "committed");
    assert_eq!(completed["stages"][0]["rounds"], 2);
    assert_eq!(completed["stages"][0]["review_budget"], 3);
    assert_eq!(completed["stages"][0]["attempt_id"], p["stages"][0]["attempt_id"]);
    let settings = ctx.app.settings.lock().unwrap();
    let requests = settings["mock_agent_requests"].as_array().unwrap();
    assert!(!requests.iter().any(|r| r["role"] == "planner"));
    let implementer = requests.iter().find(|r| r["role"] == "implementer").unwrap();
    let prompt = implementer["prompt"].as_str().unwrap();
    assert!(prompt.contains(explanation));
    assert!(prompt.contains("Preserve the existing empty-input check"));
}

fn install_scope_cli(f: &Fixture, repeat: bool) {
    install_escalating_cli(f, repeat, "scope");
}

fn install_escalating_cli(f: &Fixture, repeat: bool, kind: &str) {
    use std::os::unix::fs::PermissionsExt;
    let cli = f.root.join("fake-scope-codex");
    fs::write(&cli, format!(r#"#!/usr/bin/env python3
import sys,json,pathlib
model=sys.argv[sys.argv.index('-m')+1]
prompt=sys.argv[-1]
outcome,_=json.JSONDecoder().raw_decode(prompt.split('Finish with ONLY JSON: ',1)[1])
with pathlib.Path('unfinished.txt').open('a') as output: output.write('preserved work\n')
clarified='Interim warnings are expected; wire the caller in the next stage.' in prompt
if not clarified or {repeat}:
    outcome.update(status='escalation',evidence=['Caller is introduced in the next stage'],request={{'kind':'{kind}','reason':'Interim warnings','required_capability':'Clarify stage boundaries'}})
print(json.dumps({{'type':'thread.started','thread_id':'11111111-2222-4333-8444-555555555555','model':model}}))
print(json.dumps({{'type':'item.completed','item':{{'type':'agent_message','text':json.dumps(outcome)}}}}))
print(json.dumps({{'type':'turn.completed','usage':{{'input_tokens':10,'output_tokens':5}}}}))
"#, repeat=if repeat {"True"} else {"False"}, kind=kind)).unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    f.set("test_cli_codex", json!(cli));
    f.set("test_real_implementation_cli", json!(true));
    f.set("mock_scope_output", json!({"refused":"Interim warnings are expected; wire the caller in the next stage."}));
}

#[test]
fn refused_scope_returns_to_implementation_then_obeys_each_review_cadence() {
    for cadence in ["per_stage", "per_plan"] {
        let f = Fixture::new();
        f.set("review_cadence", json!({"architect":cadence,"reviewer":cadence}));
        install_scope_cli(&f, false);
        let p = f.run();
        let s = &p["stages"][0];
        assert_eq!(s["status"], "committed", "{p}");
        assert_eq!(s["rounds"], 2);
        assert_eq!(s["instructions"], "Implement greeting");
        assert_eq!(s["acceptance"], "Greeting works");
        assert_eq!(s["scope_clarification"]["delivered_round"], 2);
        assert_eq!(fs::read_to_string(f.root.join("unfinished.txt")).unwrap(), "preserved work\npreserved work\n");
        let roles: Vec<_> = s["model_invocations"].as_array().unwrap().iter()
            .filter(|r| matches!(r["role"].as_str(), Some("implementer" | "fixer")))
            .map(|r| r["role"].clone()).collect();
        assert_eq!(roles, vec![json!("implementer"), json!("implementer")]);
        if cadence == "per_plan" {
            assert_eq!(s["review_gate"]["status"], "deferred");
            assert!(s["reviews"].as_array().unwrap().is_empty());
            assert_eq!(p["plan_review"]["gate"]["status"], "approved");
        } else {
            assert_eq!(s["review_gate"]["status"], "approved");
            assert!(!s["reviews"].as_array().unwrap().is_empty());
        }
    }
}

#[test]
fn repeated_scope_escalation_after_clarification_blocks_without_replaying_after_restart() {
    let f = Fixture::new();
    install_scope_cli(&f, true);
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "blocked");
    assert_eq!(p["stages"][0]["review_gate"]["status"], "scope_blocked");
    assert_eq!(p["stages"][0]["rounds"], 2);
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let scope_calls = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").count();
    assert_eq!(scope_calls, 1);
    let work = fs::read_to_string(f.root.join("unfinished.txt")).unwrap();
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    ctx.run_worker();
    let resumed = ctx.load_plan().unwrap();
    assert_eq!(resumed["stages"][0]["status"], "blocked");
    assert_eq!(resumed["stages"][0]["rounds"], 2);
    assert_eq!(fs::read_to_string(f.root.join("unfinished.txt")).unwrap(), work);
}

#[test]
fn pending_scope_revision_survives_routing_failure_and_restart_before_implementation() {
    let f = Fixture::new();
    let mut p = f.attempt();
    let outcome = json!({"status":"escalation","evidence":["The old acceptance is impossible"],
        "request":{"kind":"scope","reason":"Contradictory acceptance","required_capability":"Revise acceptance"}});
    f.set("mock_scope_output", json!({"revised":{
        "instructions":"Implement greeting with normalised text","acceptance":"Greeting works on normalised text"},
        "removed":"Dropped impossible byte equality"}));
    f.set("mock_routing_planner_outputs", json!(vec![json!({"proposals":[]}); 4]));
    let error = f.ctx.renegotiate_scope(&mut p, 0, &outcome).unwrap_err();
    assert!(error.contains("proposal count mismatch"), "{error}");
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["acceptance"], "Greeting works");
    assert_eq!(saved["stages"][0]["scope_renegotiations"], 1);
    assert_eq!(saved["stages"][0]["scope_revision_pending"]["acceptance"], "Greeting works on normalised text");
    assert_eq!(p, saved);

    // A fresh engine resumes publication before running the implementer. No
    // replacement scope answer is available, so another negotiation would fail.
    let mut settings = f.ctx.app.settings.lock().unwrap().clone();
    settings["mock_scope_output"] = Value::Null;
    settings["mock_agent_requests"] = json!([]);
    let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = app.context(f.root.to_str().unwrap());
    ctx.run_worker();
    let completed = ctx.load_plan().unwrap();
    let stage = &completed["stages"][0];
    assert_eq!(stage["status"], "committed", "{completed}");
    assert_eq!(stage["acceptance"], "Greeting works on normalised text");
    assert_eq!(stage["scope_renegotiations"], 1);
    assert_eq!(stage["scope_history"].as_array().unwrap().len(), 1);
    assert!(stage.get("scope_revision_pending").is_none());
    let settings = ctx.app.settings.lock().unwrap();
    let requests = settings["mock_agent_requests"].as_array().unwrap();
    assert!(!requests.iter().any(|r| r["role"] == "planner"));
    let implementer = requests.iter().find(|r| r["role"] == "implementer").unwrap();
    assert!(implementer["prompt"].as_str().unwrap().contains("Greeting works on normalised text"));
}

#[test]
fn pending_scope_revision_cannot_overwrite_changed_stage_inputs() {
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["scope_revision_pending"] = json!({
        "source_inputs":crate::plan::stage_inputs(&p, 0),
        "instructions":"Old planner instructions", "acceptance":"Old planner acceptance",
    });
    p["stages"][0]["instructions"] = json!("New user instructions");
    f.ctx.save_plan(&p).unwrap();
    let error = f.ctx.resume_scope_revision(&mut p, 0).unwrap_err();
    assert!(error.contains("no longer matches stage inputs"), "{error}");
    assert_eq!(f.ctx.load_plan().unwrap()["stages"][0]["instructions"], "New user instructions");
}

#[test]
fn tier_plan_executes_local_selection_and_preserves_bounded_failure_escalation() {
    let f = Fixture::new();
    f.set("mock_routing_planner_outputs", json!([{"proposals":[{"stage_id":1,"proposal":{
        "risk":"standard","complexity":"standard","task":"functionality","tier":"standard",
        "rationale":"Ordinary greeting implementation needs standard capability."}}]}]));
    let mut candidate = f.ctx.load_plan().unwrap();
    for key in ["plan_id", "revision", "architecture"] { candidate.as_object_mut().unwrap().remove(key); }
    for key in ["model_proposal", "model_proposal_inputs", "model_agreement"] {
        candidate["stages"][0].as_object_mut().unwrap().remove(key);
    }
    let p = f.ctx.architect_publish(candidate, None, "tier fixture").unwrap();
    assert_eq!(p["stages"][0]["model_agreement"]["version"], 2);
    let tier_agreement = p["stages"][0]["model_agreement"]["id"].clone();
    f.failures();
    f.choose("codex", "large", "provider_default");
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{s}");
    assert_eq!(s["model_invocations"][0]["agreement_id"], tier_agreement);
    assert_eq!(s["model_invocations"][0]["requested"]["model"], "small");
    assert_eq!(s["model_invocations"][2]["requested"]["model"], "large");
    assert_eq!(s["reassessment"]["count"], 1);
    assert_eq!(s["rounds"], 3);
    assert_eq!(s["reassessment"]["history"][0]["old_agreement"]["kind"], "selection");
}

fn archived_history(ctx: &Ctx) -> Vec<Value> {
    let (mut entries, mut cursor) = (Vec::new(), 0);
    loop {
        let page = ctx.architecture_store().history(None, cursor, 100).unwrap();
        for item in page["items"].as_array().unwrap() {
            for group in item["payload"]["archived_reassessment_history"].as_array().into_iter().flatten() {
                assert_eq!(group["stage_id"], 1);
                entries.extend(group["entries"].as_array().unwrap().iter().cloned());
            }
        }
        match page["next_cursor"].as_u64() {
            Some(next) => cursor = next,
            None => return entries,
        }
    }
}

#[test]
fn history_positions_are_monotonic_and_legacy_histories_are_numbered_once() {
    let mut state = json!({"history":[]});
    for n in 0..3 {
        push_history(&mut state, json!({"kind":"operational_retry","retry":n}));
    }
    assert_eq!(state["history_count"], 3);
    assert_eq!(state["history"].as_array().unwrap().iter().map(|h| h["seq"].clone()).collect::<Vec<_>>(), vec![json!(0), json!(1), json!(2)]);
    // Legacy arrays are numbered from their index; missing, duplicate or
    // decreasing positions follow their predecessor so nothing is skipped.
    let mut legacy = json!({"history":[{"kind":"a"},{"kind":"b","seq":5},{"kind":"c","seq":5},{"kind":"d","seq":1},{"kind":"e"}]});
    normalize_history(&mut legacy);
    assert_eq!(legacy["history"].as_array().unwrap().iter().map(|h| h["seq"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0, 5, 6, 7, 8]);
    assert_eq!((legacy["history_count"].clone(), legacy["history_archived_through"].clone()), (json!(9), json!(0)));
    let before = legacy.clone();
    normalize_history(&mut legacy);
    assert_eq!(legacy, before);
    push_history(&mut legacy, json!({"kind":"f"}));
    assert_eq!(legacy["history"][5]["seq"], 9);
    // A trimmed legacy array without positions continues after the archived prefix.
    let mut trimmed = json!({"history":[{"kind":"x"},{"kind":"y"}],"history_count":7,"history_archived_through":5});
    normalize_history(&mut trimmed);
    assert_eq!((trimmed["history"][0]["seq"].clone(), trimmed["history"][1]["seq"].clone()), (json!(5), json!(6)));
}

#[test]
fn publication_keeps_recent_history_and_archives_older_entries_exactly_once() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    let entries: Vec<Value> = (0..12).map(|n| json!({"kind":"operational_retry","role":"implementer",
        "failure_kind":"transient","error":format!("error {n}"),"retry":n,
        "old_agreement":{"id":format!("old-{n}"),"dialogue":[format!("turn {n}")]}})).collect();
    for entry in &entries {
        push_history(&mut p["stages"][0]["reassessment"], entry.clone());
    }
    let stale = p.clone();
    f.ctx.save_plan(&p).unwrap();
    let saved = f.ctx.load_plan().unwrap();
    let state = &saved["stages"][0]["reassessment"];
    assert_eq!(state["history"].as_array().unwrap().len(), HISTORY_KEEP);
    assert_eq!(state["history_count"], 12);
    assert_eq!(state["history_archived_through"], 8);
    assert_eq!(state["history"], json!(stale["stages"][0]["reassessment"]["history"].as_array().unwrap()[8..]));
    let expected: Vec<Value> = stale["stages"][0]["reassessment"]["history"].as_array().unwrap()[..8].to_vec();
    assert_eq!(archived_history(&f.ctx), expected);
    for (n, entry) in expected.iter().enumerate() {
        assert_eq!(entry["seq"], n);
        assert_eq!(entry["old_agreement"], entries[n]["old_agreement"]);
    }
    let polled = f.ctx.architecture_store().state_plan(saved.clone());
    assert_eq!(polled["stages"][0]["reassessment"]["history_count"], 12);

    // A stale in-memory copy that still holds every entry archives nothing again,
    // through the plan save path and directly through publication.
    f.ctx.save_plan(&stale).unwrap();
    assert_eq!(f.ctx.load_plan().unwrap(), saved);
    let mut direct = stale.clone();
    direct["architecture"] = saved["architecture"].clone();
    let cp = f.ctx.architecture_store().checkpoint(&saved).unwrap();
    let published = f.ctx.architecture_store().publish(direct, cp, json!({"kind":"stale_copy"})).unwrap();
    assert_eq!(published["stages"][0]["reassessment"], saved["stages"][0]["reassessment"]);
    assert_eq!(archived_history(&f.ctx), expected);

    // The stale copy keeps working: a new entry archives only the next oldest one.
    let mut next = stale.clone();
    push_history(&mut next["stages"][0]["reassessment"], json!({"kind":"operational_retry","retry":12}));
    f.ctx.save_plan(&next).unwrap();
    let saved = f.ctx.load_plan().unwrap();
    let state = &saved["stages"][0]["reassessment"];
    assert_eq!(state["history_count"], 13);
    assert_eq!(state["history"].as_array().unwrap().iter().map(|h| h["seq"].as_u64().unwrap()).collect::<Vec<_>>(), vec![9, 10, 11, 12]);
    let archived = archived_history(&f.ctx);
    assert_eq!(archived.iter().map(|h| h["seq"].as_u64().unwrap()).collect::<Vec<_>>(), (0..9).collect::<Vec<_>>());
    assert_eq!(archived[8], stale["stages"][0]["reassessment"]["history"][8]);
}

#[test]
fn archive_batches_are_bounded_and_drain_over_successive_publications() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    // Legacy entries: no positions or counters, as saved before bounding.
    let legacy: Vec<Value> = (0..40).map(|n| json!({"kind":"operational_retry","retry":n})).collect();
    p["stages"][0]["reassessment"]["history"] = json!(legacy);
    for key in ["history_count", "history_archived_through"] {
        p["stages"][0]["reassessment"].as_object_mut().unwrap().remove(key);
    }
    f.ctx.save_plan(&p).unwrap();
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["reassessment"]["history"].as_array().unwrap().len(), 40 - ARCHIVE_MAX_ENTRIES);
    assert_eq!(saved["stages"][0]["reassessment"]["history_count"], 40);
    f.ctx.save_plan(&saved).unwrap();
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["reassessment"]["history"].as_array().unwrap().len(), HISTORY_KEEP);
    let archived = archived_history(&f.ctx);
    assert_eq!(archived.iter().map(|h| h["retry"].as_u64().unwrap()).collect::<Vec<_>>(), (0..36).collect::<Vec<_>>());
    assert_eq!(archived.iter().map(|h| h["seq"].as_u64().unwrap()).collect::<Vec<_>>(), (0..36).collect::<Vec<_>>());
    // Nothing is left to drain: an unchanged save stays a no-op.
    let end = saved["architecture"]["event_end"].clone();
    f.ctx.save_plan(&saved).unwrap();
    assert_eq!(f.ctx.load_plan().unwrap()["architecture"]["event_end"], end);
}

#[test]
fn archived_history_leaves_reassessment_budget_and_blocking_unchanged() {
    let f = Fixture::new();
    f.set("reassessment_limits", json!({"max_reassessments":2,"max_operational_retries":2,"repeat_threshold":2,"context_percent":85}));
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    for n in 0..12 {
        push_history(&mut p["stages"][0]["reassessment"], json!({"kind":"operational_retry","retry":n}));
    }
    f.ctx.save_plan(&p).unwrap();
    let mut p = f.ctx.load_plan().unwrap();
    assert_eq!(p["stages"][0]["reassessment"]["history"].as_array().unwrap().len(), HISTORY_KEEP);
    let blocked = "model routing blocked: reassessment/signature budget exhausted; partial work and checkpoint retained";
    f.choose("codex", "large", "provider_default");
    f.ctx.reassess(&mut p, 0, "repeated_reasoning_failure", json!(["[reviewer] test failed"])).unwrap();
    let mut p = f.ctx.load_plan().unwrap();
    let state = &p["stages"][0]["reassessment"];
    assert_eq!((state["count"].clone(), state["signatures"].as_array().unwrap().len()), (json!(1), 1));
    assert_eq!(state["history_count"], 13);
    // A repeated signature blocks with the same message as before.
    assert_eq!(f.ctx.reassess(&mut p, 0, "repeated_reasoning_failure", json!(["[reviewer] test failed"])).unwrap_err(), blocked);
    // With the stage limit reached, a new trigger blocks the same way.
    p["stages"][0]["reassessment"]["limits"]["max_reassessments"] = json!(1);
    // The limit counts reassessments, not history entries.
    assert_eq!(f.ctx.reassess(&mut p, 0, "material_scope_change", json!(["new"])).unwrap_err(), blocked);
    let saved = f.ctx.load_plan().unwrap();
    assert_eq!(saved["stages"][0]["reassessment"]["count"], 1);
    assert_eq!(saved["stages"][0]["reassessment"]["signatures"].as_array().unwrap().len(), 1);
}

fn conflict_outcome(p: &Value, reason: Value) -> Value {
    json!({"version":1,"plan_id":p["plan_id"],"stage_id":1,"attempt_id":"attempt","turn_id":"turn","status":"escalation",
        "evidence":["docs-only stage flips the milestone test"],
        "request":{"kind":"constraint_conflict","reason":reason,"required_capability":"Planner correction of the stage constraints"}})
}

#[test]
fn implementers_and_fixers_can_report_a_validated_constraint_conflict() {
    let f = Fixture::new();
    let mut p = f.attempt();
    f.ctx.reassessment_init(&mut p, 0);
    assert!(f.ctx.outcome_prompt(&p, 0, "turn").contains("reasoning|scope|capability|constraint_conflict"));
    let valid = conflict_outcome(&p, json!("Change only docs contradicts keeping the milestone test passing"));
    assert_eq!(parse_outcome(&p, 0, "turn", &valid.to_string()).unwrap().unwrap().request.unwrap().kind, "constraint_conflict");
    for reason in [json!(""), json!("  ")] {
        let error = parse_outcome(&p, 0, "turn", &conflict_outcome(&p, reason).to_string()).err().unwrap();
        assert!(error.contains("invalid implementer escalation request"), "{error}");
    }
    let mut wrong_type = valid.clone();
    wrong_type["request"]["kind"] = json!(7);
    assert!(parse_outcome(&p, 0, "turn", &wrong_type.to_string()).is_err());
    let mut unknown = valid.clone();
    unknown["request"]["kind"] = json!("constraint");
    assert!(parse_outcome(&p, 0, "turn", &unknown.to_string()).is_err());
    let result = crate::agent::AgentResult { output: valid.to_string(), ..Default::default() };
    let (kind, evidence) = f.ctx.implementer_outcome(&mut p, 0, "turn", &result).unwrap().unwrap();
    assert_eq!(kind, "constraint_conflict");
    assert_eq!(evidence["request"]["reason"], valid["request"]["reason"]);
    // Each round's reply is kept, bounded, for a later planner hand-back.
    for _ in 0..crate::constraint_conflict::OUTCOME_HISTORY_KEEP + 2 {
        f.ctx.implementer_outcome(&mut p, 0, "turn", &result).unwrap();
    }
    let saved = f.ctx.load_plan().unwrap();
    let history = saved["stages"][0]["outcome_history"].as_array().unwrap();
    assert_eq!(history.len(), crate::constraint_conflict::OUTCOME_HISTORY_KEEP);
    assert_eq!(history[0]["attempt_id"], "attempt");
    assert_eq!(history[0]["request"]["kind"], "constraint_conflict");
}

#[test]
fn the_mock_planner_answers_a_constraint_escalation_through_the_validated_contract() {
    let f = Fixture::new();
    let mut p = f.attempt();
    p["stages"][0]["rounds"] = json!(1);
    p["stages"][0]["review_gate"] = json!({"status":"blocked","requests":[{"role":"reviewer","text":"Milestone test fails"}]});
    p["stages"][0]["attempt_head"] = json!(f.ctx.git(&["rev-parse", "HEAD"]).unwrap());
    f.ctx.save_plan(&p).unwrap();
    fs::write(f.root.join("changed.md"), "work\n").unwrap();
    let statement = "Change only docs contradicts keeping the milestone test passing".to_string();
    let at = f.ctx.begin_constraint_escalation(&mut p, 0, "fixer", "constraint_conflict", &statement,
        std::slice::from_ref(&statement)).unwrap();
    let pending = f.ctx.load_plan().unwrap()["stages"][0]["constraint_escalations"][at].clone();
    assert_eq!(pending["outcome"], "pending");
    assert_eq!(pending["trigger"]["source"], "fixer");
    assert_eq!(pending["inputs"]["changed_files"]["items"], json!(["changed.md"]));
    assert_eq!(pending["signature"], crate::constraint_conflict::signature(
        std::slice::from_ref(&statement), &[json!({"role":"architect","text":"milestone  test fails"})]));
    let revise = json!({"analysis":"The docs-only limit collides with a test reading real docs.",
        "decision":{"revise":{"stages":[{"id":1,"instructions":"Implement greeting with a fixture test","acceptance":"Greeting works"}]}}});
    f.set("mock_scope_output", json!(["{broken", {"analysis":"x","decision":{"refused":"a","revise":{}}}, revise]));
    let answer = f.ctx.consult_conflict_planner(&mut p, 0, at).unwrap();
    assert_eq!(answer.decision.name(), "revise");
    let saved = f.ctx.load_plan().unwrap();
    let record = &saved["stages"][0]["constraint_escalations"][at];
    assert_eq!(record["analysis"], revise["analysis"]);
    assert_eq!(record["decision"], "revise");
    assert_eq!(record["correction"]["revise"]["stages"], revise["decision"]["revise"]["stages"]);
    assert_eq!(record["correction"]["revise"]["insert_before"], json!([]));
    // Validation happens before anything changes the plan: the stage is intact.
    assert_eq!(saved["stages"][0]["instructions"], "Implement greeting");
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let planner: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
    assert_eq!(planner.len(), 3);
    let prompt = planner[0]["prompt"].as_str().unwrap();
    assert!(prompt.contains("Milestone test fails") && prompt.contains("changed.md") && prompt.contains("\"source\": \"fixer\""));
    assert!(planner[1]["prompt"].as_str().unwrap().contains("invalid constraint conflict answer"));
    drop(settings);
    f.ctx.settle_constraint_escalation(&mut p, 0, at, "awaiting_approval", Some("later stage changed")).unwrap();
    let settled = &f.ctx.load_plan().unwrap()["stages"][0]["constraint_escalations"][at];
    assert_eq!(settled["outcome"], "awaiting_approval");
    assert_eq!(settled["plan_revision"], saved["revision"]);
    assert_eq!(p["stages"][0]["constraint_escalations"][at], *settled);
    assert!(f.ctx.settle_constraint_escalation(&mut p, 0, at, "unknown", None).is_err());
}

fn conflict_refusal() -> Value {
    json!({"analysis":"The warnings are interim; the caller arrives in the next stage.",
        "decision":{"refused":"Interim warnings are expected; wire the caller in the next stage."}})
}

#[test]
fn an_implementer_constraint_conflict_goes_to_the_planner_once_and_is_recorded() {
    // Refused: the planner keeps the stage and the implementer completes it.
    let f = Fixture::new();
    install_escalating_cli(&f, false, "constraint_conflict");
    f.set("mock_scope_output", conflict_refusal());
    let p = f.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "committed", "{p}");
    assert_eq!(s["scope_clarification"]["message"], "Interim warnings are expected; wire the caller in the next stage.");
    assert_eq!(s["scope_clarification"]["source"], "constraint_conflict");
    let records = s["constraint_escalations"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["trigger"], json!({"source":"implementer","kind":"constraint_conflict","reason":"Interim warnings"}));
    assert_eq!(records[0]["decision"], "refused");
    assert_eq!(records[0]["outcome"], "refused");
    assert_eq!(records[0]["inputs"]["statements"]["items"], json!(["Interim warnings"]));
    assert_eq!(records[0]["inputs"]["fixer_replies"]["items"][0]["request"]["kind"], "constraint_conflict");
    let settings = f.ctx.app.settings.lock().unwrap();
    let planner: Vec<_> = settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").collect();
    assert_eq!(planner.len(), 1);
    let prompt = planner[0]["prompt"].as_str().unwrap();
    assert!(prompt.contains("REPORTED CONFLICT STATEMENTS") && prompt.contains("Interim warnings"), "{prompt}");
    drop(settings);

    // Repeated after the planner's answer: the same conflict blocks the stage
    // without a second planner pass, with the escalation record attached.
    let g = Fixture::new();
    install_escalating_cli(&g, true, "constraint_conflict");
    g.set("mock_scope_output", conflict_refusal());
    let p = g.run();
    let s = &p["stages"][0];
    assert_eq!(s["status"], "blocked");
    let outcomes: Vec<_> = s["constraint_escalations"].as_array().unwrap().iter()
        .map(|r| r["outcome"].clone()).collect();
    assert_eq!(outcomes, vec![json!("refused"), json!("blocked")]);
    assert_eq!(s["constraint_escalations"][1]["repeats"], 0);
    assert_eq!(s["review_gate"]["constraint_escalation"]["outcome"], "blocked");
    assert!(s["review_gate"]["reason"].as_str().unwrap().contains("already had its planner pass"));
    let settings = g.ctx.app.settings.lock().unwrap();
    assert_eq!(settings["mock_agent_requests"].as_array().unwrap().iter().filter(|r| r["role"] == "planner").count(), 1);
}

#[path = "constraint_escalation_tests.rs"]
mod constraint_escalation;
