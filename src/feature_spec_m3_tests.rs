//! Business tests for milestone M3 (feature to plans) scenarios S27-S35.
//! See docs/features/feature-specs/scenarios.md for the full scenario text
//! and docs/features/feature-specs/decisions.md (D5, D7, D13-D16) for the
//! contract.
//!
//! M3 is not implemented yet: every test here exercises an endpoint, a runtime
//! state field or a prompt section that does not exist. They compile against
//! only the existing test surface (crate::test_support, temporary git
//! repositories, the mock providers and the settings hooks the engine already
//! records) and are expected to fail on their assertions until later stages of
//! the M3 plan implement the contract.
//!
//! Prompt contract used below. Prompts carry these section headings whenever
//! the section applies, and never when it does not:
//! - `FEATURE REFERENCE`: compact reference (feature folder, milestone ID and
//!   title, covered scenario IDs, and for the planner the failing-tests-first
//!   and final-stage rules) in the planner and reviewer prompts of a milestone
//!   plan. The architect context carries the same facts without a heading.
//! - `REGISTERED BUSINESS TESTS`: the business test files registered by every
//!   feature's `Business tests:` lines, in reviewer prompts of every plan.
//! - `CHANGED BUSINESS TESTS`: the registered files a stage or plan-fix diff
//!   modifies, deletes or renames, in reviewer prompts.

use crate::app::Ctx;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

const SLUG: &str = "demo-feature";
const FOLDER: &str = "docs/features/demo-feature/";
/// A sentence that only appears in the body of scenario S1, so its presence in a
/// prompt proves that feature file contents were copied into it.
const MARKER: &str = "ZEBRA-MARKER-7431-unique-scenario-body-sentence";
const FEATURE_REFERENCE: &str = "FEATURE REFERENCE";
const REGISTRY: &str = "REGISTERED BUSINESS TESTS";
const CHANGED: &str = "CHANGED BUSINESS TESTS";
const ALPHA: &str = "tests/alpha_business.rs";
const BETA: &str = "tests/beta_business.mjs";
/// A registry line in the first form of D16 (paths with notes, joined by `and`).
const ALPHA_REGISTRY: &str = "tests/alpha_business.rs (S3)";

// ------------------------------------------------------------------ fixtures

fn template_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/features/_template")
}

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// Recursive (relative path, bytes) snapshot, used to prove no file under
/// `dir` was created, modified or removed. Empty when `dir` does not exist.
fn snapshot_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push((path.strip_prefix(base).unwrap().to_path_buf(), fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, dir, &mut out);
    }
    out
}

/// `scenarios.md` with three scenarios. `padding` extra bytes of narrative are
/// added to S1, to prove that prompts do not grow with the size of the spec.
fn scenarios_md(padding: usize) -> String {
    let filler = "Additional narrative detail that only makes the file larger. ".repeat(padding / 60 + 1);
    let filler = if padding == 0 { String::new() } else { format!("\n{filler}\n") };
    format!(
        "# Scenarios\n\n\
        ## S1: First scenario\n\n\
        - Given: {MARKER}\n- When: something happens\n- Then: an outcome follows\n{filler}\n\
        ## S2: Second scenario\n\n\
        - Given: another starting point\n- When: something else happens\n- Then: another outcome follows\n\n\
        ## S3: Third scenario\n\n\
        - Given: a third starting point\n- When: a third thing happens\n- Then: a third outcome follows\n"
    )
}

/// `milestones.md`: M1 is the planned milestone under test, M2 is implemented
/// (and carries `registry` as its `Business tests:` line), M3 covers nothing yet.
fn milestones_md(registry: Option<&str>) -> String {
    let registry = registry.map(|line| format!("\nBusiness tests: {line}\n")).unwrap_or_default();
    format!(
        "# Milestones\n\n\
        ## M1: Alpha milestone\n\nStatus: planned\nCovers: S1, S2\n\n\
        ## M2: Finished milestone\n\nStatus: implemented\nCovers: S3\n{registry}\n\
        ## M3: Later milestone\n\nStatus: planned\nCovers: none yet\n"
    )
}

fn readme(title: &str) -> String {
    format!(
        "# {title}\n\n## Goal\n\nDo the thing.\n\n## Scope\n\nIn scope.\n\n## Out of scope\n\nNothing yet.\n\n\
        ## Behavior\n\nWorks as expected.\n\n## Open questions\n\nNone.\n"
    )
}

/// A plan the mock planner returns for the goal `M1` (S1, S2): the first stage
/// names `first_stage_ids`, the last stage carries the milestone completion.
fn milestone_plan(first_stage_ids: &[&str]) -> Value {
    let ids = first_stage_ids.join(", ");
    json!({"goal": "planner goal", "status": "draft", "stages": [
        {"id": 1, "title": "Failing business tests",
         "instructions": format!("Write executable tests named after {ids} that fail before implementation."),
         "acceptance": format!("Executable tests for {ids} exist and fail before implementation."),
         "commit": "test: add failing business tests"},
        {"id": 2, "title": "Implement milestone",
         "instructions": "Implement the milestone until its tests pass, then set the milestone Status: implemented and its Business tests: line.",
         "acceptance": "The business tests pass and the milestone is marked implemented.",
         "commit": "feat: implement milestone"},
    ]})
}

/// A two-stage plan for goal, refactor, discussion and queue planning.
fn goal_plan() -> Value {
    json!({"goal": "Ship the thing", "status": "draft", "stages": [
        {"id": 1, "title": "First step", "instructions": "Add the first part.",
         "acceptance": "The first part works.", "commit": "feat: first part"},
        {"id": 2, "title": "Second step", "instructions": "Add the second part.",
         "acceptance": "The second part works.", "commit": "feat: second part"},
    ]})
}

struct Env {
    test: QueueTest,
    project: String,
}

impl Env {
    fn new() -> Self {
        let test = QueueTest::new(false);
        let project = test.path.display().to_string();
        Self { test, project }
    }

    /// A project with the demo feature, its two registered-able business test
    /// files committed, and the feature's spec and scenarios approved.
    fn ready() -> Self {
        Self::ready_with(0, None)
    }

    fn ready_with(padding: usize, registry: Option<&str>) -> Self {
        let env = Self::draft(padding, registry);
        env.approve(SLUG);
        env
    }

    /// Same project, but the feature stays a draft.
    fn draft(padding: usize, registry: Option<&str>) -> Self {
        let env = Self::new();
        env.commit_files(&[(ALPHA, "// alpha business test\n"), (BETA, "// beta business test\n")]);
        env.write_feature(SLUG, "Demo Feature", &scenarios_md(padding), &milestones_md(registry));
        env
    }

    fn ctx(&self) -> &Ctx {
        &self.test.app
    }

    fn features_dir(&self) -> PathBuf {
        self.test.path.join("docs/features")
    }

    fn write_feature(&self, slug: &str, title: &str, scenarios: &str, milestones: &str) {
        let dir = self.features_dir().join(slug);
        copy_dir_all(&template_root(), &dir);
        fs::write(dir.join("README.md"), readme(title)).unwrap();
        fs::write(dir.join("scenarios.md"), scenarios).unwrap();
        fs::write(dir.join("decisions.md"), "## D1: A decision\n\nDecided.\n").unwrap();
        fs::write(dir.join("milestones.md"), milestones).unwrap();
    }

