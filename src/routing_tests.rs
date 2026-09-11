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
        let mut s = crate::plan::default_settings();
        s["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        Self::with_settings(s)
    }
    fn with_settings(mut s: Value) -> Self {
        let root = std::env::temp_dir().join(format!("forge-routing-test-{}", identity()));
        fs::create_dir_all(&root).unwrap();
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
    fn with_standard_model() -> Self {
        let f = Self::new();
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"]
            .as_array_mut().unwrap().push(json!({
                "provider":"codex","model":"standard-test","tier":"standard",
                "relative_cost_preference":2
            }));
        f
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
fn default_cadence_defers_reviewer_and_reuses_all_stage_agreements() {
    let f = Fixture::with_settings(crate::plan::default_settings());
    f.set("reviewer", json!("claude"));
    let mut candidate = plan();
    candidate["stages"].as_array_mut().unwrap().push(stage(2));
    // Keep implementer constraints stable when automatic routing is disabled,
    // so only the changed reviewer selector could invalidate these agreements.
    for stage in candidate["stages"].as_array_mut().unwrap() {
        stage["model_constraint"] = json!({"provider":"codex"});
    }
    let p = f.publish(candidate).unwrap();
    let cp = f.ctx.architecture_store().checkpoint(&p).unwrap();
    let counts = f.counts();
    assert_eq!(counts, (1, 1));
    for idx in 0..2 {
        let agreement = &p["stages"][idx]["model_agreement"];
        assert_eq!(agreement["reviewer"], json!({"status":"deferred"}));
        assert_eq!(cp["agreements"][(idx + 1).to_string()], *agreement);
    }

    f.set("reviewer", json!("codex"));
    for (reviewer_model, automatic_routing) in [("", true), ("other-test", true), ("", false)] {
        f.set("reviewer_model", json!(reviewer_model));
        f.set("automatic_routing", json!(automatic_routing));
        assert_eq!(f.ctx.routing_required(&p, &cp).unwrap(), Vec::<i64>::new(),
            "reviewer_model={reviewer_model:?}, automatic_routing={automatic_routing}");
        for idx in 0..2 {
            assert_eq!(f.ctx.validated_assignment(&p, idx).unwrap(), p["stages"][idx]["model_agreement"]);
        }
        assert_eq!(f.counts(), counts);
    }
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
fn standard_documentation_prefers_cheaper_adequate_and_reuses_agreement() {
    let f = Fixture::with_standard_model();
    let mut candidate = plan();
    candidate["stages"][0]["instructions"] = json!("Document greeting usage in README");
    f.select(proposal("strong-test", "standard", "documentation"));
    assert!(f.publish(candidate.clone()).unwrap_err().contains("cheaper"));
    assert!(f.ctx.load_plan().is_none());

    f.select(proposal("standard-test", "standard", "documentation"));
    let published = f.publish(candidate).unwrap();
    let agreement = &published["stages"][0]["model_agreement"];
    assert_eq!(agreement["effective"]["model"], "standard-test");
    assert_eq!(agreement["policy_inputs"]["minimum_tier"], 2);
    assert_eq!(agreement["policy_inputs"]["relative_cost_preference"], 2);
    let checkpoint = f.ctx.architecture_store().checkpoint(&published).unwrap();
    let counts = f.counts();
    assert!(f.ctx.routing_required(&published, &checkpoint).unwrap().is_empty());
    assert_eq!(f.ctx.validated_assignment(&published, 0).unwrap(), *agreement);
    assert_eq!(f.counts(), counts);
}

#[test]
fn standard_functionality_still_accepts_strong() {
    let f = Fixture::with_standard_model();
    f.select(proposal("strong-test", "standard", "functionality"));
    let published = f.publish(plan()).unwrap();
    let inputs = &published["stages"][0]["model_agreement"]["policy_inputs"];
    assert_eq!(inputs["minimum_tier"], 2);
    assert_eq!(inputs["model"], "strong-test");
    for key in ["relative_cost_preference", "pricing", "billing_basis"] {
        assert!(inputs[key].is_null(), "{key}");
    }
}

#[test]
fn sensitive_or_critical_documentation_keeps_strong_floor() {
    for (risk, complexity, instructions) in [
        ("standard", "standard", "Document normative security policy requirements"),
        ("critical", "standard", "Document greeting usage in README"),
        ("standard", "complex", "Document greeting usage in README"),
    ] {
        let f = Fixture::with_standard_model();
        let mut candidate = plan();
        candidate["stages"][0]["instructions"] = json!(instructions);
        let mut choice = proposal("standard-test", risk, "documentation");
        choice["complexity"] = json!(complexity);
        f.select(choice.clone());
        let error = f.publish(candidate.clone()).unwrap_err();
        assert!(error.contains("at least the configured strong capability tier"), "{error}");
        assert!(error.contains("codex/standard-test is configured standard"), "{error}");
        choice["model"] = json!("strong-test");
        f.select(choice);
        let published = f.publish(candidate).unwrap();
        let inputs = &published["stages"][0]["model_agreement"]["policy_inputs"];
        assert_eq!(inputs["minimum_tier"], 3);
        assert_eq!(inputs["relative_cost_preference"], 5);
    }
}

#[test]
fn inadequacy_names_required_floor_and_configured_tier() {
    let f = Fixture::new();
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    for (risk, floor) in [("simple", "basic"), ("standard", "standard"), ("critical", "strong")] {
        let p: Proposal = serde_json::from_value(proposal("budget-test", risk, "documentation")).unwrap();
        for tier in [Value::Null, json!("basic")] {
            if risk == "simple" && tier == "basic" { continue; }
            let option = json!({"provider":"codex","model":"budget-test","effort":"provider_default","eligible":true,"tier":tier});
            let error = policy_inputs(&settings, &stage(1), &p, &[option]).unwrap_err();
            assert!(error.contains(&format!("at least the configured {floor} capability tier")), "{error}");
            let configured = tier.as_str().unwrap_or("unclassified");
            assert!(error.contains(&format!("codex/budget-test is configured {configured}")), "{error}");
        }
    }
}

#[test]
fn unknown_and_incomparable_prices_do_not_create_an_order() {
    for risk in ["simple", "standard"] {
        let f = Fixture::with_standard_model();
        let mut settings = f.ctx.app.settings.lock().unwrap().clone();
        let p: Proposal =
            serde_json::from_value(proposal("strong-test", risk, "documentation")).unwrap();
        let mut options = f.ctx.routing_options().unwrap();
        // Keep both priced options adequate even for standard documentation.
        options.retain(|o| o["model"] == "strong-test" || o["model"] == "standard-test");
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
        let cheaper_proposal: Proposal =
            serde_json::from_value(proposal("standard-test", risk, "documentation")).unwrap();
        let inputs = policy_inputs(&settings, &stage(1), &cheaper_proposal, &options).unwrap();
        assert_eq!(inputs["pricing"], billing_facts(&options[1]["pricing"]));
        assert_eq!(inputs["billing_basis"], "API");
        assert!(inputs["relative_cost_preference"].is_null());
        for (key, value) in [("unit", "thousand tokens"), ("basis", "subscription")] {
            let mut incomparable = options.clone();
            incomparable[1]["pricing"][key] = json!(value);
            assert!(policy_inputs(&settings, &stage(1), &p, &incomparable).is_ok());
        }
        options[1]["pricing"]["output"] = json!(30); // Different input/output tradeoff is incomparable.
        assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok());
        options[1]["pricing"]["output"] = json!(2);
        settings["routing_billing_basis"] = Value::Null;
        assert!(policy_inputs(&settings, &stage(1), &p, &options).is_ok()); // API price is not subscription billing.
    }
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
        f.set("mock_routing_planner_outputs", json!(vec![json!({"proposals":[{"stage_id":1,"proposal":p}]}); 4]));
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
    f.set("mock_model_evaluations", json!([[],[],[],[]]));
    assert!(f.publish(plan()).is_err());
    assert_eq!(f.counts(), (1, 4)); // Only the architect corrects its missing evaluations.
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

#[test]
fn malformed_routing_proposal_is_returned_to_planner_for_correction() {
    let f = Fixture::new();
    let valid = proposal("strong-test", "standard", "functionality");
    let mut malformed = valid.clone();
    malformed["native_effort_note"] = json!("A note belongs in rationale");
    f.set("mock_routing_planner_outputs", json!([
        {"proposals":[{"stage_id":1,"proposal":malformed}]},
        {"proposals":[{"stage_id":1,"proposal":valid}]},
    ]));
    let p = f.publish(plan()).unwrap();
    assert_eq!(f.counts(), (2, 1));
    assert_eq!(p["stages"][0]["model_agreement"]["validated_proposal"], valid);
    let settings = f.ctx.app.settings.lock().unwrap();
    let correction = settings["mock_routing_planner_requests"][1]["prompt"].as_str().unwrap();
    assert!(correction.contains("unknown field `native_effort_note`"));
    assert!(correction.contains("Put explanatory notes in rationale"));
}

#[test]
fn invalid_routing_batch_is_bounded_and_never_partially_applied() {
    let f = Fixture::new();
    let valid = proposal("strong-test", "standard", "functionality");
    let mut malformed = valid.clone();
    malformed["native_effort_note"] = json!("unexpected field");
    let response = json!({"proposals":[
        {"stage_id":1,"proposal":valid}, {"stage_id":2,"proposal":malformed},
    ]});
    f.set("mock_routing_planner_outputs", json!(vec![response; 4]));
    let mut p = plan();
    p["stages"].as_array_mut().unwrap().push(stage(2));
    let before = p.clone();
    let error = f.ctx.propose_routing(&mut p, &[1, 2], &Value::Null).unwrap_err();
    assert!(error.contains("unknown field `native_effort_note`"), "{error}");
    assert_eq!(f.counts(), (4, 0));
    assert_eq!(p, before);
}

#[test]
fn malformed_architect_evaluations_return_to_architect_without_repeating_planner() {
    let f = Fixture::new();
    f.set("mock_model_evaluations", json!([[], [evaluation(true, "standard", "standard")]]));
    assert!(f.publish(plan()).is_ok());
    assert_eq!(f.counts(), (1, 2));
}

#[test]
fn invalid_routing_json_and_then_invalid_schema_share_one_correction_budget() {
    let f = Fixture::new();
    f.set("mock_routing_planner_outputs", json!(["{broken", {"proposals":[]}, "{also broken", {"proposals":[]}]));
    let mut p = plan();
    let before = p.clone();
    assert!(f.ctx.propose_routing(&mut p, &[1], &Value::Null).is_err());
    assert_eq!(f.counts(), (4, 0));
    assert_eq!(p, before);
}

#[test]
fn plan_fixer_uses_maximum_agreed_stage_floor_and_preserves_adequate_pins() {
    for (risks, pin, saved_floor, expected) in [
        (vec!["simple"], "", None, "budget-test"),
        (vec!["standard"], "", None, "standard-test"),
        (vec!["critical"], "", None, "strong-test"),
        (vec!["simple", "critical"], "", None, "strong-test"),
        (vec!["simple"], "standard-test", None, "standard-test"),
        (vec!["standard"], "standard-test", None, "standard-test"),
        (vec!["critical"], "budget-test", None, ""),
        (vec!["simple"], "", Some(0), "budget-test"),
        (vec!["simple"], "", Some(4), ""),
    ] {
        let f = Fixture::with_standard_model();
        f.set("test_fake_providers", json!(true));
        f.set(
            "review_cadence",
            json!({"architect":"per_plan","reviewer":"per_plan"}),
        );
        f.set("test_plan_crash_action", json!("fix_pending"));
        f.set(
            "mock_verdicts",
            json!([{"approved":false,"issues":["Correct greeting"]},{"approved":true,"issues":[]}]),
        );
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][3]["effort"] =
            json!("high");
        let mut candidate = plan();
        candidate["stages"] = json!(
            risks
                .iter()
                .enumerate()
                .map(|(i, risk)| {
                    let mut s = stage(i as i64 + 1);
                    let model = match *risk {
                        "simple" => "budget-test",
                        "standard" => "standard-test",
                        _ => "strong-test",
                    };
                    s["model_proposal"] = proposal(model, risk, "functionality");
                    s
                })
                .collect::<Vec<_>>()
        );
        f.set(
            "mock_routing_planner_outputs",
            json!([{"proposals":candidate["stages"].as_array().unwrap().iter().map(|s|
            json!({"stage_id":s["id"],"proposal":s["model_proposal"]})).collect::<Vec<_>>()}]),
        );
        f.ctx.publish_plan(&candidate, true).unwrap();
        f.ctx.run_worker();
        let mut p = f.ctx.load_plan().unwrap();
        assert!(
            p["stages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["status"] == "committed"),
            "{p}"
        );
        assert_eq!(p["plan_review"]["next_action"], "fix_pending", "{p}");
        let floor = match risks
            .iter()
            .max_by_key(|risk| match **risk {
                "simple" => 1,
                "standard" => 2,
                _ => 3,
            })
            .unwrap()
        {
            &"simple" => 1,
            &"standard" => 2,
            _ => 3,
        };
        assert_eq!(p["plan_review"]["minimum_tier"], floor);
        if let Some(saved) = saved_floor {
            p["plan_review"]["minimum_tier"] = json!(saved);
            f.ctx.save_plan(&p).unwrap();
        }
        f.set("implementer_model", json!(pin));
        f.set("test_plan_crash_action", Value::Null);
        let result = f.ctx.run_plan_review(&mut p).unwrap();
        let calls = p["plan_review"]["model_invocations"].as_array().unwrap();
        let requests = f.ctx.app.settings.lock().unwrap()["mock_agent_requests"].clone();
        let launches: Vec<_> = requests
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["role"] == "fixer")
            .collect();
        assert_eq!(launches.len(), calls.len());
        if expected.is_empty() {
            assert!(!result);
            assert!(calls.is_empty(), "{p}");
            let error = p["plan_review"]["gate"]["error"].as_str().unwrap();
            if pin.is_empty() {
                assert!(
                    error.contains("unknown agreed plan capability requirement"),
                    "{error}"
                );
            } else {
                for detail in [
                    "pinned implementer model codex/budget-test",
                    "configured tier basic",
                    "requires at least strong",
                ] {
                    assert!(error.contains(detail), "{error}");
                }
            }
        } else {
            assert!(result, "{p}");
            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0]["requested"],
                json!({"provider":"codex","model":expected,
                "native_effort":if expected == "standard-test" { "high" } else { "provider_default" }})
            );
            assert_eq!(calls[0]["effective"], calls[0]["requested"]);
            assert_eq!(calls[0]["verification_state"], "execution_verified");
        }
    }
}

