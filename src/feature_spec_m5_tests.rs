//! Business tests for the engine part of milestone M5 (panel viewer): scenarios
//! S37 (`GET /api/features/content`), S38 (scenario results recorded from plan
//! reviews) and the engine half of S39 (results served with the scenarios). See
//! docs/features/feature-specs/scenarios.md and decisions.md (D6, D10, D17, D18).
//!
//! M5 is not implemented yet: the content endpoint does not exist and no review
//! records scenario results, so these tests are expected to fail on their
//! assertions until the M5 stages implement the contract. They only use the
//! existing test surface (crate::test_support, temporary git repositories, the
//! mock providers and the `mock_verdicts` setting).
//!
//! Contract pinned here:
//! - `GET /api/features/content?project=<p>&slug=<slug>` answers 200 with
//!   `slug`, `files` (README.md, scenarios.md, decisions.md, milestones.md, each
//!   `{text, error}`), `scenarios` (`{id, title, given, when, then, milestone,
//!   result}`), `milestones` (`{id, title, status, covers, business_tests}`) and
//!   `design` (entries `{path, kind, ...}`).
//! - `.forge/features/<slug>.json` gets an append-only `scenario_results` array
//!   of `{scenario_id, status, evidence, role, plan_id, milestone, unix,
//!   scenario_hash}`, one entry per scenario criterion of a recorded verdict.
//! - A scenario's `result` is its latest entry plus `out_of_date`, true when the
//!   scenario's current section text hashes differently from `scenario_hash`.

use crate::app::Ctx;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const SLUG: &str = "demo-feature";
const MARKER: &str = "ZEBRA-MARKER-7431-unique-scenario-body-sentence";
/// Content that only exists outside the feature folder under test.
const OUTSIDE_MARKER: &str = "OUTSIDE-MARKER-9917-must-never-be-served";
const ALPHA: &str = "tests/alpha_business.rs";
const BETA: &str = "tests/beta_business.mjs";
const FILES: [&str; 4] = ["README.md", "scenarios.md", "decisions.md", "milestones.md"];
/// The signature every PNG starts with; enough for a design export fixture.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n";

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

/// Recursive snapshot of everything under `dir`, symlinks recorded by their
/// target text and never followed, so a created, modified or removed file,
/// directory or link shows up as a difference. Empty when `dir` is missing.
fn tree_snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_symlink() {
                let mut record = b"link:".to_vec();
                record.extend_from_slice(fs::read_link(&path).unwrap().as_os_str().as_encoded_bytes());
                out.push((rel, record));
            } else if meta.is_dir() {
                out.push((rel, b"dir".to_vec()));
                walk(base, &path, out);
            } else {
                let mut record = b"file:".to_vec();
                record.extend(fs::read(&path).unwrap());
                out.push((rel, record));
            }
        }
    }
    let mut out = Vec::new();
    if fs::symlink_metadata(dir).is_ok() {
        walk(dir, dir, &mut out);
    }
    out
}

fn scenarios_md() -> String {
    format!(
        "# Scenarios\n\n\
        ## S1: First scenario\n\n\
        - Given: {MARKER}\n- When: something happens\n- Then: an outcome follows\n\n\
        ## S2: Second scenario\n\n\
        - Given: another starting point\n- When: something else happens\n- Then: another outcome follows\n\n\
        ## S3: Third scenario\n\n\
        - Given: a third starting point\n- When: a third thing happens\n- Then: a third outcome follows\n"
    )
}

/// M1 (S1, S2) is the planned milestone under test, M2 (S3) is implemented and
/// registers ALPHA, M3 covers nothing yet.
fn milestones_md() -> String {
    format!(
        "# Milestones\n\n\
        ## M1: Alpha milestone\n\nStatus: planned\nCovers: S1, S2\n\n\
        ## M2: Finished milestone\n\nStatus: implemented\nCovers: S3\n\nBusiness tests: {ALPHA} (S3)\n\n\
        ## M3: Later milestone\n\nStatus: planned\nCovers: none yet\n"
    )
}

fn readme(title: &str) -> String {
    format!(
        "# {title}\n\n## Goal\n\nDo the thing.\n\n## Scope\n\nIn scope.\n\n## Out of scope\n\nNothing yet.\n\n\
        ## Behavior\n\nWorks as expected.\n\n## Open questions\n\nNone.\n"
    )
}

