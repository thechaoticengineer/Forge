//! Business tests for milestone M2 (spec phase) scenarios S10-S19.
//! See docs/features/feature-specs/scenarios.md for the full scenario text
//! and docs/features/feature-specs/decisions.md (D6-D10) for the contract.
//!
//! M2 is not implemented yet: every test here exercises endpoints and
//! runtime state that do not exist. They compile against only the existing
//! test surface (crate::test_support, std::fs) and are expected to fail —
//! typically on a 404 from an unmatched route — until later stages of this
//! plan implement the M2 contract.

use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

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

/// Copies this repository's real `_template` folder and overwrites its
/// placeholder scenarios/milestones with content that passes M1 validation,
/// so M2 tests exercise a genuinely valid feature folder derived from the
/// same template the create endpoint will use.
fn write_valid_feature(features_dir: &Path, slug: &str, title: &str) {
    let dir = features_dir.join(slug);
    copy_dir_all(&template_root(), &dir);
    fs::write(
        dir.join("README.md"),
        format!(
            "# {title}\n\n\
            ## Goal\n\nDo the thing.\n\n\
            ## Scope\n\nIn scope.\n\n\
            ## Out of scope\n\nNothing yet.\n\n\
            ## Behavior\n\nWorks as expected.\n\n\
            ## Open questions\n\nNone.\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("scenarios.md"),
        "# Scenarios\n\n\
        ## S1: First scenario\n\n\
        - Given: a starting point\n- When: something happens\n- Then: an outcome follows\n\n\
        ## S2: Second scenario\n\n\
        - Given: another starting point\n- When: something else happens\n- Then: another outcome follows\n",
    )
    .unwrap();
    fs::write(
        dir.join("milestones.md"),
        "# Milestones\n\n## M1: First milestone\n\nCovers: S1, S2\n",
    )
    .unwrap();
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn feature_state_query(project: &Path, slug: &str) -> String {
    format!(
        "/api/features/state?project={}&slug={}",
        url_encode(&project.display().to_string()),
        url_encode(slug)
    )
}

/// Recursive (relative path, bytes) snapshot, used to prove no file under
/// `dir` was created, modified or removed.
fn snapshot_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap().to_path_buf();
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn s10_create_feature_from_template() {
    // S10: A new feature is created from the template
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    fs::create_dir_all(&features_dir).unwrap();
    // The scenario creates the feature "from docs/features/_template/", so the
    // project needs this repository's template just like every other fixture.
    copy_dir_all(&template_root(), &features_dir.join("_template"));
    let project = test.path.display().to_string();

    let before_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/create",
        json!({"project": project, "slug": "new-feature", "title": "New Feature"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["ok"], true, "response was {resp}");
    assert_eq!(resp["slug"], "new-feature", "response was {resp}");

    let dir = features_dir.join("new-feature");
    for name in ["README.md", "scenarios.md", "decisions.md", "milestones.md"] {
        assert!(dir.join(name).exists(), "created feature must have {name}");
    }
    assert!(dir.join("design").is_dir(), "created feature must have a design/ folder");
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(
        readme.starts_with("# New Feature\n"),
        "README should start with the title heading, got {readme:?}"
    );

    let (code, resp) = api_request(&test.app.app, "GET", "/api/features", json!({"project": project}));
    assert_eq!(code, 200, "response was {resp}");
    let listed = resp["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["slug"] == "new-feature")
        .expect("created feature should be listed");
    assert_eq!(listed["spec_status"], "draft", "listed feature was {listed}");

    let after_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(before_head, after_head, "creating a feature must not commit anything");
    let staged = test.app.git(&["diff", "--cached", "--name-only"]).unwrap();
    assert_eq!(staged.trim(), "", "creating a feature must not stage anything");

    // An invalid slug is refused.
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/create",
        json!({"project": project, "slug": "Not Valid", "title": "X"}),
    );
    assert_eq!(code, 400, "response was {resp}");

    // A duplicate slug is refused.
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/create",
        json!({"project": project, "slug": "new-feature", "title": "Again"}),
    );
    assert_eq!(code, 409, "response was {resp}");
}

#[test]
fn s11_chat_writes_only_inside_feature_folder() {
    // S11: The co-authoring agent edits only its feature folder
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "draft-feature";
    write_valid_feature(&features_dir, slug, "Draft Feature");
    let project = test.path.display().to_string();

    let before_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!({
        "reply": "Added an open question.",
        "files": [{"path": format!("docs/features/{slug}/README.md"),
            "content": "# Draft Feature\n\n## Open questions\n\nWhat about X?\n"}],
    });

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add an open question"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    assert!(resp["request_id"].is_number(), "response was {resp}");

    wait_for_worker(&test.app);

    let content = fs::read_to_string(features_dir.join(slug).join("README.md")).unwrap();
    assert!(content.contains("What about X?"), "chat file write should apply, got {content:?}");

    let (code, state) = api_request(&test.app.app, "GET", &feature_state_query(&test.path, slug), json!({}));
    assert_eq!(code, 200, "response was {state}");
    let chat = state["state"]["chat"].as_array().unwrap_or_else(|| panic!("state was {state}"));
    let contains = |v: &Value, needle: &str| v.to_string().contains(needle);
    assert!(
        chat.iter().any(|m| contains(m, "add an open question")),
        "chat history should hold the user message, got {chat:?}"
    );
    assert!(
        chat.iter().any(|m| contains(m, "Added an open question.")),
        "chat history should hold the reply, got {chat:?}"
    );

    let after_head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(before_head, after_head, "chat must not commit anything");
    let status = test.app.git(&["status", "--porcelain"]).unwrap();
    for line in status.lines() {
        let path = line.get(3..).unwrap_or("").trim();
        assert!(
            path.starts_with(&format!("docs/features/{slug}/")) || path.starts_with(".forge/"),
            "chat must not touch files outside the feature folder or .forge/: {line}"
        );
    }
}

#[test]
fn s12_rejects_write_to_other_feature_folder() {
    // S12: Co-authoring writes outside the feature folder are rejected
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "escape-other";
    write_valid_feature(&features_dir, slug, "Escape Other");
    let project = test.path.display().to_string();
    let valid_path = format!("docs/features/{slug}/scratch.md");

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!([
        {"reply": "trying", "files": [{"path": "docs/features/some-other-feature/x.md", "content": "nope"}]},
        {"reply": "fixed", "files": [{"path": valid_path, "content": "ok"}]},
    ]);

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add a file"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let prompt = test.app.app.settings.lock().unwrap()["mock_chat_prompt"].as_str().unwrap_or("").to_string();
    assert!(
        prompt.contains("RESPONSE CORRECTION"),
        "an escaping write must be rejected through the response-correction budget, prompt was {prompt:?}"
    );
    assert!(!features_dir.join("some-other-feature/x.md").exists(), "the escaping path must never be written");
    assert!(features_dir.join(slug).join("scratch.md").exists(), "the corrected, valid file must be written");
}

#[test]
fn s12_rejects_absolute_path_write() {
    // S12: Co-authoring writes outside the feature folder are rejected
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "escape-absolute";
    write_valid_feature(&features_dir, slug, "Escape Absolute");
    let project = test.path.display().to_string();
    let escape_target = std::env::temp_dir().join(format!("forge-m2-abs-escape-{}.md", std::process::id()));
    let valid_path = format!("docs/features/{slug}/scratch.md");

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!([
        {"reply": "trying", "files": [{"path": escape_target.display().to_string(), "content": "nope"}]},
        {"reply": "fixed", "files": [{"path": valid_path, "content": "ok"}]},
    ]);

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add a file"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let prompt = test.app.app.settings.lock().unwrap()["mock_chat_prompt"].as_str().unwrap_or("").to_string();
    assert!(prompt.contains("RESPONSE CORRECTION"), "prompt was {prompt:?}");
    assert!(!escape_target.exists(), "an absolute path must never be written: {}", escape_target.display());
    assert!(features_dir.join(slug).join("scratch.md").exists(), "the corrected, valid file must be written");
    let _ = fs::remove_file(&escape_target);
}

#[test]
fn s12_rejects_path_traversal_write() {
    // S12: Co-authoring writes outside the feature folder are rejected
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "escape-traversal";
    write_valid_feature(&features_dir, slug, "Escape Traversal");
    let project = test.path.display().to_string();
    let valid_path = format!("docs/features/{slug}/scratch.md");

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!([
        {"reply": "trying", "files": [{"path": format!("docs/features/{slug}/../escape.md"), "content": "nope"}]},
        {"reply": "fixed", "files": [{"path": valid_path, "content": "ok"}]},
    ]);

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add a file"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let prompt = test.app.app.settings.lock().unwrap()["mock_chat_prompt"].as_str().unwrap_or("").to_string();
    assert!(prompt.contains("RESPONSE CORRECTION"), "prompt was {prompt:?}");
    assert!(!features_dir.join("escape.md").exists(), "a `..` traversal must never escape the feature folder");
    assert!(features_dir.join(slug).join("scratch.md").exists(), "the corrected, valid file must be written");
}

#[test]
fn s12_rejects_symlink_escape_write() {
    // S12: Co-authoring writes outside the feature folder are rejected
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "escape-symlink";
    write_valid_feature(&features_dir, slug, "Escape Symlink");
    let project = test.path.display().to_string();
    let outside = std::env::temp_dir().join(format!("forge-m2-symlink-target-{}", std::process::id()));
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, features_dir.join(slug).join("link")).unwrap();
    let valid_path = format!("docs/features/{slug}/scratch.md");

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!([
        {"reply": "trying", "files": [{"path": format!("docs/features/{slug}/link/evil.md"), "content": "nope"}]},
        {"reply": "fixed", "files": [{"path": valid_path, "content": "ok"}]},
    ]);

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add a file"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let prompt = test.app.app.settings.lock().unwrap()["mock_chat_prompt"].as_str().unwrap_or("").to_string();
    assert!(prompt.contains("RESPONSE CORRECTION"), "prompt was {prompt:?}");
    assert!(!outside.join("evil.md").exists(), "a symlink must never let a write escape the repository");
    assert!(features_dir.join(slug).join("scratch.md").exists(), "the corrected, valid file must be written");
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn s12_exhausted_corrections_write_nothing_and_mark_activity_failed() {
    // S12: Co-authoring writes outside the feature folder are rejected
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "escape-exhausted";
    write_valid_feature(&features_dir, slug, "Escape Exhausted");
    let project = test.path.display().to_string();

    let escape = json!({"reply": "trying", "files": [{"path": "/tmp/forge-m2-should-not-exist.md", "content": "nope"}]});
    test.app.app.settings.lock().unwrap()["mock_chat_output"] =
        json!([escape.clone(), escape.clone(), escape.clone(), escape]);

    let before = snapshot_dir(&features_dir);

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": project, "slug": slug, "message": "add a file"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let after = snapshot_dir(&features_dir);
    assert_eq!(before, after, "an exhausted correction budget must write nothing under docs/features/");
    assert!(
        !Path::new("/tmp/forge-m2-should-not-exist.md").exists(),
        "an exhausted correction budget must never write the escaping path"
    );

    let (code, resp) = api_request(&test.app.app, "GET", "/api/features", json!({"project": project}));
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["activity"]["kind"], "chat", "activity was {}", resp["activity"]);
    assert_eq!(resp["activity"]["slug"], slug, "activity was {}", resp["activity"]);
    assert_eq!(resp["activity"]["status"], "failed", "activity was {}", resp["activity"]);
}

#[test]
fn s13_review_returns_verdict_and_resumes_session() {
    // S13: The architect reviews the spec
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "reviewed-feature";
    write_valid_feature(&features_dir, slug, "Reviewed Feature");
    let project = test.path.display().to_string();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    assert!(resp["request_id"].is_number(), "response was {resp}");
    wait_for_worker(&test.app);

    let query = feature_state_query(&test.path, slug);
    let (code, state) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(code, 200, "response was {state}");
    let content_hash = state["content_hash"].as_str().unwrap_or("").to_string();
    assert!(!content_hash.is_empty(), "state should carry a content hash, state was {state}");
    let reviews = state["state"]["reviews"].as_array().unwrap_or_else(|| panic!("state was {state}"));
    assert_eq!(reviews.len(), 1, "reviews were {reviews:?}");
    let review = reviews[0].clone();
    assert_eq!(review["approved"], true, "review was {review}");
    assert_eq!(review["content_hash"], content_hash, "review was {review}");
    assert!(review["issues"].is_array(), "review was {review}");
    assert!(review["questions"].is_array(), "review was {review}");
    let session = review["session"].as_str().unwrap_or("").to_string();

    let (code, list) = api_request(&test.app.app, "GET", "/api/features", json!({"project": project}));
    assert_eq!(code, 200, "response was {list}");
    let listed = list["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["slug"] == slug)
        .expect("listed");
    assert_eq!(listed["latest_review"], review, "listed feature was {listed}");

    // A scripted rejection of the same (still current) content.
    test.app.app.settings.lock().unwrap()["mock_spec_review_output"] = json!({
        "slug": slug, "content_hash": content_hash, "approved": false,
        "summary": "Needs more detail.", "issues": ["missing rationale"], "questions": ["what about Y?"],
    });
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let (code, state2) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(code, 200, "response was {state2}");
    let reviews2 = state2["state"]["reviews"].as_array().unwrap();
    assert_eq!(reviews2.len(), 2, "reviews are append-only, got {reviews2:?}");
    let second_review = &reviews2[1];
    assert_eq!(second_review["approved"], false, "review was {second_review}");
    assert_eq!(second_review["issues"], json!(["missing rationale"]), "review was {second_review}");
    assert_eq!(second_review["questions"], json!(["what about Y?"]), "review was {second_review}");

    let requests = test.app.app.settings.lock().unwrap()["mock_spec_review_requests"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("mock_spec_review_requests should have been recorded"));
    assert_eq!(requests.len(), 2, "requests were {requests:?}");
    assert_eq!(
        requests[0]["session"], requests[1]["session"],
        "a second review of the same feature must resume the first review's architect session"
    );
    assert_eq!(requests[0]["session"], json!(session), "requests were {requests:?}");
}

#[test]
fn s14_invalid_feature_review_is_refused() {
    // S14: An invalid feature cannot be sent to spec review
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "invalid-feature";
    write_valid_feature(&features_dir, slug, "Invalid Feature");
    fs::write(features_dir.join(slug).join("milestones.md"), "## M1: Only milestone\n\nCovers: S99\n").unwrap();
    let project = test.path.display().to_string();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 409, "response was {resp}");
    let error = resp["error"].as_str().unwrap_or("");
    assert!(error.starts_with("feature is invalid:"), "response was {resp}");
    let reasons = resp["reasons"].as_array().unwrap_or_else(|| panic!("response was {resp}"));
    assert!(
        reasons.iter().any(|r| r.as_str().unwrap_or("").contains("S99")),
        "reasons were {reasons:?}"
    );

    assert!(
        test.app.app.settings.lock().unwrap()["mock_spec_review_requests"].is_null(),
        "an invalid feature must never start an agent"
    );
    assert!(
        test.app.app.settings.lock().unwrap()["mock_agent_requests"].is_null(),
        "an invalid feature must never start an agent"
    );
    assert!(!test.app.session.busy.load(Ordering::SeqCst), "a refused review must not leave the engine busy");
}

#[test]
fn s15_approve_spec_commits_only_feature_folder() {
    // S15: Approving the spec commits the feature folder
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "approve-feature";
    write_valid_feature(&features_dir, slug, "Approve Feature");
    let project = test.path.display().to_string();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    fs::write(test.path.join("other.txt"), "unrelated\n").unwrap();
    test.app.git(&["add", "other.txt"]).unwrap();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_spec",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["ok"], true, "response was {resp}");
    assert_eq!(resp["spec_status"], "spec approved", "response was {resp}");
    let commit = resp["commit"].as_str().unwrap_or_else(|| panic!("response was {resp}")).to_string();
    let content_hash = resp["content_hash"].as_str().unwrap_or_else(|| panic!("response was {resp}")).to_string();

    let head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(head, commit, "the returned commit must be HEAD");
    let subject = test.app.git(&["log", "-1", "--format=%s"]).unwrap();
    assert_eq!(subject, format!("docs(features): approve {slug} spec"));
    let files = test.app.git(&["show", "--name-only", "--format=", "HEAD"]).unwrap();
    for line in files.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with(&format!("docs/features/{slug}/")),
            "approval commit must only touch the feature folder, saw {line}"
        );
    }
    let staged = test.app.git(&["diff", "--cached", "--name-only"]).unwrap();
    assert_eq!(staged.trim(), "other.txt", "unrelated staged changes must survive the approval commit");

    let state_file = test.path.join(".forge/features").join(format!("{slug}.json"));
    let state: Value = serde_json::from_str(&fs::read_to_string(&state_file).unwrap()).unwrap();
    let approvals = state["approvals"].as_array().unwrap();
    let spec_approval = approvals
        .iter()
        .find(|a| a["kind"] == "spec")
        .unwrap_or_else(|| panic!("spec approval should be recorded, approvals were {approvals:?}"));
    assert_eq!(spec_approval["commit"], commit);
    assert_eq!(spec_approval["content_hash"], content_hash);
}

