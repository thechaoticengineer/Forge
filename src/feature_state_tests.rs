//! Unit tests for the feature-spec runtime state module (M2): content
//! hashing, derived status, durable state round-trips and feature creation.
//! The end-to-end behaviour of the scenarios themselves is covered by the
//! business tests in src/feature_spec_m2_tests.rs.

use crate::feature_state::{
    Snapshot, append_plan_link, approved_scenario_ids, content_hash, create, default_state,
    latest_plan_link, load, plan_link_view, set_plan_link_status, snapshot, spec_status,
    state_path, update, validate_slug, validate_title,
};
use crate::test_support::{QueueTest, api_request};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "forge-feature-state-{label}-{}",
            crate::durable_json::identity()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn approval(kind: &str, hash: &str) -> Value {
    json!({"kind": kind, "commit": "0".repeat(40), "content_hash": hash, "unix": 1})
}

#[test]
fn content_hash_is_stable_and_sensitive_to_content_names_and_symlinks() {
    let temp = Temp::new("hash");
    let dir = temp.0.join("feature");
    write(&dir, "README.md", "# Title\n");
    write(&dir, "design/README.md", "designs\n");

    let base = content_hash(&dir).unwrap();
    assert_eq!(base.len(), 64, "the hash should be sha256 hex: {base}");
    assert_eq!(base, content_hash(&dir).unwrap(), "hashing must be stable");

    // An empty directory contributes nothing by itself.
    fs::create_dir_all(dir.join("empty")).unwrap();
    assert_eq!(base, content_hash(&dir).unwrap(), "a directory alone must not change the hash");

    write(&dir, "README.md", "# Title\n\nmore\n");
    let changed_content = content_hash(&dir).unwrap();
    assert_ne!(base, changed_content, "changed file content must change the hash");

    fs::rename(dir.join("README.md"), dir.join("READMEE.md")).unwrap();
    let renamed = content_hash(&dir).unwrap();
    assert_ne!(changed_content, renamed, "a renamed file must change the hash");
    fs::rename(dir.join("READMEE.md"), dir.join("README.md")).unwrap();
    assert_eq!(changed_content, content_hash(&dir).unwrap(), "renaming back must restore the hash");

    // Symlinks are hashed by their target text and never followed.
    let outside = temp.0.join("outside.md");
    fs::write(&outside, "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
    let with_link = content_hash(&dir).unwrap();
    assert_ne!(changed_content, with_link, "an added symlink must change the hash");
    fs::write(&outside, "outside, edited\n").unwrap();
    assert_eq!(
        with_link,
        content_hash(&dir).unwrap(),
        "editing a symlink's target outside the folder must not change the hash"
    );
    fs::remove_file(dir.join("link")).unwrap();
    std::os::unix::fs::symlink(temp.0.join("other.md"), dir.join("link")).unwrap();
    assert_ne!(with_link, content_hash(&dir).unwrap(), "a retargeted symlink must change the hash");
}

#[test]
fn spec_status_follows_the_latest_approvals_of_the_current_content() {
    let current = "a".repeat(64);
    let stale = "b".repeat(64);
    let mut state = default_state("demo");
    assert_eq!(spec_status(&state, &current), "draft");

    state["approvals"] = json!([approval("spec", &stale)]);
    assert_eq!(spec_status(&state, &current), "draft", "a stale spec approval leaves it draft");

    state["approvals"] = json!([approval("spec", &stale), approval("spec", &current)]);
    assert_eq!(spec_status(&state, &current), "spec approved");

    state["approvals"] = json!([
        approval("spec", &stale),
        approval("spec", &current),
        approval("scenarios", &current),
    ]);
    assert_eq!(spec_status(&state, &current), "scenarios approved");

    // A later edit reopens both approvals without deleting the history.
    let after_edit = "c".repeat(64);
    assert_eq!(spec_status(&state, &after_edit), "draft");
    assert_eq!(state["approvals"].as_array().unwrap().len(), 3, "history is append-only");

    // Only the latest approval of each kind counts.
    state["approvals"] = json!([approval("scenarios", &current), approval("spec", &stale)]);
    assert_eq!(spec_status(&state, &current), "draft");
}

#[test]
fn state_round_trips_through_durable_publication() {
    let test = QueueTest::new(false);
    let ctx = &test.app;

    // A missing file means the default, and reading it writes nothing.
    assert_eq!(load(ctx, "demo").unwrap(), default_state("demo"));
    assert!(!state_path(ctx, "demo").exists(), "a read must not create the state file");

    update(ctx, "demo", |state| {
        state["reviews"].as_array_mut().unwrap().push(json!({"id": "r1", "approved": true}));
        state["architect_session"] = json!("session-1");
        Ok(())
    })
    .unwrap();

    let path = state_path(ctx, "demo");
    assert_eq!(path, test.path.join(".forge/features/demo.json"));
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(raw, load(ctx, "demo").unwrap(), "what was published is what loads back");
    assert_eq!(raw["reviews"][0]["id"], "r1");
    assert_eq!(raw["architect_session"], "session-1");
    // publish_pretty leaves no temp file behind.
    let names: Vec<String> = fs::read_dir(test.path.join(".forge/features"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["demo.json".to_string()], "stray files were {names:?}");

    // Corrupt, mistyped or foreign state is an error, never a silent reset.
    for broken in [
        "{".to_string(),
        json!({"version": 2, "slug": "demo", "reviews": [], "approvals": [], "chat": []}).to_string(),
        json!({"version": 1, "slug": "other", "reviews": [], "approvals": [], "chat": []}).to_string(),
        json!({"version": 1, "slug": "demo", "reviews": {}, "approvals": [], "chat": []}).to_string(),
    ] {
        fs::write(&path, &broken).unwrap();
        let error = load(ctx, "demo").unwrap_err();
        assert!(error.contains("demo.json"), "error for {broken} was {error}");
    }
}

#[test]
fn snapshot_reports_status_and_review_currency_for_the_current_content() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let dir = test.path.join("docs/features/demo");
    write(&dir, "README.md", "# Demo\n");
    let hash = content_hash(&dir).unwrap();

    update(ctx, "demo", |state| {
        state["reviews"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "r1", "approved": true, "content_hash": hash}));
        state["approvals"].as_array_mut().unwrap().push(approval("spec", &hash));
        Ok(())
    })
    .unwrap();

    let Snapshot { content_hash: hashed, spec_status, latest_review, review_current, state } =
        snapshot(ctx, "demo").unwrap();
    assert_eq!(hashed, hash);
    assert_eq!(spec_status, "spec approved");
    assert_eq!(latest_review["id"], "r1");
    assert!(review_current);
    assert_eq!(state["approvals"].as_array().unwrap().len(), 1);

    write(&dir, "README.md", "# Demo\n\nedited\n");
    let after = snapshot(ctx, "demo").unwrap();
    assert_ne!(after.content_hash, hash);
    assert_eq!(after.spec_status, "draft");
    assert!(!after.review_current, "the recorded review no longer describes the folder");
}

