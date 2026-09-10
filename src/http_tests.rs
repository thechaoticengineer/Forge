use crate::app::{App, FORGE_DIR, WorkerGuard};
use crate::plan::{default_settings, review_cadence};
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn state_plan_review_is_bounded_and_never_loads_verdict_payloads() {
    let test = QueueTest::new(false);
    let store = test.app.architecture_store();
    let long = "界🙂\u{0001}".repeat(2000);
    let identity = json!({"plan_id":"plan", "scope":"plan", "stage_id":null,
        "attempt_id":"attempt", "round":12, "policy":{"rationale":long}});
    let reviews: Vec<_> = (0..12).map(|round| json!({"round":round,
        "role":if round % 2 == 0 {"architect"} else {"reviewer"},
        "identity":identity, "approved":false, "summary":long,
        "criteria":[{"evidence":"complete evidence must not enter state"}],
        "issues":[format!("full request {round}: {long}")]})).collect();
    let plan = json!({"stages":[{"id":1,"reviews":[{"summary":"stage review"}]}],
        "plan_review":{"version":1,"status":"blocked","base":"base-sha","head":"head-sha",
            "attempt_id":"attempt","rounds":12,"budget":11,"fix_sha":"fix-sha",
            "required_roles":["architect","reviewer"],"reviews":reviews,
            "gate":{"status":"blocked","roles":{"architect":"blocked","reviewer":"blocked"},
                "identity":identity,"requests":vec![json!({"role":"architect","text":long});20]},
            "model_invocations":vec![json!({"role":"fixer","requested":{"model":long},"failure":long});20],
            "subject":{"large":long},"acceptance":long,"retries":{"history":vec![long.clone();20]},
            "outstanding_requests":vec![format!("[reviewer] {long}");20],"unknown":long}});
    let mut plan = plan;
    plan["plan_review"]["reviews"] = json!([]);
    for verdict in &reviews {
        plan["plan_review"]["reviews"].as_array_mut().unwrap().push(verdict.clone());
        plan = store.publish(plan, crate::architecture::checkpoint_default(),
            json!({"kind":"plan_review","reviews":[verdict]})).unwrap();
    }
    let raw = store.load_raw().unwrap().unwrap();
    assert_eq!(raw["architecture"]["review_history"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["1"]);
    assert!(raw["plan_review"]["reviews"]["$forge_reviews"].is_string());
    assert_eq!(store.load().unwrap().unwrap()["plan_review"]["reviews"], json!(reviews));
    let (code, state) = api_request(&test.app.app, "GET", "/api/state", json!({}));
    assert_eq!(code, 200);
    let review = &state["plan"]["plan_review"];
    for (key, expected) in [("status",json!("blocked")),("base",json!("base-sha")),
        ("rounds",json!(12)),("budget",json!(11)),("fix_sha",json!("fix-sha")),
        ("required_roles",json!(["architect","reviewer"]))] { assert_eq!(review[key],expected); }
    assert_eq!(review["gate"]["roles"], json!({"architect":"blocked","reviewer":"blocked"}));
    assert_eq!(review["gate"]["identity"]["attempt_id"], "attempt");
    assert_eq!(review["gate"]["requests_truncated"], true);
    assert_eq!(review["gate"]["identity_truncated"], true);
    assert_eq!(review["gate"]["requests"][0]["role"],"architect");
    assert!(review["gate"]["requests"][0]["text"].as_str().unwrap().chars().count() <= 240);
    assert_eq!(review["review_count"],12);
    assert_eq!(review["reviews_truncated"],true);
    assert_eq!(review["reviews"].as_array().unwrap().len(),crate::review_history::PREVIEW_COUNT);
    assert_eq!(review["reviews"][0]["round"],4);
    assert!(review["reviews"].as_array().unwrap().iter().all(|r|
        r["truncated"] == true && r.get("criteria").is_none() && r.get("identity").is_none() && r.to_string().len() <= 4096));
    assert_eq!(review["model_invocation_count"],20);
    assert_eq!(review["model_invocations_truncated"],true);
    assert_eq!(review["model_invocations"].as_array().unwrap().len(),8);
    for key in ["subject","acceptance","retries","unknown"] { assert!(review.get(key).is_none()); }
    assert!(review.to_string().len() < 100 * 1024);
    let mut twice = state["plan"].clone();
    crate::review_history::bounded(&mut twice);
    assert_eq!(twice,state["plan"]);
    let mut cursor = 0;
    let mut complete = Vec::new();
    loop {
        let (code, history) = api_request(&test.app.app,"GET",
            &format!("/api/architecture/history?limit=100&cursor={cursor}"),json!({}));
        assert_eq!(code,200,"{}",history["error"]);
        let items = history["items"].as_array().unwrap();
        assert!(items.len() <= 100);
        let next = history["next_cursor"].as_u64().unwrap_or(history["event_end"].as_u64().unwrap());
        assert!(next > cursor && next - cursor <= 256 * 1024);
        assert!(items.iter().map(|v| v.to_string().len()).sum::<usize>() <= 4 * 1024 * 1024);
        for event in items {
            assert!(event["id"].is_string());
            assert_eq!(event["payload"]["kind"],"plan_review");
            complete.extend(event["payload"]["reviews"].as_array().unwrap().iter().cloned());
        }
        if history["next_cursor"].is_null() { break; }
        cursor = next;
    }
    assert!(complete == reviews,"paged verdicts must preserve every full field and identity");

    // Neither indexed verdicts nor even the latest event's full payload is read
    // by polling. Keep file sizes and the committed newline intact.
    let dir = test.app.forge_path("architecture").join(raw["plan_id"].as_str().unwrap());
    let reference = &raw["architecture"]["plan_review_history"];
    fs::write(dir.join("reviews").join(format!("{}.jsonl",reference["file"].as_str().unwrap())),
        vec![b'x';reference["bytes"].as_u64().unwrap() as usize]).unwrap();
    let mut event = vec![b'x';raw["architecture"]["event_end"].as_u64().unwrap() as usize];
    *event.last_mut().unwrap() = b'\n';
    fs::write(dir.join("events.jsonl"),event).unwrap();
    assert!(store.load().is_err());
    let (_, again) = api_request(&test.app.app,"GET","/api/state",json!({}));
    assert_eq!(again["plan"],state["plan"]);
    assert_eq!(again["architecture"],state["architecture"]);
}

#[test]
fn state_omits_absent_plan_review_and_projects_small_verdicts() {
    let test = QueueTest::new(false);
    for optional in [None, Some(Value::Null)] {
        let mut plan = json!({"stages":[]});
        if let Some(value) = optional { plan["plan_review"] = value; }
        test.app.save_plan(&plan).unwrap();
        let (_, state) = api_request(&test.app.app,"GET","/api/state",json!({}));
        assert!(state["plan"].get("plan_review").is_none());
    }
    let mut plan = test.app.load_plan().unwrap();
    plan["plan_review"] = json!({"status":"pending","reviews":[]});
    test.app.save_plan(&plan).unwrap();
    let (_, state) = api_request(&test.app.app,"GET","/api/state",json!({}));
    assert_eq!(state["plan"]["plan_review"]["review_count"],0);
    assert_eq!(state["plan"]["plan_review"]["reviews_truncated"],false);
    plan["plan_review"]["reviews"] = json!([{"role":"reviewer","summary":"small", "criteria":["private evidence"]}]);
    test.app.save_plan(&plan).unwrap();
    let (_, state) = api_request(&test.app.app,"GET","/api/state",json!({}));
    assert_eq!(state["plan"]["plan_review"]["reviews"][0]["summary"],"small");
    assert!(state["plan"]["plan_review"]["reviews"][0].get("criteria").is_none());
    assert_eq!(state["plan"]["plan_review"]["reviews_truncated"],true);
}

#[test]
fn plan_review_history_keeps_record_limit_and_cursor_identity() {
    let test = QueueTest::new(false);
    let store = test.app.architecture_store();
    let mut plan = json!({"stages":[],"plan_review":{"reviews":[]}});
    let mut expected = Vec::new();
    for round in 1..=101 {
        let role = if round % 2 == 0 { "architect" } else { "reviewer" };
        let verdict = json!({"id":format!("review-{round}"),"role":role,"round":round,
            "identity":{"stage_id":null,"scope":"plan","role":role,"attempt_id":"attempt","round":round},
            "issues":[format!("full request {round}")],"criteria":[{"evidence":"full evidence"}]});
        plan["plan_review"]["reviews"].as_array_mut().unwrap().push(verdict.clone());
        expected.push(verdict.clone());
        plan = store.publish(plan,crate::architecture::checkpoint_default(),
            json!({"kind":"plan_review","reviews":[verdict]})).unwrap();
    }
    let (code, first) = api_request(&test.app.app,"GET","/api/architecture/history?limit=999",json!({}));
    assert_eq!(code,200);
    assert_eq!(first["items"].as_array().unwrap().len(),100);
    let cursor = first["next_cursor"].as_u64().unwrap();
    assert!(cursor <= 256 * 1024);
    assert_eq!(api_request(&test.app.app,"GET",
        &format!("/api/architecture/history?cursor={}",cursor - 1),json!({})).0,400);
    let path = format!("/api/architecture/history?plan_id={}&cursor={cursor}&limit=999",plan["plan_id"].as_str().unwrap());
    let (code, last) = api_request(&test.app.app,"GET",&path,json!({}));
    assert_eq!(code,200);
    assert_eq!(last["items"].as_array().unwrap().len(),1);
    assert!(last["next_cursor"].is_null());
    let all: Vec<_> = first["items"].as_array().unwrap().iter().chain(last["items"].as_array().unwrap())
        .map(|event| event["payload"]["reviews"][0].clone()).collect();
    assert_eq!(all,expected);
    store.reset().unwrap();
    assert_eq!(api_request(&test.app.app,"GET",&path,json!({})).1,last);
}

#[test]
fn plan_review_publication_respects_page_and_expanded_record_budgets() {
    let test = QueueTest::new(false);
    let store = test.app.architecture_store();
    let evidence = "complete evidence ".repeat(8000);
    let verdict = json!({"role":"architect","identity":{"scope":"plan","stage_id":null,
        "attempt_id":"attempt","round":1,"role":"architect"},
        "criteria":vec![json!({"evidence":evidence});8],"issues":[evidence]});
    let plan = store.publish(json!({"stages":[],"plan_review":{"reviews":[verdict.clone()]}}),
        crate::architecture::checkpoint_default(),json!({"kind":"plan_review","reviews":[verdict.clone()]})).unwrap();
    let (code, page) = api_request(&test.app.app,"GET","/api/architecture/history",json!({}));
    assert_eq!(code,200);
    assert!(page["event_end"].as_u64().unwrap() <= 256 * 1024);
    assert!(page.to_string().len() > 256 * 1024);
    assert!(page.to_string().len() < 4 * 1024 * 1024);
    assert!(page["items"][0]["payload"]["reviews"][0] == verdict);
    let before = fs::read(test.app.forge_path("plan.json")).unwrap();
    for oversized in [json!({"summary":"x".repeat(300 * 1024)}),
        json!({"criteria":vec![json!({"evidence":"x".repeat(100 * 1024)});42]})] {
        let mut next = plan.clone();
        next["plan_review"]["reviews"].as_array_mut().unwrap().push(oversized.clone());
        let error = store.publish(next,store.checkpoint(&plan).unwrap(),
            json!({"kind":"plan_review","reviews":[oversized]})).unwrap_err();
        assert!(error.contains("limit"),"{error}");
        assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(),before);
        assert_eq!(store.load().unwrap().unwrap(),plan);
    }
}

