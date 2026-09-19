//! The architect synthesis turn: completion trigger, session resume and fresh
//! fallback, failure isolation, one-time catch-up for blocked, reset and
//! abandoned plans, duplicate prevention, limits across many plans, and
//! survival of the runtime files across reset and archiving.
use super::*;
use crate::app::App;
use crate::test_support::{QueueTest, wait_for_worker};
use std::fs;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-synthesis-turn-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        for role in ["planner", "architect", "implementer", "reviewer"] { settings[role] = json!("mock"); }
        settings["test_fake_providers"] = json!(true);
        settings["auto_push"] = json!(false);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        for args in [vec!["init", "-q"], vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"], vec!["config", "commit.gpgsign", "false"]] {
            ctx.git(&args).unwrap();
        }
        fs::write(root.join("README.md"), "Fixture.\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }

    fn setting(&self, key: &str, value: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = value;
    }

    /// Publishes a new plan identity: its first architect turn.
    fn start(&self, goal: &str, stages: i64) -> Value {
        let stages: Vec<Value> = (1..=stages).map(|id| json!({"id":id,"title":format!("{goal} stage {id}"),
            "instructions":format!("Implement part {id} of {goal}"),"acceptance":"The part works.",
            "commit":format!("feat: part {id}"),"status":"pending","rounds":0})).collect();
        self.ctx.architect_publish(json!({"goal":goal,"status":"ready","stages":stages}), None, "draft").unwrap()
    }

    /// Runs a plan to done through the worker.
    fn complete(&self, goal: &str) -> Value {
        self.start(goal, 1);
        self.ctx.run_worker();
        let done = self.ctx.load_plan().unwrap();
        assert_eq!(done["status"], "done", "{}", self.ctx.read_history());
        done
    }

    /// Commits the first stage only and leaves the plan unfinished.
    fn commit_first_stage(&self, goal: &str) -> Value {
        self.start(goal, 2);
        self.ctx.run_single_stage_for_test(0).unwrap();
        let plan = self.ctx.load_plan().unwrap();
        assert_eq!(plan["stages"][0]["status"], "committed", "{}", self.ctx.read_history());
        plan
    }

    fn requests(&self) -> Vec<Value> {
        self.ctx.app.settings.lock().unwrap()["mock_synthesis_requests"].as_array().cloned().unwrap_or_default()
    }

    fn architect_prompts(&self) -> Vec<String> {
        self.ctx.app.settings.lock().unwrap()["mock_architect_requests"].as_array().into_iter().flatten()
            .map(|r| r["prompt"].as_str().unwrap().to_string()).collect()
    }

    fn synthesis(&self) -> Option<Value> { self.ctx.load_project_synthesis().unwrap() }

    fn synthesis_bytes(&self) -> Option<Vec<u8>> {
        fs::read(crate::architecture_synthesis::path(&self.ctx.forge_path(""))).ok()
    }

    fn history_texts(&self) -> Vec<String> {
        self.ctx.read_history().as_array().unwrap().iter()
            .filter_map(|e| e["text"].as_str().map(str::to_owned)).collect()
    }

    fn reset(&self) {
        let _guard = self.ctx.session.persistence_lock.lock().unwrap();
        self.ctx.architecture_store().reset().unwrap();
    }

    fn seed(&self, texts: &[&str]) -> Vec<u8> {
        let output = json!({"constraints": texts, "interfaces": [], "decisions": [], "retired": []});
        let source = json!({"plan_id": "seeded-plan", "checkpoint": null, "sha": "0123abcd", "goal": "seed", "unix": 1});
        self.ctx.save_project_synthesis(&crate::architecture_synthesis::build(&output, None, source).unwrap()).unwrap();
        self.synthesis_bytes().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}

fn entries(doc: &Value, kind: &str) -> Vec<String> {
    doc[kind].as_array().unwrap().iter().map(|e| e["text"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_done_plan_writes_the_synthesis_and_the_next_plan_sees_it() {
    let f = Fixture::new();
    let done = f.complete("Ship the greeting service");

    let doc = f.synthesis().expect("the completed plan must be synthesized");
    assert_eq!(doc["source"]["plan_id"], done["plan_id"]);
    assert_eq!(doc["source"]["checkpoint"], done["architecture"]["checkpoint"]);
    assert_eq!(doc["source"]["sha"], done["stages"][0]["sha"], "the source SHA is the plan's final committed stage");
    assert_eq!(doc["source"]["goal"], "Ship the greeting service");
    assert_eq!(entries(&doc, "constraints"), ["Delivered goal: Ship the greeting service"]);
    let requests = f.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["plan_id"], done["plan_id"]);
    let prompt = requests[0]["prompt"].as_str().unwrap();
    assert!(prompt.contains("Keep-or-retire rule") && prompt.contains("\"previous_synthesis\":null"));
    assert!(!prompt.contains("relevant_inputs") && !prompt.contains("review_gate"), "the synthesis input is the compact prompt view");
    let state: Value = serde_json::from_slice(&fs::read(state_path(&f.ctx.forge_path(""))).unwrap()).unwrap();
    assert_eq!(state["last_attempted_plan_id"], done["plan_id"]);

    // The visible step was shown during the turn and cleared afterwards;
    // the plan, phase and report are those of a normal completion.
    let history = f.history_texts();
    assert!(history.iter().any(|t| t.starts_with("synthesizing architecture from plan")), "{history:?}");
    assert_eq!(f.ctx.session.state.lock().unwrap().current_step, "");
    assert_eq!(f.ctx.session.state.lock().unwrap().phase, "done");
    assert_eq!(f.ctx.read_reports().as_array().unwrap().len(), 1);

    // The next plan's first architect turn carries the stored entries.
    f.start("Add a farewell endpoint", 1);
    let first = f.architect_prompts().last().unwrap().clone();
    assert!(first.contains("Delivered goal: Ship the greeting service"), "the next architect turn must see the synthesis");
    assert_eq!(f.requests().len(), 1, "the synthesized plan is not caught up again");
}

#[test]
fn the_completion_turn_runs_after_done_under_the_synthesizing_step() {
    let f = Fixture::new();
    let done = f.complete("Ship the greeting service");
    assert_eq!(f.requests()[0]["step"], STEP, "the turn is shown as its own step");
    // The turn read the plan as saved with status done: its source is the
    // done plan's own checkpoint.
    assert_eq!(f.synthesis().unwrap()["source"]["checkpoint"], done["architecture"]["checkpoint"]);
    // The completion message stays the run's last event, after the turn.
    let history = f.history_texts();
    let synthesized = history.iter().position(|t| t.starts_with("synthesis written from plan")).unwrap();
    assert!(history.last().unwrap().starts_with("all stages committed"), "{history:?}");
    assert!(synthesized < history.len() - 1);
}

#[test]
fn the_turn_resumes_the_architect_session_and_falls_back_to_a_fresh_one() {
    // Resume: the source plan's architect session is used.
    let f = Fixture::new();
    let done = f.complete("Ship the greeting service");
    let cp = f.ctx.architecture_store().checkpoint(&done).unwrap();
    let session = cp["session"]["reference"].as_str().unwrap().to_string();
    let requests = f.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["session"], json!(session), "the turn resumes the plan's architect session");

    // Fallback: the resume fails, and exactly one fresh session gets the
    // identical compact input.
    let f = Fixture::new();
    f.setting("mock_synthesis_errors", json!(["no conversation found with session ID"]));
    let done = f.complete("Ship the greeting service");
    let requests = f.requests();
    assert_eq!(requests.len(), 2, "one resume and one fresh attempt");
    assert!(requests[0]["session"].is_string());
    assert!(requests[1]["session"].is_null(), "the fallback starts a fresh session");
    assert_eq!(requests[0]["prompt"], requests[1]["prompt"], "the fresh session gets the same input");
    assert_eq!(f.synthesis().unwrap()["source"]["plan_id"], done["plan_id"]);
    assert!(f.history_texts().iter().any(|t| t.contains("could not resume the architect session")));
}

#[test]
fn a_checkpoint_that_needs_recovery_starts_a_fresh_session() {
    let f = Fixture::new();
    f.commit_first_stage("Ship the greeting service");
    // A checkpoint that needs reconstruction cannot be resumed.
    let plan = f.ctx.load_plan().unwrap();
    let mut cp = f.ctx.architecture_store().checkpoint(&plan).unwrap();
    cp["context_status"] = json!("needs_recovery");
    {
        let _guard = f.ctx.session.persistence_lock.lock().unwrap();
        f.ctx.architecture_store().publish(plan, cp, json!({"kind":"test_session_change"})).unwrap();
    }
    f.start("Next goal", 1);
    let requests = f.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]["session"].is_null(), "an unusable session is never resumed");
}

