use super::*;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use std::sync::atomic::Ordering;

fn base() -> Policy {
    Policy::from_settings(&json!({"model_catalogue":{
        "policy_revision":"before", "codex_scope":"custom", "claude_scope":"default",
        "claude_bridge":"/tmp/bridge", "metadata_research":false,
        "entries":[{"provider":"codex","model":"gpt-6-astra","tier":"strong",
            "effort":"high","suitability":["security"],"limits":{"context":8192}},
            {"provider":"claude","model":"opus[1m]","tier":"strong"}]
    }})).unwrap()
}

fn discovery() -> Value {
    json!({"providers":[{"provider":"codex","status":"discovered","models":[
        {"id":"gpt-6-astra"},{"id":"gpt-5.6-luna"}]},
        {"provider":"claude","status":"unsupported_discovery","models":[]}]})
}

fn proposal(entries: &[Entry]) -> Value {
    json!({"summary":"Capability judgments; relative preferences are not measured prices.",
        "assignments":entries.iter().map(|e| json!({"provider":e.provider,"model":e.model,
            "tier":if e.model == "gpt-5.6-luna" {"basic"} else {"strong"},
            "rationale":"Suggested capability tier and relative routing preference."})).collect::<Vec<_>>()})
}

#[test]
fn tier_proposal_covers_discovered_and_configured_models_preserving_other_policy_fields() {
    let base = base();
    let entries = candidates(&base, &discovery()).unwrap();
    assert_eq!(entries.iter().map(|e| e.model.as_str()).collect::<Vec<_>>(), ["gpt-6-astra","opus[1m]","gpt-5.6-luna"]);
    let result = validate(&proposal(&entries).to_string(), &base, &entries, "new-revision", &json!([])).unwrap();
    let policy = Policy::from_settings(&json!({"model_catalogue":result["policy"]})).unwrap();
    assert_eq!(policy.entries[0].effort, base.entries[0].effort);
    assert_eq!(policy.entries[0].suitability, base.entries[0].suitability);
    assert_eq!(policy.entries[0].limits, base.entries[0].limits);
    assert_eq!(policy.entries[1].effort, None);
    assert_eq!(policy.entries[2].tier, Tier::Basic);
    assert_eq!(policy.entries[2].effort.as_deref(), Some("provider_default"));
    assert_eq!(policy.claude_bridge, base.claude_bridge);
    assert_eq!(policy.codex_scope, base.codex_scope);
    assert!(!policy.metadata_research);
    assert_eq!(policy.policy_revision, "new-revision");
    assert_eq!(base.entries[0].relative_cost_preference, None);
}

#[test]
fn incomplete_invented_duplicate_and_invalid_tier_proposals_are_rejected() {
    let base = base();
    let entries = candidates(&base, &discovery()).unwrap();
    let valid = proposal(&entries);
    for bad in ["missing", "invented", "duplicate", "tier", "preference", "rationale", "extra"] {
        let mut value = valid.clone();
        match bad {
            "missing" => { value["assignments"].as_array_mut().unwrap().pop(); },
            "invented" => value["assignments"][0]["model"] = json!("invented"),
            "duplicate" => value["assignments"][0] = value["assignments"][1].clone(),
            "tier" => value["assignments"][0]["tier"] = json!("premium"),
            "preference" => value["assignments"][0]["relative_cost_preference"] = json!(1001),
            "rationale" => value["assignments"][0]["rationale"] = json!(" "),
            _ => value["claude_bridge"] = json!("/tmp/other"),
        }
        assert!(validate(&value.to_string(), &base, &entries, "new", &json!([])).is_err(), "{bad}");
    }
    assert!(candidates(&Policy::default(), &json!({})).is_err());
    let too_many = json!({"providers":[{"provider":"codex","models":
        (0..65).map(|i| json!({"id":format!("m{i}")})).collect::<Vec<_>>()}]});
    assert_eq!(candidates(&Policy::default(), &too_many).unwrap().len(), 65);
}

#[test]
fn ai_cannot_invent_cost_preferences_and_hidden_endpoints_are_not_added() {
    let base = base();
    let mut details = discovery();
    details["providers"][0]["models"].as_array_mut().unwrap().push(
        json!({"id":"internal","capabilities":{"hidden":true}}));
    let entries = candidates(&base, &details).unwrap();
    assert!(!entries.iter().any(|e| e.model == "internal"));
    let costs = json!([{"provider":"codex","model":"gpt-6-astra","relative_cost_preference":20},
        {"provider":"codex","model":"gpt-5.6-luna","relative_cost_preference":10}]);
    let output = validate(&proposal(&entries).to_string(), &base, &entries, "next", &costs).unwrap();
    assert_eq!(output["policy"]["entries"][0]["relative_cost_preference"],20);
    assert!(output["policy"]["entries"][1]["relative_cost_preference"].is_null());
    assert_eq!(output["policy"]["entries"][2]["relative_cost_preference"],10);
    let mut invented = proposal(&entries);
    invented["assignments"][0]["relative_cost_preference"] = json!(480);
    assert!(validate(&invented.to_string(), &base, &entries, "next", &costs).is_err());
}