#[test]
fn create_requires_a_real_template_and_leaves_no_partial_folder() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let features = test.path.join("docs/features");
    fs::create_dir_all(&features).unwrap();

    let refusal = create(ctx, "demo", "Demo").unwrap_err();
    assert_eq!(refusal.status, 409);
    assert_eq!(refusal.message, "docs/features/_template/ is missing");
    assert!(!features.join("demo").exists(), "a refused creation must not leave a folder");
    assert!(!state_path(ctx, "demo").exists(), "a refused creation must not write state");

    // A symlinked template is not a real directory.
    let elsewhere = test.path.join("template-source");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, features.join("_template")).unwrap();
    let refusal = create(ctx, "demo", "Demo").unwrap_err();
    assert_eq!(refusal.status, 409);
    assert_eq!(refusal.message, "docs/features/_template/ is missing");

    // A template holding a symlink is refused, and the partial copy removed.
    fs::remove_file(features.join("_template")).unwrap();
    let template = features.join("_template");
    write(&template, "README.md", "# <Feature name>\n");
    std::os::unix::fs::symlink(test.path.join("other.md"), template.join("link")).unwrap();
    let refusal = create(ctx, "demo", "Demo").unwrap_err();
    assert_eq!(refusal.status, 500);
    assert!(refusal.message.contains("symlink"), "message was {}", refusal.message);
    assert!(!features.join("demo").exists(), "a partial copy must be removed again");

    // The working template: the title replaces the first heading only.
    fs::remove_file(template.join("link")).unwrap();
    write(
        &template,
        "README.md",
        "# <Feature name>\n\n```md\n# not a heading\n```\n\n# second heading\n",
    );
    create(ctx, "demo", "  Demo Feature  ").unwrap();
    assert_eq!(
        fs::read_to_string(features.join("demo/README.md")).unwrap(),
        "# Demo Feature\n\n```md\n# not a heading\n```\n\n# second heading\n"
    );
    assert_eq!(load(ctx, "demo").unwrap(), default_state("demo"), "creation initializes state");

    // Creating it twice is refused, and the existing folder is untouched.
    let refusal = create(ctx, "demo", "Demo Feature").unwrap_err();
    assert_eq!(refusal.status, 409);
    assert!(refusal.message.contains("already exists"), "message was {}", refusal.message);
    assert!(features.join("demo/README.md").exists());
}