    fn commit_files(&self, files: &[(&str, &str)]) {
        for (path, content) in files {
            let full = self.test.path.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        let mut args = vec!["add", "--"];
        args.extend(files.iter().map(|(path, _)| *path));
        self.ctx().git(&args).unwrap();
        self.ctx().git(&["commit", "-qm", "add business tests"]).unwrap();
    }

    fn post(&self, path: &str, body: Value) -> (u16, Value) {
        api_request(&self.ctx().app, "POST", path, body)
    }

    fn get(&self, path: &str) -> (u16, Value) {
        api_request(&self.ctx().app, "GET", path, json!({}))
    }

    /// Architect review, spec approval and scenario approval of `slug`.
    fn approve(&self, slug: &str) {
        self.approve_spec(slug);
        let (code, resp) = self.post("/api/features/approve_scenarios", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: approving scenarios of {slug} failed: {resp}");
    }

    fn approve_spec(&self, slug: &str) {
        let (code, resp) = self.post("/api/features/review", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: reviewing {slug} failed: {resp}");
        wait_for_worker(self.ctx());
        let (code, resp) = self.post("/api/features/approve_spec", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: approving the spec of {slug} failed: {resp}");
    }

    fn set(&self, key: &str, value: Value) {
        self.ctx().app.settings.lock().unwrap()[key] = value;
    }

    fn setting(&self, key: &str) -> Value {
        self.ctx().app.settings.lock().unwrap()[key].clone()
    }

    fn per_plan(&self) {
        self.set("review_cadence", json!({"architect": "per_plan", "reviewer": "per_plan"}));
    }

    fn state_path(&self, slug: &str) -> PathBuf {
        self.test.path.join(".forge/features").join(format!("{slug}.json"))
    }

    fn state_bytes(&self, slug: &str) -> Option<Vec<u8>> {
        fs::read(self.state_path(slug)).ok()
    }

    fn state(&self, slug: &str) -> Value {
        let bytes = fs::read(self.state_path(slug)).unwrap_or_else(|e| panic!("no feature state for {slug}: {e}"));
        serde_json::from_slice(&bytes).unwrap()
    }

    fn plan_bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.test.path.join(".forge/plan.json")).ok()
    }

    fn plan_file(&self) -> Value {
        let bytes = self.plan_bytes().expect("no .forge/plan.json was written");
        serde_json::from_slice(&bytes).unwrap()
    }

    fn head(&self) -> String {
        self.ctx().git(&["rev-parse", "HEAD"]).unwrap()
    }

    fn plan_milestone(&self, slug: &str, milestone: &str) -> (u16, Value) {
        self.post("/api/features/plan", json!({"project": self.project, "slug": slug, "milestone": milestone}))
    }

    /// Starts milestone planning of `M1` with the given mock planner output and
    /// waits for the planner to finish. Asserts the request was accepted.
    fn plan_m1(&self, output: Value) {
        self.set("mock_plan_output", output);
        let (code, resp) = self.plan_milestone(SLUG, "M1");
        assert_eq!(code, 200, "setup: POST /api/features/plan should start planning M1: {resp}");
        wait_for_worker(self.ctx());
    }

    fn start_goal_plan(&self, body: Value) {
        self.set("mock_plan_output", goal_plan());
        let (code, resp) = self.post("/api/plan", body);
        assert_eq!(code, 200, "setup: POST /api/plan failed: {resp}");
        wait_for_worker(self.ctx());
    }

    fn approve_and_run(&self) {
        let (code, resp) = self.post("/api/approve", json!({}));
        assert_eq!(code, 200, "setup: approving the plan failed: {resp}");
        let (code, resp) = self.post("/api/run", json!({}));
        assert_eq!(code, 200, "setup: running the plan failed: {resp}");
        wait_for_worker(self.ctx());
    }

    /// Approves the queue's first planned goal and lets its worker run it.
    fn approve_and_run_queue_goal(&self) {
        let (code, resp) = self.post("/api/approve", json!({}));
        assert_eq!(code, 200, "setup: approving the queued plan failed: {resp}");
        wait_for_worker(self.ctx());
    }

    fn run_goal_plan(&self) {
        self.start_goal_plan(json!({"goal": "Ship the thing"}));
        self.approve_and_run();
    }

    fn planner_prompts(&self) -> Vec<String> {
        self.agent_prompts("planner")
    }

    fn agent_prompts(&self, role: &str) -> Vec<String> {
        self.setting("mock_agent_requests")
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["role"] == role)
            .map(|r| r["prompt"].as_str().unwrap_or("").to_string())
            .collect()
    }

    fn architect_prompts(&self) -> Vec<String> {
        self.setting("mock_architect_requests")
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| r["prompt"].as_str().unwrap_or("").to_string())
            .collect()
    }

    /// Prompts of every review invocation of `role` ("reviewer" or "architect").
    fn review_prompts(&self, role: &str) -> Vec<String> {
        self.setting("test_review_sessions")
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["role"] == role)
            .map(|r| r["prompt"].as_str().unwrap_or("").to_string())
            .collect()
    }

    fn stage_review_prompts(&self, role: &str) -> Vec<String> {
        self.review_prompts(role).into_iter().filter(|p| !is_plan_scope(p)).collect()
    }

    fn plan_review_prompts(&self, role: &str) -> Vec<String> {
        self.review_prompts(role).into_iter().filter(|p| is_plan_scope(p)).collect()
    }

    fn feature_state_snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        snapshot_dir(&self.test.path.join(".forge/features"))
    }

    fn statuses_outside_forge(&self) -> Vec<String> {
        self.ctx()
            .git(&["status", "--porcelain"])
            .unwrap()
            .lines()
            .filter(|line| !line.get(3..).unwrap_or("").starts_with(".forge"))
            .map(str::to_string)
            .collect()
    }
}

fn is_plan_scope(prompt: &str) -> bool {
    prompt.contains("\"scope\":\"plan\"")
}

/// The part of `prompt` from `heading` on, or a panic naming the missing heading.
fn section<'a>(prompt: &'a str, heading: &str) -> &'a str {
    let start = prompt
        .find(heading)
        .unwrap_or_else(|| panic!("prompt has no {heading:?} section; it starts with {:?}", &prompt[..prompt.len().min(300)]));
    &prompt[start..]
}

fn assert_all_contain(prompts: &[String], what: &str, needles: &[&str]) {
    assert!(!prompts.is_empty(), "no {what} prompts were recorded");
    for prompt in prompts {
        for needle in needles {
            assert!(prompt.contains(needle), "a {what} prompt lacks {needle:?}; it starts with {:?}", &prompt[..prompt.len().min(400)]);
        }
    }
}

fn assert_none_contain(prompts: &[String], what: &str, needles: &[&str]) {
    for prompt in prompts {
        for needle in needles {
            assert!(!prompt.contains(needle), "a {what} prompt must not contain {needle:?}");
        }
    }
}

// ----------------------------------------------------------------------- S27