#[test]
fn model_policy_worker_repairs_response_without_publishing_settings_or_touching_plan() {
    let f = QueueTest::new(false);
    let base = base();
    let details = discovery();
    let entries = candidates(&base, &details).unwrap();
    {
        let mut settings = f.app.app.settings.lock().unwrap();
        settings["mock_model_policy_output"] = json!(["{broken",{"assignments":[],"summary":"Incomplete"},proposal(&entries)]);
    }
    let settings_before = f.app.app.settings.lock().unwrap()["model_catalogue"].clone();
    f.app.ensure_forge_dir();
    std::fs::write(f.app.forge_path("plan.json"), "saved plan bytes").unwrap();
    f.app.session.state.lock().unwrap().model_policy_suggestion_serial = 1;
    f.app.acquire_busy().unwrap();
    f.app.model_policy_worker(base, details, entries, 1);
    let state = f.app.session.state.lock().unwrap();
    assert_eq!(state.model_policy_suggestion["status"], "ready");
    assert_eq!(state.model_policy_suggestion["policy"]["entries"].as_array().unwrap().len(), 3);
    assert!(state.model_policy_suggestion["warnings"].as_array().unwrap().len() >= 2);
    assert!(state.model_policy_suggestion["warnings"][0].as_str().unwrap().contains("claude"));
    assert!(!f.app.session.busy.load(Ordering::SeqCst));
    drop(state);
    let settings = f.app.app.settings.lock().unwrap();
    assert_eq!(settings["model_catalogue"], settings_before);
    let calls = settings["mock_agent_requests"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c["role"] == "model_policy" && c["session"].is_null()));
    assert!(calls[0]["prompt"].as_str().unwrap().contains("Select exactly four distinct models per provider"));
    assert_eq!(std::fs::read_to_string(f.app.forge_path("plan.json")).unwrap(), "saved plan bytes");
}

#[test]
fn model_policy_api_is_async_scoped_and_returns_draft_only_on_details_endpoint() {
    let first = QueueTest::new(false);
    let second = QueueTest::with_engine(false, Some(first.app.app.clone()));
    let base = base();
    first.app.app.settings.lock().unwrap()["mock_model_policy_output"] = proposal(&base.entries);
    let body = json!({"project":second.app.project,"policy":base});
    let (code, reply) = api_request(&first.app.app, "POST", "/api/models/suggest", body);
    assert_eq!(code, 202, "{reply}");
    wait_for_worker(&second.app);
    assert!(first.app.session.state.lock().unwrap().model_policy_suggestion.is_null());
    let (_, state) = api_request(&first.app.app, "GET", &format!("/api/state?project={}",second.app.project), json!({}));
    assert_eq!(state["model_policy_suggestion"]["status"], "ready");
    assert!(state["model_policy_suggestion"]["policy"].is_null());
    let (_, result) = api_request(&first.app.app, "GET", &format!("/api/models/suggestion?project={}",second.app.project), json!({}));
    assert_eq!(result["request_id"], reply["request_id"]);
    assert_eq!(result["policy"]["entries"].as_array().unwrap().len(), 2);
    assert!(first.app.app.settings.lock().unwrap()["model_catalogue"]["entries"].as_array().unwrap().is_empty());
}

#[test]
fn model_policy_api_rejects_busy_and_invalid_requests_and_worker_reports_failure() {
    let f = QueueTest::new(false);
    let body = json!({"policy":base()});
    f.app.acquire_busy().unwrap();
    assert_eq!(api_request(&f.app.app,"POST","/api/models/suggest",body.clone()).0,409);
    f.app.session.busy.store(false,Ordering::SeqCst);
    f.app.session.queue_active.store(true,Ordering::SeqCst);
    assert_eq!(api_request(&f.app.app,"POST","/api/models/suggest",body.clone()).0,409);
    f.app.session.queue_active.store(false,Ordering::SeqCst);
    assert_eq!(api_request(&f.app.app,"POST","/api/models/suggest",json!({"policy":{}})).0,400);
    assert_eq!(api_request(&f.app.app,"POST","/api/models/suggest",body).0,202);
    wait_for_worker(&f.app);
    let state = f.app.session.state.lock().unwrap();
    assert_eq!(state.model_policy_suggestion["status"],"failed");
    assert!(state.model_policy_suggestion["policy"].is_null());
    assert!(!f.app.session.busy.load(Ordering::SeqCst));
}

