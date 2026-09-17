//! Unit tests for spec and scenario approvals (M2, D10): the Git discipline
//! of a path-scoped approval commit, the immutability of a refused approval,
//! and the idempotency of a retry after a published commit. The scenarios
//! themselves are covered by the business tests in src/feature_spec_m2_tests.rs.

use crate::features::scenario_ids;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// A feature folder that passes M1 validation, written directly so these unit
/// tests do not depend on the create endpoint or on the repository template.
fn write_feature(features_dir: &Path, slug: &str) {
    let dir = features_dir.join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), format!("# {slug}\n\nGoal.\n")).unwrap();
    fs::write(
        dir.join("scenarios.md"),
        "# Scenarios\n\n## S1: One\n\n- Given: a\n- When: b\n- Then: c\n\n\
         ## S2: Two\n\n- Given: a\n- When: b\n- Then: c\n",
    )
    .unwrap();
    fs::write(dir.join("decisions.md"), "# Decisions\n\n## D1: One\n\nReason.\n").unwrap();
    fs::write(dir.join("milestones.md"), "# Milestones\n\n## M1: One\n\nCovers: S1, S2\n").unwrap();
}

fn post(test: &QueueTest, path: &str, slug: &str) -> (u16, Value) {
    api_request(
        &test.app.app,
        "POST",
        path,
        json!({"project": test.path.display().to_string(), "slug": slug}),
    )
}

/// An approved feature, reviewed with the mock provider's default verdict.
fn reviewed(test: &QueueTest, slug: &str) {
    let (code, resp) = post(test, "/api/features/review", slug);
    assert_eq!(code, 200, "response was {resp}");
    wait_for_worker(&test.app);
}

fn state_of(test: &QueueTest, slug: &str) -> Value {
    let path = test.path.join(".forge/features").join(format!("{slug}.json"));
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn approval_commit_preserves_unrelated_staged_and_unstaged_changes() {
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "keeps-work";
    write_feature(&features_dir, slug);

    // A tracked file with an unstaged edit, plus a staged new file: neither
    // may be swept into the path-scoped approval commit.
    fs::write(test.path.join("tracked.txt"), "one\n").unwrap();
    test.app.git(&["add", "tracked.txt"]).unwrap();
    test.app.git(&["commit", "-qm", "add tracked"]).unwrap();
    fs::write(test.path.join("tracked.txt"), "one\ntwo\n").unwrap();
    fs::write(test.path.join("staged.txt"), "staged\n").unwrap();
    test.app.git(&["add", "staged.txt"]).unwrap();

    reviewed(&test, slug);
    let (code, resp) = post(&test, "/api/features/approve_spec", slug);
    assert_eq!(code, 200, "response was {resp}");

    let files = test.app.git(&["show", "--name-only", "--format=", "HEAD"]).unwrap();
    assert!(
        files
            .lines()
            .filter(|line| !line.trim().is_empty())
            .all(|line| line.starts_with(&format!("docs/features/{slug}/"))),
        "the approval commit touched more than the feature folder: {files}"
    );
    assert_eq!(
        test.app.git(&["diff", "--cached", "--name-only"]).unwrap().trim(),
        "staged.txt",
        "an unrelated staged file must stay staged"
    );
    assert_eq!(
        test.app.git(&["diff", "--name-only"]).unwrap().trim(),
        "tracked.txt",
        "an unrelated unstaged edit must stay unstaged"
    );
    assert_eq!(fs::read_to_string(test.path.join("tracked.txt")).unwrap(), "one\ntwo\n");
}

#[test]
fn refused_approval_leaves_head_index_and_state_untouched() {
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "no-review-yet";
    write_feature(&features_dir, slug);
    let head = test.app.git(&["rev-parse", "HEAD"]).unwrap();

    let (code, resp) = post(&test, "/api/features/approve_spec", slug);
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(resp["error"], "no architect spec review", "response was {resp}");
    let (code, resp) = post(&test, "/api/features/approve_scenarios", slug);
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(resp["error"], "spec is not approved", "response was {resp}");

    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head, "a refusal must not commit");
    assert_eq!(
        test.app.git(&["diff", "--cached", "--name-only"]).unwrap().trim(),
        "",
        "a refusal must not stage the feature folder"
    );
    assert!(
        !test.path.join(".forge/features").join(format!("{slug}.json")).exists(),
        "a refusal must not record anything"
    );
    assert!(!test.app.session.busy.load(std::sync::atomic::Ordering::SeqCst), "busy must be released");
}