/// The plan the mock planner returns for the goal `M1` (S1, S2).
fn milestone_plan() -> Value {
    json!({"goal": "planner goal", "status": "draft", "stages": [
        {"id": 1, "title": "Failing business tests",
         "instructions": "Write executable tests named after S1, S2 that fail before implementation.",
         "acceptance": "Executable tests for S1, S2 exist and fail before implementation.",
         "commit": "test: add failing business tests"},
        {"id": 2, "title": "Implement milestone",
         "instructions": "Implement the milestone until its tests pass, then set the milestone Status: implemented and its Business tests: line.",
         "acceptance": "The business tests pass and the milestone is marked implemented.",
         "commit": "feat: implement milestone"},
    ]})
}

struct Env {
    test: QueueTest,
    project: String,
}

impl Env {
    /// A project with the demo feature (spec and scenarios approved) and its
    /// registered business test files committed.
    fn ready() -> Self {
        let env = Self::draft();
        env.approve(SLUG);
        env
    }

    /// Same project, but the feature stays a draft and has no runtime state.
    fn draft() -> Self {
        let test = QueueTest::new(false);
        let project = test.path.display().to_string();
        let env = Self { test, project };
        env.commit_files(&[(ALPHA, "// alpha business test\n"), (BETA, "// beta business test\n")]);
        env.write_feature(SLUG, "Demo Feature");
        env
    }

    fn ctx(&self) -> &Ctx {
        &self.test.app
    }

    fn features_dir(&self) -> PathBuf {
        self.test.path.join("docs/features")
    }

    fn feature_dir(&self, slug: &str) -> PathBuf {
        self.features_dir().join(slug)
    }

    fn write_feature(&self, slug: &str, title: &str) {
        let dir = self.feature_dir(slug);
        copy_dir_all(&template_root(), &dir);
        fs::write(dir.join("README.md"), readme(title)).unwrap();
        fs::write(dir.join("scenarios.md"), scenarios_md()).unwrap();
        fs::write(dir.join("decisions.md"), "## D1: A decision\n\nDecided.\n").unwrap();
        fs::write(dir.join("milestones.md"), milestones_md()).unwrap();
    }