#[test]
fn a_failing_turn_keeps_the_old_synthesis_and_the_plan_completes() {
    for (key, value) in [("mock_synthesis_errors", json!(["provider exploded", "provider exploded again"])),
        ("mock_synthesis_output", json!("not json at all")),
        // A valid shape that drops the seeded entry without retiring it.
        ("mock_synthesis_output", json!({"constraints": ["Something else"], "interfaces": [], "decisions": [], "retired": []}))] {
        let f = Fixture::new();
        let before = f.seed(&["The service keeps one shared error type."]);
        f.setting(key, value.clone());
        let done = f.complete("Ship the greeting service");
        assert_eq!(f.synthesis_bytes().unwrap(), before, "{key}={value}: the previous synthesis must stay byte for byte");
        assert!(f.history_texts().iter().any(|t| t.starts_with("synthesis failed:") && t.ends_with("previous synthesis kept")),
            "{key}={value}: the failure is logged: {:?}", f.history_texts());
        assert_eq!(done["status"], "done");
        assert_eq!(f.ctx.session.state.lock().unwrap().phase, "done");
        assert_eq!(f.ctx.session.state.lock().unwrap().current_step, "");
        let report = f.ctx.read_reports()[0].clone();
        assert_eq!(report["plan_id"], done["plan_id"]);
        // The failed attempt is not repeated by the next plan's catch-up.
        let calls = f.requests().len();
        f.start("Next goal", 1);
        assert_eq!(f.requests().len(), calls, "{key}={value}: a failed attempt is not retried");
    }
}