#[test]
fn slug_and_title_validation_match_the_documented_rules() {
    for slug in ["a", "feature-specs", "m2-step-3", &"a".repeat(64)] {
        assert!(validate_slug(slug).is_ok(), "{slug} should be valid");
    }
    for slug in ["", "-lead", "trail-", "two--dashes", "Upper", "with space", "under_score", &"a".repeat(65)] {
        assert!(validate_slug(slug).is_err(), "{slug:?} should be refused");
    }

    assert_eq!(validate_title("Two\nLines").unwrap_err(), "title must be a single line");
    assert_eq!(validate_title("  Spaced Title \n").unwrap(), "Spaced Title");
    assert_eq!(validate_title("   ").unwrap_err(), "title required");
    assert!(validate_title(&"t".repeat(200)).is_ok());
    assert!(validate_title(&"t".repeat(201)).is_err());
}

/// Percent-encodes a query value; temporary project paths are otherwise safe.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            },
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[test]
fn the_feature_state_endpoint_answers_reads_and_reports_a_corrupt_file() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let project = test.path.display().to_string();
    let dir = test.path.join("docs/features/demo");
    write(&dir, "README.md", "# Demo\n");
    write(&dir, "scenarios.md", "# Scenarios\n\n## S1: One\n\n- Given: a\n- When: b\n- Then: c\n");
    write(&dir, "decisions.md", "# Decisions\n");
    write(&dir, "milestones.md", "# Milestones\n\n## M1: One\n\nCovers: S1\n");
    let query = |slug: &str| {
        format!("/api/features/state?project={}&slug={}", encode(&project), encode(slug))
    };

    let (code, resp) = api_request(&ctx.app, "GET", &query("demo"), json!({}));
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["project"], project);
    assert_eq!(resp["slug"], "demo");
    assert_eq!(resp["valid"], true, "reasons were {}", resp["reasons"]);
    assert_eq!(resp["reasons"], json!([]));
    assert_eq!(resp["spec_status"], "draft");
    assert_eq!(resp["content_hash"], json!(content_hash(&dir).unwrap()));
    assert_eq!(resp["state"], default_state("demo"));
    assert_eq!(resp["activity"], Value::Null);

    let (code, resp) = api_request(&ctx.app, "GET", &query("Not Valid"), json!({}));
    assert_eq!(code, 400, "an invalid slug is refused, response was {resp}");
    let (code, resp) = api_request(&ctx.app, "GET", &query("missing"), json!({}));
    assert_eq!(code, 404, "an undiscovered slug is unknown, response was {resp}");

    // A corrupt state file is reported, never silently reset to the default.
    fs::create_dir_all(test.path.join(".forge/features")).unwrap();
    fs::write(state_path(ctx, "demo"), "{").unwrap();
    // A leftover temp file next to it is not state and must be ignored.
    fs::write(test.path.join(".forge/features/.partial.tmp"), "{").unwrap();
    let (code, resp) = api_request(&ctx.app, "GET", &query("demo"), json!({}));
    assert_eq!(code, 500, "response was {resp}");
    assert!(resp["error"].as_str().unwrap().contains("demo.json"), "response was {resp}");

    // The listing keeps the feature, with the reason instead of a status that
    // would look like ordinary unapproved work.
    let (code, resp) = api_request(
        &ctx.app,
        "GET",
        &format!("/api/features?project={}", encode(&project)),
        json!({}),
    );
    assert_eq!(code, 200, "response was {resp}");
    let listed = &resp["features"][0];
    assert_eq!(listed["slug"], "demo");
    assert_eq!(listed["status"], "valid", "M1 discovery is unaffected: {listed}");
    assert_eq!(listed["spec_status"], "error", "listed feature was {listed}");
    assert_eq!(listed["content_hash"], Value::Null);
    assert_eq!(listed["review_current"], false);
    assert!(listed["state_error"].as_str().unwrap().contains("demo.json"), "was {listed}");

    // Valid state again: the stray temp file still does not disturb the read.
    fs::write(state_path(ctx, "demo"), default_state("demo").to_string()).unwrap();
    let (code, resp) = api_request(&ctx.app, "GET", &query("demo"), json!({}));
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["state"], default_state("demo"));
}

