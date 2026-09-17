//! Unit and integration tests for the architect spec review (M2, S13/S14,
//! decision D9): what the verdict validator accepts, how an invalid verdict is
//! corrected on the same session inside the shared correction budget, and how
//! a ready plan architect session is resumed and its turn published.
//! The scenarios themselves are covered by the business tests in
//! src/feature_spec_m2_tests.rs.

use crate::app::feature_review::{MAX_ENTRY_BYTES, MAX_VERDICT_ENTRIES, validate_verdict};
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const SLUG: &str = "spec-one";
const HASH: &str = "a1b2c3";

fn identity() -> Value {
    json!({"slug": SLUG, "content_hash": HASH})
}

fn verdict(extra: Value) -> String {
    let mut value = json!({"slug": SLUG, "content_hash": HASH, "approved": true,
        "summary": "Consistent.", "issues": [], "questions": []});
    for (key, entry) in extra.as_object().unwrap() {
        value[key] = entry.clone();
    }
    value.to_string()
}

fn rejected(text: &str) -> String {
    validate_verdict(&identity(), text).expect_err(&format!("should be rejected: {text}"))
}

// ---------------------------------------------------------------- validator

#[test]
fn an_approving_verdict_of_the_reviewed_content_is_accepted() {
    let value = validate_verdict(&identity(), &verdict(json!({}))).unwrap();
    assert_eq!(
        value,
        json!({"approved": true, "summary": "Consistent.", "issues": [], "questions": []})
    );
}

#[test]
fn a_rejecting_verdict_keeps_its_issues_and_questions() {
    let text = verdict(json!({"approved": false, "summary": "Needs work.",
        "issues": ["missing rationale"], "questions": ["what about Y?"]}));
    let value = validate_verdict(&identity(), &text).unwrap();
    assert_eq!(value["approved"], false);
    assert_eq!(value["issues"], json!(["missing rationale"]));
    assert_eq!(value["questions"], json!(["what about Y?"]));
}

#[test]
fn a_verdict_about_another_feature_or_other_content_is_refused() {
    assert!(rejected(&verdict(json!({"slug": "other-feature"}))).contains("slug"));
    assert!(rejected(&verdict(json!({"content_hash": "beef"}))).contains("content_hash"));
    // An omitted identity is a mismatch too, never an implicit echo.
    assert!(rejected(r#"{"approved":true,"summary":"ok","issues":[],"questions":[]}"#).contains("slug"));
}

#[test]
fn an_inconsistent_verdict_is_refused() {
    assert!(
        rejected(&verdict(json!({"issues": ["still open"]})))
            .contains("approving verdict must leave issues and questions empty")
    );
    assert!(
        rejected(&verdict(json!({"questions": ["really?"]})))
            .contains("approving verdict must leave issues and questions empty")
    );
    assert!(
        rejected(&verdict(json!({"approved": false})))
            .contains("rejecting verdict needs at least one issue or question")
    );
}

#[test]
fn a_malformed_verdict_is_refused_with_the_reason() {
    assert!(rejected("not JSON at all").contains("invalid spec review JSON"));
    // Duplicate keys are never silently resolved to the last one.
    let duplicate = format!(
        r#"{{"slug":"{SLUG}","slug":"other","content_hash":"{HASH}","approved":true,"summary":"ok","issues":[],"questions":[]}}"#
    );
    assert!(rejected(&duplicate).contains("duplicate response key"));
    assert!(rejected(&verdict(json!({"stage_id": 1}))).contains("unknown response field `stage_id`"));
    assert!(rejected(&verdict(json!({"approved": "yes"}))).contains("`approved`"));
    assert!(rejected(&verdict(json!({"summary": "   "}))).contains("`summary`"));
    assert!(rejected(&verdict(json!({"approved": false, "issues": "missing"}))).contains("`issues`"));
    assert!(
        rejected(&verdict(json!({"approved": false, "issues": [""]})))
            .contains("every entry of `issues` must be a non-empty string")
    );
    assert!(
        rejected(&verdict(json!({"approved": false, "questions": [7]})))
            .contains("every entry of `questions` must be a non-empty string")
    );
}

#[test]
fn an_oversized_verdict_is_refused() {
    let many: Vec<String> = (0..=MAX_VERDICT_ENTRIES).map(|i| format!("issue {i}")).collect();
    assert!(rejected(&verdict(json!({"approved": false, "issues": many}))).contains("too many issues"));
    let long = "x".repeat(MAX_ENTRY_BYTES + 1);
    assert!(
        rejected(&verdict(json!({"approved": false, "issues": [long]})))
            .contains("an entry of `issues` is too long")
    );
    assert!(rejected(&"y".repeat(65 * 1024)).contains("verdict is too large"));
}

// ---------------------------------------------------------------- correction

/// A feature folder that passes M1 validation, so the review endpoint admits it.
fn write_feature(root: &Path, slug: &str) {
    let dir = root.join("docs/features").join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), format!("# {slug}\n\nGoal.\n")).unwrap();
    fs::write(
        dir.join("scenarios.md"),
        "# Scenarios\n\n## S1: One\n\n- Given: a start\n- When: it runs\n- Then: it works\n",
    )
    .unwrap();
    fs::write(dir.join("decisions.md"), "# Decisions\n\n## D1: One\n\nDecision: keep it.\n").unwrap();
    fs::write(dir.join("milestones.md"), "# Milestones\n\n## M1: One\n\nCovers: S1\n").unwrap();
}

