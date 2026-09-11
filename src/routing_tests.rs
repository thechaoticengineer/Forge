use super::*;
use crate::{
    app::{App, PlanMode},
    architecture::identity,
};
use std::{fs, path::PathBuf, sync::Arc};

struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-routing-test-{}", identity()));
        fs::create_dir_all(&root).unwrap();
        let mut s = crate::plan::default_settings();
        s["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        s["planner"] = json!("mock");
        s["architect"] = json!("mock");
        s["reviewer"] = json!("claude");
        s["auto_push"] = json!(false);
        s["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"strong-test","tier":"strong","relative_cost_preference":5},
            {"provider":"codex","model":"budget-test","tier":"basic","relative_cost_preference":1},
            {"provider":"claude","model":"other-test","tier":"strong","relative_cost_preference":1}]);
        let app = Arc::new(App::new(root.to_str().unwrap(), s));
        let ctx = app.context(root.to_str().unwrap());
        ctx.git(&["init", "-q"]).unwrap();
        ctx.git(&["config", "user.name", "Test"]).unwrap();
        ctx.git(&["config", "user.email", "test@example.invalid"])
            .unwrap();
        ctx.git(&["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(root.join("README.md"), "A test project.\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }
    fn set(&self, key: &str, v: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = v;
    }
    fn counts(&self) -> (usize, usize) {
        let s = self.ctx.app.settings.lock().unwrap();
        (
            s["mock_routing_planner_requests"]
                .as_array()
                .map_or(0, Vec::len),
            s["mock_routing_architect_requests"]
                .as_array()
                .map_or(0, Vec::len),
        )
    }
    fn publish(&self, plan: Value) -> Result<Value, String> {
        self.ctx.architect_publish(plan, None, "test")
    }
    fn select(&self, p: Value) {
        self.set(
            "mock_routing_planner_outputs",
            json!([{"proposals":[{"stage_id":1,"proposal":p}]}]),
        );
    }
    fn mock_execution(&self) {
        self.set("implementer", json!("mock"));
        self.set("reviewer", json!("mock"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn stage(id: i64) -> Value {
    json!({"id":id,"title":"Small feature","instructions":"Add a greeting","acceptance":"Greeting works","commit":"feat: greeting","depends_on":[],"status":"pending","rounds":0})
}
fn plan() -> Value {
    json!({"goal":"Improve greetings","status":"draft","stages":[stage(1)]})
}
fn proposal(model: &str, risk: &str, task: &str) -> Value {
    json!({"risk":risk,"complexity":if risk == "critical" {"complex"} else {risk},"task":task,"provider":"codex","model":model,"native_effort":"provider_default","rationale":"Planner assessed this stage and configured adequacy separately from cost."})
}
fn evaluation(agree: bool, risk: &str, complexity: &str) -> Value {
    json!({"stage_id":1,"agree":agree,"risk":risk,"complexity":complexity,"task":"functionality","rationale":"Architect checked downstream interface compatibility and failure impact independently."})
}

#[test]
fn configured_strong_without_official_metadata_and_explicit_unverified_effort() {
    let f = Fixture::new();
    let mut p = plan();
    p["stages"][0]["instructions"] = json!("Implement complex concurrent persistence safely");
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] = json!("high");
    let mut choice = proposal("strong-test", "critical", "persistence");
    choice["native_effort"] = json!("high");
    f.select(choice);
    let p = f.publish(p).unwrap();
    let a = &p["stages"][0]["model_agreement"];
    assert_eq!(a["effective"]["native_effort"], "high");
    assert_eq!(a["availability"], "unverified");
    assert_eq!(a["policy_inputs"]["tier_provenance"], "configured");
    assert_eq!(a["policy_inputs"]["minimum_tier"], 3);
    assert_eq!(a["provenance"]["official_sources"], json!([]));
    assert_ne!(a["planner_reason"], a["architect_reason"]);
    assert_eq!(f.counts(), (1, 1));
    assert_eq!(a["proposal_ids"].as_array().unwrap().len(), 2);
}

#[test]
fn automatic_reviewer_selector_changes_reuse_all_stage_agreements() {
    let f = Fixture::new();
    f.set("reviewer", json!("codex"));
    let mut candidate = plan();
    candidate["stages"].as_array_mut().unwrap().push(stage(2));
    let p = f.publish(candidate).unwrap();
    let cp = f.ctx.architecture_store().checkpoint(&p).unwrap();
    assert_eq!(cp["agreements"]["1"]["policy_inputs"]["reviewer"], "codex");
    assert_eq!(cp["agreements"]["1"]["reviewer"]["provider"], "claude");
    f.set("reviewer", json!("claude"));
    assert!(f.ctx.routing_required(&p, &cp).unwrap().is_empty());
    for idx in 0..2 {
        assert_eq!(f.ctx.validated_assignment(&p, idx).unwrap(), p["stages"][idx]["model_agreement"]);
    }
    assert_eq!(f.counts(), (1, 1));
    // An explicit reviewer or manual routing keeps the selector authoritative.
    f.set("reviewer_model", json!("other-test"));
    assert!(f.ctx.routing_required(&p, &cp).is_err());
    f.set("reviewer_model", json!(""));
    f.set("automatic_routing", json!(false));
    assert_eq!(f.ctx.routing_required(&p, &cp).unwrap(), vec![1, 2]);
}
#[test]
fn critical_work_never_uses_cheapest_unclassified_or_inadequate_tier() {
    for model in ["budget-test", "hallucinated"] {
        let f = Fixture::new();
        f.select(proposal(model, "critical", "security"));
        assert!(f.publish(plan()).is_err());
        assert!(f.ctx.load_plan().is_none());
        assert_eq!(f.counts(), (1, 1));
    }
    let f = Fixture::new();
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([]);
    assert!(f.publish(plan()).unwrap_err().contains("no eligible model"));
    let s = f.ctx.app.settings.lock().unwrap().clone();
    let p: Proposal =
        serde_json::from_value(proposal("unclassified", "critical", "concurrency")).unwrap();
    assert!(policy_inputs(&s, &stage(1), &p, &[json!({"provider":"codex","model":"unclassified","effort":"provider_default","eligible":true})]).is_err());
    // Even if both participants underclassify, engine protects persistence implementation.
    let f = Fixture::new();
    f.select(proposal("budget-test", "simple", "functionality"));
    let mut plan = plan();
    plan["stages"][0]["instructions"] = json!("Add atomic persistent storage");
    assert!(f.publish(plan).unwrap_err().contains("strong"));
}
#[test]
fn simple_documentation_and_functionality_prefer_configured_cheaper_adequate() {
    for task in ["documentation", "functionality"] {
        for (model, valid) in [("strong-test", false), ("budget-test", true)] {
            let f = Fixture::new();
            f.select(proposal(model, "simple", task));
            assert_eq!(f.publish(plan()).is_ok(), valid, "{model} {task}");
        }
    }
}
#[test]
fn unknown_and_incomparable_prices_do_not_create_an_order() {
    let f = Fixture::new();
    let mut settings = f.ctx.app.settings.lock().unwrap().clone();
    let p: Proposal =
        serde_json::from_value(proposal("strong-test", "simple", "documentation")).unwrap();
    let mut options = f.ctx.routing_options().unwrap();
    for o in &mut options {
        o["relative_cost_preference"] = Value::Null;
    }
    assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok());
    options[0]["pricing"] =
        json!({"currency":"USD","unit":"million tokens","basis":"API","input":10,"output":20});
    options[1]["pricing"] =
        json!({"currency":"EUR","unit":"million tokens","basis":"API","input":1,"output":2});
    settings["routing_billing_basis"] = json!("API");
    assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok());
    options[1]["pricing"]["currency"] = json!("USD");
    assert!(
        policy_inputs(&settings, &stage(1), &p, &options)
            .unwrap_err()
            .contains("cheaper")
    );
    options[1]["pricing"]["output"] = json!(30); // Different input/output tradeoff is incomparable.
    assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok());
    options[1]["pricing"]["output"] = json!(2);
    settings["routing_billing_basis"] = Value::Null;
    assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok()); // API price is not subscription billing.
}
#[test]
fn exact_ids_native_efforts_constraints_and_review_conflicts_are_validated() {
    for (key, value) in [
        ("model", "invented"),
        ("native_effort", "ultra"),
        ("provider", "imaginary"),
    ] {
        let f = Fixture::new();
        let mut p = proposal("strong-test", "critical", "security");
        p[key] = json!(value);
        f.select(p);
        assert!(f.publish(plan()).is_err());
    }
    let f = Fixture::new();
    f.set("implementer_model", json!("budget-test"));
    f.select(proposal("strong-test", "critical", "persistence"));
    assert!(f.publish(plan()).unwrap_err().contains("constraint"));
    let mut p = plan();
    p["stages"][0]["model_constraint"] = json!({"provider":"codex","model":"strong-test"});
    f.select(proposal("strong-test", "critical", "persistence"));
    assert!(f.publish(p).is_ok()); // Stage overrides global.
    let f = Fixture::new();
    f.set("implementer", json!("claude"));
    f.select(proposal("strong-test", "critical", "persistence"));
    assert!(f.publish(plan()).is_ok()); // Legacy provider alone is a preference.
    let f = Fixture::new();
    f.set("automatic_routing", json!(false));
    f.set("implementer", json!("claude"));
    f.select(proposal("strong-test", "critical", "persistence"));
    assert!(f.publish(plan()).unwrap_err().contains("constraint"));
    let f = Fixture::new();
    f.set("reviewer", json!("codex"));
    f.set("automatic_routing", json!(false));
    f.select(proposal("strong-test", "critical", "persistence"));
    assert!(f.publish(plan()).unwrap_err().contains("cross-provider"));
    for c in [
        json!({}),
        json!({"provider":"bad"}),
        json!({"model":"--flag"}),
        json!({"tier":"basic"}),
    ] {
        assert!(validate_constraint(&c).is_err());
    }
}
#[test]
fn disagreement_gets_exactly_one_exchange_and_both_participants_must_agree() {
    for agree in [false, true] {
        let f = Fixture::new();
        f.set(
            "mock_model_evaluations",
            json!([[evaluation(false, "standard", "standard")]]),
        );
        f.set(
            "mock_routing_architect_outputs",
            json!([{"model_evaluations":[evaluation(agree,"standard","standard")]}]),
        );
        let result = f.publish(plan());
        assert_eq!(result.is_ok(), agree);
        assert_eq!(f.counts(), (2, 2));
        if !agree {
            assert!(result.unwrap_err().contains("after one reconciliation"));
            assert!(f.ctx.load_plan().is_none());
        }
    }
    let f = Fixture::new();
    f.set("mock_model_evaluations", json!([[]]));
    assert!(f.publish(plan()).is_err());
    assert_eq!(f.counts(), (1, 1)); // Planner cannot substitute for missing architect.
}
#[test]
fn unchanged_boundaries_restart_refresh_and_unrelated_edits_reuse_only_valid_agreements() {
    let f = Fixture::new();
    let mut p = plan();
    p["stages"] = json!([stage(1), stage(2), stage(3)]);
    p["stages"][2]["depends_on"] = json!([1]);
    let mut p = f.publish(p).unwrap();
    assert_eq!(f.counts(), (1, 1));
    for reason in ["approval", "stage start", "normal fix"] {
        p = f
            .ctx
            .architect_publish(p.clone(), Some(&p), reason)
            .unwrap();
        assert_eq!(f.counts(), (1, 1));
    }
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["policy_revision"] =
        json!("unrelated-refresh");
    p = f
        .ctx
        .architect_publish(p.clone(), Some(&p), "catalogue refresh")
        .unwrap();
    assert_eq!(f.counts(), (1, 1));
    let app = Arc::new(App::new(
        f.root.to_str().unwrap(),
        f.ctx.app.settings.lock().unwrap().clone(),
    ));
    let ctx = app.context(f.root.to_str().unwrap());
    let saved = ctx.load_plan().unwrap();
    assert_eq!(
        ctx.architect_publish(saved.clone(), Some(&saved), "restart")
            .unwrap(),
        saved
    );
    assert_eq!(
        app.settings.lock().unwrap()["mock_routing_planner_requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut incoming = p.clone();
    incoming["stages"][1]["instructions"] = json!("Another independent greeting");
    let edited = crate::plan::edit_plan(&p, &json!({"plan":incoming})).unwrap();
    let changed = f
        .ctx
        .architect_publish(edited, Some(&p), "manual edit")
        .unwrap();
    assert_eq!(f.counts(), (2, 2));
    assert_eq!(
        changed["stages"][0]["model_agreement"],
        p["stages"][0]["model_agreement"]
    );
    assert_eq!(
        changed["stages"][2]["model_agreement"],
        p["stages"][2]["model_agreement"]
    );
    assert_eq!(
        f.ctx.app.settings.lock().unwrap()["mock_routing_planner_requests"][1]["ids"],
        json!([2])
    );
    let mut incoming = changed.clone();
    incoming["stages"][0]["instructions"] = json!("Change shared interface");
    let edited = crate::plan::edit_plan(&changed, &json!({"plan":incoming})).unwrap();
    let changed = f
        .ctx
        .architect_publish(edited, Some(&changed), "dependency edit")
        .unwrap();
    assert_eq!(
        f.ctx.app.settings.lock().unwrap()["mock_routing_planner_requests"][2]["ids"],
        json!([1, 3])
    );
    assert_eq!(f.counts(), (3, 3));
    let history = f
        .ctx
        .architecture_store()
        .history(changed["plan_id"].as_str(), 0, 100)
        .unwrap();
    assert!(
        history
            .to_string()
            .contains(p["stages"][0]["model_agreement"]["id"].as_str().unwrap())
    );
}
#[test]
fn material_changes_block_without_calls_and_constraint_changes_reconcile_pending_only() {
    let f = Fixture::new();
    let p = f.publish(plan()).unwrap();
    let policy = Policy::from_settings(&f.ctx.app.settings.lock().unwrap()).unwrap();
    f.ctx.app.catalogue.observe(
        &policy,
        Provider::Codex,
        "strong-test",
        Err(crate::catalogue::Failure::new(
            crate::catalogue::FailureKind::Rejected,
        )),
    );
    assert!(
        f.ctx
            .architect_publish(p.clone(), Some(&p), "start")
            .unwrap_err()
            .contains("materially invalid")
    );
    assert_eq!(f.counts(), (1, 1));
    assert_eq!(f.ctx.load_plan().unwrap(), p);
    let f = Fixture::new();
    let p = f.publish(plan()).unwrap();
    f.set("implementer_model", json!("strong-test"));
    let p2 = f
        .ctx
        .architect_publish(p.clone(), Some(&p), "changed global constraint")
        .unwrap();
    assert_eq!(f.counts(), (2, 2)); // Changed constraints require both participants.
    assert_ne!(
        p["stages"][0]["model_agreement"]["id"],
        p2["stages"][0]["model_agreement"]["id"]
    );
}
#[test]
fn generation_batches_proposals_revision_reuses_and_failure_is_atomic() {
    for mode in [
        PlanMode::Standard,
        PlanMode::Refactor {
            focus: "Simplify greetings".into(),
        },
    ] {
        let f = Fixture::new();
        let mut candidate = plan();
        candidate["stages"][0]["model_proposal"] =
            proposal("budget-test", "simple", "documentation");
        f.set("mock_plan_output", candidate.clone());
        f.ctx.plan_worker("Improve greetings", &mode);
        let p = f.ctx.load_plan().unwrap();
        assert_eq!(f.counts(), (0, 1));
        assert_eq!(
            p["stages"][0]["model_agreement"]["effective"]["model"],
            "budget-test"
        );
        f.ctx.revise_worker(&p, "Keep the same plan");
        assert_eq!(f.counts(), (0, 1));
        let before = f.ctx.load_plan().unwrap();
        candidate["stages"][0]["instructions"] = json!("New greeting");
        candidate["stages"][0]["model_proposal"]["model"] = json!("hallucinated");
        f.set("mock_plan_output", candidate);
        f.ctx.revise_worker(&before, "Change the greeting");
        assert_eq!(f.ctx.load_plan().unwrap(), before);
        let history_before = f
            .ctx
            .architecture_store()
            .history(before["plan_id"].as_str(), 0, 100)
            .unwrap();
        f.ctx.chat_worker(&before, "What will this do?");
        assert_eq!(f.ctx.load_plan().unwrap(), before);
        assert_eq!(
            f.ctx
                .architecture_store()
                .history(before["plan_id"].as_str(), 0, 100)
                .unwrap(),
            history_before
        );
    }
}
#[test]
fn initial_and_fix_invocations_use_agreement_and_complete_partial_work_handoff() {
    let f = Fixture::new();
    f.mock_execution();
    f.set("mock_routing_options", json!([{"provider":"mock","model":"approved-model","effort":"provider_default","eligible":true,"tier":"strong","provenance":"configured","availability_unverified":true}]));
    f.set("mock_verdicts", json!([{"approved":false,"issues":["Preserve the old greeting"]},{"approved":true,"issues":[]}]));
    f.ctx.plan_worker("Improve greetings", &PlanMode::Standard);
    let p = f.ctx.load_plan().unwrap();
    let a = p["stages"][0]["model_agreement"].clone();
    fs::write(f.root.join("partial.txt"), "Preserve this partial work\n").unwrap();
    f.ctx.run_worker();
    let done = f.ctx.load_plan().unwrap();
    assert_eq!(done["status"], "done");
    assert_eq!(f.counts(), (1, 1));
    assert_eq!(done["stages"][0]["model_agreement"], a);
    let settings = f.ctx.app.settings.lock().unwrap();
    let calls: Vec<_> = settings["mock_agent_requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["role"] == "implementer" || r["role"] == "fixer")
        .collect();
    assert!(calls.iter().any(|r| r["role"] == "fixer"));
    for call in calls {
        assert_eq!(call["model"], "approved-model");
        assert_eq!(call["effort"], "provider_default");
        let prompt = call["prompt"].as_str().unwrap();
        for required in [
            "SAVED ARCHITECTURAL SUMMARY",
            "Relevant decisions",
            "Completed interfaces",
            "WORKTREE",
            "DIFF",
            "OUTSTANDING FINDINGS",
            "Inspect and preserve",
        ] {
            assert!(prompt.contains(required), "{required}");
        }
    }
    assert_eq!(
        fs::read_to_string(f.root.join("partial.txt")).unwrap(),
        "Preserve this partial work\n"
    );
    assert_eq!(
        done["stages"][0]["model_invocations"][0]["requested"]["model"],
        "approved-model"
    );
}

#[test]
fn committed_records_and_user_constraints_survive_ai_revision_and_goal_edits() {
    let f = Fixture::new();
    let mut candidate = plan();
    candidate["stages"] = json!([stage(1), stage(2)]);
    candidate["stages"][1]["model_constraint"] = json!({"model":"strong-test"});
    let mut p = f.publish(candidate).unwrap();
    p["stages"][0]["status"] = json!("committed");
    f.ctx.save_plan(&p).unwrap();
    let p = f.ctx.load_plan().unwrap();
    let committed = p["stages"][0].clone();
    let mut malicious = p.clone();
    malicious["stages"][0]["acceptance"] = json!("Skip checks");
    malicious["stages"][1]["model_constraint"] = json!({"model":"budget-test"});
    malicious["stages"][1]["status"] = json!("committed");
    f.set("mock_plan_output", malicious);
    f.ctx.revise_worker(&p, "Retain the plan");
    let revised = f.ctx.load_plan().unwrap();
    assert_eq!(revised["stages"][0], committed);
    assert_eq!(
        revised["stages"][1]["model_constraint"],
        p["stages"][1]["model_constraint"]
    );
    assert_ne!(revised["stages"][1]["status"], "committed");
    assert_eq!(f.counts(), (1, 1));
    let cp_before = f.ctx.architecture_store().checkpoint(&revised).unwrap();
    let mut incoming = revised.clone();
    incoming["goal"] = json!("New goal");
    let edited = crate::plan::edit_plan(&revised, &json!({"plan":incoming})).unwrap();
    let edited = f
        .ctx
        .architect_publish(edited, Some(&revised), "goal edit")
        .unwrap();
    assert_eq!(edited["stages"][0], committed);
    assert_eq!(
        f.ctx.architecture_store().checkpoint(&edited).unwrap()["agreements"]["1"],
        cp_before["agreements"]["1"]
    );
    assert_eq!(f.counts(), (2, 2));
}

#[test]
fn prelaunch_material_changes_and_unrelated_metadata_are_checked_locally() {
    let f = Fixture::new();
    let p = f.publish(plan()).unwrap();
    let options = f.ctx.routing_options().unwrap();
    let mut changed = options.clone();
    changed[0]["resolved_id"] = json!("retargeted-model");
    f.set("mock_routing_options", json!(changed));
    assert!(f.ctx.validated_assignment(&p, 0).is_err());
    assert_eq!(f.counts(), (1, 1));
    let mut changed = options.clone();
    changed[0]["pricing"] = json!({"input":10,"output":20,"basis":"unrelated"});
    changed[0]["availability"] = json!("discovered");
    changed[0]["availability_unverified"] = json!(false);
    changed[0]["policy_revision"] = json!("refreshed");
    changed[0]["resolved_id"] = json!("strong-test"); // Newly confirmed identical resolution is immaterial.
    f.set("mock_routing_options", json!(changed));
    assert!(f.ctx.validated_assignment(&p, 0).is_ok());
    assert_eq!(f.counts(), (1, 1));
    let mut changed = options.clone();
    changed[0]["tier"] = json!("basic");
    f.set("mock_routing_options", json!(changed));
    assert!(f.ctx.validated_assignment(&p, 0).is_err());
    assert_eq!(f.counts(), (1, 1));
}

#[test]
fn unexpected_provider_substitution_records_effective_model_and_blocks_with_saved_work() {
    let f = Fixture::new();
    f.mock_execution();
    f.set(
        "mock_effective_models",
        json!(["unexpected-provider-model"]),
    );
    f.ctx.plan_worker("Improve greetings", &PlanMode::Standard);
    f.ctx.run_worker();
    let p = f.ctx.load_plan().unwrap();
    let invocation = &p["stages"][0]["model_invocations"][0];
    assert_eq!(invocation["unexpected_substitution"], true);
    assert_eq!(
        invocation["effective"]["model"],
        "unexpected-provider-model"
    );
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "blocked");
    assert!(f.root.join("mock.txt").exists());
    let counts = f.counts();
    f.ctx.run_worker();
    assert_eq!(f.counts(), counts);
    assert_eq!(
        f.ctx.load_plan().unwrap()["stages"][0]["model_invocations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn explanatory_docs_and_unknown_incomparable_prices_work_through_both_roles() {
    let f = Fixture::new();
    let mut p = plan();
    p["stages"][0]["title"] = json!("Document persistent architect defaults");
    p["stages"][0]["instructions"] =
        json!("Update README to explain existing persistence behavior");
    f.select(proposal("budget-test", "simple", "documentation"));
    assert!(f.publish(p).is_ok());
    assert_eq!(f.counts(), (1, 1));
    for pricing in [
        Value::Null,
        json!({"currency":"EUR","unit":"million","basis":"API","input":1,"output":2,"as_of":"today"}),
    ] {
        let f = Fixture::new();
        let mut options = f.ctx.routing_options().unwrap();
        for o in &mut options {
            o["relative_cost_preference"] = Value::Null;
        }
        options[0]["pricing"] = json!({"currency":"USD","unit":"million","basis":"API","input":10,"output":20,"as_of":"today"});
        options[1]["pricing"] = pricing;
        f.set("routing_billing_basis", json!("API"));
        f.set("mock_routing_options", json!(options));
        f.select(proposal("strong-test", "simple", "documentation"));
        let p = f.publish(plan()).unwrap();
        assert_eq!(f.counts(), (1, 1));
        options[0]["pricing"]["as_of"] = json!("tomorrow");
        f.set("mock_routing_options", json!(options));
        assert!(f.ctx.validated_assignment(&p, 0).is_ok());
        assert_eq!(f.counts(), (1, 1));
    }
}

#[test]
fn routing_input_is_bounded_by_the_selected_model_context_not_a_fixed_ceiling() {
    use super::context_budget_error;
    let million = 1_000_000;
    // Four bytes per token: 3.6 MB is ~900k tokens, exactly 90% of a 1M window.
    assert_eq!(context_budget_error(3_600_000, million, 90, "claude", "opus[1m]"), None);
    let over = context_budget_error(3_600_008, million, 90, "claude", "opus[1m]").unwrap();
    assert!(over.contains("roughly 900002 tokens"), "{over}");
    assert!(over.contains("over 90% of claude/opus[1m]'s 1000000-token context"), "{over}");
    // The old fixed 256 KiB ceiling rejected prompts every configured model reads.
    assert_eq!(context_budget_error(512 * 1024, million, 85, "claude", "opus[1m]"), None);
    // A small window still bounds the input, and an unknown one imposes nothing.
    assert!(context_budget_error(512 * 1024, 100_000, 85, "codex", "small").is_some());
    assert_eq!(context_budget_error(64 * 1024 * 1024, 0, 85, "codex", "unknown"), None);
}