#[test]
fn the_create_endpoint_validates_and_refuses_while_the_engine_is_busy() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let project = test.path.display().to_string();
    let features = test.path.join("docs/features");
    write(&features.join("_template"), "README.md", "# <Feature name>\n");
    let create = |body: Value| api_request(&ctx.app, "POST", "/api/features/create", body);

    let (code, resp) = create(json!({"project": project, "slug": "demo", "title": ""}));
    assert_eq!(code, 400, "an empty title is refused, response was {resp}");
    assert!(!features.join("demo").exists());

    ctx.session.busy.store(true, std::sync::atomic::Ordering::SeqCst);
    let (code, resp) = create(json!({"project": project, "slug": "demo", "title": "Demo"}));
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(resp["error"], "busy");
    assert!(!features.join("demo").exists(), "a refused creation writes nothing");

    ctx.session.busy.store(false, std::sync::atomic::Ordering::SeqCst);
    let (code, resp) = create(json!({"project": project, "slug": "demo", "title": "Demo"}));
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp, json!({"ok": true, "slug": "demo"}));
    assert_eq!(fs::read_to_string(features.join("demo/README.md")).unwrap(), "# Demo\n");
    assert!(state_path(ctx, "demo").exists(), "creation initializes durable state");
    // Nothing under docs/features/ holds runtime state.
    assert!(!features.join("demo/demo.json").exists());
}

#[test]
fn an_old_state_file_without_plans_loads_with_an_empty_list_and_is_not_rewritten() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    fs::create_dir_all(test.path.join(".forge/features")).unwrap();
    let old = json!({"version": 1, "slug": "demo", "reviews": [], "approvals": [approval("spec", "h")],
        "chat": [], "architect_session": Value::Null});
    let bytes = serde_json::to_vec_pretty(&old).unwrap();
    fs::write(state_path(ctx, "demo"), &bytes).unwrap();

    let state = load(ctx, "demo").unwrap();
    assert_eq!(state["plans"], json!([]), "a missing plans field defaults to an empty list");
    assert_eq!(state["approvals"], old["approvals"], "the rest of the file is kept");
    assert_eq!(fs::read(state_path(ctx, "demo")).unwrap(), bytes, "a read must not rewrite the file");
    assert_eq!(latest_plan_link(&state, "M1"), Value::Null);
    assert_eq!(plan_link_view(&state, "M1"), Value::Null);

    // A present plans field must be a list; anything else is an error.
    let mut broken = old.clone();
    broken["plans"] = json!({});
    fs::write(state_path(ctx, "demo"), broken.to_string()).unwrap();
    let error = load(ctx, "demo").unwrap_err();
    assert!(error.contains("plans must be an array"), "error was {error}");
}