#[test]
fn s16_approve_spec_refused_without_current_approving_review() {
    // S16: The spec cannot be approved without an approving review of the current content
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let project = test.path.display().to_string();

    // No review at all.
    let slug_a = "no-review";
    write_valid_feature(&features_dir, slug_a, "No Review");
    let head_before = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_spec",
        json!({"project": project, "slug": slug_a}),
    );
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head_before, "a refused approval must not commit");

    // A rejecting review.
    let slug_b = "rejected-review";
    write_valid_feature(&features_dir, slug_b, "Rejected Review");
    test.app.app.settings.lock().unwrap()["mock_spec_review_output"] =
        json!({"approved": false, "summary": "no", "issues": ["bad"], "questions": []});
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug_b}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    let head_before_b = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_spec",
        json!({"project": project, "slug": slug_b}),
    );
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head_before_b, "a refused approval must not commit");

    // An approving review followed by an edit to the folder.
    let slug_c = "stale-review";
    write_valid_feature(&features_dir, slug_c, "Stale Review");
    test.app.app.settings.lock().unwrap().as_object_mut().unwrap().remove("mock_spec_review_output");
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug_c}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    fs::write(features_dir.join(slug_c).join("README.md"), "# Stale Review\n\nEdited after review.\n").unwrap();
    let head_before_c = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_spec",
        json!({"project": project, "slug": slug_c}),
    );
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head_before_c, "a refused approval must not commit");
}

