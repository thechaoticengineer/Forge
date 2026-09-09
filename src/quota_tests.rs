use super::*;
use crate::catalogue::{Failure, FailureKind};
use crate::catalogue_process::Channel;
use std::sync::atomic::{AtomicUsize, Ordering};

fn usage() -> Usage {
    serde_json::from_value(json!({"available":true,"extra_usage_enabled":false,"windows":[
        {"name":"Claude · 5h","model":null,"used_percent":57,"resets_at":"future","resets_unix":2000},
        {"name":"Fable · weekly","model":"Fable","used_percent":100,"resets_at":"future","resets_unix":2000}
    ]})).unwrap()
}
#[test]
fn scoped_exhaustion_respects_model_reset_and_overage() {
    let mut u = usage();
    assert!(exhausted(&u, "claude-fable-5-1[1m]", 1000).is_some());
    assert!(exhausted(&u, "claude-sonnet-4", 1000).is_none());
    assert!(exhausted(&u, "claude-notfable-5", 1000).is_none());
    assert!(exhausted(&u, "claude-fable-5", 2000).is_none());
    u.extra_usage_enabled = Some(true);
    assert!(exhausted(&u, "claude-fable-5", 1000).is_none());
    u.extra_usage_enabled = None;
    assert!(exhausted(&u, "claude-fable-5", 1000).is_none());
    u.extra_usage_enabled = Some(false);
    u.windows[0].used_percent = Some(100.0);
    assert!(exhausted(&u, "claude-sonnet-4", 1000).is_some());
    u.windows[0].used_percent = None;
    u.windows[1].resets_unix = None;
    assert!(exhausted(&u, "claude-fable-5", 1000).is_none());
}

struct FakeLauncher { calls: AtomicUsize, fail: bool }
struct FakeChannel(Option<String>);
impl Channel for FakeChannel {
    fn send(&mut self, _: &str) -> Result<(), Failure> { panic!("quota must never send a prompt") }
    fn line(&mut self) -> Result<Option<String>, Failure> { Ok(self.0.take()) }
    fn finish(&mut self) -> Result<(), Failure> { Ok(()) }
}
impl Launcher for FakeLauncher {
    fn spawn(&self, spec: &CommandSpec, _: Budget) -> Result<Box<dyn Channel>, Failure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail { return Err(Failure::new(FailureKind::Auth)); }
        if spec.executable == "claude" {
            assert_eq!(spec.args, ["--version"]);
            return Ok(Box::new(FakeChannel(Some("2.1.263 (Claude Code)".into()))));
        }
        assert_eq!(spec.args, ["--forge-usage-v1", "claude", "2.1.263 (Claude Code)"]);
        let mut u = usage();
        u.windows[1].resets_unix = Some(unix_timestamp() + 3600);
        Ok(Box::new(FakeChannel(Some(json!({"bridge_version":1,"source":"claude_code_usage",
            "cli_version":"2.1.263 (Claude Code)","sdk_version":"0.3.261","usage":u}).to_string()))))
    }
}
#[test]
fn launch_check_blocks_before_generation_and_reuses_fresh_reading() {
    let fake = Arc::new(FakeLauncher { calls: AtomicUsize::new(0), fail: false });
    let service = Service { launcher: fake.clone(), ..Service::default() };
    assert_eq!(service.snapshot()["status"], "pending");
    for _ in 0..2 { assert!(service.check("bridge", "claude-fable-5-1").unwrap_err().contains("0% remaining")); }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    assert!(service.check("bridge", "claude-sonnet-4").is_ok());
    assert!(!service.refresh("bridge".into(), true));
    assert_eq!(service.snapshot()["status"], "available");
    service.cache.lock().unwrap().checked -= FRESH_SECONDS;
    assert_eq!(service.snapshot()["status"], "stale");
    assert!(service.check("bridge", "claude-fable-5").is_ok());
}
#[test]
fn unavailable_usage_is_unknown_and_retry_is_throttled() {
    let fake = Arc::new(FakeLauncher { calls: AtomicUsize::new(0), fail: true });
    let service = Service { launcher: fake.clone(), ..Service::default() };
    for _ in 0..2 { assert!(service.check("bridge", "claude-fable-5").is_ok()); }
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(service.snapshot()["status"], "unavailable");
    assert!(service.snapshot()["windows"].is_null());
}
#[test]
fn launch_waits_for_background_reading_without_blocking_snapshot() {
    let service = Service::default();
    assert!(service.claim("bridge", 60));
    let other = service.clone();
    let check = std::thread::spawn(move || other.check("bridge", "claude-fable-5"));
    assert_eq!(service.snapshot()["refreshing"], true);
    let mut u = usage();
    u.windows[1].resets_unix = Some(unix_timestamp() + 3600);
    {
        let mut c = service.cache.lock().unwrap();
        c.usage = Some(u); c.checked = unix_timestamp(); c.attempted = c.checked; c.refreshing = false;
    }
    service.refreshed.notify_all();
    assert!(check.join().unwrap().unwrap_err().contains("quota exhausted"));
}

