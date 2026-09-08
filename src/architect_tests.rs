use super::*;
use crate::app::App;
use std::path::PathBuf;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-architect-test-{}", identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["planner"] = json!("mock");
        settings["implementer"] = json!("mock");
        settings["reviewer"] = json!("mock");
        settings["auto_push"] = json!(false);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }
    fn plan(&self) -> Value {
        json!({"goal":"preserve contracts","status":"draft","stages":[
        {"id":1,"title":"API","instructions":"define interface","acceptance":"compatible API","commit":"feat: api","status":"pending","depends_on":[]},
        {"id":2,"title":"UI","instructions":"consume interface","acceptance":"renders","commit":"feat: ui","status":"pending","depends_on":[1]}]})
    }
    fn initial(&self) -> Value {
        self.ctx
            .architect_publish(self.plan(), None, "initial")
            .unwrap()
    }
    fn cp(&self, plan: &Value) -> Value {
        self.ctx.architecture_store().checkpoint(plan).unwrap()
    }
    fn requests(&self) -> Vec<Value> {
        self.ctx.app.settings.lock().unwrap()["mock_architect_requests"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
    fn revision(&self, plan: &Value) -> Value {
        let mut candidate = plan.clone();
        candidate["stages"][1]["instructions"] = json!("consume interface with explicit errors");
        crate::plan::edit_plan(plan, &json!({"plan":candidate})).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn architect_keeps_identity_across_revisions_stages_reconstruction_and_completion() {
    let f = Fixture::new();
    let plan = f.initial();
    let session = f.cp(&plan)["session"].clone();
    assert_eq!(f.requests().len(), 1);
    let same = f
        .ctx
        .architect_publish(plan.clone(), Some(&plan), "unchanged start")
        .unwrap();
    assert_eq!(same, plan);
    assert_eq!(f.requests().len(), 1);
    let revised = f
        .ctx
        .architect_publish(f.revision(&plan), Some(&plan), "revision")
        .unwrap();
    assert_eq!(f.cp(&revised)["session"]["reference"], session["reference"]);
    assert_eq!(f.requests()[1]["session"], session["reference"]);
    let mut committed = revised.clone();
    committed["stages"][0]["status"] = json!("committed");
    committed["stages"][0]["sha"] = json!("verified-sha");
    f.ctx.save_plan(&committed).unwrap();
    let committed = f.ctx.load_plan().unwrap();
    let cp = f.cp(&committed);
    assert_eq!(cp["execution_outcomes"][0]["sha"], "verified-sha");
    assert_eq!(cp["session"]["reference"], session["reference"]);
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    let rebuilt = Arc::new(App::new(f.root.to_str().unwrap(), settings));
    let ctx = rebuilt.context(f.root.to_str().unwrap());
    let loaded = ctx.load_plan().unwrap();
    let mut completed = ctx
        .architect_publish(loaded.clone(), Some(&loaded), "next stage after restart")
        .unwrap();
    assert_eq!(
        rebuilt.settings.lock().unwrap()["mock_architect_requests"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    completed["stages"][1]["status"] = json!("committed");
    completed["status"] = json!("done");
    ctx.save_plan(&completed).unwrap();
    let completed = ctx.load_plan().unwrap();
    assert_eq!(
        ctx.architecture_store().checkpoint(&completed).unwrap()["session"]["reference"],
        session["reference"]
    );
    assert_eq!(
        ctx.architecture_store().checkpoint(&completed).unwrap()["execution_outcomes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn missing_session_reconstructs_from_saved_checkpoint_decisions_risks_and_full_plan() {
    let f = Fixture::new();
    let p = f.initial();
    let d = json!({"version":1,"id":"api-choice","plan_id":p["plan_id"],"revision":p["revision"],"stage_id":1,
        "summary":"Use stable identifiers","rationale":"Clients store these identifiers across restarts","alternatives":[],"status":"accepted","supersedes":null,"created_unix":1,"updated_unix":1});
    let p = f
        .ctx
        .architecture_store()
        .record(&p, "decision", d)
        .unwrap();
    let mut cp = f.cp(&p);
    cp["constraints"] = json!(["Never renumber client identifiers"]);
    cp["completed_interfaces"] = json!(["GET /stable returns immutable IDs"]);
    cp["unresolved_risks"] =
        json!([{"id":"migration","text":"Existing client migration remains unresolved"}]);
    let p = f
        .ctx
        .architecture_store()
        .publish(p, cp, json!({"kind":"saved_context"}))
        .unwrap();
    f.ctx.app.settings.lock().unwrap()["mock_architect_errors"] = json!(["session not found"]);
    let revised = f
        .ctx
        .architect_publish(f.revision(&p), Some(&p), "revision")
        .unwrap();
    let requests = f.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["session"], f.cp(&p)["session"]["reference"]);
    assert!(requests[2]["session"].is_null());
    let prompt = requests[2]["prompt"].as_str().unwrap();
    for text in [
        "Never renumber",
        "GET /stable",
        "migration remains unresolved",
        "Clients store",
        "preserve contracts",
        "consume interface with explicit errors",
    ] {
        assert!(prompt.contains(text), "{text}");
    }
    let cp = f.cp(&revised);
    assert_ne!(cp["session"]["reference"], f.cp(&p)["session"]["reference"]);
    assert!(
        cp["recovery"]["reason"]
            .as_str()
            .unwrap()
            .contains("missing/expired")
    );
    assert_eq!(cp["unresolved_risks"][0]["id"], "migration");
}

#[test]
fn malformed_and_stale_revision_output_keep_good_state_and_quarantine_provider_tail() {
    for bad in ["not JSON", "{\"version\":1,\"plan_id\":\"stale\"}"] {
        let f = Fixture::new();
        let p = f.initial();
        let cp = f.cp(&p);
        let bytes = fs::read(f.ctx.forge_path("plan.json")).unwrap();
        f.ctx.app.settings.lock().unwrap()["mock_architect_output"] = json!(bad);
        assert!(
            f.ctx
                .architect_publish(f.revision(&p), Some(&p), "failed revision")
                .is_err()
        );
        assert_eq!(fs::read(f.ctx.forge_path("plan.json")).unwrap(), bytes);
        assert_eq!(f.cp(&p), cp);
        assert_eq!(
            f.ctx.architecture_store().summary(Some(&p)).unwrap()["context_status"],
            "needs_recovery"
        );
        assert_eq!(
            f.ctx.session.state.lock().unwrap().architect_activity["recoverable"],
            true
        );
        f.ctx
            .app
            .settings
            .lock()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("mock_architect_output");
        let recovered = f
            .ctx
            .architect_publish(p.clone(), Some(&p), "retry original plan")
            .unwrap();
        let requests = f.requests();
        let last = requests.last().unwrap();
        assert!(last["session"].is_null());
        assert!(
            !last["prompt"]
                .as_str()
                .unwrap()
                .contains("consume interface with explicit errors")
        );
        assert_ne!(
            f.cp(&recovered)["session"]["reference"],
            cp["session"]["reference"]
        );
        assert_eq!(recovered["revision"], p["revision"]);
        let history = f.ctx.architecture_store().history(None, 0, 100).unwrap();
        assert_eq!(history["items"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn provider_change_recovers_and_project_and_goal_sessions_are_isolated() {
    let a = Fixture::new();
    let b = Fixture::new();
    let pa = a.initial();
    let pb = b.initial();
    assert_ne!(pa["plan_id"], pb["plan_id"]);
    assert_ne!(
        a.cp(&pa)["session"]["reference"],
        b.cp(&pb)["session"]["reference"]
    );
    let mut cp = a.cp(&pa);
    cp["session"]["provider"] = json!("codex");
    cp["effective_model"]["provider"] = json!("codex");
    let pa = a
        .ctx
        .architecture_store()
        .publish(pa, cp, json!({"kind":"provider_fixture"}))
        .unwrap();
    let recovered = a
        .ctx
        .architect_publish(pa.clone(), Some(&pa), "provider changed")
        .unwrap();
    assert_eq!(a.cp(&recovered)["recovery"]["reason"], "provider changed");
    assert!(a.requests().last().unwrap()["session"].is_null());
    let mut next = a.plan();
    next["goal"] = json!("next queued goal");
    let next = a
        .ctx
        .architect_publish(next, None, "next queue goal")
        .unwrap();
    assert_ne!(next["plan_id"], recovered["plan_id"]);
    assert_ne!(
        a.cp(&next)["session"]["reference"],
        a.cp(&recovered)["session"]["reference"]
    );
    assert!(
        !a.requests().last().unwrap()["prompt"]
            .as_str()
            .unwrap()
            .contains("provider_fixture")
    );
    assert_eq!(b.ctx.load_plan().unwrap(), pb);
}

#[test]
fn qa_is_read_only_and_does_not_touch_authoritative_session_or_decisions() {
    let f = Fixture::new();
    let p = f.initial();
    let cp = f.cp(&p);
    let plan_bytes = fs::read(f.ctx.forge_path("plan.json")).unwrap();
    let pending_path = f
        .ctx
        .forge_path("architecture")
        .join(p["plan_id"].as_str().unwrap())
        .join("architect-pending.json");
    let pending = fs::read(&pending_path).unwrap();
    f.ctx.chat_worker(&p, "Why this interface?");
    assert_eq!(fs::read(f.ctx.forge_path("plan.json")).unwrap(), plan_bytes);
    assert_eq!(f.cp(&p), cp);
    assert_eq!(fs::read(pending_path).unwrap(), pending);
    assert_eq!(f.requests().len(), 1);
    assert_eq!(f.ctx.read_chat().as_array().unwrap().len(), 2);
}

#[test]
fn architect_turn_validation_preserves_constraints_and_requires_risk_resolution() {
    let f = Fixture::new();
    let p = f.initial();
    let mut cp = f.cp(&p);
    cp["constraints"] = json!(["unresolved compatibility constraint"]);
    cp["unresolved_risks"] = json!([{"id":"risk-1","text":"unresolved"}]);
    let good = f
        .ctx
        .mock_architect(&p, &cp, &[1], &[], "", None)
        .unwrap()
        .output;
    assert!(apply_turn(&p, &cp, &good, &BTreeMap::new(), &[1]).is_ok());
    let base: Value = serde_json::from_str(&good).unwrap();
    for (field, val) in [
        ("revision", json!(99)),
        ("plan_id", json!("another-plan")),
        ("guidance", json!([])),
        ("unresolved_risks", json!([])),
    ] {
        let mut bad = base.clone();
        bad[field] = val;
        assert!(
            apply_turn(&p, &cp, &bad.to_string(), &BTreeMap::new(), &[1]).is_err(),
            "{field}"
        );
    }
    let mut bad = base.clone();
    bad["checkpoint"]["constraints"] = json!([]);
    assert!(apply_turn(&p, &cp, &bad.to_string(), &BTreeMap::new(), &[1]).is_err());
    let mut bad = base.clone();
    bad["checkpoint"]["summary"] = json!("x".repeat(8001));
    assert!(apply_turn(&p, &cp, &bad.to_string(), &BTreeMap::new(), &[1]).is_err());
    let mut resolved = base;
    resolved["unresolved_risks"] = json!([]);
    resolved["resolved_risks"] = json!(["risk-1"]);
    assert!(apply_turn(&p, &cp, &resolved.to_string(), &BTreeMap::new(), &[1]).is_ok());
}

#[test]
fn strong_bootstraps_are_separate_configured_and_availability_unverified() {
    let f = Fixture::new();
    {
        let mut s = f.ctx.app.settings.lock().unwrap();
        s["planner"] = json!("claude");
        s["architect"] = json!("codex");
        s["model_catalogue"]["entries"] = json!([
        {"provider":"claude","model":"planner-strong","tier":"strong"},
        {"provider":"codex","model":"architect-strong","tier":"strong"},
        {"provider":"codex","model":"cheap","tier":"basic"}]);
    }
    assert_eq!(
        f.ctx.bootstrap("planner").unwrap(),
        (
            "claude".into(),
            "planner-strong".into(),
            "provider_default".into()
        )
    );
    assert_eq!(f.ctx.bootstrap("architect").unwrap().1, "architect-strong");
    f.ctx.app.settings.lock().unwrap()["architect_model"] = json!("cheap");
    assert!(f.ctx.bootstrap("architect").is_err());
    f.ctx.app.settings.lock().unwrap()["architect_model"] = json!("architect-strong");
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"][1]["effort"] = json!("high");
    assert_eq!(f.ctx.bootstrap("architect").unwrap().2, "high"); // Explicit configured effort supported by the adapter.
}

#[test]
fn concurrent_turns_are_serialized_and_stale_work_does_not_spend_another_turn() {
    let f = Fixture::new();
    let p = f.initial();
    let candidate = f.revision(&p);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            f.ctx
                .architect_publish(candidate.clone(), Some(&p), "concurrent revision")
        });
        let b = scope.spawn(|| {
            f.ctx
                .architect_publish(candidate.clone(), Some(&p), "concurrent revision")
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_ne!(results.0.is_ok(), results.1.is_ok());
    assert_eq!(f.requests().len(), 2);
}

#[test]
fn recovery_prompts_keep_full_design_without_repeating_review_transcripts() {
    let f = Fixture::new();
    let mut p = f.initial();
    let cp = f.cp(&p);
    p["stages"][0]["reviews"] = json!([{"summary":"historical-review-body ".repeat(10000)}]);
    f.ctx
        .architecture_store()
        .publish(p, cp, json!({"kind":"review_history_fixture"}))
        .unwrap();
    let p = f.ctx.load_plan().unwrap();
    let pending = f
        .ctx
        .forge_path("architecture")
        .join(p["plan_id"].as_str().unwrap())
        .join("architect-pending.json");
    fs::write(pending, b"interrupted marker").unwrap();
    let recovered = f
        .ctx
        .architect_publish(p.clone(), Some(&p), "restart")
        .unwrap();
    let requests = f.requests();
    let prompt = requests.last().unwrap()["prompt"].as_str().unwrap();
    assert!(!prompt.contains("historical-review-body"));
    assert!(prompt.contains("define interface"));
    assert!(prompt.contains("compatible API"));
    assert!(prompt.len() < 20000);
    assert_eq!(recovered["stages"][0]["reviews"], p["stages"][0]["reviews"]);
}