#[test]
fn s27_milestone_of_approved_feature_starts_planning_and_records_link() {
    // S27: A milestone of an approved feature is planned from the panel
    let env = Env::ready();
    env.plan_m1(milestone_plan(&["S1", "S2"]));

    let plan = env.plan_file();
    assert_eq!(
        plan["feature"],
        json!({"slug": SLUG, "milestone": "M1", "title": "Alpha milestone", "scenario_ids": ["S1", "S2"]}),
        "plan.json must record the feature reference, plan was {plan}"
    );
    let goal = plan["goal"].as_str().unwrap_or("").to_string();
    assert!(goal.contains("docs/features/demo-feature"), "the engine-built goal names the feature folder: {goal:?}");
    assert!(goal.contains("M1"), "the engine-built goal names the milestone: {goal:?}");
    assert!(
        env.setting("mock_planner_prompt").as_str().unwrap_or("").contains(&goal),
        "the planner must have been given the engine-built goal"
    );

    let state = env.state(SLUG);
    let plans = state["plans"].as_array().unwrap_or_else(|| panic!("state has no plans array: {state}"));
    assert_eq!(plans.len(), 1, "plans were {plans:?}");
    assert_eq!(plans[0]["milestone"], "M1", "link was {}", plans[0]);
    assert_eq!(plans[0]["goal"], json!(goal), "link was {}", plans[0]);
    assert!(plans[0]["started_unix"].as_i64().unwrap_or(0) > 0, "link was {}", plans[0]);
    assert_eq!(plans[0]["status"], "planned", "link was {}", plans[0]);
    assert!(
        state["approvals"].as_array().is_some_and(|a| a.len() >= 2),
        "approvals must survive, state was {state}"
    );

    // Planning is the end of this request: nothing is executed automatically.
    assert!(env.agent_prompts("implementer").is_empty(), "planning must not start execution");
    assert_ne!(plan["status"], "approved", "planning must not approve the plan");
}

#[test]
fn s27_old_state_file_without_plans_is_readable_and_not_rewritten() {
    // S27: Keep old state files readable, with missing fields defaulted
    let env = Env::ready();
    // A real M2-era state file has no `plans` key: strip it from the file on disk.
    let mut legacy = env.state(SLUG);
    legacy.as_object_mut().unwrap().remove("plans");
    fs::write(env.state_path(SLUG), serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let before = env.state_bytes(SLUG).expect("approval wrote the feature state");
    let parsed: Value = serde_json::from_slice(&before).unwrap();
    assert!(parsed.get("plans").is_none(), "an M2 state file has no plans field: {parsed}");

    let (code, state) = env.get(&format!("/api/features/state?project={}&slug={SLUG}", env.project));
    assert_eq!(code, 200, "response was {state}");
    assert_eq!(state["state"]["plans"], json!([]), "a missing plans field defaults to an empty array: {state}");
    assert_eq!(env.state_bytes(SLUG).unwrap(), before, "reading must not rewrite the state file");
}

// ----------------------------------------------------------------------- S28

/// Requests planning of `milestone` and asserts a refusal with a reason that
/// mentions one of `needles`, that no agent ran, and that neither the plan file,
/// the feature state nor the repository changed.
fn assert_refused(env: &Env, slug: &str, milestone: &str, needles: &[&str]) {
    let plan_before = env.plan_bytes();
    let state_before = env.state_bytes(slug);
    let features_before = env.feature_state_snapshot();
    let head_before = env.head();
    let agents_before = env.setting("mock_agent_requests");
    let architects_before = env.setting("mock_architect_requests");
    let busy_before = env.ctx().session.busy.load(Ordering::SeqCst);
    let queue_before = env.ctx().session.queue_active.load(Ordering::SeqCst);

    let (code, resp) = env.plan_milestone(slug, milestone);
    assert_ne!(code, 200, "planning {slug} {milestone} must be refused, response was {resp}");
    let reason = resp["error"].as_str().unwrap_or("").to_lowercase();
    assert!(
        needles.iter().any(|n| reason.contains(&n.to_lowercase())),
        "the refusal reason {reason:?} must mention one of {needles:?}, response was {resp}"
    );

    assert!(env.setting("mock_planner_prompt").is_null(), "a refusal must never invoke the planner");
    assert_eq!(env.setting("mock_agent_requests"), agents_before, "a refusal must never start an agent");
    assert_eq!(env.setting("mock_architect_requests"), architects_before, "a refusal must never start the architect");
    assert_eq!(env.plan_bytes(), plan_before, "a refusal must not change .forge/plan.json");
    assert_eq!(env.state_bytes(slug), state_before, "a refusal must not change the feature state");
    assert_eq!(env.feature_state_snapshot(), features_before, "a refusal must not touch .forge/features");
    assert_eq!(env.head(), head_before, "a refusal must not commit");
    assert_eq!(env.ctx().session.busy.load(Ordering::SeqCst), busy_before, "a refusal must not change the busy claim");
    assert_eq!(env.ctx().session.queue_active.load(Ordering::SeqCst), queue_before, "a refusal must not change the queue");
}

#[test]
fn s28_refuses_invalid_feature() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    // Approved for its content, then broken: the milestone now covers an unknown scenario.
    fs::write(
        env.features_dir().join(SLUG).join("milestones.md"),
        "## M1: Alpha milestone\n\nStatus: planned\nCovers: S99\n",
    )
    .unwrap();
    assert_refused(&env, SLUG, "M1", &["invalid"]);
}

#[test]
fn s28_refuses_draft_feature() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::draft(0, None);
    assert_refused(&env, SLUG, "M1", &["scenarios approved"]);
}

#[test]
fn s28_refuses_spec_approved_only_feature() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::draft(0, None);
    env.approve_spec(SLUG);
    assert_refused(&env, SLUG, "M1", &["scenarios approved"]);
}

#[test]
fn s28_refuses_when_content_changed_after_scenario_approval() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    fs::write(env.features_dir().join(SLUG).join("decisions.md"), "## D1: Edited\n\nEdited after approval.\n").unwrap();
    assert_refused(&env, SLUG, "M1", &["scenarios approved"]);
}

#[test]
fn s28_refuses_unknown_milestone() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    assert_refused(&env, SLUG, "M9", &["M9"]);
}

#[test]
fn s28_refuses_implemented_milestone() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    assert_refused(&env, SLUG, "M2", &["implemented"]);
}

#[test]
fn s28_refuses_milestone_without_covered_scenarios() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    assert_refused(&env, SLUG, "M3", &["none yet", "covers"]);
}

#[test]
fn s28_refuses_covered_scenario_missing_from_approved_ids() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    // The scenarios approval for the current content only recorded S1 and S3.
    let path = env.state_path(SLUG);
    let mut state = env.state(SLUG);
    for approval in state["approvals"].as_array_mut().unwrap() {
        if approval["kind"] == "scenarios" {
            approval["scenario_ids"] = json!(["S1", "S3"]);
        }
    }
    fs::write(&path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    assert_refused(&env, SLUG, "M1", &["S2"]);
}

#[test]
fn s28_refuses_when_engine_busy() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    env.ctx().session.busy.store(true, Ordering::SeqCst);
    assert_refused(&env, SLUG, "M1", &["busy"]);
}