#[test]
fn exhausted_quota_prevents_agent_spawn_and_is_visible_through_state_api() {
    use crate::test_support::{QueueTest, api_request};
    let fixture = QueueTest::new(false);
    let mut app = crate::app::App::new(fixture.app.project(), crate::plan::default_settings());
    app.quota = Service { launcher: Arc::new(FakeLauncher { calls: AtomicUsize::new(0), fail: false }), ..Service::default() };
    app.settings.lock().unwrap()["model_catalogue"]["claude_bridge"] = json!("/fake/bridge");
    // Any attempt to execute the provider would fail with a different error.
    app.settings.lock().unwrap()["test_cli_claude"] = json!("/does-not-exist/claude");
    let app = Arc::new(app);
    let ctx = app.context(fixture.app.project());
    let error = ctx.run_agent(&crate::agent::AgentRequest {role:"planner",provider:"claude",
        model:"claude-fable-5-1[1m]",effort:"provider_default",session:None,prompt:"must not send"}).unwrap_err();
    assert!(error.contains("Fable · weekly quota exhausted"), "{error}");
    let (_, state) = api_request(&app,"GET","/api/state",json!({}));
    assert_eq!(state["claude_quota"]["windows"][1]["used_percent"],100.0);
    assert_eq!(state["claude_quota"]["status"],"available");
    let (code, refresh) = api_request(&app,"POST","/api/quota/refresh",json!({}));
    assert_eq!(code,202);
    assert_eq!(refresh["started"],false);
}

#[test]
fn automatic_review_skips_exhausted_family_but_preserves_constraints_and_independence() {
    let fixture = crate::test_support::QueueTest::new(false);
    let mut app = crate::app::App::new(fixture.app.project(), crate::plan::default_settings());
    app.quota = Service { launcher: Arc::new(FakeLauncher { calls: AtomicUsize::new(0), fail: false }), ..Service::default() };
    {
        let mut settings = app.settings.lock().unwrap();
        settings["model_catalogue"]["claude_bridge"] = json!("/fake/bridge");
        settings["model_catalogue"]["entries"] = json!([
            {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
            {"provider":"claude","model":"claude-opus-5[1m]","tier":"strong"},
            {"provider":"codex","model":"gpt-6-astra","tier":"strong"}]);
    }
    let app = Arc::new(app);
    let ctx = app.context(fixture.app.project());
    // Unknown quota doesn't invent exhaustion. A later check redirects selection.
    assert_eq!(ctx.reviewer_config("codex").unwrap().1,"claude-fable-5-1[1m]");
    assert!(app.quota.check("/fake/bridge","claude-fable-5-1").is_err());
    assert_eq!(ctx.reviewer_config("codex").unwrap(),("claude".into(),"claude-opus-5[1m]".into()));
    assert_eq!(ctx.reviewer_config("claude").unwrap(),("codex".into(),"gpt-6-astra".into()));
    // Explicit choices and non-automatic routing are not silently replaced.
    app.settings.lock().unwrap()["reviewer_model"] = json!("claude-fable-5-1[1m]");
    assert!(ctx.reviewer_config("codex").unwrap_err().contains("quota exhausted"));
    app.settings.lock().unwrap()["reviewer_model"] = json!("");
    app.settings.lock().unwrap()["automatic_routing"] = json!(false);
    assert!(ctx.reviewer_config("codex").unwrap_err().contains("quota exhausted"));
    app.settings.lock().unwrap()["automatic_routing"] = json!(true);
    // A whole-provider limit cannot be escaped with another Claude model or self-review.
    app.quota.cache.lock().unwrap().usage.as_mut().unwrap().windows[0].used_percent = Some(100.0);
    app.quota.cache.lock().unwrap().usage.as_mut().unwrap().windows[0].resets_unix = Some(unix_timestamp()+3600);
    assert!(ctx.reviewer_config("codex").unwrap_err().contains("no eligible strong independent reviewer"));
    app.quota.cache.lock().unwrap().usage.as_mut().unwrap().windows[0].used_percent = Some(57.0);
    app.settings.lock().unwrap()["model_catalogue"]["entries"][1]["tier"] = json!("standard");
    assert!(ctx.reviewer_config("codex").is_err());
    // Stale data doesn't lock a family out indefinitely; probe refreshes at launch.
    app.quota.cache.lock().unwrap().checked -= FRESH_SECONDS;
    assert_eq!(ctx.reviewer_config("codex").unwrap().1,"claude-fable-5-1[1m]");
}

#[test]
fn quota_policy_is_shared_across_roles_candidates_and_fixed_execution() {
    let fixture = crate::test_support::QueueTest::new(false);
    let mut app = crate::app::App::new(fixture.app.project(), crate::plan::default_settings());
    app.quota = Service { launcher: Arc::new(FakeLauncher { calls: AtomicUsize::new(0), fail: false }), ..Service::default() };
    {
        let mut settings = app.settings.lock().unwrap();
        for role in ["planner","architect","reviewer"] { settings[role] = json!("claude"); }
        settings["model_catalogue"]["claude_bridge"] = json!("/fake/bridge");
        settings["model_catalogue"]["entries"] = json!([
            {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
            {"provider":"claude","model":"opus[1m]","tier":"strong"}]);
    }
    let app = Arc::new(app);
    let ctx = app.context(fixture.app.project());
    assert!(app.quota.check("/fake/bridge","claude-fable-5-1").is_err());
    for role in ["planner","architect","chat","reviewer"] {
        let requirements = ctx.model_requirements(role,Some("codex")).unwrap();
        assert_eq!(ctx.select_model(&requirements,&[]).unwrap().1,"opus[1m]", "{role}");
    }
    let candidates = ctx.routing_candidates().unwrap();
    assert_eq!(candidates[0]["eligible"],false);
    assert_eq!(candidates[1]["eligible"],true);
    // Quota does not invalidate a saved agreement or silently replace its model.
    assert_eq!(ctx.routing_options().unwrap()[0]["eligible"],true);
    let policy = crate::catalogue::Policy::from_settings(&app.settings.lock().unwrap()).unwrap();
    let fixed = ctx.execution_selection(&policy,crate::catalogue::Provider::Claude,"claude-fable-5-1[1m]","provider_default");
    assert_eq!(fixed["eligible"],false);
    assert!(fixed["error"].as_str().unwrap().contains("quota exhausted"));
}