#[test]
fn global_pin_preserves_configured_effort_through_publication_and_revalidation() {
    for effort in ["high", "provider_default"] {
        let f = Fixture::new();
        f.set("implementer_model", json!("strong-test"));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] =
            json!(effort);
        let effective = json!({"provider":"codex","model":"strong-test","native_effort":effort});
        assert_eq!(f.ctx.proposal_inputs(&plan(), 0)["constraint"], effective);
        let mut choice = proposal("strong-test", "standard", "functionality");
        if effort == "high" {
            f.select(choice.clone());
            let error = f.publish(plan()).unwrap_err();
            for detail in [
                "constraint",
                "codex",
                "strong-test",
                "native_effort",
                "high",
                "provider_default",
            ] {
                assert!(error.contains(detail), "{error}");
            }
            assert!(f.ctx.load_plan().is_none());
        }
        choice["native_effort"] = json!(effort);
        f.select(choice);
        let published = f.publish(plan()).unwrap();
        let agreement = &published["stages"][0]["model_agreement"];
        assert_eq!(agreement["policy_inputs"]["constraint"], effective);
        assert_eq!(agreement["effective"], effective);
        let cp = f.ctx.architecture_store().checkpoint(&published).unwrap();
        let counts = f.counts();
        assert!(f.ctx.routing_required(&published, &cp).unwrap().is_empty());
        assert_eq!(
            f.ctx.validated_assignment(&published, 0).unwrap(),
            *agreement
        );
        assert_eq!(f.counts(), counts);

        // A registry effort edit invalidates the saved assignment before launch.
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] =
            json!(if effort == "high" {
                "provider_default"
            } else {
                "high"
            });
        assert_eq!(f.ctx.routing_required(&published, &cp).unwrap(), vec![1]);
        assert!(f.ctx.validated_assignment(&published, 0).is_err());
        assert_eq!(f.counts(), counts);
    }
}