    /// Design files of every kind the content endpoint reports.
    fn add_design(&self, slug: &str) {
        let design = self.feature_dir(slug).join("design");
        fs::create_dir_all(&design).unwrap();
        fs::write(design.join("main.pen"), "{\"version\":\"2.6\",\"children\":[]}\n").unwrap();
        fs::write(design.join("main.png"), PNG_BYTES).unwrap();
        fs::write(design.join("lonely.pen"), "{\"version\":\"2.6\",\"children\":[]}\n").unwrap();
        fs::write(design.join("flow.mmd"), "graph LR\n  A --> B\n").unwrap();
        fs::write(design.join("notes.txt"), "plain design notes\n").unwrap();
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

    /// `GET /api/features/content` for `slug`.
    fn content(&self, slug: &str) -> (u16, Value) {
        self.get(&format!("/api/features/content?project={}&slug={slug}", self.project))
    }

    /// The 200 response of the content endpoint for the demo feature.
    fn served(&self) -> Value {
        let (code, body) = self.content(SLUG);
        assert_eq!(code, 200, "GET /api/features/content must serve {SLUG}, response was {body}");
        body
    }

    fn approve(&self, slug: &str) {
        let (code, resp) = self.post("/api/features/review", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: reviewing {slug} failed: {resp}");
        wait_for_worker(self.ctx());
        let (code, resp) = self.post("/api/features/approve_spec", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: approving the spec of {slug} failed: {resp}");
        let (code, resp) = self.post("/api/features/approve_scenarios", json!({"project": self.project, "slug": slug}));
        assert_eq!(code, 200, "setup: approving scenarios of {slug} failed: {resp}");
    }

    fn set(&self, key: &str, value: Value) {
        self.ctx().app.settings.lock().unwrap()[key] = value;
    }

    fn per_plan(&self) {
        self.set("review_cadence", json!({"architect": "per_plan", "reviewer": "per_plan"}));
    }

    fn state_path(&self, slug: &str) -> PathBuf {
        self.test.path.join(".forge/features").join(format!("{slug}.json"))
    }

    fn state(&self, slug: &str) -> Value {
        let bytes = fs::read(self.state_path(slug)).unwrap_or_else(|e| panic!("no feature state for {slug}: {e}"));
        serde_json::from_slice(&bytes).unwrap()
    }

    fn state_bytes(&self, slug: &str) -> Option<Vec<u8>> {
        fs::read(self.state_path(slug)).ok()
    }

    fn feature_state_snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        tree_snapshot(&self.test.path.join(".forge/features"))
    }

    /// Starts milestone planning of M1 and waits for the planner.
    fn plan_m1(&self) {
        self.set("mock_plan_output", milestone_plan());
        let (code, resp) =
            self.post("/api/features/plan", json!({"project": self.project, "slug": SLUG, "milestone": "M1"}));
        assert_eq!(code, 200, "setup: POST /api/features/plan should start planning M1: {resp}");
        wait_for_worker(self.ctx());
    }

    fn approve_and_run(&self) {
        let (code, resp) = self.post("/api/approve", json!({}));
        assert_eq!(code, 200, "setup: approving the plan failed: {resp}");
        let (code, resp) = self.post("/api/run", json!({}));
        assert_eq!(code, 200, "setup: running the plan failed: {resp}");
        wait_for_worker(self.ctx());
    }

    /// Criteria of a plan-scope verdict: every stage acceptance line under its
    /// `stage N (title): ` heading (all passed) followed by the scenario
    /// criteria S1 and S2 with the given `(status, evidence)`.
    fn plan_scope_criteria(&self, s1: (&str, &str), s2: (&str, &str)) -> Vec<Value> {
        let planned = self.ctx().load_plan().expect("setup: a plan must exist");
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
        criteria.extend(scenario_items(s1, s2));
        criteria
    }

    /// Runs the milestone plan through a per-plan review that answers with
    /// `verdicts` (each `(approved, s1, s2)`), allowing `fix_rounds` fixes.
    fn run_plan_review(&self, fix_rounds: i64, verdicts: &[(bool, (&str, &str), (&str, &str))]) {
        self.per_plan();
        self.set("max_fix_rounds", json!(fix_rounds));
        self.plan_m1();
        let verdicts: Vec<Value> = verdicts
            .iter()
            .map(|(approved, s1, s2)| {
                json!({"approved": approved, "summary": "scenario review",
                    "issues": if *approved { json!([]) } else { json!(["A scenario criterion failed"]) },
                    "criteria": self.plan_scope_criteria(*s1, *s2)})
            })
            .collect();
        self.set("mock_verdicts", json!(verdicts));
        self.approve_and_run();
    }

    /// Recorded scenario results, or a panic when the state has none.
    fn results(&self) -> Vec<Value> {
        let state = self.state(SLUG);
        state["scenario_results"]
            .as_array()
            .unwrap_or_else(|| panic!("the feature state has no scenario_results array: {state}"))
            .clone()
    }

    fn scenario(&self, id: &str) -> Value {
        scenario_of(&self.served(), id)
    }
}

/// The two per-scenario criteria of a milestone plan review.
fn scenario_items(s1: (&str, &str), s2: (&str, &str)) -> Vec<Value> {
    [("S1", s1), ("S2", s2)]
        .into_iter()
        .map(|(id, (status, evidence))| {
            let criterion = crate::feature_context::scenario_criteria(&json!({"scenario_ids": [id]}))
                .pop()
                .expect("a scenario criterion");
            json!({"criterion": criterion, "status": status, "evidence": evidence})
        })
        .collect()
}

fn scenario_of(body: &Value, id: &str) -> Value {
    body["scenarios"]
        .as_array()
        .unwrap_or_else(|| panic!("the response has no scenarios array: {body}"))
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("scenario {id} is not served: {body}"))
        .clone()
}

fn design_entry(body: &Value, path: &str) -> Value {
    body["design"]
        .as_array()
        .unwrap_or_else(|| panic!("the response has no design array: {body}"))
        .iter()
        .find(|e| e["path"] == path)
        .unwrap_or_else(|| panic!("design entry {path} is not served: {}", body["design"]))
        .clone()
}

// ----------------------------------------------------------------------- S37

#[test]
fn s37_content_api_serves_files_scenarios_milestones_and_design() {
    // S37: The engine serves a feature's content read-only
    let env = Env::draft();
    env.add_design(SLUG);
    let body = env.served();

    assert_eq!(body["slug"], SLUG, "response was {body}");
    for name in FILES {
        let file = &body["files"][name];
        let expected = fs::read_to_string(env.feature_dir(SLUG).join(name)).unwrap();
        assert_eq!(file["text"], json!(expected), "files[{name}].text must be the file's text: {file}");
        assert!(file["error"].is_null(), "files[{name}] has no error: {file}");
    }

    let scenarios = body["scenarios"].as_array().expect("scenarios must be an array");
    let ids: Vec<&Value> = scenarios.iter().map(|s| &s["id"]).collect();
    assert_eq!(ids, [&json!("S1"), &json!("S2"), &json!("S3")], "scenarios were {scenarios:?}");
    let s1 = scenario_of(&body, "S1");
    assert_eq!(s1["title"], "First scenario", "scenario was {s1}");
    assert_eq!(s1["given"], MARKER, "scenario was {s1}");
    assert_eq!(s1["when"], "something happens", "scenario was {s1}");
    assert_eq!(s1["then"], "an outcome follows", "scenario was {s1}");
    for (id, milestone) in [("S1", "M1"), ("S2", "M1"), ("S3", "M2")] {
        assert_eq!(scenario_of(&body, id)["milestone"], milestone, "the milestone covering {id}");
    }

    let milestones = body["milestones"].as_array().expect("milestones must be an array");
    assert_eq!(milestones.len(), 3, "milestones were {milestones:?}");
    for (index, (id, title, status, covers, tests)) in [
        ("M1", "Alpha milestone", "planned", json!(["S1", "S2"]), json!([])),
        ("M2", "Finished milestone", "implemented", json!(["S3"]), json!([ALPHA])),
        ("M3", "Later milestone", "planned", json!([]), json!([])),
    ]
    .into_iter()
    .enumerate()
    {
        let m = &milestones[index];
        assert_eq!((&m["id"], &m["title"], &m["status"]), (&json!(id), &json!(title), &json!(status)), "milestone was {m}");
        assert_eq!(m["covers"], covers, "milestone was {m}");
        assert_eq!(m["business_tests"], tests, "milestone was {m}");
    }

    let pen = design_entry(&body, "design/main.pen");
    assert_eq!((&pen["kind"], &pen["png"]), (&json!("pen"), &json!("design/main.png")), "entry was {pen}");
    let png = design_entry(&body, "design/main.png");
    assert_eq!(png["kind"], "png", "entry was {png}");
    let served = png["absolute_path"].as_str().unwrap_or_else(|| panic!("a png entry has an absolute_path: {png}"));
    assert_eq!(
        fs::canonicalize(served).unwrap(),
        fs::canonicalize(env.feature_dir(SLUG).join("design/main.png")).unwrap(),
        "the absolute path names the exported PNG"
    );
    let lonely = design_entry(&body, "design/lonely.pen");
    assert_eq!((&lonely["kind"], &lonely["png"]), (&json!("pen"), &Value::Null), "a .pen without export: {lonely}");
    let flow = design_entry(&body, "design/flow.mmd");
    assert_eq!(flow["kind"], "mermaid", "entry was {flow}");
    assert_eq!(flow["text"], "graph LR\n  A --> B\n", "entry was {flow}");
    assert_eq!(design_entry(&body, "design/notes.txt")["kind"], "other");
}

#[test]
fn s37_content_api_serves_an_invalid_feature() {
    // S37: a discovered feature is served whether it is valid or invalid
    let env = Env::draft();
    fs::write(
        env.feature_dir(SLUG).join("milestones.md"),
        "## M1: Alpha milestone\n\nStatus: planned\nCovers: S99\n",
    )
    .unwrap();
    let (code, listing) = env.get(&format!("/api/features?project={}", env.project));
    assert_eq!(code, 200, "response was {listing}");
    let listed = listing["features"].as_array().unwrap().iter().find(|f| f["slug"] == SLUG).unwrap().clone();
    assert_eq!(listed["status"], "invalid", "setup: the feature must be invalid: {listed}");

    let body = env.served();
    assert_eq!(body["slug"], SLUG);
    for name in FILES {
        assert!(body["files"][name]["text"].is_string(), "files[{name}] must still be served: {}", body["files"][name]);
    }
    assert_eq!(body["scenarios"].as_array().map(Vec::len), Some(3), "response was {body}");
}

#[test]
fn s37_content_api_refuses_unknown_and_invalid_slugs() {
    // S37: an unknown slug is refused; paths never leave docs/features/<slug>/
    let env = Env::draft();
    env.write_feature("other-feature", "Other");
    let before = tree_snapshot(&env.features_dir());

    let (code, body) = env.content("missing-feature");
    assert_eq!(code, 404, "an unknown slug is refused: {body}");
    assert!(body["error"].as_str().is_some_and(|e| !e.is_empty()), "a refusal carries a reason: {body}");
    for slug in ["..", "../other-feature", "Bad_Slug", "a/b", "demo-feature/..", ""] {
        let (code, body) = env.content(slug);
        assert_eq!(code, 400, "the invalid slug {slug:?} is refused: {body}");
        assert!(body["error"].as_str().is_some_and(|e| !e.is_empty()), "a refusal carries a reason: {body}");
        assert!(body["files"].is_null(), "a refused request serves no files: {body}");
    }
    let (code, body) = env.get(&format!("/api/features/content?project={}", env.project));
    assert_eq!(code, 400, "a request without a slug is refused: {body}");
    assert_eq!(tree_snapshot(&env.features_dir()), before, "refusals change no file");
}

#[test]
fn s37_a_missing_file_is_reported_with_a_reason_instead_of_failing() {
    // S37: a file that is missing is reported with a reason
    let env = Env::draft();
    fs::remove_file(env.feature_dir(SLUG).join("decisions.md")).unwrap();
    let body = env.served();

    let decisions = &body["files"]["decisions.md"];
    assert!(decisions["text"].is_null(), "a missing file has no text: {decisions}");
    assert!(decisions["error"].as_str().is_some_and(|e| !e.is_empty()), "a missing file has a reason: {decisions}");
    for name in ["README.md", "scenarios.md", "milestones.md"] {
        assert!(body["files"][name]["text"].is_string(), "files[{name}] is still served: {}", body["files"][name]);
    }
    assert_eq!(body["scenarios"].as_array().map(Vec::len), Some(3), "response was {body}");
}

#[test]
fn s37_a_non_utf8_file_is_reported_with_a_reason_instead_of_failing() {
    // S37: a file that is not UTF-8 is reported with a reason
    let env = Env::draft();
    fs::write(env.feature_dir(SLUG).join("scenarios.md"), b"# Scenarios\n\n## S1: Bad \xff\xfe bytes\n").unwrap();
    let body = env.served();

    let scenarios = &body["files"]["scenarios.md"];
    assert!(scenarios["text"].is_null(), "a non-UTF-8 file has no text: {scenarios}");
    assert!(scenarios["error"].as_str().is_some_and(|e| !e.is_empty()), "a non-UTF-8 file has a reason: {scenarios}");
    assert!(body["files"]["README.md"]["text"].is_string(), "other files are still served: {body}");
    assert!(body["scenarios"].is_array(), "scenarios stay an array: {body}");
}

/// A feature whose scenarios.md, a design file and a design directory point
/// outside `docs/features/<slug>/`, next to a feature and a file that hold
/// content that must never be served.
fn env_with_escaping_links(approved: bool) -> Env {
    let env = Env::draft();
    env.write_feature("other-feature", "Other");
    fs::write(
        env.feature_dir("other-feature").join("scenarios.md"),
        format!(
            "# Scenarios\n\n## S1: Outside\n\n- Given: {OUTSIDE_MARKER}\n- When: a\n- Then: b\n\n\
            ## S2: Two\n\n- Given: a\n- When: b\n- Then: c\n\n## S3: Three\n\n- Given: a\n- When: b\n- Then: c\n"
        ),
    )
    .unwrap();
    fs::write(env.test.path.join("outside-secret.txt"), format!("graph TD\n  {OUTSIDE_MARKER}\n")).unwrap();
    let feature = env.feature_dir(SLUG);
    fs::remove_file(feature.join("scenarios.md")).unwrap();
    fs::create_dir_all(feature.join("design")).unwrap();
    std::os::unix::fs::symlink("../other-feature/scenarios.md", feature.join("scenarios.md")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", feature.join("design/escape.png")).unwrap();
    std::os::unix::fs::symlink(env.test.path.join("outside-secret.txt"), feature.join("design/secret.mmd")).unwrap();
    std::os::unix::fs::symlink("/etc", feature.join("design/etc-dir")).unwrap();
    if approved {
        env.approve(SLUG);
    }
    env
}

#[test]
fn s37_symlinks_are_never_followed_out_of_the_feature_folder() {
    // S37: paths never leave docs/features/<slug>/ (including symlink escapes)
    let env = env_with_escaping_links(false);
    let body = env.served();
    let text = body.to_string();

    let scenarios = &body["files"]["scenarios.md"];
    assert!(scenarios["text"].is_null(), "a scenarios.md link out of the folder is not read: {scenarios}");
    assert!(scenarios["error"].as_str().is_some_and(|e| !e.is_empty()), "the link is reported with a reason: {scenarios}");
    for path in ["design/escape.png", "design/secret.mmd"] {
        let entry = design_entry(&body, path);
        assert!(entry["error"].as_str().is_some_and(|e| !e.is_empty()), "{path} is reported with a reason: {entry}");
        assert!(entry["absolute_path"].is_null(), "{path} names no path to serve: {entry}");
        assert!(entry["text"].is_null(), "{path} serves no text: {entry}");
    }
    assert!(!text.contains(OUTSIDE_MARKER), "content from outside the folder must never be served: {text}");
    assert!(!text.contains("root:"), "the content of /etc/passwd must never be served: {text}");
    let listed = body["design"].as_array().unwrap();
    assert!(
        listed.iter().all(|e| !e["path"].as_str().unwrap_or("").starts_with("design/etc-dir/")),
        "a linked directory is not entered: {listed:?}"
    );
}

#[test]
fn s37_serving_a_feature_creates_modifies_and_removes_no_file() {
    // S37: no file is created, modified or removed
    let approved = env_with_escaping_links(true);
    approved.add_design(SLUG);
    let draft = Env::draft();
    draft.add_design(SLUG);
    for (name, env) in [("an approved feature with links", &approved), ("a draft feature without state", &draft)] {
        let tree = tree_snapshot(&env.feature_dir(SLUG));
        let features = tree_snapshot(&env.features_dir());
        let state = env.feature_state_snapshot();
        assert!(!tree.is_empty(), "setup: {name} has files");

        let (code, body) = env.content(SLUG);
        assert_eq!(code, 200, "GET /api/features/content must serve {name}: {body}");
        let (code, _) = env.content("missing-feature");
        assert_eq!(code, 404);

        assert_eq!(tree_snapshot(&env.feature_dir(SLUG)), tree, "serving {name} changed docs/features/{SLUG}");
        assert_eq!(tree_snapshot(&env.features_dir()), features, "serving {name} changed docs/features");
        assert_eq!(env.feature_state_snapshot(), state, "serving {name} changed .forge/features");
    }
    assert!(draft.feature_state_snapshot().is_empty(), "a draft feature never gets a state file from a read");
}

// ----------------------------------------------------------------------- S38

#[test]
fn s38_plan_scope_review_records_one_result_per_scenario_criterion() {
    // S38: Plan review records test results per scenario
    let env = Env::ready();
    env.run_plan_review(0, &[(false, ("passed", "the S1 test passes"), ("failed", "no executable test names S2"))]);

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["plan_review"]["status"], "blocked", "the failed criterion blocks the plan: {}", plan["plan_review"]);
    let results = env.results();
    let mine: Vec<&Value> = results.iter().filter(|r| r["role"] == "reviewer").collect();
    assert_eq!(mine.len(), 2, "one entry per scenario criterion of the reviewer's verdict: {results:?}");
    for (entry, id, status, evidence) in [
        (mine[0], "S1", "passed", "the S1 test passes"),
        (mine[1], "S2", "failed", "no executable test names S2"),
    ] {
        assert_eq!(entry["scenario_id"], id, "entry was {entry}");
        assert_eq!(entry["status"], status, "entry was {entry}");
        assert_eq!(entry["evidence"], evidence, "entry was {entry}");
        assert_eq!(entry["plan_id"], plan["plan_id"], "entry was {entry}");
        assert_eq!(entry["milestone"], "M1", "entry was {entry}");
        assert!(entry["unix"].as_i64().is_some_and(|t| t > 0), "entry was {entry}");
        assert!(entry["scenario_hash"].as_str().is_some_and(|h| !h.is_empty()), "entry was {entry}");
    }
    assert_ne!(mine[0]["scenario_hash"], mine[1]["scenario_hash"], "each scenario has its own hash");
    for entry in &results {
        assert!(["S1", "S2"].contains(&entry["scenario_id"].as_str().unwrap_or("")), "only covered scenarios: {entry}");
    }
    let state = env.state(SLUG);
    assert!(state["approvals"].as_array().is_some_and(|a| a.len() >= 2), "approvals survive: {state}");
    assert!(state["plans"].as_array().is_some_and(|p| p.len() == 1), "the plan link survives: {state}");
}

#[test]
fn s38_a_second_verdict_appends_results_and_keeps_the_earlier_ones() {
    // S38: earlier results are kept as history
    let env = Env::ready();
    env.run_plan_review(
        2,
        &[
            (false, ("passed", "the S1 test passes"), ("failed", "no executable test names S2")),
            (true, ("passed", "the S1 test still passes"), ("passed", "the S2 test now passes")),
        ],
    );

    let plan = env.ctx().load_plan().unwrap();
    let recorded = plan["plan_review"]["reviews"].as_array().unwrap().iter().filter(|r| r["role"] == "reviewer").count();
    assert_eq!(recorded, 2, "setup: the reviewer must have reviewed twice: {}", plan["plan_review"]);
    let results = env.results();
    let reviewer: Vec<(&str, &str, &str)> = results
        .iter()
        .filter(|r| r["role"] == "reviewer")
        .map(|r| (r["scenario_id"].as_str().unwrap(), r["status"].as_str().unwrap(), r["evidence"].as_str().unwrap()))
        .collect();
    assert_eq!(
        reviewer,
        [
            ("S1", "passed", "the S1 test passes"),
            ("S2", "failed", "no executable test names S2"),
            ("S1", "passed", "the S1 test still passes"),
            ("S2", "passed", "the S2 test now passes"),
        ],
        "the second verdict appends and the first stays: {results:?}"
    );
    let unix: Vec<i64> = results.iter().map(|r| r["unix"].as_i64().unwrap_or(0)).collect();
    assert!(unix.iter().all(|t| *t > 0) && unix.windows(2).all(|w| w[0] <= w[1]), "history is in time order: {unix:?}");
}

#[test]
fn s38_final_stage_review_of_a_per_stage_plan_records_results() {
    // S38: a milestone plan whose roles all review per stage records the final stage review's results
    let env = Env::ready();
    env.plan_m1();
    let planned = env.ctx().load_plan().unwrap();
    let last = planned["stages"].as_array().unwrap().last().unwrap().clone();
    let mut criteria: Vec<Value> = last["acceptance"]
        .as_str()
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| json!({"criterion": line, "status": "passed", "evidence": "verified"}))
        .collect();
    criteria.extend(scenario_items(("passed", "S1 passes at the final stage"), ("passed", "S2 passes at the final stage")));
    env.set(
        "mock_verdicts",
        json!([{"approved": true, "summary": "first stage", "issues": []},
            {"approved": true, "summary": "final stage", "issues": [], "criteria": criteria}]),
    );
    env.approve_and_run();