#[test]
fn s17_approve_scenarios_records_scenario_ids() {
    // S17: Approving scenarios records their IDs
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "scenario-approval";
    write_valid_feature(&features_dir, slug, "Scenario Approval");
    let project = test.path.display().to_string();

    // Before spec approval.
    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_scenarios",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 409, "response was {resp}");

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/review",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    let (code, approve_resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_spec",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {approve_resp}");
    let spec_commit = approve_resp["commit"].as_str().unwrap().to_string();
    let content_hash = approve_resp["content_hash"].as_str().unwrap().to_string();

    let (code, resp) = api_request(
        &test.app.app,
        "POST",
        "/api/features/approve_scenarios",
        json!({"project": project, "slug": slug}),
    );
    assert_eq!(code, 200, "response was {resp}");
    assert_eq!(resp["ok"], true, "response was {resp}");
    assert_eq!(resp["spec_status"], "scenarios approved", "response was {resp}");
    assert_eq!(resp["commit"], spec_commit, "response was {resp}");
    assert_eq!(resp["content_hash"], content_hash, "response was {resp}");
    assert_eq!(resp["scenario_ids"], json!(["S1", "S2"]), "response was {resp}");

    let state_file = test.path.join(".forge/features").join(format!("{slug}.json"));
    let state: Value = serde_json::from_str(&fs::read_to_string(&state_file).unwrap()).unwrap();
    let approvals = state["approvals"].as_array().unwrap();
    let scenarios_approval = approvals
        .iter()
        .find(|a| a["kind"] == "scenarios")
        .unwrap_or_else(|| panic!("scenarios approval should be recorded, approvals were {approvals:?}"));
    assert_eq!(scenarios_approval["scenario_ids"], json!(["S1", "S2"]));
    assert_eq!(scenarios_approval["commit"], spec_commit);
    assert_eq!(scenarios_approval["content_hash"], content_hash);
}