#[test]
fn global_pin_effort_is_provider_specific_and_omitted_effort_remains_unconstrained() {
    let f = Fixture::new();
    f.set("implementer_model", json!("strong-test"));
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({
                "provider":"claude","model":"strong-test","tier":"strong","effort":"high"
            }),
        );
    f.select(proposal("strong-test", "standard", "functionality"));
    let published = f.publish(plan()).unwrap();
    let constraint = &published["stages"][0]["model_agreement"]["policy_inputs"]["constraint"];
    assert_eq!(
        *constraint,
        json!({"provider":"codex","model":"strong-test"})
    );
    assert_eq!(
        f.ctx.validated_assignment(&published, 0).unwrap()["effective"]["native_effort"],
        "provider_default"
    );
}

// Exercise the same typed normalization and disk persistence used by settings
// writes and startup, without touching the installed application policy.
fn policy_representations(f: &Fixture) -> Vec<Value> {
    let raw = f.ctx.app.settings.lock().unwrap()["model_catalogue"].clone();
    let policy = crate::catalogue::Policy::from_settings(&json!({"model_catalogue":raw})).unwrap();
    let normalized = json!(policy);
    let path = f.root.join("model-policy.json");
    crate::catalogue::save_policy(&path, &policy).unwrap();
    let reloaded = crate::catalogue::load_policy(&path).unwrap().unwrap();
    assert_eq!(reloaded, policy);
    vec![raw, normalized, json!(reloaded)]
}