#[test]
fn review_cadence_defaults_and_accessor() {
    let settings = default_settings();
    assert_eq!(settings["review_cadence"], json!({"architect":"per_stage","reviewer":"per_stage"}));
    for role in ["architect", "reviewer"] {
        assert_eq!(review_cadence(&settings, role), "per_stage");
        for value in [json!("per_plan"), json!("per_stage"), json!("unknown"), json!("PER_PLAN"), json!("per_plan "), Value::Null, json!(true), json!(1), json!([]), json!({})] {
            let mut settings = settings.clone();
            settings["review_cadence"][role] = value.clone();
            assert_eq!(review_cadence(&settings, role), if value == "per_plan" { "per_plan" } else { "per_stage" });
        }
        for legacy in [json!({}), Value::Null, json!([]), json!(false), json!("per_plan"), json!({"review_cadence":null}), json!({"review_cadence":[]}), json!({"review_cadence":"per_plan"}), json!({"review_cadence":{}})] {
            assert_eq!(review_cadence(&legacy, role), "per_stage");
        }
    }
    assert_eq!(review_cadence(&settings, "unknown"), "per_stage");
}

#[test]
fn review_cadence_settings_are_returned_in_state() {
    let test = QueueTest::new(false);
    let app = &test.app.app;
    let (code, state) = api_request(app, "GET", "/api/state", json!({}));
    assert_eq!(code, 200);
    assert_eq!(state["settings"]["review_cadence"], default_settings()["review_cadence"]);
    for architect in ["per_plan", "per_stage"] {
        for reviewer in ["per_stage", "per_plan"] {
            let cadence = json!({"architect":architect,"reviewer":reviewer});
            assert_eq!(api_request(app, "POST", "/api/settings", json!({"review_cadence":cadence})).0, 200);
            assert_eq!(app.settings.lock().unwrap()["review_cadence"], cadence);
            let (code, state) = api_request(app, "GET", "/api/state", json!({}));
            assert_eq!(code, 200);
            assert_eq!(state["settings"]["review_cadence"], cadence);
        }
    }
}

