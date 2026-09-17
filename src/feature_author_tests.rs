//! Unit tests for the engine-owned co-authoring write path (M2, S11/S12,
//! decision D8): what the path validator accepts, every escape it refuses,
//! and that a refusal or a late change publishes nothing.
//! The end-to-end scenarios themselves are covered by the business tests in
//! src/feature_spec_m2_tests.rs.

use crate::app::feature_author::{MAX_CONTENT_BYTES, MAX_FILES, check_path, publish, validate_response};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

struct Temp(PathBuf);

impl Temp {
    /// A project root with `docs/features/<slug>/` already in place, like the
    /// folder a created feature has.
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "forge-feature-author-{label}-{}",
            crate::durable_json::identity()
        ));
        fs::create_dir_all(path.join("docs/features/spec-one")).unwrap();
        fs::write(path.join("docs/features/spec-one/README.md"), "# Spec One\n").unwrap();
        Self(path)
    }

    fn project(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn response(files: Value) -> String {
    json!({"reply": "done", "files": files}).to_string()
}

fn refusal(project: &Path, path: &str) -> String {
    let error = check_path(project, "spec-one", path).expect_err(&format!("{path} must be refused"));
    if !path.is_empty() && !path.contains('\0') {
        assert!(error.contains(path), "a refusal must name the path, got {error:?}");
    }
    error
}

#[test]
fn accepts_paths_inside_the_feature_folder() {
    let temp = Temp::new("accept");
    for path in [
        "docs/features/spec-one/README.md",
        "docs/features/spec-one/scenarios.md",
        "docs/features/spec-one/design/screen.md",
        "docs/features/spec-one/a/b/c.md",
    ] {
        let target = check_path(temp.project(), "spec-one", path).expect(path);
        assert_eq!(target, temp.project().join(path));
    }
}

#[test]
fn refuses_absolute_traversal_and_foreign_paths() {
    let temp = Temp::new("refuse");
    let project = temp.project();
    refusal(project, "/etc/passwd");
    refusal(project, &format!("{}/escape.md", project.join("docs").display()));
    refusal(project, "docs/features/spec-one/../escape.md");
    refusal(project, "../escape.md");
    refusal(project, "docs/features/spec-one/./README.md");
    refusal(project, "docs/features/spec-one//README.md");
    refusal(project, "docs/features/other-spec/x.md");
    // A prefix that merely starts with the slug is a different folder.
    fs::create_dir_all(project.join("docs/features/spec-one-x")).unwrap();
    refusal(project, "docs/features/spec-one-x/x.md");
    // The folder itself, and the levels above it, are never a write target.
    refusal(project, "docs/features/spec-one");
    refusal(project, "docs/features/spec-one/");
    refusal(project, "docs/README.md");
    refusal(project, "README.md");
    refusal(project, "");
    refusal(project, "docs/features/spec-one/\0.md");
}

#[test]
fn refuses_a_symlinked_intermediate_directory() {
    let temp = Temp::new("link-dir");
    let project = temp.project();
    let outside = project.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs/features/spec-one/link")).unwrap();

    let error = refusal(project, "docs/features/spec-one/link/evil.md");
    assert!(error.contains("symlink"), "got {error:?}");
    // A directory that only exists behind the link is refused as well.
    refusal(project, "docs/features/spec-one/link/deeper/evil.md");
    assert!(!outside.join("evil.md").exists());
}

#[test]
fn refuses_a_symlinked_target_file() {
    let temp = Temp::new("link-file");
    let project = temp.project();
    let outside = project.join("outside.md");
    fs::write(&outside, "original\n").unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs/features/spec-one/link.md")).unwrap();

    let error = refusal(project, "docs/features/spec-one/link.md");
    assert!(error.contains("symlink"), "got {error:?}");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original\n");
}

#[test]
fn refuses_a_symlinked_feature_folder() {
    let temp = Temp::new("link-root");
    let project = temp.project();
    let outside = project.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs/features/linked")).unwrap();

    let error = check_path(project, "linked", "docs/features/linked/README.md")
        .expect_err("a symlinked feature folder must be refused");
    assert!(error.contains("symlink"), "got {error:?}");

    // The same holds one level up: a symlinked docs/features/ escapes too.
    let other = Temp::new("link-features");
    let project = other.project();
    let outside = project.join("elsewhere");
    fs::create_dir_all(outside.join("spec-two")).unwrap();
    fs::remove_dir_all(project.join("docs/features")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs/features")).unwrap();
    let error = check_path(project, "spec-two", "docs/features/spec-two/README.md")
        .expect_err("a symlinked docs/features/ must be refused");
    assert!(error.contains("symlink"), "got {error:?}");
}

#[test]
fn refuses_a_target_that_is_not_a_regular_file() {
    let temp = Temp::new("not-file");
    let project = temp.project();
    fs::create_dir_all(project.join("docs/features/spec-one/design")).unwrap();
    let error = refusal(project, "docs/features/spec-one/design");
    assert!(error.contains("not a regular file"), "got {error:?}");

    fs::write(project.join("docs/features/spec-one/notes.md"), "x").unwrap();
    let error = refusal(project, "docs/features/spec-one/notes.md/child.md");
    assert!(error.contains("not a directory"), "got {error:?}");
}

#[test]
fn validation_enforces_the_response_shape() {
    let temp = Temp::new("shape");
    let project = temp.project();
    let valid = "docs/features/spec-one/notes.md";

    let ok = validate_response(project, "spec-one", &response(json!([{"path": valid, "content": "x"}])))
        .expect("a valid response is accepted");
    assert_eq!(ok.reply, "done");
    assert_eq!(ok.files, vec![(valid.to_string(), "x".to_string())]);

    // An empty write set is the normal "nothing to change" answer.
    let none = validate_response(project, "spec-one", &response(json!([]))).unwrap();
    assert!(none.files.is_empty());

    let reject = |text: String, needle: &str| {
        let error = validate_response(project, "spec-one", &text)
            .err()
            .unwrap_or_else(|| panic!("must be refused: {text}"));
        assert!(error.contains(needle), "got {error:?} for {text}");
    };
    reject("not json".into(), "invalid co-authoring JSON");
    reject(json!({"reply": "hi", "files": [], "extra": 1}).to_string(), "unknown response field");
    reject(
        json!({"reply": "hi", "files": [{"path": valid, "content": "x", "mode": "append"}]}).to_string(),
        "unknown response field",
    );
    reject(json!({"files": []}).to_string(), "non-empty reply");
    reject(json!({"reply": "   ", "files": []}).to_string(), "non-empty reply");
    reject(json!({"reply": "hi", "files": {}}).to_string(), "must be an array");
    reject(response(json!([{"path": valid}])), "needs a `content` string");
    reject(response(json!([{"path": valid, "content": 7}])), "needs a `content` string");
    reject(response(json!([{"content": "x"}])), "needs a `path` string");
    reject(
        response(json!([{"path": valid, "content": "a"}, {"path": valid, "content": "b"}])),
        "duplicate proposed file path",
    );
    reject(
        response(json!([{"path": "docs/features/other/x.md", "content": "x"}])),
        "not a file inside docs/features/spec-one/",
    );
}

#[test]
fn validation_enforces_the_count_and_size_bounds() {
    let temp = Temp::new("bounds");
    let project = temp.project();
    let file = |index: usize| json!({"path": format!("docs/features/spec-one/f{index}.md"), "content": "x"});

    let most: Vec<Value> = (0..MAX_FILES).map(file).collect();
    let ok = validate_response(project, "spec-one", &response(json!(most))).expect("the maximum count is accepted");
    assert_eq!(ok.files.len(), MAX_FILES);

    let too_many: Vec<Value> = (0..MAX_FILES + 1).map(file).collect();
    let error = validate_response(project, "spec-one", &response(json!(too_many))).unwrap_err();
    assert!(error.contains("too many files"), "got {error:?}");

    let largest = "a".repeat(MAX_CONTENT_BYTES);
    validate_response(
        project,
        "spec-one",
        &response(json!([{"path": "docs/features/spec-one/big.md", "content": largest}])),
    )
    .expect("the maximum size is accepted");
    let too_big = "a".repeat(MAX_CONTENT_BYTES + 1);
    let error = validate_response(
        project,
        "spec-one",
        &response(json!([{"path": "docs/features/spec-one/big.md", "content": too_big}])),
    )
    .unwrap_err();
    assert!(error.contains("too large"), "got {error:?}");
}

#[test]
fn publication_creates_missing_directories_and_leaves_no_temp_file() {
    let temp = Temp::new("publish");
    let project = temp.project();
    let files = vec![
        ("docs/features/spec-one/README.md".to_string(), "# Spec One\n\nNew.\n".to_string()),
        ("docs/features/spec-one/design/notes.md".to_string(), "notes\n".to_string()),
    ];
    publish(project, "spec-one", &files).expect("a validated set publishes");
    assert_eq!(
        fs::read_to_string(project.join("docs/features/spec-one/README.md")).unwrap(),
        "# Spec One\n\nNew.\n"
    );
    assert_eq!(
        fs::read_to_string(project.join("docs/features/spec-one/design/notes.md")).unwrap(),
        "notes\n"
    );
    let mut names: Vec<String> = fs::read_dir(project.join("docs/features/spec-one"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["README.md".to_string(), "design".to_string()]);
}

#[test]
fn publication_writes_nothing_when_a_late_check_fails() {
    let temp = Temp::new("toctou");
    let project = temp.project();
    let outside = project.join("outside.md");
    fs::write(&outside, "original\n").unwrap();
    let files = vec![
        ("docs/features/spec-one/first.md".to_string(), "first\n".to_string()),
        ("docs/features/spec-one/second.md".to_string(), "second\n".to_string()),
    ];
    // Validation would accept both; the second target becomes a symlink out
    // of the folder before the write, as a racing agent or user could do.
    std::os::unix::fs::symlink(&outside, project.join("docs/features/spec-one/second.md")).unwrap();

    let error = publish(project, "spec-one", &files).expect_err("a late symlink must refuse the write set");
    assert!(error.contains("symlink"), "got {error:?}");
    assert!(
        !project.join("docs/features/spec-one/first.md").exists(),
        "no part of a refused write set may be published"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original\n");
}

/// A feature folder that discovery lists, with the four required documents.
fn write_feature(project: &Path, slug: &str) {
    let dir = project.join("docs/features").join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), format!("# {slug}\n\n## Goal\n\nDo the thing.\n")).unwrap();
    fs::write(
        dir.join("scenarios.md"),
        "# Scenarios\n\n## S1: First\n\n- Given: a start\n- When: it happens\n- Then: it holds\n",
    )
    .unwrap();
    fs::write(dir.join("decisions.md"), "# Decisions\n\n## D1: First\n\nDecision.\n").unwrap();
    fs::write(dir.join("milestones.md"), "# Milestones\n\n## M1: First\n\nCovers: S1\n").unwrap();
}

#[test]
fn chat_reopens_an_approved_feature_and_records_the_transcript() {
    // S18 (co-authoring case): status derives from the folder's content hash,
    // so a published chat write returns an approved feature to draft while its
    // approvals stay as history. The full scenario, which starts with an
    // architect review, is covered by the S18 business test once the review
    // and approval endpoints exist.
    let test = crate::test_support::QueueTest::new(false);
    let slug = "chat-reopen";
    write_feature(&test.path, slug);
    let hash = crate::feature_state::content_hash(&test.path.join("docs/features").join(slug)).unwrap();
    crate::feature_state::update(&test.app, slug, |state| {
        let approvals = state["approvals"].as_array_mut().unwrap();
        for kind in ["spec", "scenarios"] {
            approvals.push(json!({"kind": kind, "commit": "0".repeat(40), "content_hash": hash, "unix": 1}));
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(crate::feature_state::snapshot(&test.app, slug).unwrap().spec_status, "scenarios approved");

    let head_before = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    test.app.app.settings.lock().unwrap()["mock_chat_output"] = json!({
        "reply": "Added a decision.",
        "files": [{"path": format!("docs/features/{slug}/decisions.md"),
            "content": "# Decisions\n\n## D1: First\n\nDecision.\n\n## D2: Second\n\nDecision.\n"}],
    });
    let (code, resp) = crate::test_support::api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": test.path.display().to_string(), "slug": slug, "message": "add a decision"}),
    );
    assert_eq!(code, 200, "response was {resp}");
    crate::test_support::wait_for_worker(&test.app);

    let after = crate::feature_state::snapshot(&test.app, slug).unwrap();
    assert_eq!(after.spec_status, "draft", "a chat write must reopen the spec");
    assert_eq!(after.state["approvals"].as_array().unwrap().len(), 2, "approvals stay as history");
    assert_ne!(after.content_hash, hash);
    let chat = after.state["chat"].as_array().unwrap();
    assert_eq!(chat.len(), 2, "chat was {chat:?}");
    assert_eq!(chat[0]["role"], "user");
    assert_eq!(chat[0]["text"], "add a decision");
    assert_eq!(chat[1]["role"], "assistant");
    assert_eq!(chat[1]["text"], "Added a decision.");
    assert_eq!(chat[1]["files"], json!([format!("docs/features/{slug}/decisions.md")]));
    let activity = crate::feature_state::activity(&test.app);
    assert_eq!(activity["kind"], "chat", "activity was {activity}");
    assert_eq!(activity["status"], "ready", "activity was {activity}");
    assert_eq!(
        test.app.git(&["rev-parse", "HEAD"]).unwrap(),
        head_before,
        "co-authoring never commits"
    );
    assert!(test.app.git(&["diff", "--cached", "--name-only"]).unwrap().is_empty(), "and stages nothing");

    // The agent ran read-only in the shared chat role.
    let settings = test.app.app.settings.lock().unwrap();
    let prompt = settings["mock_chat_prompt"].as_str().unwrap();
    assert!(prompt.contains(&format!("docs/features/{slug}")), "prompt was {prompt}");
    assert!(prompt.contains("## D1: First"), "the prompt must quote the folder, got {prompt}");
    assert!(prompt.contains("READ-ONLY"), "prompt was {prompt}");
}

#[test]
fn chat_rejects_an_unknown_feature_an_empty_message_and_an_invalid_slug() {
    let test = crate::test_support::QueueTest::new(false);
    let project = test.path.display().to_string();
    write_feature(&test.path, "chat-guard");
    let post = |body: Value| crate::test_support::api_request(&test.app.app, "POST", "/api/features/chat", body);

    let (code, resp) = post(json!({"project": project, "slug": "Not A Slug", "message": "hi"}));
    assert_eq!(code, 400, "response was {resp}");
    let (code, resp) = post(json!({"project": project, "slug": "chat-guard", "message": "   "}));
    assert_eq!(code, 400, "response was {resp}");
    assert_eq!(resp["error"], "message required");
    let (code, resp) =
        post(json!({"project": project, "slug": "chat-guard", "message": "x".repeat(20001)}));
    assert_eq!(code, 400, "response was {resp}");
    assert_eq!(resp["error"], "message too long");
    let (code, resp) = post(json!({"project": project, "slug": "no-such-feature", "message": "hi"}));
    assert_eq!(code, 404, "response was {resp}");

    // Nothing was started, so no agent ran and no state file was created.
    assert!(test.app.app.settings.lock().unwrap()["mock_chat_prompt"].is_null());
    assert!(!test.path.join(".forge/features").exists());
    assert!(!test.app.session.busy.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn chat_refuses_a_second_request_while_a_worker_is_busy() {
    let test = crate::test_support::QueueTest::new(false);
    write_feature(&test.path, "chat-busy");
    test.app.acquire_busy().unwrap();
    let (code, resp) = crate::test_support::api_request(
        &test.app.app,
        "POST",
        "/api/features/chat",
        json!({"project": test.path.display().to_string(), "slug": "chat-busy", "message": "hi"}),
    );
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(resp["error"], "busy");
}