#[test]
fn an_invalid_output_gets_one_correction() {
    let f = Fixture::new();
    f.setting("mock_synthesis_output", json!(["not json", null]));
    let done = f.complete("Ship the greeting service");
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["prompt"].as_str().unwrap().contains("RESPONSE CORRECTION"));
    assert_eq!(requests[1]["session"], requests[0]["session"], "the correction continues the same session");
    assert_eq!(f.synthesis().unwrap()["source"]["plan_id"], done["plan_id"]);
}

#[test]
fn a_failing_turn_does_not_stop_the_queue() {
    let test = QueueTest::new(true);
    test.app.app.settings.lock().unwrap()["mock_synthesis_errors"] = json!(["provider exploded", "provider exploded again"]);
    test.start();
    wait_for_worker(&test.app);
    assert!(test.app.load_queue()["items"].as_array().unwrap().is_empty(), "both goals complete: {}", test.app.read_history());
    let settings = test.app.app.settings.lock().unwrap().clone();
    let requests = settings["mock_synthesis_requests"].as_array().unwrap();
    // The first goal's failed attempt (resume and fresh) is not caught up by
    // the second goal, whose own completion writes the synthesis.
    assert_eq!(requests.len(), 3, "{requests:?}");
    let doc = test.app.load_project_synthesis().unwrap().unwrap();
    let second = test.app.load_plan().unwrap();
    assert_eq!(second["goal"], "second goal");
    assert_eq!(doc["source"]["plan_id"], second["plan_id"]);
    assert_eq!(requests[2]["plan_id"], second["plan_id"]);
}

#[test]
fn a_blocked_plan_with_committed_stages_is_caught_up_once() {
    let f = Fixture::new();
    let blocked = f.commit_first_stage("Ship the greeting service");
    assert!(f.requests().is_empty(), "an unfinished plan has no completion turn");

    f.start("Add a farewell endpoint", 1);
    let requests = f.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["plan_id"], blocked["plan_id"]);
    let doc = f.synthesis().unwrap();
    assert_eq!(doc["source"]["plan_id"], blocked["plan_id"]);
    assert_eq!(doc["source"]["sha"], blocked["stages"][0]["sha"]);
    let first = f.architect_prompts().last().unwrap().clone();
    assert!(first.contains("Delivered goal: Ship the greeting service"), "the new plan's first turn sees the caught-up synthesis");

    // The second plan is abandoned without committed stages; a third plan
    // neither repeats the first catch-up nor synthesizes the empty plan.
    f.start("A third goal", 1);
    assert_eq!(f.requests().len(), 1, "no duplicate or empty-plan synthesis");
}

#[test]
fn a_reset_plan_is_caught_up_from_its_archived_checkpoint() {
    let f = Fixture::new();
    let reset = f.commit_first_stage("Ship the greeting service");
    f.reset();
    assert!(f.ctx.load_plan().is_none());
    let pointer: Value = serde_json::from_slice(&fs::read(f.ctx.forge_path("architecture").join("previous-plan.json")).unwrap()).unwrap();
    assert_eq!(pointer["plan_id"], reset["plan_id"]);
    assert_eq!(pointer["checkpoint"], reset["architecture"]["checkpoint"]);

    f.start("Add a farewell endpoint", 1);
    let requests = f.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["plan_id"], reset["plan_id"]);
    let doc = f.synthesis().unwrap();
    assert_eq!(doc["source"]["plan_id"], reset["plan_id"]);
    assert_eq!(doc["source"]["checkpoint"], reset["architecture"]["checkpoint"]);

    // Resetting the new, uncommitted plan and starting another one does not
    // repeat the catch-up.
    f.reset();
    f.start("A third goal", 1);
    assert_eq!(f.requests().len(), 1);
}

#[test]
fn a_plan_without_committed_stages_is_never_synthesized() {
    let f = Fixture::new();
    f.start("Ship the greeting service", 2);
    f.start("Add a farewell endpoint", 1);
    f.reset();
    f.start("A third goal", 1);
    assert!(f.requests().is_empty());
    assert!(f.synthesis().is_none());
    assert!(!state_path(&f.ctx.forge_path("")).exists(), "no attempt is recorded");
}

