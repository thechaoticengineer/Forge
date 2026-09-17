//! Business tests for milestone M1 (format and discovery) scenarios S1-S9.
//! See docs/features/feature-specs/scenarios.md for the full scenario text.

use crate::features;
use crate::test_support::{QueueTest, api_request, wait_for_worker};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn write_file(features_dir: &Path, slug: &str, name: &str, content: &str) {
    let dir = features_dir.join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), content).unwrap();
}

fn write_valid_feature(features_dir: &Path, slug: &str, title: &str) {
    write_file(features_dir, slug, "README.md", &format!("# {title}\n\nGoal.\n"));
    write_file(features_dir, slug, "scenarios.md",
        "## S1: First scenario\n\nGiven/When/Then.\n\n## S2: Second scenario\n\nGiven/When/Then.\n");
    write_file(features_dir, slug, "decisions.md", "## D1: A decision\n\nReason.\n");
    write_file(features_dir, slug, "milestones.md",
        "## M1: First milestone\n\nCovers: S1, S2\n\n## M2: Second milestone\n\nCovers: none yet\n");
}

/// A unique temporary project root holding a `docs/features/` folder,
/// following the QueueTest fixture pattern: temp_dir + pid + counter,
/// removed on Drop.
struct FeatureFixture {
    root: PathBuf,
}

impl FeatureFixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "forge-feature-spec-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst),
        ));
        fs::create_dir_all(root.join("docs/features")).unwrap();
        Self { root }
    }

    fn features_dir(&self) -> PathBuf {
        self.root.join("docs/features")
    }

    fn write_file(&self, slug: &str, name: &str, content: &str) {
        write_file(&self.features_dir(), slug, name, content);
    }

    fn valid_feature(&self, slug: &str, title: &str) {
        write_valid_feature(&self.features_dir(), slug, title);
    }
}