#[test]
fn global_pin_agreement_survives_policy_normalization_and_save_reload() {
    for effort in [None, Some("provider_default"), Some("high")] {
        let f = Fixture::new();
        f.set("implementer_model", json!("strong-test"));
        if let Some(effort) = effort {
            f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] = json!(effort);
        }
        // Discovered support permits a non-default proposal even when registry
        // effort is omitted. This is the restart-invalidated agreement regression.
        let mut options = f.ctx.routing_options().unwrap();
        let mut high = options[0].clone();
        high["effort"] = json!("high");
        options.push(high);
        f.set("mock_routing_options", json!(options));
        let mut choice = proposal("strong-test", "standard", "functionality");
        choice["native_effort"] = json!(effort.unwrap_or("high"));
        f.select(choice);
        let published = f.publish(plan()).unwrap();
        let agreement = &published["stages"][0]["model_agreement"];
        let cp = f.ctx.architecture_store().checkpoint(&published).unwrap();
        let counts = f.counts();
        let expected = f.ctx.proposal_inputs(&published, 0)["constraint"].clone();
        assert_eq!(expected.get("native_effort"), effort.map(|e| json!(e)).as_ref());
        for policy in policy_representations(&f) {
            assert_eq!(policy["entries"][0].get("effort"), effort.map(|e| json!(e)).as_ref());
            f.set("model_catalogue", policy);
            assert_eq!(f.ctx.proposal_inputs(&published, 0)["constraint"], expected);
            assert!(f.ctx.routing_required(&published, &cp).unwrap().is_empty());
            assert_eq!(f.ctx.validated_assignment(&published, 0).unwrap(), *agreement);
            assert_eq!(f.counts(), counts);
        }
    }
}