#[test]
fn no_duplicate_turn_after_a_successful_completion_turn() {
    let f = Fixture::new();
    let done = f.complete("Ship the greeting service");
    assert_eq!(f.requests().len(), 1);
    // Running the completion trigger again, or catching up from the next plan,
    // never repeats the turn, even without the attempt marker.
    fs::remove_file(state_path(&f.ctx.forge_path(""))).unwrap();
    f.ctx.synthesize_completed_plan();
    f.start("Add a farewell endpoint", 1);
    assert_eq!(f.requests().len(), 1);
    assert_eq!(f.synthesis().unwrap()["source"]["plan_id"], done["plan_id"]);
}

#[test]
fn every_stored_synthesis_stays_within_limits_across_many_plans() {
    use crate::architecture_synthesis::{MAX_BYTES, MAX_CONSTRAINTS, MAX_DECISIONS, MAX_ENTRY_BYTES, MAX_INTERFACES};
    let f = Fixture::new();
    let long = |kind: &str, plan: usize, n: usize| format!("{kind} {n} of plan {plan}: {}", "keep the handlers on one shared error type ".repeat(5));
    // Start close to the limits, so keeping every entry and adding one more
    // forces the merge to retire the oldest entries.
    let seeded: Vec<String> = (1..=22).map(|n| long("seeded constraint", 0, n)).collect();
    f.seed(&seeded.iter().map(String::as_str).collect::<Vec<_>>());
    let mut previous = f.synthesis();
    let mut retired = 0;
    for plan in 1..=6 {
        // Each plan's output keeps every old entry verbatim and tries to add
        // many long entries; the first output breaks the limits and is
        // corrected once by the default merge, which retires the oldest.
        let old = previous.clone();
        let keep = |kind: &str| old.as_ref().map(|d| entries(d, kind)).unwrap_or_default();
        let grow = |kind: &str, count: usize| {
            let mut all = keep(kind);
            all.extend((1..=count).map(|n| long(kind, plan, n)));
            all
        };
        f.setting("mock_synthesis_output", json!([
            {"constraints": grow("constraints", 20), "interfaces": grow("interfaces", 20), "decisions": grow("decisions", 12), "retired": []},
            null]));
        f.complete(&format!("Goal number {plan}"));
        let doc = f.synthesis().expect("every plan is synthesized");
        let bytes = serde_json::to_vec_pretty(&doc).unwrap();
        assert!(bytes.len() <= MAX_BYTES, "plan {plan}: {} bytes", bytes.len());
        assert!(doc["constraints"].as_array().unwrap().len() <= MAX_CONSTRAINTS);
        assert!(doc["interfaces"].as_array().unwrap().len() <= MAX_INTERFACES);
        assert!(doc["decisions"].as_array().unwrap().len() <= MAX_DECISIONS);
        for kind in ["constraints", "interfaces", "decisions"] {
            assert!(entries(&doc, kind).iter().all(|t| t.len() <= MAX_ENTRY_BYTES));
        }
        crate::architecture_synthesis::validate(&doc, previous.as_ref()).expect("keep-or-retire holds against the previous synthesis");
        assert!(entries(&doc, "constraints").contains(&format!("Delivered goal: Goal number {plan}")));
        retired += doc["retired"].as_array().unwrap().len();
        previous = Some(doc);
    }
    assert!(retired > 0, "the limits forced retirements");
    assert_eq!(f.requests().len(), 12, "each oversized output was corrected once");
    assert_eq!(f.history_texts().iter().filter(|t| t.starts_with("synthesis written from plan")).count(), 6);
}

#[test]
fn the_synthesis_and_its_attempt_marker_survive_reset_and_archiving() {
    let f = Fixture::new();
    f.complete("Ship the greeting service");
    let root = f.ctx.forge_path("");
    let synthesis = f.synthesis_bytes().unwrap();
    let marker = fs::read(state_path(&root)).unwrap();

    // Publishing a new plan identity archives the done plan.
    let next = f.start("Add a farewell endpoint", 1);
    assert_ne!(next["plan_id"], json!(null));
    assert_eq!(f.synthesis_bytes().unwrap(), synthesis, "archiving keeps the synthesis");
    assert_eq!(fs::read(state_path(&root)).unwrap(), marker, "archiving keeps the attempt marker");

    f.reset();
    assert_eq!(f.synthesis_bytes().unwrap(), synthesis, "reset keeps the synthesis");
    assert_eq!(fs::read(state_path(&root)).unwrap(), marker, "reset keeps the attempt marker");
    assert!(!crate::durable_json::safe_id(STATE_FILE), "the marker can never be a plan directory");
}
