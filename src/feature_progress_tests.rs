//! Milestone progress: the `Status:` line under each `## M<n>` heading of
//! `milestones.md`, its validation and the `progress` and `milestones`
//! fields of `/api/features`.

use crate::features;
use crate::test_support::{QueueTest, api_request};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn write_feature(features_dir: &Path, slug: &str, milestones: &str) {
    let dir = features_dir.join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), format!("# {slug}\n")).unwrap();
    fs::write(dir.join("scenarios.md"), "## S1: One\n\n## S2: Two\n").unwrap();
    fs::write(dir.join("decisions.md"), "## D1: A decision\n").unwrap();
    fs::write(dir.join("milestones.md"), milestones).unwrap();
}

fn listed(test: &QueueTest, slug: &str) -> Value {
    let (code, resp) = api_request(&test.app.app, "GET", "/api/features", json!({}));
    assert_eq!(code, 200, "response was {resp}");
    resp["features"].as_array().unwrap().iter().find(|f| f["slug"] == slug).cloned().unwrap()
}

/// The id, title and status of each milestone, without its later-added keys.
fn milestone_projection(feature: &Value) -> Value {
    Value::Array(feature["milestones"].as_array().unwrap().iter()
        .map(|m| json!({"id": m["id"], "title": m["title"], "status": m["status"]})).collect())
}

#[test]
fn milestone_status_lines_drive_feature_progress() {
    let test = QueueTest::new(false);
    let dir = test.path.join("docs/features");
    write_feature(&dir, "done", "## M1: First\n\nStatus: implemented\n\nCovers: S1\n\n## M2: Second\n\nStatus: implemented\nCovers: S2\n");
    write_feature(&dir, "partial", "## M1: First\n\nStatus: implemented\nCovers: S1\n\n## M2: Second\n\nStatus: planned\nCovers: S2\n");
    write_feature(&dir, "fresh", "## M1: First\n\nCovers: S1, S2\n");

    let done = listed(&test, "done");
    assert_eq!(done["status"], "valid", "reasons: {}", done["reasons"]);
    assert_eq!(done["progress"], "implemented");
    assert_eq!(
        milestone_projection(&done),
        json!([
            {"id": "M1", "title": "First", "status": "implemented"},
            {"id": "M2", "title": "Second", "status": "implemented"},
        ])
    );

    let partial = listed(&test, "partial");
    assert_eq!(partial["progress"], "in progress");
    assert_eq!(partial["milestones"][1]["status"], "planned");

    // A milestone without a Status line is planned, so older specs stay valid.
    let fresh = listed(&test, "fresh");
    assert_eq!(fresh["status"], "valid", "reasons: {}", fresh["reasons"]);
    assert_eq!(fresh["progress"], "planned");
    assert_eq!(milestone_projection(&fresh), json!([{"id": "M1", "title": "First", "status": "planned"}]));
    // The milestone also carries its covered scenarios, registered tests and plan link.
    assert_eq!(fresh["milestones"][0]["covers"], json!(["S1", "S2"]));
    assert_eq!(fresh["milestones"][0]["business_tests"], json!([]));
    assert_eq!(fresh["milestones"][0]["plan"], Value::Null);
}

#[test]
fn malformed_or_repeated_status_lines_are_invalid() {
    let test = QueueTest::new(false);
    let dir = test.path.join("docs/features");
    write_feature(&dir, "typo", "## M1: First\n\nStatus: done\nCovers: S1\n");
    write_feature(&dir, "twice", "## M1: First\n\nStatus: planned\nStatus: implemented\nCovers: S1\n");
    write_feature(&dir, "orphan", "Status: implemented\n\n## M1: First\n\nCovers: S1\n");

    let reasons = |slug: &str| {
        let feature = listed(&test, slug);
        assert_eq!(feature["status"], "invalid", "{slug}: {feature}");
        feature["reasons"].to_string()
    };
    assert!(reasons("typo").contains("malformed Status in M1: done"));
    assert!(reasons("twice").contains("repeated Status line in M1"));
    assert!(reasons("orphan").contains("Status line outside a milestone"));
    // A rejected Status value never counts as implemented.
    assert_eq!(listed(&test, "typo")["progress"], "planned");
}

#[test]
fn status_lines_inside_fences_are_ignored() {
    let test = QueueTest::new(false);
    let dir = test.path.join("docs/features");
    write_feature(&dir, "fenced", "## M1: First\n\n```\nStatus: implemented\n```\n\nCovers: S1\n");
    let feature = listed(&test, "fenced");
    assert_eq!(feature["status"], "valid", "reasons: {}", feature["reasons"]);
    assert_eq!(feature["progress"], "planned");
}

#[test]
fn repository_feature_specs_report_their_progress() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = features::discover(repo_root);
    let progress = |slug: &str| {
        let feature = found.iter().find(|f| f.slug == slug).expect(slug);
        assert_eq!(feature.status(), "valid", "{slug}: {:?}", feature.reasons);
        feature.progress()
    };
    assert_eq!(progress("panel-redesign"), "implemented");
    assert_eq!(progress("feature-specs"), "implemented");
}