    let plan = env.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    assert!(!plan["plan_review"].is_object(), "every role reviewed per stage, so no plan review ran: {}", plan["plan_review"]);
    let results = env.results();
    let mine: Vec<&Value> = results.iter().filter(|r| r["role"] == "reviewer").collect();
    assert_eq!(mine.len(), 2, "only the final stage review carries scenario criteria: {results:?}");
    for (entry, id, evidence) in
        [(mine[0], "S1", "S1 passes at the final stage"), (mine[1], "S2", "S2 passes at the final stage")]
    {
        assert_eq!((&entry["scenario_id"], &entry["status"], &entry["evidence"]), (&json!(id), &json!("passed"), &json!(evidence)));
        assert_eq!(entry["plan_id"], plan["plan_id"], "entry was {entry}");
        assert_eq!(entry["milestone"], "M1", "entry was {entry}");
        assert!(entry["unix"].as_i64().is_some_and(|t| t > 0), "entry was {entry}");
        assert!(entry["scenario_hash"].as_str().is_some_and(|h| !h.is_empty()), "entry was {entry}");
    }
}

#[test]
fn s38_plans_not_started_from_a_feature_record_nothing() {
    // S38: plans not started from a feature record nothing
    let feature = Env::ready();
    feature.run_plan_review(0, &[(true, ("passed", "the S1 test passes"), ("passed", "the S2 test passes"))]);
    assert!(!feature.results().is_empty(), "setup: a milestone plan review records results");

    // A plain goal whose acceptance even reads like a scenario criterion, in a
    // project that has a feature with runtime state.
    let goal = Env::ready();
    let before = goal.state_bytes(SLUG).expect("approval wrote the feature state");
    let snapshot = goal.feature_state_snapshot();
    let criterion = "an executable test traceable to S1 exists and passes";
    goal.set(
        "mock_plan_output",
        json!({"goal": "Ship the thing", "status": "draft", "stages": [
            {"id": 1, "title": "First step", "instructions": "Add the first part.",
             "acceptance": criterion, "commit": "feat: first part"}]}),
    );
    let (code, resp) = goal.post("/api/plan", json!({"goal": "Ship the thing"}));
    assert_eq!(code, 200, "setup: POST /api/plan failed: {resp}");
    wait_for_worker(goal.ctx());
    goal.approve_and_run();

    let plan = goal.ctx().load_plan().unwrap();
    assert_eq!(plan["status"], "done", "plan was {plan}");
    assert_eq!(goal.state_bytes(SLUG), Some(before), "a plan without a feature must not touch the feature state");
    assert_eq!(goal.feature_state_snapshot(), snapshot, "no feature state file is created or changed");

    // No feature at all: no state directory content appears.
    let bare = QueueTest::new(false);
    let bare = Env { project: bare.path.display().to_string(), test: bare };
    bare.set(
        "mock_plan_output",
        json!({"goal": "Ship the thing", "status": "draft", "stages": [
            {"id": 1, "title": "First step", "instructions": "Add the first part.",
             "acceptance": criterion, "commit": "feat: first part"}]}),
    );
    let (code, resp) = bare.post("/api/plan", json!({"goal": "Ship the thing"}));
    assert_eq!(code, 200, "setup: POST /api/plan failed: {resp}");
    wait_for_worker(bare.ctx());
    bare.approve_and_run();
    assert!(bare.feature_state_snapshot().is_empty(), "no feature state is written for a plain goal");
}

