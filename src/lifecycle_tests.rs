//! Offline lifecycle fixtures: every provider and official HTTP boundary is injected.
use crate::app::{App, Ctx};
use crate::catalogue::{Catalogue, Discovery, Failure, FailureKind, Policy, Probe, Provider};
use crate::catalogue_process::Budget;
use crate::metadata::{Clock, Fetch, FetchRequest, FetchResponse, Scheduler, Service};
use crate::test_support::api_request;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicI64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

struct DiscoveryFixture(AtomicUsize);
impl Discovery for DiscoveryFixture {
    fn probe(&self, _: Provider, _: &Policy, _: Budget) -> Probe {
        self.0.fetch_add(1, Ordering::SeqCst);
        Probe { cli_version: Some("fixture-1".into()), result: Err(Failure::new(FailureKind::Unsupported)) }
    }
}
struct Time(AtomicI64);
impl Clock for Time { fn now_unix(&self) -> i64 { self.0.load(Ordering::SeqCst) } }
#[derive(Default)]
struct OfficialFixture(Mutex<Vec<FetchRequest>>);
impl Fetch for OfficialFixture {
    fn fetch(&self, r: &FetchRequest) -> Result<FetchResponse, String> {
        assert!(matches!(r.url.as_str(), "https://learn.chatgpt.com/docs/models.md" | "https://platform.claude.com/docs/en/models/overview.md"));
        let mut calls = self.0.lock().unwrap();
        let revalidate = calls.iter().any(|old| old.url == r.url);
        if revalidate { assert!(r.etag.is_some()); }
        calls.push(r.clone());
        let ids = if r.url.contains("chatgpt") { vec!["small", "large"] } else { vec!["other"] };
        Ok(FetchResponse { status: if revalidate {304} else {200}, location: None,
            etag: Some("fixture-v1".into()), last_modified: None,
            body: if revalidate { vec![] } else { json!({"models":ids.iter().map(|id| json!({
                "id":id, "context_window":100000, "max_output_tokens":10000,
                "reasoning":true, "lifecycle":"available"
            })).collect::<Vec<_>>()}).to_string().into_bytes() } })
    }
}
struct Fixture {
    root: PathBuf, ctx: Ctx, discovery: Arc<DiscoveryFixture>, fetch: Arc<OfficialFixture>, time: Arc<Time>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-lifecycle-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        settings["planner"] = json!("mock"); settings["architect"] = json!("mock");
        settings["test_fake_providers"] = json!(true); settings["auto_push"] = json!(false);
        settings["mock_usage"] = json!({"input":10,"output":5,"total":15});
        settings["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"small","tier":"standard","relative_cost_preference":1},
            {"provider":"codex","model":"large","tier":"strong","relative_cost_preference":5},
            {"provider":"claude","model":"other","tier":"strong","relative_cost_preference":5}]);
        let discovery = Arc::new(DiscoveryFixture(AtomicUsize::new(0)));
        let fetch = Arc::new(OfficialFixture::default());
        let time = Arc::new(Time(AtomicI64::new(1_000_000)));
        let app = Self::app(&root, settings, &discovery, &fetch, &time);
        let ctx = app.context(root.to_str().unwrap());
        for args in [vec!["init","-q"], vec!["config","user.name","Fixture"],
            vec!["config","user.email","fixture@example.invalid"], vec!["config","commit.gpgsign","false"]] {
            ctx.git(&args).unwrap();
        }
        fs::write(root.join("README.md"), "The program prints a greeting.\n").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        ctx.git(&["add","README.md","Cargo.toml","src/main.rs"]).unwrap();
        ctx.git(&["commit","-qm","initial"]).unwrap(); ctx.ensure_forge_dir();
        Self { root, ctx, discovery, fetch, time }
    }
    fn app(root: &std::path::Path, settings: Value, discovery: &Arc<DiscoveryFixture>, fetch: &Arc<OfficialFixture>, time: &Arc<Time>) -> Arc<App> {
        let mut app = App::new(root.to_str().unwrap(), settings);
        app.catalogue = Catalogue::new(discovery.clone(), root.join(".forge/test-cache"), Duration::from_secs(1));
        app.metadata = Service::new(fetch.clone(), time.clone(), root.join(".forge/test-cache/metadata.json"));
        app.scheduler = Scheduler::new(time.now_unix());
        Arc::new(app)
    }
    fn restart(&mut self) {
        let settings = self.ctx.app.settings.lock().unwrap().clone();
        let app = Self::app(&self.root, settings, &self.discovery, &self.fetch, &self.time);
        self.ctx = app.context(self.root.to_str().unwrap());
    }
    fn set(&self, key: &str, value: Value) { self.ctx.app.settings.lock().unwrap()[key] = value; }
    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.ctx.app.catalogue.running() || self.ctx.app.metadata.running() {
            assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn calls(&self) -> (usize, usize) {
        let s = self.ctx.app.settings.lock().unwrap();
        (s["mock_routing_planner_requests"].as_array().map_or(0, Vec::len),
            s["mock_routing_architect_requests"].as_array().map_or(0, Vec::len))
    }
    fn proposal(id: i64, model: &str, risk: &str, task: &str) -> Value {
        json!({"stage_id":id,"proposal":{"provider":"codex","model":model,"native_effort":"provider_default",
            "risk":risk,"complexity":if risk == "critical" {"complex"} else {risk},"task":task,
            "rationale":"Configured adequacy meets the stage risk; prefer lower relative cost when sufficient."}})
    }
    fn stage(id: i64, intent: &str) -> Value {
        json!({"id":id,"title":intent,"instructions":intent,"acceptance":"Greeting works.\nExisting checks pass.",
            "commit":"feat: greeting","depends_on":[],"status":"pending","rounds":0})
    }
    fn verdict(approved: bool) -> Value {
        json!({"approved":approved,"issues":if approved {vec![]} else {vec!["Greeting test still fails"]},
            "project_checks":[{"command":"cargo build","status":"passed","evidence":"Fixture build success"},
                {"command":"cargo test","status":"passed","evidence":"Fixture tests success"}]})
    }
    fn state(&self) -> Value {
        let (code, state) = api_request(&self.ctx.app, "GET", "/api/state", json!({}));
        assert_eq!(code,200); state
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

#[test]
fn offline_lifecycle_refresh_reuse_escalation_restart_reports_and_next_queue_plan() {
    let mut f = Fixture::new();
    // Startup uses unsupported discovery plus explicit unverified configuration.
    assert!(f.ctx.app.refresh_catalogue()); f.wait();
    assert_eq!(f.discovery.0.load(Ordering::SeqCst), 2);
    // Bootstrap strength is configured independently of metadata/availability.
    f.set("planner", json!("claude")); f.set("architect", json!("codex"));
    assert_eq!(f.ctx.bootstrap("planner").unwrap().1, "other");
    assert_eq!(f.ctx.bootstrap("architect").unwrap().1, "large");
    f.set("planner", json!("mock")); f.set("architect", json!("mock"));
    f.ctx.app.scheduler_tick(f.time.now_unix()); f.wait();
    assert_eq!(f.fetch.0.lock().unwrap().len(), 2); // One document per provider, three models.
    let policy = Policy::from_settings(&f.ctx.app.settings.lock().unwrap()).unwrap();
    let details = f.ctx.app.metadata.details(&f.ctx.app.catalogue.metadata_snapshot(&policy));
    assert!(details["records"].as_array().unwrap().iter().all(|r| r["pricing"].is_null()));
    // Cache refresh is local on unchanged input; TTL expiry performs conditional official research.
    f.ctx.app.refresh_metadata(); f.wait(); assert_eq!(f.fetch.0.lock().unwrap().len(), 2);
    f.set("mock_routing_planner_outputs", json!([{"proposals":[Fixture::proposal(1,"small","simple","documentation"),Fixture::proposal(2,"small","simple","functionality")]}]));
    let draft = f.ctx.architect_publish(json!({"goal":"Improve greetings","status":"draft","stages":[
        Fixture::stage(1,"Fix prose spelling"), Fixture::stage(2,"Implement greeting")]}), None, "fixture draft").unwrap();
    let id = draft["plan_id"].clone();
    assert_eq!(f.calls(), (1,1));
    let a = &draft["stages"][1]["model_agreement"];
    assert_eq!(a["policy_inputs"]["tier_provenance"], "configured");
    assert_eq!(a["policy_inputs"]["relative_cost_preference"], 1);
    assert_eq!(a["availability"], "unverified"); assert!(a["policy_inputs"]["pricing"].is_null());
    assert_ne!(a["planner_reason"],a["architect_reason"]);
    let mut recorded = draft.clone();
    for (decision, supersedes) in [("first-decision",Value::Null),("second-decision",json!("first-decision"))] {
        recorded = f.ctx.architecture_store().record(&recorded,"decision",json!({
            "version":1,"id":decision,"plan_id":id,"revision":recorded["revision"],"stage_id":null,
            "summary":"Preserve the greeting interface", "rationale":"Consumers depend on its format",
            "alternatives":[{"description":"Replace format","tradeoffs":"Breaks callers"}],
            "status":"accepted","supersedes":supersedes,"created_unix":1,"updated_unix":1
        })).unwrap();
    }
    assert_eq!(api_request(&f.ctx.app,"POST","/api/approve",json!({})).0,200);
    assert_eq!(f.calls(),(1,1));
    f.set("mock_edits",json!([{"README.md":"The program prints a friendly greeting.\n"},
        {"src/main.rs":"fn main() { println!(\"hello\"); }\n"},{},{},{}]));
    f.set("mock_verdicts",json!([Fixture::verdict(true),Fixture::verdict(true),Fixture::verdict(false),Fixture::verdict(false),Fixture::verdict(true)]));
    f.set("mock_architect_verdicts",json!([Fixture::verdict(true),Fixture::verdict(true),Fixture::verdict(true),Fixture::verdict(true)]));
    f.set("mock_architect_actions",json!([{"stop":true}]));
    f.ctx.run_worker();
    let stopped = f.ctx.load_plan().unwrap();
    assert_eq!(stopped["stages"][0]["status"],"committed");
    assert_eq!(stopped["stages"][0]["review_gate"]["roles"]["architect"],"not_required");
    assert_eq!(stopped["stages"][1]["review_gate"]["status"],"interrupted");
    assert_eq!(f.calls(),(1,1));
    assert_eq!(f.ctx.app.settings.lock().unwrap()["mock_architect_requests"].as_array().unwrap().len(),1);
    f.restart();
    assert!(f.ctx.app.refresh_catalogue()); f.wait();
    f.ctx.app.scheduler_tick(f.time.now_unix()); f.wait();
    assert_eq!(f.fetch.0.lock().unwrap().len(),2); // Fresh persisted metadata needs no research.
    let state = f.state();
    assert_eq!(state["architecture"]["context_status"],"needs_recovery");
    assert_eq!(state["role_usage"], stopped["role_usage"]);
    assert_eq!(state["plan"]["stages"][1]["review_gate"]["status"],"interrupted");
    f.time.0.fetch_add(168*3600,Ordering::SeqCst);
    f.ctx.app.scheduler_tick(f.time.now_unix()); f.wait();
    f.ctx.app.scheduler_tick(f.time.now_unix()); f.wait();
    assert_eq!(f.fetch.0.lock().unwrap().len(),4);
    f.set("mock_routing_planner_outputs",json!([{"proposals":[Fixture::proposal(2,"large","simple","functionality")]}]));
    f.ctx.run_worker();
    let done = f.ctx.load_plan().unwrap();
    assert_eq!(done["status"],"done", "{}", f.ctx.read_history());
    let s = &done["stages"][1];
    assert_eq!(s["attempt_id"],stopped["stages"][1]["attempt_id"]);
    assert_eq!(s["rounds"],4); assert_eq!(s["reassessment"]["count"],1);
    assert_eq!(f.calls(),(2,2));
    assert_eq!(s["model_invocations"][0]["requested"]["model"],"small");
    assert_eq!(s["model_invocations"][3]["requested"]["model"],"large");
    assert_eq!(s["review_gate"]["roles"],json!({"architect":"approved","reviewer":"approved"}));
    let settings = f.ctx.app.settings.lock().unwrap().clone();
    // Initial guidance, recovery reconstruction, and one concrete escalation only.
    assert_eq!(settings["mock_architect_requests"].as_array().unwrap().len(),3);
    let reviews = settings["test_review_sessions"].as_array().unwrap();
    assert_eq!(reviews.iter().filter(|r| r["role"]=="reviewer").count(),5);
    assert_eq!(reviews.iter().filter(|r| r["role"]=="architect").count(),4);
    for r in reviews {
        assert!(r["prompt"].as_str().unwrap().contains("Greeting works."));
        if r["role"]=="reviewer" { assert!(r["session"].is_null()); }
    }
    let handoff = settings["mock_agent_requests"].as_array().unwrap().iter()
        .find(|r| r["model"]=="large" && r["role"]=="fixer").unwrap()["prompt"].as_str().unwrap();
    for text in ["ARCHITECT GUIDANCE","SAVED ARCHITECTURAL SUMMARY","Greeting test still fails","Inspect and preserve"] { assert!(handoff.contains(text),"{text}"); }
    let report = f.ctx.read_reports()[0].clone();
    assert_eq!(report["plan_id"],id); assert_eq!(report["role_usage"],done["role_usage"]);
    assert_eq!(report["stage_outcomes"][1]["model_agreement"]["effective"]["model"],"large");
    assert!(report["architecture"]["summary"].is_string());
    assert_eq!(report["architecture"]["recent_decisions"][0]["status"],"superseded");
    // The next queued goal has critical contract scope, a fresh identity and no inherited budget.
    let next = json!({"goal":"Define persistence contract","status":"draft","stages":[Fixture::stage(1,"Define atomic persistence contract")]});
    f.set("mock_plan_output",next); f.set("queue_auto_approve",json!(true));
    f.set("mock_routing_planner_outputs",json!([{"proposals":[Fixture::proposal(1,"large","critical","persistence")]}]));
    f.set("mock_edits",json!([{"README.md":"The API MUST preserve the atomic persistence contract.\n"}]));
    f.set("mock_verdicts",json!([Fixture::verdict(true)]));
    f.set("mock_architect_verdicts",json!([Fixture::verdict(true)]));
    assert_eq!(api_request(&f.ctx.app,"POST","/api/queue/add",json!({"goal":"Define persistence contract"})).0,200);
    f.ctx.session.queue_active.store(true,Ordering::SeqCst); f.ctx.queue_worker(None);
    let next = f.ctx.load_plan().unwrap();
    assert_eq!(next["status"],"done", "{}", f.ctx.read_history());
    assert_ne!(next["plan_id"],id); assert_eq!(next["stages"][0]["rounds"],1);
    assert_eq!(next["stages"][0]["model_agreement"]["policy_inputs"]["minimum_tier"],3);
    assert_eq!(next["stages"][0]["review_gate"]["roles"],json!({"architect":"approved","reviewer":"approved"}));
    assert_eq!(f.calls(),(3,3)); assert!(f.ctx.load_queue()["items"].as_array().unwrap().is_empty());
    assert_eq!(f.ctx.app.settings.lock().unwrap()["mock_architect_requests"].as_array().unwrap().len(),4);
    f.restart(); assert_eq!(f.ctx.read_reports()[0],report);
    let archived = format!("/api/architecture/history?plan_id={}&limit=1",id.as_str().unwrap());
    let (_, page) = api_request(&f.ctx.app,"GET",&archived,json!({}));
    assert_eq!(page["plan_id"],id); assert_eq!(page["items"].as_array().unwrap().len(),1);
    assert!(page["next_cursor"].is_number());
    let mut events = vec![];
    let mut cursor = 0;
    loop {
        let (code,page) = api_request(&f.ctx.app,"GET",&format!("/api/architecture/history?plan_id={}&cursor={cursor}&limit=20",id.as_str().unwrap()),json!({}));
        assert_eq!(code,200); assert_eq!(page["plan_id"],id);
        events.extend(page["items"].as_array().unwrap().clone());
        match page["next_cursor"].as_u64() { Some(next) => { assert!(next>cursor); cursor=next; }, None => break }
    }
    let history = json!(events).to_string();
    for expected in ["second-decision","supersedes","Consumers depend on its format","model_agreements","repeated_reasoning_failure"] {
        assert!(history.contains(expected),"{expected}");
    }
    let (_, reviews) = api_request(&f.ctx.app,"GET",&format!("/api/architecture/reviews?plan_id={}&stage_id=2&limit=1",id.as_str().unwrap()),json!({}));
    assert_eq!(reviews["plan_id"],id); assert!(reviews["next_cursor"].is_number());
    let other = Fixture::new();
    assert!(other.ctx.read_reports().as_array().unwrap().is_empty());
    assert_eq!(api_request(&other.ctx.app,"GET",&archived,json!({})).0,400);
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
}

#[test]
fn reported_failed_checks_cannot_be_bypassed_or_buy_extra_selection_rounds() {
    let f = Fixture::new();
    f.set("max_fix_rounds",json!(0));
    f.set("mock_routing_planner_outputs",json!([{"proposals":[Fixture::proposal(1,"small","simple","documentation")]}]));
    f.ctx.architect_publish(json!({"goal":"Improve prose","status":"ready","stages":[Fixture::stage(1,"Fix prose spelling")]}),None,"draft").unwrap();
    f.set("mock_edits",json!([{"README.md":"The program prints a friendly greeting.\n"}]));
    let mut bad = Fixture::verdict(true);
    bad["project_checks"] = json!([
        {"command":"./scripts/build.sh","status":"failed","evidence":"Fixture build exited 1"},
        {"command":"cargo test","status":"passed","evidence":"Fixture tests passed"}
    ]);
    f.set("mock_verdicts",json!(vec![bad; 4]));
    f.ctx.run_worker();
    let p = f.ctx.load_plan().unwrap();
    assert_ne!(p["stages"][0]["status"],"committed");
    assert_eq!(p["stages"][0]["review_gate"]["roles"]["architect"],"not_required");
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"1");
    assert_eq!(f.calls(),(1,1));
    f.ctx.run_worker();
    assert_eq!(f.calls(),(1,1));
    let settings = f.ctx.app.settings.lock().unwrap();
    let sessions = settings["test_review_sessions"].as_array().unwrap();
    assert_eq!(sessions.len(),4); assert!(sessions.iter().all(|session| session["role"] == "reviewer"));
}

#[test]
fn polling_bounds_routing_history_without_changing_saved_or_editable_content() {
    let f = Fixture::new();
    let legacy = json!({"goal":"Legacy","stages":[{"id":1,"title":"Editable title","instructions":"Keep this text",
        "model_invocations":(0..1000).map(|id| json!({"id":id,"requested":{"model":"small"}})).collect::<Vec<_>>(),
        "reviews":[],"last_verdict":{"approved":true,"notes":["Legacy note"]}}]});
    let path = f.ctx.forge_path("plan.json"); fs::write(&path,legacy.to_string()).unwrap();
    let bytes = fs::read(&path).unwrap();
    for _ in 0..3 {
        let state = f.state(); let s = &state["plan"]["stages"][0];
        assert_eq!(s["model_invocation_count"],1000); assert_eq!(s["model_invocations"].as_array().unwrap().len(),8);
        assert_eq!(s["model_invocations"][0]["id"],992); assert_eq!(s["instructions"],legacy["stages"][0]["instructions"]);
        assert_eq!(s["last_verdict"],legacy["stages"][0]["last_verdict"]);
        assert!(state["plan"].to_string().len()<2048);
    }
    assert_eq!(fs::read(path).unwrap(),bytes);
}

#[test]
fn report_write_failure_keeps_queue_goal_and_recovers_without_implementation_or_selection_calls() {
    let mut f = Fixture::new();
    f.set("queue_auto_approve",json!(true));
    f.set("mock_plan_output",json!({"goal":"Improve prose","status":"draft","stages":[Fixture::stage(1,"Fix prose spelling")]}));
    f.set("mock_routing_planner_outputs",json!([{"proposals":[Fixture::proposal(1,"small","simple","documentation")]}]));
    f.set("mock_edits",json!([{"README.md":"The program prints a friendly greeting.\n"}]));
    f.set("mock_verdicts",json!([Fixture::verdict(true)]));
    fs::create_dir(f.ctx.forge_path("reports.jsonl")).unwrap(); // Deterministic write failure.
    assert_eq!(api_request(&f.ctx.app,"POST","/api/queue/add",json!({"goal":"Improve prose"})).0,200);
    f.ctx.session.queue_active.store(true,Ordering::SeqCst); f.ctx.queue_worker(None);
    assert_eq!(f.ctx.load_queue()["items"][0]["status"],"failed");
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"2");
    assert!(f.ctx.read_history().to_string().contains("run report:"));
    fs::remove_dir(f.ctx.forge_path("reports.jsonl")).unwrap();
    let requests = f.ctx.app.settings.lock().unwrap()["mock_agent_requests"].clone();
    f.restart();
    f.ctx.session.queue_active.store(true,Ordering::SeqCst); f.ctx.queue_worker(Some(1));
    assert!(f.ctx.load_queue()["items"].as_array().unwrap().is_empty());
    assert_eq!(f.ctx.read_reports().as_array().unwrap().len(),1);
    assert_eq!(f.ctx.app.settings.lock().unwrap()["mock_agent_requests"],requests);
    assert_eq!(f.calls(),(1,1));
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"2");
}