#[test]
fn saving_a_draft_cannot_overwrite_a_policy_changed_since_loading() {
    let f = QueueTest::new(false);
    let (_, loaded) = api_request(&f.app.app,"GET","/api/models",json!({}));
    let mut draft = loaded["policy"].clone();
    draft["policy_revision"] = json!("next");
    let body = json!({"model_catalogue":draft,"expected_model_policy":loaded["policy"]});
    assert_eq!(api_request(&f.app.app,"POST","/api/settings",body.clone()).0,200);
    assert_eq!(api_request(&f.app.app,"POST","/api/settings",body).0,409);
    assert_eq!(f.app.app.settings.lock().unwrap()["model_catalogue"]["policy_revision"],"next");
}

#[test]
fn future_families_can_replace_old_models_but_shortlist_is_four_per_provider() {
    let base = Policy::default();
    let details = json!({"providers":[
        {"provider":"codex","models":[{"id":"old-codex"},{"id":"future-comet"},
            {"id":"future-ocean"},{"id":"future-forest"},{"id":"future-sand"}]},
        {"provider":"claude","models":[{"id":"old-claude"},{"id":"future-a"},
            {"id":"future-b"},{"id":"future-c"},{"id":"future-d"}]}]});
    let entries = candidates(&base, &details).unwrap();
    let current: Vec<_> = entries.iter().filter(|e| !e.model.starts_with("old-")).cloned().collect();
    let reply = proposal(&current);
    let result = validate(&reply.to_string(), &base, &entries, "future", &json!([])).unwrap();
    let selected = result["policy"]["entries"].as_array().unwrap();
    assert_eq!(selected.len(), 8);
    assert!(selected.iter().all(|e| e["model"].as_str().unwrap().starts_with("future-")));
    assert!(validate(&proposal(&entries).to_string(), &base, &entries, "future", &json!([])).is_err());
    let mut incomplete = reply.clone();
    incomplete["assignments"].as_array_mut().unwrap().pop();
    assert!(validate(&incomplete.to_string(), &base, &entries, "future", &json!([])).is_err());
}

#[test]
fn aliases_count_once_and_known_ineligible_models_are_excluded() {
    let mut base = base();
    base.entries[1].model = "default".into();
    let details = json!({"providers":[{"provider":"claude","models":[
        {"id":"default","resolved_id":"claude-next-7[1m]"},
        {"id":"specific","resolved_id":"claude-next-7[1m]"},
        {"id":"claude-next-7","resolved_id":"claude-next-7"}]}],
        "options":[{"provider":"codex","model":"gpt-6-astra","eligible":false}]});
    let entries = candidates(&base, &details).unwrap();
    assert_eq!(entries.len(),1);
    assert_eq!(entries[0].model,"specific");
    assert!(entries.iter().all(|e| e.model != "default"));
}

#[test]
fn each_request_fetches_both_official_sources_and_does_not_label_errors_as_fresh() {
    use crate::metadata::{Fetch, FetchRequest, FetchResponse};
    struct Source { calls: std::sync::Mutex<Vec<String>>, bad: bool }
    impl Fetch for Source {
        fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse,String> {
            self.calls.lock().unwrap().push(request.url.clone());
            assert!(request.etag.is_none());
            Ok(FetchResponse {status:200, location:None, etag:None, last_modified:None,
                body:if self.bad { b"<html>bot challenge</html>".to_vec() }
                    else { b"<ModelDetails slug=\"future-model\" description=\"Current recommended model\" />".to_vec() }})
        }
    }
    let source = Source { calls:std::sync::Mutex::new(vec![]), bad:false };
    for _ in 0..2 {
        let result = official_sources_with(&base().entries,true,&source);
        assert_eq!(result.as_array().unwrap().len(),2);
        assert!(result.as_array().unwrap().iter().all(|s| s["status"] == "fresh"
            && s["document"].as_str().unwrap().contains("future-model") && s["checked_unix"].is_i64()));
    }
    assert_eq!(source.calls.lock().unwrap().len(),4);
    let result = official_sources_with(&base().entries,false,&source);
    assert!(result.as_array().unwrap().iter().all(|s|s["status"] == "unavailable"));
    assert_eq!(source.calls.lock().unwrap().len(),4);
    let bad = Source {calls:std::sync::Mutex::new(vec![]),bad:true};
    let result = official_sources_with(&base().entries,true,&bad);
    assert!(result.as_array().unwrap().iter().all(|s|s["status"] == "unavailable" && s["document"].is_null()));
}