#[test]
fn approving_an_unchanged_folder_again_records_head_without_a_new_commit() {
    // Recovery path of M2-RISK-GIT-STATE: a retry after a commit whose state
    // publication failed must record the existing commit, not create another.
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "retry-safe";
    write_feature(&features_dir, slug);
    reviewed(&test, slug);

    let (code, first) = post(&test, "/api/features/approve_spec", slug);
    assert_eq!(code, 200, "response was {first}");
    let commit = first["commit"].as_str().unwrap().to_string();
    let count = test.app.git(&["rev-list", "--count", "HEAD"]).unwrap();

    let (code, second) = post(&test, "/api/features/approve_spec", slug);
    assert_eq!(code, 200, "response was {second}");
    assert_eq!(second["commit"], json!(commit), "a clean retry must record the same commit");
    assert_eq!(
        test.app.git(&["rev-list", "--count", "HEAD"]).unwrap(),
        count,
        "a clean retry must not create another commit"
    );
    let approvals = state_of(&test, slug)["approvals"].as_array().unwrap().len();
    assert_eq!(approvals, 2, "approvals are append-only history");
}

#[test]
fn scenario_approval_refuses_a_spec_approval_of_older_content() {
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "stale-spec-approval";
    write_feature(&features_dir, slug);
    reviewed(&test, slug);
    let (code, resp) = post(&test, "/api/features/approve_spec", slug);
    assert_eq!(code, 200, "response was {resp}");

    fs::write(features_dir.join(slug).join("README.md"), format!("# {slug}\n\nEdited.\n")).unwrap();
    let head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    let (code, resp) = post(&test, "/api/features/approve_scenarios", slug);
    assert_eq!(code, 409, "response was {resp}");
    assert_eq!(resp["error"], "feature changed since spec approval", "response was {resp}");
    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head, "a refusal must not commit");
}

#[test]
fn approving_an_invalid_feature_is_refused_with_its_reasons() {
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    let slug = "invalid-feature";
    write_feature(&features_dir, slug);
    reviewed(&test, slug);
    fs::write(
        features_dir.join(slug).join("milestones.md"),
        "# Milestones\n\n## M1: One\n\nCovers: S9\n",
    )
    .unwrap();

    let head = test.app.git(&["rev-parse", "HEAD"]).unwrap();
    for path in ["/api/features/approve_spec", "/api/features/approve_scenarios"] {
        let (code, resp) = post(&test, path, slug);
        assert_eq!(code, 409, "response was {resp}");
        assert!(
            resp["error"].as_str().unwrap().starts_with("feature is invalid:"),
            "response was {resp}"
        );
        assert!(!resp["reasons"].as_array().unwrap().is_empty(), "response was {resp}");
    }
    assert_eq!(test.app.git(&["rev-parse", "HEAD"]).unwrap(), head, "a refusal must not commit");
}

#[test]
fn approval_endpoints_validate_the_slug_and_the_feature() {
    let test = QueueTest::new(false);
    fs::create_dir_all(test.path.join("docs/features")).unwrap();
    for path in ["/api/features/approve_spec", "/api/features/approve_scenarios"] {
        let (code, resp) = post(&test, path, "Not A Slug");
        assert_eq!(code, 400, "response was {resp}");
        let (code, resp) = post(&test, path, "missing-feature");
        assert_eq!(code, 404, "response was {resp}");
    }
}

#[test]
fn scenario_ids_are_returned_in_document_order_outside_fences() {
    let content = "# Scenarios\n\n## S3: Third\n\ntext\n\n```\n## S99: fenced\n```\n\n\
                   ## S1: First\n\n## S3: Repeated\n\n## Notes\n\n## SX: malformed\n";
    assert_eq!(scenario_ids(content), vec!["S3".to_string(), "S1".to_string()]);
    assert!(scenario_ids("no headings here\n").is_empty());
}