#[test]
fn s28_refuses_when_queue_active() {
    // S28: Milestone planning is refused unless the feature is ready
    let env = Env::ready();
    env.ctx().session.queue_active.store(true, Ordering::SeqCst);
    assert_refused(&env, SLUG, "M1", &["busy", "queue"]);
}

// ----------------------------------------------------------------------- S29

#[test]
fn s29_planner_prompt_names_feature_context_and_rules_without_file_contents() {
    // S29: The planner gets the feature context and plans failing tests first
    let env = Env::ready();
    env.plan_m1(milestone_plan(&["S1", "S2"]));

    let prompts = env.planner_prompts();
    assert_eq!(prompts.len(), 1, "one planner invocation expected, got {}", prompts.len());
    let prompt = &prompts[0];
    let reference = section(prompt, FEATURE_REFERENCE);
    for needle in [FOLDER, "M1", "Alpha milestone", "S1", "S2"] {
        assert!(reference.contains(needle), "the feature reference must name {needle:?}");
    }
    let lower = prompt.to_lowercase();
    assert!(
        lower.contains("first stage") && lower.contains("fail"),
        "the prompt must state the failing-tests-first rule for the first stage"
    );
    assert!(
        lower.contains("final stage") && prompt.contains("Status: implemented") && prompt.contains("Business tests:"),
        "the prompt must state the final-stage rule (Status: implemented and the Business tests: line)"
    );
    assert_none_contain(&prompts, "planner", &[MARKER, "First scenario", "Second scenario", "another starting point"]);
}

#[test]
fn s29_first_stage_missing_a_covered_id_is_corrected_through_the_shared_budget() {
    // S29: a plan whose first stage does not name every covered scenario ID is
    // returned to the planner through the shared response-correction budget
    let env = Env::ready();
    env.plan_m1(json!([milestone_plan(&["S1"]), milestone_plan(&["S1", "S2"])]));

    let prompts = env.planner_prompts();
    assert_eq!(prompts.len(), 2, "the incomplete plan must be sent back to the planner once");
    let correction = &prompts[1];
    assert!(correction.contains("RESPONSE CORRECTION"), "the second invocation is a correction: {correction:?}");
    let error = correction
        .split_once("rejected by the engine: ")
        .and_then(|(_, rest)| rest.split_once("\n\nCorrect the complete response"))
        .map(|(error, _)| error)
        .unwrap_or_else(|| panic!("no engine error in the correction prompt"));
    assert!(error.contains("S2"), "the correction must name the missing ID S2: {error:?}");
    assert!(!error.contains("S1"), "the correction must not name IDs the plan already covers: {error:?}");

    let plan = env.plan_file();
    assert_eq!(plan["feature"]["scenario_ids"], json!(["S1", "S2"]), "the corrected plan is accepted, plan was {plan}");
    let first = format!("{} {}", plan["stages"][0]["instructions"], plan["stages"][0]["acceptance"]);
    assert!(first.contains("S1") && first.contains("S2"), "the accepted first stage names every ID: {first}");
}

#[test]
fn s29_plan_that_still_misses_an_id_after_the_budget_fails_planning() {
    // S29: the shared response-correction budget is bounded
    let env = Env::ready();
    // A single scripted output repeats, so every correction returns the same incomplete plan.
    env.set("mock_plan_output", milestone_plan(&["S1"]));
    let (code, resp) = env.plan_milestone(SLUG, "M1");
    assert_eq!(code, 200, "POST /api/features/plan should start planning M1: {resp}");
    wait_for_worker(env.ctx());

    assert_eq!(env.planner_prompts().len(), 4, "the initial response plus three corrections");
    assert!(env.ctx().load_plan().is_none(), "an incomplete plan must never be published");
    assert_eq!(env.ctx().session.state.lock().unwrap().phase, "failed", "planning must fail");
    assert!(!env.ctx().session.busy.load(Ordering::SeqCst), "a failed planning must release the engine");
    let state = env.state(SLUG);
    assert!(
        state["plans"].as_array().is_none_or(|plans| plans.iter().all(|link| link["status"] != "planning")),
        "a failed planning must not leave its link planning forever: {state}"
    );
}

// ----------------------------------------------------------------------- S30

#[test]
fn s30_architect_context_holds_compact_feature_reference() {
    // S30: The architect gets a compact feature context
    let env = Env::ready();
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    env.approve_and_run();

    let prompts = env.architect_prompts();
    assert!(!prompts.is_empty(), "at least one architect work-status turn expected, got {}", prompts.len());
    assert_all_contain(&prompts, "architect", &[SLUG, FOLDER, "M1", "Alpha milestone", "S1", "S2"]);
    assert_none_contain(&prompts, "architect", &[MARKER, "First scenario", "another starting point"]);
}

#[test]
fn s30_architect_prompt_does_not_grow_with_the_spec() {
    // S30: Architect contexts must not grow with the size of the spec
    let small = Env::ready_with(0, None);
    small.plan_m1(milestone_plan(&["S1", "S2"]));
    let large = Env::ready_with(24 * 1024, None);
    large.plan_m1(milestone_plan(&["S1", "S2"]));
    let scenarios = fs::metadata(large.features_dir().join(SLUG).join("scenarios.md")).unwrap().len();
    assert!(scenarios > 20 * 1024, "the large fixture must have a large scenarios.md, has {scenarios} bytes");

    let (small, large) = (small.architect_prompts(), large.architect_prompts());
    assert!(!small.is_empty() && !large.is_empty(), "both plans must have produced an architect turn");
    assert_all_contain(&small, "architect", &[FOLDER]);
    assert_all_contain(&large, "architect", &[FOLDER]);
    assert!(
        large[0].len() <= small[0].len() + 1024,
        "a {scenarios}-byte scenarios.md grew the architect prompt from {} to {} bytes",
        small[0].len(),
        large[0].len()
    );
}

// ----------------------------------------------------------------------- S31

#[test]
fn s31_stage_and_plan_reviewers_get_the_feature_reference() {
    // S31: stage and plan reviewers are pointed to the feature folder
    for cadence in ["per_stage", "per_plan"] {
        let env = Env::ready();
        if cadence == "per_plan" {
            env.per_plan();
        }
        env.plan_m1(milestone_plan(&["S1", "S2"]));
        env.approve_and_run();

        let (reviewers, architects) = if cadence == "per_plan" {
            (env.plan_review_prompts("reviewer"), env.plan_review_prompts("architect"))
        } else {
            (env.stage_review_prompts("reviewer"), env.stage_review_prompts("architect"))
        };
        assert_all_contain(&reviewers, &format!("{cadence} reviewer"), &[FEATURE_REFERENCE, FOLDER, "M1", "S1", "S2"]);
        // The architect only reviews the stages its scope policy requires it to.
        for (prompts, role) in [(&reviewers, "reviewer"), (&architects, "architect")] {
            if role == "architect" && !prompts.is_empty() {
                assert_all_contain(prompts, &format!("{cadence} architect"), &[FEATURE_REFERENCE, FOLDER, "M1", "S1", "S2"]);
            }
            assert_none_contain(prompts, &format!("{cadence} {role}"), &[MARKER]);
        }
    }
}