#[test]
fn s18_editing_after_approval_reopens_to_draft() {
    // S18: Changing an approved spec reopens it
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "reopen-feature";
    write_valid_feature(&features_dir, slug, "Reopen Feature");
    let project = test.path.display().to_string();

    let (code, resp) = api_request(&test.app.app, "POST", "/api/features/review", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_spec", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_scenarios", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");

    let query = feature_state_query(&test.path, slug);
    let (_, before_state) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(before_state["spec_status"], "scenarios approved", "state was {before_state}");
    let approvals_before = before_state["state"]["approvals"].as_array().unwrap().len();

    fs::write(features_dir.join(slug).join("decisions.md"), "## D1: Manually edited\n\nReason.\n").unwrap();

    let (_, after_state) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(after_state["spec_status"], "draft", "state was {after_state}");
    assert_eq!(after_state["review_current"], false, "state was {after_state}");
    assert_eq!(
        after_state["state"]["approvals"].as_array().unwrap().len(),
        approvals_before,
        "approvals must remain as history"
    );

    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_spec", json!({"project": project, "slug": slug}));
    assert_eq!(code, 409, "response was {resp}");
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_scenarios", json!({"project": project, "slug": slug}));
    assert_eq!(code, 409, "response was {resp}");

    // Reopen once more, this time via a co-authoring write instead of a manual edit.
    test.app.app.settings.lock().unwrap()["mock_spec_review_output"] =
        json!({"approved": true, "summary": "ok", "issues": [], "questions": []});
    let (code, resp) = api_request(&test.app.app, "POST", "/api/features/review", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_spec", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_scenarios", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");

    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!({
        "reply": "tweaked",
        "files": [{"path": format!("docs/features/{slug}/README.md"), "content": "# Reopen Feature\n\nTweaked by chat.\n"}],
    });
    let (code, resp) = api_request(&test.app.app, "POST", "/api/features/chat", json!({"project": project, "slug": slug, "message": "tweak the readme"}));
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);

    let (_, final_state) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(
        final_state["spec_status"], "draft",
        "a co-authoring write must reopen an approved feature just like a manual edit, state was {final_state}"
    );
}

#[test]
fn s19_runtime_state_lives_outside_the_repository() {
    // S19: Runtime state is outside the repository
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "runtime-state-feature";
    write_valid_feature(&features_dir, slug, "Runtime State Feature");
    let project = test.path.display().to_string();

    let (code, resp) = api_request(&test.app.app, "POST", "/api/features/review", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_spec", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");
    let (code, resp) =
        api_request(&test.app.app, "POST", "/api/features/approve_scenarios", json!({"project": project, "slug": slug}));
    assert_eq!(code, 200, "response was {resp}");

    let state_file = test.path.join(".forge/features").join(format!("{slug}.json"));
    assert!(state_file.exists(), "runtime state must be written to .forge/features/{slug}.json");
    let parsed: Value =
        serde_json::from_str(&fs::read_to_string(&state_file).unwrap()).expect("runtime state file must be valid JSON");
    assert!(parsed["approvals"].as_array().unwrap().len() >= 2, "state was {parsed}");

    fn find_json_or_temp(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                find_json_or_temp(&path, found);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name.ends_with(".json") || name.ends_with(".tmp"))
            {
                found.push(path);
            }
        }
    }
    let mut stray = Vec::new();
    find_json_or_temp(&features_dir, &mut stray);
    assert!(stray.is_empty(), "no .json or temp file may live under docs/features/: {stray:?}");

    // A partial temp file next to the state file must not affect reads.
    let partial = test.path.join(".forge/features/.partial.tmp");
    fs::write(&partial, "{").unwrap();

    let query = feature_state_query(&test.path, slug);
    let (code, state) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_eq!(code, 200, "response was {state}");
    assert_eq!(
        state["state"]["approvals"].as_array().unwrap().len(),
        parsed["approvals"].as_array().unwrap().len(),
        "an unrelated partial temp file must not disturb reads of recorded state"
    );
    let _ = fs::remove_file(&partial);
}