impl Drop for FeatureFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Recursively snapshots every file under `dir` as (relative path, bytes,
/// mtime), used to prove a read-only scan touched nothing.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>, std::time::SystemTime)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap().to_path_buf();
                let bytes = fs::read(&path).unwrap();
                out.push((rel, bytes, meta.modified().unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
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

#[test]
fn s1_valid_feature_folder_is_discovered() {
    // S1: A valid feature folder is discovered
    let fx = FeatureFixture::new();
    fx.valid_feature("sample-feature", "Sample Feature");

    let found = features::discover(&fx.root);
    let sample = found
        .iter()
        .find(|f| f.slug == "sample-feature")
        .expect("a valid feature folder should be discovered");
    assert_eq!(sample.title, "Sample Feature");
    assert_eq!(sample.path, fx.features_dir().join("sample-feature"));
    assert_eq!(sample.status(), "valid");
    assert!(sample.reasons.is_empty());

    // A README without any `# ` heading is titled by its slug.
    fx.write_file("no-heading", "README.md", "No heading in this file.\n");
    fx.write_file("no-heading", "scenarios.md", "## S1: Only scenario\n\nGiven/When/Then.\n");
    fx.write_file("no-heading", "decisions.md", "## D1: A decision\n\nReason.\n");
    fx.write_file("no-heading", "milestones.md", "## M1: Only milestone\n\nCovers: S1\n");
    let found = features::discover(&fx.root);
    let no_heading = found
        .iter()
        .find(|f| f.slug == "no-heading")
        .expect("a feature folder without a heading should still be discovered");
    assert_eq!(no_heading.title, "no-heading");

    // A `# ` line inside a fenced code block is not treated as the title.
    fx.write_file(
        "fenced-heading",
        "README.md",
        "```\n# Not a title\n```\n\n# Real Title\n\nGoal.\n",
    );
    fx.write_file("fenced-heading", "scenarios.md", "## S1: Only scenario\n\nGiven/When/Then.\n");
    fx.write_file("fenced-heading", "decisions.md", "## D1: A decision\n\nReason.\n");
    fx.write_file("fenced-heading", "milestones.md", "## M1: Only milestone\n\nCovers: S1\n");
    let found = features::discover(&fx.root);
    let fenced = found
        .iter()
        .find(|f| f.slug == "fenced-heading")
        .expect("a feature folder with a fenced heading should still be discovered");
    assert_eq!(fenced.title, "Real Title");
}

#[test]
fn s2_underscore_folders_are_ignored() {
    // S2: Folders starting with an underscore are ignored
    let fx = FeatureFixture::new();
    fx.valid_feature("visible-feature", "Visible Feature");
    fx.valid_feature("_template", "Template");
    fx.valid_feature("_drafts", "Drafts");

    let found = features::discover(&fx.root);
    assert!(found.iter().any(|f| f.slug == "visible-feature"));
    assert!(!found.iter().any(|f| f.slug == "_template"));
    assert!(!found.iter().any(|f| f.slug == "_drafts"));
}

#[test]
fn s3_missing_required_file_is_invalid() {
    // S3: A folder missing a required file is reported invalid
    for missing in ["README.md", "scenarios.md", "decisions.md", "milestones.md"] {
        let fx = FeatureFixture::new();
        fx.valid_feature("incomplete", "Incomplete");
        fs::remove_file(fx.features_dir().join("incomplete").join(missing)).unwrap();

        let found = features::discover(&fx.root);
        let incomplete = found
            .iter()
            .find(|f| f.slug == "incomplete")
            .unwrap_or_else(|| panic!("folder missing {missing} should still be discovered"));
        assert_eq!(incomplete.status(), "invalid", "missing {missing}");
        assert!(
            incomplete.reasons.iter().any(|r| r.contains(missing)),
            "missing {missing}: reasons were {:?}",
            incomplete.reasons
        );
    }
}

#[test]
fn s4_duplicate_or_malformed_scenario_ids_are_invalid() {
    // S4: A scenarios.md with duplicate or malformed IDs is reported invalid
    let fx = FeatureFixture::new();
    fx.valid_feature("dup-ids", "Dup Ids");
    fx.write_file(
        "dup-ids",
        "scenarios.md",
        "## S2: First\n\nGiven/When/Then.\n\n## S2: Second\n\nGiven/When/Then.\n",
    );
    fx.write_file("dup-ids", "milestones.md", "## M1: Only milestone\n\nCovers: S2\n");
    let found = features::discover(&fx.root);
    let dup = found.iter().find(|f| f.slug == "dup-ids").expect("discovered");
    assert_eq!(dup.status(), "invalid");
    assert!(dup.reasons.iter().any(|r| r.contains("S2")), "reasons were {:?}", dup.reasons);

    let fx2 = FeatureFixture::new();
    fx2.valid_feature("bad-ids", "Bad Ids");
    fx2.write_file(
        "bad-ids",
        "scenarios.md",
        "## X3: bad\n\nGiven/When/Then.\n\n## S3a: bad\n\nGiven/When/Then.\n",
    );
    fx2.write_file("bad-ids", "milestones.md", "## M1: Only milestone\n\nCovers: none yet\n");
    let found = features::discover(&fx2.root);
    let bad = found.iter().find(|f| f.slug == "bad-ids").expect("discovered");
    assert_eq!(bad.status(), "invalid");
    assert!(bad.reasons.iter().any(|r| r.contains("X3")), "reasons were {:?}", bad.reasons);
    assert!(bad.reasons.iter().any(|r| r.contains("S3a")), "reasons were {:?}", bad.reasons);
}

#[test]
fn s5_unknown_covers_reference_is_invalid() {
    // S5: A milestones.md referencing an unknown scenario ID is reported invalid
    let fx = FeatureFixture::new();
    fx.valid_feature("unknown-covers", "Unknown Covers");
    fx.write_file("unknown-covers", "milestones.md", "## M1: Only milestone\n\nCovers: S1, S9\n");
    let found = features::discover(&fx.root);
    let unknown = found.iter().find(|f| f.slug == "unknown-covers").expect("discovered");
    assert_eq!(unknown.status(), "invalid");
    assert!(
        unknown.reasons.iter().any(|r| r.contains("S9")),
        "reasons were {:?}",
        unknown.reasons
    );

    let fx2 = FeatureFixture::new();
    fx2.valid_feature("none-yet", "None Yet");
    fx2.write_file("none-yet", "milestones.md", "## M1: Only milestone\n\nCovers: none yet\n");
    let found = features::discover(&fx2.root);
    let none_yet = found.iter().find(|f| f.slug == "none-yet").expect("discovered");
    assert_eq!(none_yet.status(), "valid");
    assert!(none_yet.reasons.is_empty());
}

#[test]
fn s6_api_lists_features_read_only() {
    // S6: The read-only API lists features without modifying files
    let test = QueueTest::new(false);
    let features_dir = test.path.join("docs/features");
    fs::create_dir_all(&features_dir).unwrap();
    write_valid_feature(&features_dir, "alpha-feature", "Alpha Feature");
    write_valid_feature(&features_dir, "beta-feature", "Beta Feature");
    fs::remove_file(features_dir.join("beta-feature").join("decisions.md")).unwrap();
    write_valid_feature(&features_dir, "_template", "Template");

    let before = snapshot(&features_dir);

    let assert_response = |code: u16, resp: &Value| {
        assert_eq!(code, 200, "response was {resp}");
        assert_eq!(resp["project"], json!(test.path.display().to_string()));
        let list = resp["features"].as_array().expect("features array");
        assert!(!list.iter().any(|f| f["slug"] == "_template"));
        let slugs: Vec<&str> = list.iter().map(|f| f["slug"].as_str().unwrap()).collect();
        let mut sorted = slugs.clone();
        sorted.sort();
        assert_eq!(slugs, sorted, "features must be sorted by slug");

        let alpha = list.iter().find(|f| f["slug"] == "alpha-feature").expect("alpha-feature listed");
        assert_eq!(alpha["title"], "Alpha Feature");
        assert_eq!(alpha["path"], json!(features_dir.join("alpha-feature").display().to_string()));
        assert_eq!(alpha["status"], "valid");
        assert_eq!(alpha["reasons"], json!([]));

        let beta = list.iter().find(|f| f["slug"] == "beta-feature").expect("beta-feature listed");
        assert_eq!(beta["title"], "Beta Feature");
        assert_eq!(beta["status"], "invalid");
        assert!(
            beta["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.as_str().unwrap().contains("decisions.md")),
            "reasons were {}",
            beta["reasons"]
        );
    };

    let (code, resp) = api_request(&test.app.app, "GET", "/api/features", json!({}));
    assert_response(code, &resp);

    let query = format!("/api/features?project={}", url_encode(&test.path.display().to_string()));
    let (code, resp) = api_request(&test.app.app, "GET", &query, json!({}));
    assert_response(code, &resp);

    let after = snapshot(&features_dir);
    assert_eq!(before, after, "GET /api/features must not create, modify or remove any file");
}

#[test]
fn s9_existing_flow_unchanged_without_feature_specs() {
    // S9: The existing goal/discussion/queue flow is unaffected without feature specs
    let absent = QueueTest::new(true);
    let empty = QueueTest::new(true);
    fs::create_dir_all(empty.path.join("docs/features")).unwrap();

    for test in [&absent, &empty] {
        test.start();
        wait_for_worker(&test.app);
    }

    assert!(absent.statuses().is_empty());
    assert!(empty.statuses().is_empty());

    let plan_a = absent.app.load_plan().unwrap();
    let plan_b = empty.app.load_plan().unwrap();
    let stage_statuses = |plan: &Value| -> Vec<Value> {
        plan["stages"].as_array().unwrap().iter().map(|s| s["status"].clone()).collect()
    };
    let statuses_a = stage_statuses(&plan_a);
    let statuses_b = stage_statuses(&plan_b);
    assert_eq!(statuses_a, statuses_b);
    assert!(statuses_a.iter().all(|s| s == "committed"), "{statuses_a:?}");

    let (code_a, state_a) = api_request(&absent.app.app, "GET", "/api/state", json!({}));
    let (code_b, state_b) = api_request(&empty.app.app, "GET", "/api/state", json!({}));
    assert_eq!(code_a, 200);
    assert_eq!(code_b, 200);
    let keys = |v: &Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let keys_a = keys(&state_a);
    let keys_b = keys(&state_b);
    assert_eq!(keys_a, keys_b, "the existing flow's /api/state shape must not change");
    assert!(
        keys_a.iter().all(|k| !k.to_lowercase().contains("feature")),
        "/api/state must not gain a feature-related key: {keys_a:?}"
    );

    // Discovery must be strictly read-only: it neither creates docs/features
    // where it was absent, nor writes into it where it was present but empty.
    assert!(!absent.path.join("docs/features").exists());
    assert_eq!(fs::read_dir(empty.path.join("docs/features")).unwrap().count(), 0);

    for test in [&absent, &empty] {
        let (code, resp) = api_request(&test.app.app, "GET", "/api/features", json!({}));
        assert_eq!(code, 200, "response was {resp}");
        assert_eq!(resp["features"], json!([]));
    }
}
