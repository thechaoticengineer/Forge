use crate::app::{App, FORGE_DIR, WorkerGuard};
use crate::plan::default_settings;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    let mut invalid=changed.clone(); invalid["entries"][0]["effort"]=json!("hallucinated");
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