// ----------------------------------------------------------------------- S39

#[test]
fn s39_scenarios_never_checked_are_served_without_a_result() {
    // S39: scenarios never checked by a review are shown as not recorded
    let env = Env::ready();
    let body = env.served();
    for id in ["S1", "S2", "S3"] {
        let scenario = scenario_of(&body, id);
        assert!(scenario.get("result").is_some(), "scenario {id} must carry a result field: {scenario}");
        assert!(scenario["result"].is_null(), "scenario {id} was never recorded: {scenario}");
    }
}

#[test]
fn s39_scenarios_carry_their_latest_recorded_result() {
    // S39: each scenario shows its latest recorded result, its time and plan
    let env = Env::ready();
    env.run_plan_review(0, &[(false, ("passed", "the S1 test passes"), ("failed", "no executable test names S2"))]);
    let plan_id = env.ctx().load_plan().unwrap()["plan_id"].clone();

    let s1 = env.scenario("S1")["result"].clone();
    assert_eq!(s1["status"], "passed", "result was {s1}");
    assert_eq!(s1["evidence"], "the S1 test passes", "result was {s1}");
    let s2 = env.scenario("S2")["result"].clone();
    assert_eq!(s2["status"], "failed", "result was {s2}");
    assert_eq!(s2["evidence"], "no executable test names S2", "result was {s2}");
    for result in [&s1, &s2] {
        assert_eq!(result["role"], "reviewer", "result was {result}");
        assert_eq!(result["plan_id"], plan_id, "result was {result}");
        assert_eq!(result["milestone"], "M1", "result was {result}");
        assert!(result["unix"].as_i64().is_some_and(|t| t > 0), "result was {result}");
        assert_eq!(result["out_of_date"], false, "unchanged scenarios are not out of date: {result}");
    }
    assert!(env.scenario("S3")["result"].is_null(), "S3 is not covered by the reviewed milestone");
}