#[test]
fn invalid_review_cadence_settings_are_atomic() {
    let test = QueueTest::new(false);
    let mut app = App::new(test.app.project(), default_settings());
    app.model_policy_path = Some(test.path.join("model-policy.json"));
    let app = Arc::new(app);
    let mut policy = json!(crate::catalogue::Policy::default());
    policy["policy_revision"] = json!("2");
    assert_eq!(api_request(&app, "POST", "/api/settings", json!({
        "model_catalogue":policy,"review_cadence":{"architect":"per_plan","reviewer":"per_stage"}
    })).0, 200);
    let saved = app.settings.lock().unwrap().clone();
    let policy_path = app.model_policy_path.as_ref().unwrap();
    let saved_policy = fs::read(policy_path).unwrap();
    policy["policy_revision"] = json!("3");
    let mut invalid = vec![json!({}), json!({"architect":"per_plan"}), json!({"reviewer":"per_stage"}),
        json!({"architect":"per_plan","reviewer":"per_stage","extra":"per_stage"}),
        Value::Null, json!("per_plan"), json!([]), json!(true), json!(1)];
    for role in ["architect", "reviewer"] {
        for value in [json!("unknown"), json!("PER_PLAN"), json!("per_plan "), Value::Null, json!(true), json!(1), json!([]), json!({})] {
            let mut cadence = saved["review_cadence"].clone();
            cadence[role] = value;
            invalid.push(cadence);
        }
    }
    for cadence in invalid {
        let (code, response) = api_request(&app, "POST", "/api/settings", json!({
            "review_cadence":cadence,"auto_push":false,"model_catalogue":policy
        }));
        assert_eq!(code, 400, "{cadence}");
        let error = response["error"].as_str().unwrap();
        for text in ["invalid review_cadence", "exactly", "architect", "reviewer", "per_stage", "per_plan"] {
            assert!(error.contains(text), "{error}");
        }
        assert_eq!(app.settings.lock().unwrap()["auto_push"], true);
        assert_eq!(*app.settings.lock().unwrap(), saved);
        assert_eq!(fs::read(policy_path).unwrap(), saved_policy);
    }
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

#[test]
fn agent_records_api_is_project_scoped_additive_and_reset_aware() {
    use crate::agent_log::{InvocationLog, Message, Sink};
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(engine.clone()));
    let get = |suffix: &str| api_request(engine, "GET", &format!("/api/agent_records{suffix}"), json!({}));
    let legacy = format!("\n  {}\r\nlast \t", "界".repeat(20_000));
    fs::write(second.app.forge_path("agent.log"), &legacy).unwrap();
    let (_, old) = get(&format!("?project={}", second.app.project()));
    assert_eq!(old["project"], second.app.project());
    assert_eq!(old["entries"][0]["text"], legacy);
    assert_eq!(old["entries"][0]["kind"], "legacy");
    let mut a = InvocationLog::start(&first.app.forge_path(""), "A\n").unwrap();
    let mut b = InvocationLog::start(&second.app.forge_path(""), "B\n").unwrap();
    a.append(&Message::new("message", "stdout", "", "A only\nsecret")).unwrap();
    b.append(&Message::new("message", "stdout", "", "B first\n  exact \t")).unwrap();
    b.append(&Message::new("error", "stderr", "", "B second\r\nline")).unwrap();
    let (status, page) = get(&format!("?project={}&limit=1", second.app.project()));
    assert_eq!(status, 200);
    assert_eq!(page["version"], 1);
    assert_eq!(page["project"], second.app.project());
    assert_eq!(page["entries"][0]["text"], "B first\n  exact \t");
    assert_eq!(page["next_cursor"], 1);
    assert_eq!(page["more"], true);
    let id = page["session"].as_str().unwrap();
    let (status, next) = get(&format!("?project={}&session={id}&cursor=1", second.app.project()));
    assert_eq!(status, 200);
    assert_eq!(next["entries"][0]["stream"], "stderr");
    assert_eq!(next["next_cursor"], 2);
    assert_eq!(next["more"], false);
    assert_eq!(get("").1["entries"][0]["text"], "A only\nsecret");
    assert_eq!(get(&format!("?archive={id}")).0, 500); // B archive cannot escape A
    assert_eq!(get("?archive=../agent.log").0, 400);
    for query in ["?project=%ZZ", "?project=/not-a-repository", "?cursor=nope", "?limit=nope", "?cursor=1"] {
        assert_eq!(get(query).0, 400, "{query}");
    }
    // Existing byte-offset clients retain the exact log/size contract.
    let full = fs::read(second.app.forge_path("agent.log")).unwrap();
    for offset in [0, 2, full.len(), full.len()+1] {
        let (status, old_api) = api_request(engine, "GET", &format!("/api/agent_log?project={}&offset={offset}", second.app.project()), json!({}));
        assert_eq!(status, 200);
        assert_eq!(old_api["size"], full.len());
        let expected = if offset <= full.len() { String::from_utf8_lossy(&full[offset..]).into_owned() }
            else { crate::util::last_chars(&String::from_utf8_lossy(&full), 30_000) };
        assert_eq!(old_api["log"], expected);
    }
    let _replacement = InvocationLog::start(&second.app.forge_path(""), "replacement with a much longer header\n").unwrap();
    let reset = get(&format!("?project={}&session={id}&cursor=2", second.app.project())).1;
    assert_eq!(reset["reset"], true);
    assert_eq!(reset["next_cursor"], 0);
    assert_ne!(reset["session"], id);
    let history = get(&format!("?project={}&history=true", second.app.project())).1;
    let legacy_id = history["sources"].as_array().unwrap().iter().find(|s| s["legacy"] == true).unwrap()["session"].as_str().unwrap();
    assert_eq!(get(&format!("?project={}&archive={legacy_id}", second.app.project())).1["entries"][0]["text"], legacy);
    // A fresh engine reads the same on-disk session without provider identity.
    let restarted = Arc::new(App::new(second.app.project(), default_settings()));
    let (_, resumed) = api_request(&restarted, "GET", "/api/agent_records", json!({}));
    assert_eq!(resumed["session"], reset["session"]);
    assert_eq!(resumed["reset"], false);
}