#[test]
fn global_pin_reassessment_preserves_omitted_and_explicit_effort_semantics() {
    for effort in [None, Some("low"), Some("provider_default")] {
        for representation in 0..3 {
            let f = Fixture::new();
            f.set("implementer_model", json!("strong-test"));
            if let Some(effort) = effort {
                f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] = json!(effort);
            }
            let mut options = f.ctx.routing_options().unwrap();
            for effort in ["low", "high"] {
                let mut option = options[0].clone();
                option["effort"] = json!(effort);
                options.push(option);
            }
            f.set("mock_routing_options", json!(options));
            let mut choice = proposal("strong-test", "standard", "functionality");
            choice["native_effort"] = json!(effort.unwrap_or("low"));
            f.select(choice.clone());
            let mut published = f.publish(plan()).unwrap();
            let original = published["stages"][0]["model_agreement"].clone();
            f.set("model_catalogue", policy_representations(&f)[representation].clone());
            choice["native_effort"] = json!("high");
            published["stages"][0]["reassessment"] = json!({
                "visited":[original["effective"]],
                "pending":{"kind":"implementer_escalation","old_agreement":original}
            });
            let settings = f.ctx.app.settings.lock().unwrap().clone();
            let options = f.ctx.routing_options().unwrap();
            let choice: Proposal = serde_json::from_value(choice).unwrap();
            let result = policy_inputs(&settings, &published["stages"][0], &choice, &options);
            if let Some(effort) = effort {
                let error = result.unwrap_err();
                for detail in ["constraint", "strong-test", effort, "high"] {
                    assert!(error.contains(detail), "{error}");
                }
            } else {
                let inputs = result.unwrap();
                assert_eq!(inputs["effort"], "high");
                assert_eq!(inputs["constraint"], original["policy_inputs"]["constraint"]);
                assert_eq!(inputs["minimum_tier"], original["policy_inputs"]["minimum_tier"]);
                // A model pin still forbids switching models during escalation.
                let mut switched = choice.clone();
                switched.model = "budget-test".into();
                switched.native_effort = "provider_default".into();
                assert!(policy_inputs(&settings, &published["stages"][0], &switched, &options)
                    .unwrap_err().contains("constraint"));
            }
        }
    }
}