#[test]
fn s39_the_latest_of_several_results_wins() {
    // S39: the latest recorded result is shown
    let env = Env::ready();
    env.run_plan_review(
        2,
        &[
            (false, ("passed", "the S1 test passes"), ("failed", "no executable test names S2")),
            (true, ("passed", "the S1 test still passes"), ("passed", "the S2 test now passes")),
        ],
    );
    let s2 = env.scenario("S2")["result"].clone();
    assert_eq!(s2["status"], "passed", "the newer result replaces the failed one: {s2}");
    assert_eq!(s2["evidence"], "the S2 test now passes", "result was {s2}");
    assert_eq!(env.scenario("S1")["result"]["evidence"], "the S1 test still passes");
}

#[test]
fn s39_editing_a_scenario_marks_only_its_result_out_of_date() {
    // S39: scenarios whose content changed after that result are marked as possibly out of date
    let env = Env::ready();
    env.run_plan_review(0, &[(false, ("passed", "the S1 test passes"), ("failed", "no executable test names S2"))]);
    let path = env.feature_dir(SLUG).join("scenarios.md");
    let original = fs::read_to_string(&path).unwrap();
    for id in ["S1", "S2"] {
        assert_eq!(env.scenario(id)["result"]["out_of_date"], false, "setup: {id} is current");
    }

    fs::write(&path, original.replace("- Then: another outcome follows", "- Then: a different outcome follows")).unwrap();
    let body = env.served();
    assert_eq!(scenario_of(&body, "S2")["result"]["out_of_date"], true, "S2's Then line changed: {body}");
    assert_eq!(scenario_of(&body, "S2")["then"], "a different outcome follows");
    assert_eq!(scenario_of(&body, "S1")["result"]["out_of_date"], false, "S1 did not change: {body}");
    assert!(scenario_of(&body, "S3")["result"].is_null(), "S3 still has no result: {body}");

    // Freshness compares content, not time: restoring the text restores the result.
    fs::write(&path, original).unwrap();
    assert_eq!(env.scenario("S2")["result"]["out_of_date"], false, "the restored scenario is current again");
}