#[test]
fn agent_records_api_recovers_startup_without_hiding_incomplete_history() {
    use crate::agent_log::{InvocationLog, Message, Sink};
    let first = QueueTest::new(false);
    let engine = &first.app.app;
    let second = QueueTest::with_engine(false, Some(engine.clone()));
    let forge = second.app.forge_path("");
    let get = |endpoint: &str, suffix: &str| api_request(engine, "GET",
        &format!("/api/{endpoint}?project={}{suffix}", second.app.project()), json!({}));
    let mut writer = InvocationLog::start(&forge, "old\n").unwrap();
    writer.append(&Message::new("message", "stdout", "", "exact old\r\n  text\t")).unwrap();
    let old = get("agent_records", "").1;
    let id = old["session"].as_str().unwrap();
    let root = forge.join("agent-records");
    fs::write(root.join(id).join("pending.json"), r#"{"publication":"incomplete"}"#).unwrap();
    fs::write(root.join("pending.json"), r#"{"publication":"incomplete"}"#).unwrap();
    fs::hard_link(root.join(id).join("readable.log"), root.join("agent.log.next")).unwrap();
    assert_eq!(get("agent_records", "").0, 500);
    let readable = "old\nexact old\r\n  text\t\n";
    assert_eq!(get("agent_log", "&offset=0"), (200, json!({"log":readable, "size":readable.len()})));
    // The active project's healthy feed is unaffected by B's interrupted startup.
    assert_eq!(api_request(engine, "GET", "/api/agent_records", json!({})).0, 200);
    let mut next = InvocationLog::start(&forge, "new\n").unwrap();
    next.append(&Message::new("message", "stdout", "", "new 界\n  full\t")).unwrap();
    let (status, reset) = get("agent_records", &format!("&session={id}&cursor=1"));
    assert_eq!(status, 200);
    assert_eq!(reset["reset"], true);
    assert_eq!(reset["entries"][0]["text"], "new 界\n  full\t");
    assert_eq!(reset["next_cursor"], 1);
    let empty = get("agent_records", &format!("&session={}&cursor=1", reset["session"].as_str().unwrap())).1;
    assert_eq!(empty["reset"], false);
    assert_eq!(empty["entries"], json!([]));
    assert_eq!(get("agent_records", &format!("&archive={id}")).0, 500);
    let history = get("agent_records", "&history=true").1;
    let source = history["sources"].as_array().unwrap().iter().find(|s| s["session"] == id).unwrap();
    assert_eq!(source["incomplete"], true);
    assert_eq!(source["count"], 1);
    assert_eq!(fs::read_to_string(root.join(id).join("readable.log")).unwrap(), "old\nexact old\r\n  text\t\n");
}