#[test]
fn plan_links_move_from_planning_to_planned_or_failed_on_the_latest_link() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let feature = json!({"slug": "demo", "milestone": "M1", "title": "Alpha",
        "scenario_ids": ["S1", "S2"], "folder": "docs/features/demo/"});
    update(ctx, "demo", |state| {
        state["approvals"].as_array_mut().unwrap().push(approval("spec", "h"));
        Ok(())
    })
    .unwrap();

    // Settling a milestone that is not planning is reported, not invented.
    let error = set_plan_link_status(ctx, "demo", "M1", "planned", Some("p0"), None).unwrap_err();
    assert!(error.contains("M1"), "error was {error}");

    append_plan_link(ctx, &feature, "Implement M1").unwrap();
    let state = load(ctx, "demo").unwrap();
    let link = latest_plan_link(&state, "M1");
    assert_eq!(link["status"], "planning");
    assert_eq!(link["goal"], "Implement M1");
    assert_eq!(link["title"], "Alpha");
    assert_eq!(link["scenario_ids"], json!(["S1", "S2"]));
    assert_eq!(link["plan_id"], Value::Null);
    assert_eq!(link["completed_unix"], Value::Null);
    assert_eq!(link["commit_range"], Value::Null);
    assert!(link["started_unix"].as_i64().unwrap() > 0);
    assert_eq!(state["approvals"].as_array().unwrap().len(), 1, "approvals survive a link append");

    set_plan_link_status(ctx, "demo", "M1", "failed", None, Some("planner broke")).unwrap();
    append_plan_link(ctx, &feature, "Implement M1").unwrap();
    set_plan_link_status(ctx, "demo", "M1", "planned", Some("plan-2"), None).unwrap();

    let state = load(ctx, "demo").unwrap();
    let plans = state["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0]["status"], "failed");
    assert_eq!(plans[0]["error"], "planner broke");
    assert_eq!(plans[1]["status"], "planned");
    assert_eq!(plans[1]["plan_id"], "plan-2");
    assert!(plans[1].get("error").is_none());
    assert_eq!(latest_plan_link(&state, "M1"), plans[1]);
    let view = plan_link_view(&state, "M1");
    assert_eq!(view["status"], "planned");
    assert_eq!(view["plan_id"], "plan-2");
    assert_eq!(view["commit_range"], Value::Null);
    assert_eq!(latest_plan_link(&state, "M2"), Value::Null);
}

#[test]
fn approved_scenario_ids_come_from_the_latest_current_scenarios_approval() {
    let mut state = default_state("demo");
    assert_eq!(approved_scenario_ids(&state, "h"), None);
    let mut older = approval("scenarios", "h");
    older["scenario_ids"] = json!(["S1"]);
    let mut latest = approval("scenarios", "h");
    latest["scenario_ids"] = json!(["S1", "S2"]);
    state["approvals"] = json!([older, latest]);
    assert_eq!(approved_scenario_ids(&state, "h"), Some(vec!["S1".to_string(), "S2".to_string()]));
    assert_eq!(approved_scenario_ids(&state, "other"), None, "an approval of other content does not count");
}

#[test]
fn a_planner_supplied_feature_key_never_survives_in_a_goal_plan() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    ctx.app.settings.lock().unwrap()["mock_plan_output"] = json!({
        "goal": "Ship", "status": "draft",
        "feature": {"slug": "forged", "milestone": "M1", "title": "Forged", "scenario_ids": ["S1"]},
        "stages": [{"id": 1, "title": "One", "instructions": "Do one.", "acceptance": "Done.",
            "commit": "feat: one"}],
    });
    let (code, resp) = api_request(&ctx.app, "POST", "/api/plan", json!({"goal": "Ship"}));
    assert_eq!(code, 200, "response was {resp}");
    crate::test_support::wait_for_worker(ctx);
    let plan = ctx.load_plan().expect("the goal plan was published");
    assert!(plan.get("feature").is_none(), "plan was {plan}");
    assert!(!test.path.join(".forge/features").exists(), "a goal plan writes no feature state");
}