fn review(test: &QueueTest, slug: &str) -> (u16, Value) {
    let project = test.path.display().to_string();
    let response = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    wait_for_worker(&test.app);
    response
}

fn requests(test: &QueueTest) -> Vec<Value> {
    test.app.app.settings.lock().unwrap()["mock_spec_review_requests"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn reviews(test: &QueueTest, slug: &str) -> Vec<Value> {
    crate::feature_state::load(&test.app, slug).unwrap()["reviews"]
        .as_array()
        .cloned()
        .unwrap()
}

#[test]
fn an_invalid_verdict_is_corrected_on_the_same_session_within_the_budget() {
    let test = QueueTest::new(false);
    write_feature(&test.path, SLUG);
    // Approval with an open issue, an unknown field, duplicate keys, then a
    // valid verdict: three rejections fit inside MAX_CORRECTIONS.
    test.app.app.settings.lock().unwrap()["mock_spec_review_output"] = json!([
        {"approved": true, "summary": "fine", "issues": ["but not really"], "questions": []},
        {"approved": false, "summary": "no", "issues": ["one"], "questions": [], "stage_id": 1},
        r#"{"approved":true,"approved":true,"summary":"ok","issues":[],"questions":[]}"#,
        Value::Null,
    ]);
    let (code, response) = review(&test, SLUG);
    assert_eq!(code, 200, "response was {response}");

    let requests = requests(&test);
    assert_eq!(requests.len(), 4, "one turn plus three corrections, got {requests:?}");
    for request in &requests[1..] {
        let prompt = request["prompt"].as_str().unwrap();
        assert!(prompt.contains("RESPONSE CORRECTION"), "{prompt}");
    }
    assert!(
        requests.windows(2).all(|pair| pair[0]["session"] == pair[1]["session"]),
        "corrections must continue the same architect session, got {requests:?}"
    );
    let reviews = reviews(&test, SLUG);
    assert_eq!(reviews.len(), 1, "only the corrected verdict is recorded, got {reviews:?}");
    assert_eq!(reviews[0]["approved"], true, "review was {}", reviews[0]);
    assert_eq!(reviews[0]["session"], requests[0]["session"]);
}

#[test]
fn an_exhausted_correction_budget_records_no_review() {
    let test = QueueTest::new(false);
    write_feature(&test.path, SLUG);
    test.app.app.settings.lock().unwrap()["mock_spec_review_output"] = json!("not JSON");
    let (code, response) = review(&test, SLUG);
    assert_eq!(code, 200, "response was {response}");

    assert_eq!(requests(&test).len(), 4, "one turn plus the full correction budget");
    assert!(reviews(&test, SLUG).is_empty(), "an unusable verdict is never recorded");
    let activity = crate::feature_state::activity(&test.app);
    assert_eq!(activity["status"], "failed", "activity was {activity}");
    assert_eq!(activity["kind"], "review", "activity was {activity}");
    assert!(!test.app.session.busy.load(std::sync::atomic::Ordering::SeqCst));
}

// ---------------------------------------------------------------- plan session

#[test]
fn a_ready_plan_architect_session_is_resumed_and_its_turn_published() {
    let test = QueueTest::new(false);
    write_feature(&test.path, SLUG);
    let candidate = json!({"goal":"keep contracts","status":"draft","stages":[
        {"id":1,"title":"API","instructions":"define interface","acceptance":"compatible API",
         "commit":"feat: api","status":"pending","depends_on":[]}]});
    let plan = test.app.architect_publish(candidate, None, "initial").unwrap();
    let cp = test.app.architecture_store().checkpoint(&plan).unwrap();
    assert_eq!(cp["context_status"], "ready");
    let plan_session = cp["session"]["reference"].as_str().unwrap().to_string();

    let (code, response) = review(&test, SLUG);
    assert_eq!(code, 200, "response was {response}");
    let requests = requests(&test);
    assert_eq!(requests.len(), 1, "requests were {requests:?}");
    assert_eq!(
        requests[0]["session"], json!(plan_session),
        "a ready plan architect session is continued, not duplicated"
    );

    let after = test.app.load_plan().unwrap();
    for key in ["goal", "stages", "revision", "plan_id"] {
        assert_eq!(after[key], plan[key], "the review must not change plan content");
    }
    let history = test.app.architecture_store().history(None, 0, 50).unwrap();
    let turn = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["payload"]["kind"] == "feature_spec_review")
        .unwrap_or_else(|| panic!("the turn must be recorded, history was {history}"));
    assert_eq!(turn["payload"]["slug"], SLUG);
    assert_eq!(turn["payload"]["verdict"]["approved"], true);
    assert_eq!(
        test.app.architecture_store().checkpoint(&after).unwrap()["last_turn"],
        turn["payload"]["turn"]
    );

    // The plan owns that session, so the feature does not claim it as its own.
    let state = crate::feature_state::load(&test.app, SLUG).unwrap();
    assert_eq!(state["architect_session"], Value::Null, "state was {state}");
    assert_eq!(state["reviews"].as_array().unwrap().len(), 1);
}