#[test]
fn s31_plan_review_criteria_have_one_item_per_covered_scenario() {
    // S31: the review criteria contain one item per covered scenario ID
    let env = Env::ready();
    env.per_plan();
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    let expected = ["S1", "S2"].map(|id| format!("an executable test traceable to {id} exists and passes"));
    let prompts = env.plan_review_prompts("reviewer");
    assert_all_contain(&prompts, "plan reviewer", &["CRITERIA TO EVIDENCE"]);
    for prompt in prompts.iter().chain(env.plan_review_prompts("architect").iter()) {
        let criteria = &prompt[prompt.rfind("CRITERIA TO EVIDENCE:").expect("prompt lists its criteria to evidence")..];
        let lines: Vec<&str> = criteria.lines().map(str::trim).collect();
        for item in &expected {
            assert_eq!(
                lines.iter().filter(|line| *line == item).count(),
                1,
                "exactly one criterion {item:?} expected in {criteria:?}"
            );
        }
        assert!(
            !lines.iter().any(|line| line.contains("traceable to S3")),
            "only the milestone's covered IDs get criteria: {criteria:?}"
        );
    }
    let acceptance = plan["plan_review"]["acceptance"].as_str().unwrap_or("");
    for item in &expected {
        assert!(acceptance.lines().any(|line| line.trim() == item), "the plan review acceptance lacks {item:?}: {acceptance:?}");
    }
    for review in plan["plan_review"]["reviews"].as_array().unwrap() {
        let criteria: Vec<&str> = review["criteria"].as_array().unwrap().iter().filter_map(|c| c["criterion"].as_str()).collect();
        for item in &expected {
            assert!(criteria.contains(&item.as_str()), "verdict criteria lack {item:?}: {criteria:?}");
        }
    }
}