#[test]
fn stage_constraints_replace_global_pin_including_registry_effort() {
    for constraint in [
        json!({"provider":"codex"}),
        json!({"model":"budget-test"}),
        json!({"native_effort":"provider_default"}),
        json!({"provider":"codex","model":"budget-test","native_effort":"provider_default"}),
    ] {
        let f = Fixture::new();
        f.set("implementer_model", json!("strong-test"));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] =
            json!("high");
        let mut candidate = plan();
        candidate["stages"][0]["model_constraint"] = constraint.clone();
        f.select(proposal("budget-test", "simple", "functionality"));
        let published = f.publish(candidate).unwrap();
        let agreement = f.ctx.validated_assignment(&published, 0).unwrap();
        assert_eq!(agreement["policy_inputs"]["constraint"], constraint);
        assert_eq!(
            agreement["effective"],
            json!({"provider":"codex","model":"budget-test","native_effort":"provider_default"})
        );
    }
}

#[test]
fn global_pin_cannot_replace_ineligible_effort_with_provider_default() {
    let f = Fixture::new();
    f.set("implementer_model", json!("strong-test"));
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] =
        json!("unsupported");
    let mut choice = proposal("strong-test", "standard", "functionality");
    choice["native_effort"] = json!("unsupported");
    f.select(choice);
    let error = f.publish(plan()).unwrap_err();
    assert!(
        error.contains("unsupported or unknown native effort"),
        "{error}"
    );
    f.select(proposal("strong-test", "standard", "functionality"));
    let error = f.publish(plan()).unwrap_err();
    for detail in [
        "constraint",
        "strong-test",
        "unsupported",
        "provider_default",
    ] {
        assert!(error.contains(detail), "{error}");
    }
    assert!(f.ctx.load_plan().is_none());
}

#[test]
fn stage_implementer_and_fixer_launch_exact_effective_identity_and_effort() {
    for stage_override in [false, true] {
        let f = Fixture::new();
        f.set("test_fake_providers", json!(true));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][0]["effort"] =
            json!("high");
        f.set(
            "mock_verdicts",
            json!([{"approved":false,"issues":["Correct greeting"]},{"approved":true,"issues":[]}]),
        );
        let mut p = plan();
        f.set("implementer_model", json!("strong-test"));
        let effort = if stage_override {
            "provider_default"
        } else {
            "high"
        };
        let effective = json!({"provider":"codex","model":"strong-test","native_effort":effort});
        if stage_override {
            p["stages"][0]["model_constraint"] = json!({"native_effort":effort});
        }
        let mut selected = proposal("strong-test", "standard", "functionality");
        selected["native_effort"] = json!(effort);
        f.select(selected.clone());
        p["stages"][0]["model_proposal"] = selected;
        f.ctx.publish_plan(&p, true).unwrap();
        f.ctx.run_worker();
        let done = f.ctx.load_plan().unwrap();
        assert_eq!(done["status"], "done", "{done}");
        assert_eq!(done["stages"][0]["model_agreement"]["effective"], effective);
        let calls = done["stages"][0]["model_invocations"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        for (call, role) in calls.iter().zip(["implementer", "fixer"]) {
            assert_eq!(call["role"], role);
            assert_eq!(call["requested"], effective);
            assert_eq!(call["effective"], effective);
            assert_eq!(call["unexpected_substitution"], false);
        }
        let settings = f.ctx.app.settings.lock().unwrap();
        let requests: Vec<_> = settings["mock_agent_requests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| matches!(r["role"].as_str(), Some("implementer" | "fixer")))
            .collect();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request["provider"], effective["provider"]);
            assert_eq!(request["model"], effective["model"]);
            assert_eq!(request["effort"], effective["native_effort"]);
        }
    }
}