/// A scenarios-approved feature `demo` whose M1 covers S1 and S2, approved by
/// writing the approval records the way the approval endpoints record them.
fn approved_feature(test: &QueueTest) {
    let dir = test.path.join("docs/features/demo");
    write(&dir, "README.md", "# Demo\n\n## Goal\n\nDo it.\n\n## Scope\n\nIn.\n\n## Out of scope\n\nNone.\n\n\
        ## Behavior\n\nWorks.\n\n## Open questions\n\nNone.\n");
    write(&dir, "scenarios.md", "# Scenarios\n\n## S1: One\n\n- Given: a\n- When: b\n- Then: c\n\n\
        ## S2: Two\n\n- Given: d\n- When: e\n- Then: f\n");
    write(&dir, "decisions.md", "# Decisions\n");
    write(&dir, "milestones.md", "# Milestones\n\n## M1: Alpha\n\nStatus: planned\nCovers: S1, S2\n");
    let hash = content_hash(&dir).unwrap();
    let mut scenarios = approval("scenarios", &hash);
    scenarios["scenario_ids"] = json!(["S1", "S2"]);
    update(&test.app, "demo", |state| {
        state["approvals"] = json!([approval("spec", &hash), scenarios]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_failed_milestone_planning_marks_its_link_failed_and_releases_the_engine() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let project = test.path.display().to_string();
    approved_feature(&test);
    let plan_m1 = || {
        api_request(&ctx.app, "POST", "/api/features/plan",
            json!({"project": project, "slug": "demo", "milestone": "M1"}))
    };
    let listed_plan = || {
        let (code, resp) = api_request(&ctx.app, "GET", &format!("/api/features?project={}", encode(&project)), json!({}));
        assert_eq!(code, 200, "response was {resp}");
        resp["features"][0]["milestones"][0]["plan"].clone()
    };
    assert_eq!(listed_plan(), Value::Null, "an unplanned milestone has no plan");

    ctx.app.settings.lock().unwrap()["mock_plan_output"] = json!({"goal": "x", "stages": "not a list"});
    let (code, resp) = plan_m1();
    assert_eq!(code, 200, "response was {resp}");
    assert!(resp["goal"].as_str().unwrap().contains("docs/features/demo/"), "response was {resp}");
    crate::test_support::wait_for_worker(ctx);
    let link = latest_plan_link(&load(ctx, "demo").unwrap(), "M1");
    assert_eq!(link["status"], "failed", "link was {link}");
    assert!(link["error"].as_str().is_some_and(|e| !e.is_empty()), "link was {link}");
    assert_eq!(listed_plan()["status"], "failed");
    assert!(!ctx.session.busy.load(std::sync::atomic::Ordering::SeqCst), "planning failure releases the engine");

    ctx.app.settings.lock().unwrap()["mock_plan_output"] = json!({"goal": "x", "status": "draft",
        "stages": [{"id": 1, "title": "One", "instructions": "Write failing tests for S1, S2.",
            "acceptance": "Tests for S1, S2 fail.", "commit": "test: one"}]});
    let (code, resp) = plan_m1();
    assert_eq!(code, 200, "response was {resp}");
    crate::test_support::wait_for_worker(ctx);
    let plan = ctx.load_plan().unwrap();
    assert_eq!(plan["feature"], json!({"slug": "demo", "milestone": "M1", "title": "Alpha", "scenario_ids": ["S1", "S2"]}));
    let view = listed_plan();
    assert_eq!(view["status"], "planned", "plan was {view}");
    assert_eq!(view["plan_id"], plan["plan_id"]);
    assert_eq!(load(ctx, "demo").unwrap()["plans"].as_array().unwrap().len(), 2);

    // A manual edit keeps the engine-owned feature record.
    let stages = json!([{"id": 1, "title": "One edited", "instructions": "Write failing tests for S1, S2.",
        "acceptance": "Tests fail.", "commit": "test: one"}]);
    let (code, resp) = api_request(&ctx.app, "POST", "/api/plan/edit", json!({"plan": {"goal": plan["goal"], "stages": stages}}));
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(ctx.load_plan().unwrap()["feature"], plan["feature"], "an edit keeps the feature record");
}