#[test]
fn s31_failed_scenario_criterion_prevents_approval() {
    // S31: the plan cannot be approved while any of those items fails
    let env = Env::ready();
    env.per_plan();
    env.set("max_fix_rounds", json!(0));
    env.plan_m1(milestone_plan(&["S1", "S2"]));

    let planned = env.ctx().load_plan().unwrap();
    let mut criteria: Vec<Value> = planned["stages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|stage| {
            let heading = format!("stage {} ({})", stage["id"].as_i64().unwrap(), stage["title"].as_str().unwrap());
            stage["acceptance"]
                .as_str()
                .unwrap()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| json!({"criterion": format!("{heading}: {line}"), "status": "passed", "evidence": "verified"}))
                .collect::<Vec<_>>()
        })
        .collect();
    for id in ["S1", "S2"] {
        criteria.push(json!({
            "criterion": format!("an executable test traceable to {id} exists and passes"),
            "status": if id == "S2" { "failed" } else { "passed" },
            "evidence": if id == "S2" { "no executable test names S2" } else { "the S1 test passes" },
        }));
    }
    env.set(
        "mock_verdicts",
        json!([{"approved": false, "summary": "S2 has no executable test",
            "issues": ["No executable test traceable to S2 exists"], "criteria": criteria}]),
    );
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_ne!(plan["status"], "done", "a failed scenario criterion must prevent approval: {plan}");
    assert_eq!(plan["plan_review"]["status"], "blocked", "plan review was {}", plan["plan_review"]);
    let recorded = plan["plan_review"]["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|review| review["role"] == "reviewer")
        .unwrap_or_else(|| panic!("the reviewer verdict must be recorded: {}", plan["plan_review"]));
    let s2 = recorded["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["criterion"] == "an executable test traceable to S2 exists and passes")
        .unwrap_or_else(|| panic!("the S2 criterion must be part of the verdict: {recorded}"));
    assert_eq!(s2["status"], "failed");
    let state = env.state(SLUG);
    assert!(
        state["plans"].as_array().unwrap().iter().all(|link| link["status"] != "completed"),
        "an unapproved plan must not be recorded as completed: {state}"
    );
}

// ----------------------------------------------------------------------- S32

fn feature_entry(listing: &Value, slug: &str) -> Value {
    listing["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["slug"] == slug)
        .unwrap_or_else(|| panic!("feature {slug} is not listed: {listing}"))
        .clone()
}

fn milestone_entry(feature: &Value, id: &str) -> Value {
    feature["milestones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("milestone {id} is not listed: {feature}"))
        .clone()
}

#[test]
fn s32_registry_parses_both_forms_and_api_lists_files() {
    // S32: `Business tests:` lines are parsed as defined in D16
    let env = Env::new();
    env.commit_files(&[
        ("src/a.rs", "// a\n"),
        ("tests/b.test.mjs", "// b\n"),
        ("tests/c.rs", "// c\n"),
        ("tests/d.mjs", "// d\n"),
        ("tests/e.py", "# e\n"),
    ]);
    let scenarios = scenarios_md(0);
    env.write_feature(
        "notes-form",
        "Notes Form",
        &scenarios,
        &milestones_md(Some("src/a.rs (S1-S6, S9) and tests/b.test.mjs (S7-S8)")),
    );
    env.write_feature(
        "backtick-form",
        "Backtick Form",
        &scenarios,
        &milestones_md(Some("`tests/c.rs`, `tests/d.mjs` and `tests/e.py`")),
    );
    env.write_feature("no-registry", "No Registry", &scenarios, &milestones_md(None));

    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    for (slug, expected) in [
        ("notes-form", json!(["src/a.rs", "tests/b.test.mjs"])),
        ("backtick-form", json!(["tests/c.rs", "tests/d.mjs", "tests/e.py"])),
        ("no-registry", json!([])),
    ] {
        let feature = feature_entry(&listing, slug);
        assert_eq!(feature["status"], "valid", "both forms must stay valid without edits: {feature}");
        assert_eq!(milestone_entry(&feature, "M2")["business_tests"], expected, "feature was {feature}");
        // A milestone that omits the line registers nothing.
        assert_eq!(milestone_entry(&feature, "M1")["business_tests"], json!([]), "feature was {feature}");
    }
}

#[test]
fn s32_repository_registry_lines_parse_without_edits() {
    // S32: both forms in the repository today parse without edits
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let env = Env::new();
    copy_dir_all(&repo.join("docs/features"), &env.features_dir());
    // Mirror every file a `Business tests:` line names, so only the grammar is under test.
    for feature in fs::read_dir(repo.join("docs/features")).unwrap() {
        let milestones = feature.unwrap().path().join("milestones.md");
        let Ok(text) = fs::read_to_string(&milestones) else { continue };
        for line in text.lines().filter(|line| line.starts_with("Business tests:")) {
            for token in line.split(|c: char| c.is_whitespace() || "(),`".contains(c)) {
                if token.contains('/') && repo.join(token).is_file() {
                    let target = env.test.path.join(token);
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    fs::copy(repo.join(token), target).unwrap();
                }
            }
        }
    }

    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    for slug in ["feature-specs", "panel-redesign"] {
        let feature = feature_entry(&listing, slug);
        assert_eq!(feature["status"], "valid", "{slug} must stay valid without edits: {}", feature["reasons"]);
    }
    let specs = feature_entry(&listing, "feature-specs");
    assert_eq!(
        milestone_entry(&specs, "M1")["business_tests"],
        json!(["src/feature_spec_tests.rs", "tests/panel_features.test.mjs"])
    );
    assert_eq!(
        milestone_entry(&specs, "M2")["business_tests"],
        json!(["src/feature_spec_m2_tests.rs", "tests/panel_feature_spec.test.mjs"])
    );
    assert_eq!(milestone_entry(&specs, "M4")["business_tests"], json!(["src/pen_dev_tests.rs"]));
    let redesign = feature_entry(&listing, "panel-redesign");
    assert_eq!(
        milestone_entry(&redesign, "M1")["business_tests"],
        json!([
            "tests/panel_redesign_m1.test.mjs",
            "tests/qml/tst_panel_shell.qml",
            "tests/qml/tst_panel_overview.qml",
            "tests/qml/tst_panel_settings.qml",
            "tests/qml/tst_panel_queue.qml",
        ])
    );
    assert_eq!(
        milestone_entry(&redesign, "M2")["business_tests"],
        json!([
            "tests/panel_redesign_m2.test.mjs",
            "tests/qml/tst_panel_plan.qml",
            "tests/qml/tst_panel_stage_detail.qml",
            "tests/qml/tst_panel_overview_attention.qml",
            "tests/qml/tst_panel_settings_compact.qml",
        ])
    );
}

#[test]
fn s32_invalid_registry_entries_make_the_feature_invalid_naming_the_entry() {
    // S32: an entry that is not a path, a path to a missing file, or a repeated
    // `Business tests:` line makes the feature invalid with a reason naming it
    let env = Env::new();
    env.commit_files(&[("tests/present.rs", "// present\n")]);
    let scenarios = scenarios_md(0);
    let with_line = |line: &str| milestones_md(Some(line));
    // The second line must sit in the same milestone as the first to be a repeat.
    let repeated = with_line("tests/present.rs\nBusiness tests: tests/present.rs");
    let cases: [(&str, String, &str); 5] = [
        ("has-spaces", with_line("tests/present.rs and some words here"), "some words here"),
        ("not-a-path", with_line("none"), "none"),
        ("missing-file", with_line("tests/does_not_exist.rs (S1)"), "tests/does_not_exist.rs"),
        ("missing-backticked", with_line("`tests/present.rs`, `tests/also_missing.mjs`"), "tests/also_missing.mjs"),
        ("repeated-line", repeated, "Business tests"),
    ];
    for (slug, milestones, _) in &cases {
        env.write_feature(slug, slug, &scenarios, milestones);
    }

    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    for (slug, _, entry) in &cases {
        let feature = feature_entry(&listing, slug);
        assert_eq!(feature["status"], "invalid", "{slug} must be invalid: {feature}");
        let reasons = feature["reasons"].as_array().unwrap();
        assert!(
            reasons.iter().any(|r| r.as_str().unwrap_or("").contains(entry)),
            "a reason must name {entry:?} for {slug}: {reasons:?}"
        );
    }
}

#[test]
fn s32_every_plan_gives_reviewers_the_registered_business_test_files_of_all_features() {
    // S32: reviewers of any plan are given the registered files of all features and must run them
    for cadence in ["per_stage", "per_plan"] {
        let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
        // A second feature that is not even approved still registers its files.
        env.write_feature("second-feature", "Second Feature", &scenarios_md(0), &milestones_md(Some(&format!("`{BETA}`"))));
        if cadence == "per_plan" {
            env.per_plan();
        }
        env.run_goal_plan();

        let plan = env.ctx().load_plan().unwrap();
        assert_eq!(plan["status"], "done", "{cadence}: plan was {plan}");
        let prompts = if cadence == "per_plan" { env.plan_review_prompts("reviewer") } else { env.stage_review_prompts("reviewer") };
        assert_all_contain(&prompts, &format!("{cadence} reviewer"), &[REGISTRY, ALPHA, BETA]);
        for prompt in &prompts {
            let registry: String = section(prompt, REGISTRY).chars().take(1500).collect();
            assert!(
                registry.to_lowercase().contains("must run"),
                "the registry section must tell reviewers they must run the files: {registry:?}"
            );
        }
    }
}

#[test]
fn s32_valid_entries_of_an_invalid_feature_stay_registered() {
    // S32: review protection must not vanish exactly when feature validation degrades
    // Registration does not depend on approval, and an invalid feature cannot be approved.
    let env = Env::draft(0, Some(&format!("{ALPHA_REGISTRY} and tests/does_not_exist.rs")));
    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    assert_eq!(feature_entry(&listing, SLUG)["status"], "invalid", "a missing registered file invalidates the feature");

    env.run_goal_plan();
    let prompts = env.stage_review_prompts("reviewer");
    assert_all_contain(&prompts, "reviewer", &[REGISTRY, ALPHA]);
}

// ----------------------------------------------------------------------- S33

/// Runs a goal plan whose single stage is done by the mock implementer with the
/// given `mock_edits` / `mock_implementer_actions`, and returns the environment.
fn run_single_stage_with(edits: Value, actions: Value) -> Env {
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    env.set(
        "mock_plan_output",
        json!({"goal": "Ship the thing", "status": "draft", "stages": [goal_plan()["stages"][0].clone()]}),
    );
    if !edits.is_null() {
        env.set("mock_edits", edits);
    }
    if !actions.is_null() {
        env.set("mock_implementer_actions", actions);
    }
    let (code, resp) = env.post("/api/plan", json!({"goal": "Ship the thing"}));
    assert_eq!(code, 200, "setup: POST /api/plan failed: {resp}");
    wait_for_worker(env.ctx());
    env.approve_and_run();
    env
}

fn assert_stage_committed(env: &Env) {
    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["stages"][0]["status"], "committed", "the engine must not block the change: {plan}");
    assert_eq!(plan["status"], "done", "plan was {plan}");
}

#[test]
fn s33_stage_diff_modifying_a_registered_file_is_listed_and_not_blocked() {
    // S33: the engine lists the affected business test files in the reviewer prompts
    let env = run_single_stage_with(json!([{ALPHA: "// alpha business test, edited\n"}]), Value::Null);
    let prompts = env.stage_review_prompts("reviewer");
    assert_all_contain(&prompts, "stage reviewer", &[CHANGED]);
    for prompt in &prompts {
        assert!(section(prompt, CHANGED).contains(ALPHA), "the changed section must list {ALPHA}");
        assert!(!section(prompt, CHANGED).contains(BETA), "an untouched registered file must not be listed");
    }
    assert_stage_committed(&env);
}

#[test]
fn s33_stage_diff_deleting_a_registered_file_is_listed_and_not_blocked() {
    // S33: a deleted registered business test file is surfaced too
    let env = run_single_stage_with(Value::Null, json!([{"remove": [ALPHA]}]));
    let prompts = env.stage_review_prompts("reviewer");
    assert_all_contain(&prompts, "stage reviewer", &[CHANGED]);
    for prompt in &prompts {
        assert!(section(prompt, CHANGED).contains(ALPHA), "the changed section must list the deleted {ALPHA}");
    }
    assert_stage_committed(&env);
}

#[test]
fn s33_stage_diff_renaming_a_registered_file_lists_its_source_path() {
    // S33: a rename removes the registered path, so the source path is listed
    let env = run_single_stage_with(Value::Null, json!([{"git": ["mv", ALPHA, "tests/alpha_renamed.rs"]}]));
    let prompts = env.stage_review_prompts("reviewer");
    assert_all_contain(&prompts, "stage reviewer", &[CHANGED]);
    for prompt in &prompts {
        assert!(section(prompt, CHANGED).contains(ALPHA), "the changed section must list the renamed source {ALPHA}");
    }
    assert_stage_committed(&env);
}

#[test]
fn s33_stage_diff_only_adding_or_touching_other_files_lists_nothing() {
    // S33: a diff that only adds a new test file does not list it
    let env = run_single_stage_with(
        json!([{"tests/new_added.rs": "// a new test\n", "README.md": "# unrelated edit\n"}]),
        Value::Null,
    );
    let prompts = env.stage_review_prompts("reviewer");
    assert_all_contain(&prompts, "stage reviewer", &[REGISTRY]);
    assert_none_contain(&prompts, "stage reviewer", &[CHANGED]);
    assert_stage_committed(&env);
}

#[test]
fn s33_plan_fix_diff_modifying_a_registered_file_is_listed_for_the_re_review() {
    // S33: a plan-fix diff that modifies a registered file is listed for the reviewers
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    env.per_plan();
    env.set("max_fix_rounds", json!(2));
    // Round 1: the architect asks for a fix; the fixer then edits a registered file.
    env.set("mock_architect_verdicts", json!([{"approved": false, "issues": ["Fix the integration"]}]));
    env.set("mock_edits", json!([{"first.rs": "fn first() {}\n"}, {"second.rs": "fn second() {}\n"}, {ALPHA: "// alpha, edited by the fixer\n"}]));
    env.run_goal_plan();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "the engine must not block the change: {plan}");
    let reviewers = env.plan_review_prompts("reviewer");
    assert_eq!(reviewers.len(), 2, "one review per round expected");
    assert!(!reviewers[0].contains(CHANGED), "the first round has no plan-fix diff yet");
    assert!(section(&reviewers[1], CHANGED).contains(ALPHA), "the re-review must list {ALPHA}");
    let architects = env.plan_review_prompts("architect");
    assert!(section(architects.last().unwrap(), CHANGED).contains(ALPHA), "the architect's re-review must list {ALPHA}");
}

#[test]
fn s33_implementer_and_fixer_prompts_escalate_scenario_conflicts() {
    // S33: conflicts with an approved scenario go to the architect, never to the test
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    env.set("max_fix_rounds", json!(1));
    env.set("mock_verdicts", json!([{"approved": false, "issues": ["Needs one fix"]}]));
    env.run_goal_plan();

    let implementers = env.agent_prompts("implementer");
    let fixers = env.agent_prompts("fixer");
    assert!(!implementers.is_empty() && !fixers.is_empty(), "both an implementer and a fixer must have run");
    for (role, prompts) in [("implementer", implementers), ("fixer", fixers)] {
        for prompt in &prompts {
            let lower = prompt.to_lowercase();
            for needle in ["approved scenario", "architectural context gap", "business test"] {
                assert!(lower.contains(needle), "a {role} prompt must mention {needle:?}");
            }
            assert!(lower.contains("architect"), "a {role} prompt must name the architect as the escalation target");
        }
    }
}

// ----------------------------------------------------------------------- S34

#[test]
fn s34_completed_milestone_plan_records_range_and_makes_no_commit_after_approval() {
    // S34: a completed milestone plan marks its milestone implemented
    let env = Env::ready();
    env.per_plan();
    let completed = milestones_md(None).replace(
        "Status: planned\nCovers: S1, S2\n",
        "Status: implemented\nCovers: S1, S2\n\nBusiness tests: tests/m1_business.rs (S1-S2)\n",
    );
    env.set(
        "mock_edits",
        json!([{"tests/m1_business.rs": "// the M1 business tests\n"},
            {"docs/features/demo-feature/milestones.md": completed}]),
    );
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    let base = env.head();
    let commits_before: usize = env.ctx().git(&["rev-list", "--count", "HEAD"]).unwrap().parse().unwrap();
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    assert_eq!(plan["plan_review"]["status"], "approved", "plan review was {}", plan["plan_review"]);

    let head = env.head();
    assert_eq!(
        plan["plan_review"]["finalization"]["head"], json!(head),
        "HEAD must equal the finalized plan-review HEAD: the engine commits nothing after approval"
    );
    let commits_after: usize = env.ctx().git(&["rev-list", "--count", "HEAD"]).unwrap().parse().unwrap();
    assert_eq!(commits_after, commits_before + 2, "only the two stage commits are expected");
    assert_eq!(env.statuses_outside_forge(), Vec::<String>::new(), "the engine must leave no repository change");
    let milestones = env.ctx().git(&["show", &format!("HEAD:{FOLDER}milestones.md")]).unwrap();
    assert!(
        milestones.contains("Status: implemented") && milestones.contains("Business tests: tests/m1_business.rs"),
        "the reviewed final stage sets the milestone status and its Business tests: line: {milestones}"
    );

    let state = env.state(SLUG);
    let plans = state["plans"].as_array().unwrap_or_else(|| panic!("state has no plans array: {state}"));
    assert_eq!(plans.len(), 1, "plans were {plans:?}");
    assert_eq!(plans[0]["milestone"], "M1", "link was {}", plans[0]);
    assert_eq!(plans[0]["status"], "completed", "link was {}", plans[0]);
    assert_eq!(plans[0]["commit_range"], json!({"base": base, "head": head}), "link was {}", plans[0]);

    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    let feature = feature_entry(&listing, SLUG);
    assert_eq!(feature["status"], "valid", "feature was {feature}");
    let m1 = milestone_entry(&feature, "M1");
    assert_eq!(m1["status"], "implemented", "milestone was {m1}");
    assert_eq!(m1["business_tests"], json!(["tests/m1_business.rs"]), "milestone was {m1}");
}

#[test]
fn s34_per_stage_reviewed_milestone_plan_records_the_first_stage_base() {
    // S34: without a deferred plan review the range starts at the first stage's base
    let env = Env::ready();
    env.set("review_cadence", json!({"architect": "per_stage", "reviewer": "per_stage"}));
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    let base = env.head();
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    assert!(!plan["plan_review"].is_object(), "no plan review is deferred: {}", plan["plan_review"]);
    let state = env.state(SLUG);
    let link = &state["plans"][0];
    assert_eq!(link["status"], "completed", "link was {link}");
    assert_eq!(link["plan_id"], plan["plan_id"], "link was {link}");
    assert!(link["completed_unix"].as_i64().is_some_and(|t| t > 0), "link was {link}");
    assert_eq!(link["commit_range"], json!({"base": base, "head": env.head()}), "link was {link}");
    assert_eq!(state["plans"].as_array().unwrap().len(), 1);
}

#[test]
fn s34_blocked_plan_review_records_no_completion() {
    // S34: only an approved plan records its completion
    let env = Env::ready();
    env.per_plan();
    env.set("max_fix_rounds", json!(0));
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    let before = env.state_bytes(SLUG);
    env.set("mock_verdicts", json!([{"approved": false, "issues": ["Not done"]}]));
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_ne!(plan["status"], "done", "plan was {plan}");
    assert_eq!(plan["plan_review"]["status"], "blocked", "plan review was {}", plan["plan_review"]);
    assert_eq!(env.state_bytes(SLUG), before, "a blocked plan must not write feature state");
    assert_eq!(env.state(SLUG)["plans"][0]["status"], "planned");
}

#[test]
fn s34_unwritable_feature_state_is_logged_without_failing_the_completed_plan() {
    // S34: completion is runtime metadata; a failed write never undoes the run
    let env = Env::ready();
    env.per_plan();
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    fs::write(env.state_path(SLUG), "not json").unwrap();
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    assert_eq!(plan["plan_review"]["status"], "approved", "plan review was {}", plan["plan_review"]);
    assert_eq!(env.head(), plan["plan_review"]["finalization"]["head"].as_str().unwrap());
    let history = fs::read_to_string(env.test.path.join(".forge/history.jsonl")).unwrap();
    let plan_id = plan["plan_id"].as_str().unwrap();
    assert!(
        history.lines().any(|line| line.contains("\"kind\":\"error\"")
            && line.contains(SLUG) && line.contains(plan_id) && line.contains("completion")),
        "an error naming the feature and plan must be logged: {history}"
    );
}

#[test]
fn s34_planner_prompt_states_the_final_stage_rule() {
    // S34: the planner writes a final stage that marks the milestone implemented
    let env = Env::ready();
    env.plan_m1(milestone_plan(&["S1", "S2"]));
    let prompts = env.planner_prompts();
    assert_eq!(prompts.len(), 1);
    let reference = section(&prompts[0], FEATURE_REFERENCE);
    for needle in ["Status: implemented", "Business tests:", "milestones.md"] {
        assert!(reference.contains(needle), "the feature reference must state the final-stage rule ({needle:?})");
    }
}

// ----------------------------------------------------------------------- S35

const FEATURE_TEXT: [&str; 4] = [FEATURE_REFERENCE, FOLDER, "Alpha milestone", MARKER];

/// Feature-neutral plan assertions: no `feature`, no feature context in the
/// planner or architect prompts, no feature runtime state written, while the
/// registry section (S32) still reaches the reviewers.
fn assert_feature_neutral(env: &Env, state_before: &[(PathBuf, Vec<u8>)]) {
    let plan = env.plan_file();
    assert!(plan.get("feature").is_none() || plan["feature"].is_null(), "plan.json must have no feature: {plan}");
    assert_none_contain(&env.planner_prompts(), "planner", &FEATURE_TEXT);
    assert_none_contain(&env.architect_prompts(), "architect", &FEATURE_TEXT);
    assert_eq!(env.feature_state_snapshot(), state_before, "no feature runtime state may be written or changed");
    let reviewers = env.review_prompts("reviewer");
    assert_all_contain(&reviewers, "reviewer", &[REGISTRY, ALPHA]);
    assert_none_contain(&reviewers, "reviewer", &[FEATURE_REFERENCE, "Alpha milestone"]);
}

#[test]
fn s35_goal_plan_gets_no_feature_context_but_keeps_the_registry_rules() {
    // S35: Plans not started from a feature get no feature context
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    let state_before = env.feature_state_snapshot();
    env.run_goal_plan();
    assert_eq!(env.ctx().load_plan().unwrap()["status"], "done");
    assert_feature_neutral(&env, &state_before);
}

#[test]
fn s35_refactor_plan_gets_no_feature_context_but_keeps_the_registry_rules() {
    // S35: Plans not started from a feature get no feature context
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    let state_before = env.feature_state_snapshot();
    env.start_goal_plan(json!({"mode": "refactor", "goal": "tidy the modules"}));
    env.approve_and_run();
    assert_eq!(env.ctx().load_plan().unwrap()["status"], "done");
    assert_feature_neutral(&env, &state_before);
}

#[test]
fn s35_discussion_plan_gets_no_feature_context_but_keeps_the_registry_rules() {
    // S35: Plans not started from a feature get no feature context
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    let state_before = env.feature_state_snapshot();
    env.ctx().ensure_forge_dir();
    fs::write(
        env.ctx().forge_path("discussion.jsonl"),
        "{\"role\":\"user\",\"text\":\"Add a thing\",\"unix\":1}\n{\"role\":\"assistant\",\"text\":\"Sure\",\"unix\":1}\n",
    )
    .unwrap();
    env.start_goal_plan(json!({"discussion": true}));
    env.approve_and_run();
    assert_eq!(env.ctx().load_plan().unwrap()["status"], "done");
    assert_feature_neutral(&env, &state_before);
}

#[test]
fn s35_queue_plan_gets_no_feature_context_but_keeps_the_registry_rules() {
    // S35: Plans not started from a feature get no feature context
    let env = Env::ready_with(0, Some(ALPHA_REGISTRY));
    let state_before = env.feature_state_snapshot();
    env.test.start();
    env.approve_and_run_queue_goal();
    assert_feature_neutral(&env, &state_before);
}

#[test]
fn s35_without_registered_tests_or_a_milestone_prompts_are_unchanged() {
    // S35: with no docs/features at all, or a feature that registers nothing,
    // reviewer, implementer and planner prompts carry no registry or feature text
    // Control: with a registered file the same flow does add the registry section,
    // so the absence checks below are not vacuous.
    let control = Env::ready_with(0, Some(ALPHA_REGISTRY));
    control.run_goal_plan();
    assert_all_contain(&control.review_prompts("reviewer"), "control reviewer", &[REGISTRY, ALPHA]);

    for with_unregistered_feature in [false, true] {
        let env = if with_unregistered_feature { Env::ready() } else { Env::new() };
        env.set("max_fix_rounds", json!(1));
        env.set("mock_verdicts", json!([{"approved": false, "issues": ["Needs one fix"]}]));
        env.run_goal_plan();

        let label = if with_unregistered_feature { "a feature without a registry" } else { "no docs/features" };
        assert_eq!(env.ctx().load_plan().unwrap()["status"], "done", "{label}");
        let mut prompts = env.planner_prompts();
        for role in ["implementer", "fixer"] {
            prompts.extend(env.agent_prompts(role));
        }
        prompts.extend(env.review_prompts("reviewer"));
        assert!(prompts.len() >= 5, "{label}: planner, implementer, fixer and reviewer prompts expected");
        for prompt in &prompts {
            let lower = prompt.to_lowercase();
            for heading in [FEATURE_REFERENCE, REGISTRY, CHANGED] {
                assert!(!prompt.contains(heading), "{label}: no {heading:?} section may appear");
            }
            for phrase in ["business test", "approved scenario"] {
                assert!(!lower.contains(phrase), "{label}: prompt text must stay unchanged, found {phrase:?}");
            }
        }
        if !with_unregistered_feature {
            assert!(!env.test.path.join(".forge/features").exists(), "{label}: no feature state may be created");
        }
    }
}
