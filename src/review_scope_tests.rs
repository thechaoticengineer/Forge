use super::*;
use super::test_support::*;

#[test]
fn stages_review_and_commit_with_ignored_or_unignored_runtime() {
    for ignored in [true, false] {
        for code in [false, true] {
            let f = Fixture::with_ignored_runtime(
                if code {
                    "Implement feature"
                } else {
                    "Fix prose spelling"
                },
                0,
                ignored,
            );
            if code {
                f.setting("mock_edits", json!([{"new.rs":"fn main() {}\n"}]));
            } else {
                f.docs();
            }
            let p = f.run();
            let stage = &p["stages"][0];
            assert_eq!(
                stage["status"], "committed",
                "ignored={ignored}, code={code}: {p}"
            );
            assert_eq!(stage["review_gate"]["status"], "approved");
            assert_eq!(f.count("reviewer"), 1);
            assert_eq!(f.count("architect"), usize::from(code));
            assert_eq!(f.count("fixer"), 0);
            assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
            assert_eq!(
                f.ctx.git(&["rev-parse", "HEAD^{tree}"]).unwrap(),
                stage["review_gate"]["identity"]["snapshot"]["tree"]
            );
            assert!(
                f.ctx
                    .git(&["ls-tree", "-r", "--name-only", "HEAD", "--", ".forge"])
                    .unwrap()
                    .is_empty()
            );
            assert!(f.ctx.git(&["ls-files", "--", ".forge"]).unwrap().is_empty());
            assert!(f.ctx.git(&["diff", "HEAD", "--", "."]).unwrap().is_empty());
        }
    }
}

#[test]
fn ordinary_documentation_needs_one_fresh_independent_and_records_not_required_outcome() {
    let f = Fixture::new("Fix prose spelling in documentation", 0);
    f.docs();
    let p = f.run();
    let stage = &p["stages"][0];
    assert_eq!(stage["status"], "committed");
    assert_eq!(f.count("reviewer"), 1);
    assert_eq!(f.count("architect"), 0);
    assert_eq!(f.count("fixer"), 0);
    assert_eq!(stage["review_gate"]["roles"]["architect"], "not_required");
    assert_eq!(stage["reviews"].as_array().unwrap().len(), 1);
    let cp = f.ctx.architecture_store().checkpoint(&p).unwrap();
    assert_eq!(
        cp["execution_outcomes"][0]["review_gate"]["roles"]["architect"],
        "not_required"
    );
    assert_eq!(
        f.ctx.app.settings.lock().unwrap()["mock_architect_requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn code_mixed_contracts_markdown_and_uncertain_intent_require_both() {
    for (intent, edits) in [
        ("Implement feature", json!({"new.rs":"fn main() {}"})),
        (
            "Fix prose spelling",
            json!({"new.rs":"fn main() {}","README.md":"A friendly greeting.\n"}),
        ),
        (
            "Fix prose spelling",
            json!({"README.md":"The API must return a greeting.\n"}),
        ),
        (
            "Fix prose spelling",
            json!({"README.md":"Example:\n```sh\n./app\n```\n"}),
        ),
        (
            "Change configuration",
            json!({"README.md":"The program prints greetings.\n"}),
        ),
    ] {
        let f = Fixture::new(intent, 0);
        f.setting("mock_edits", json!([edits]));
        let p = f.run();
        assert_eq!(p["stages"][0]["status"], "committed", "{p}");
        assert_eq!(f.count("architect"), 1);
        assert_eq!(f.count("reviewer"), 1);
        assert_eq!(f.count("fixer"), 0);
        let reviews = p["stages"][0]["reviews"].as_array().unwrap();
        assert_eq!(
            reviews[0]["identity"]["snapshot"],
            reviews[1]["identity"]["snapshot"]
        );
    }
}

#[test]
fn independent_promotion_rebinds_both_verdicts_and_keeps_history() {
    let f = Fixture::new("Fix prose spelling", 0);
    f.docs();
    f.setting("mock_verdicts", json!([{"approved":true,"issues":[],"requires_dual":true,"scope_reason":"Behavioral guarantee found"},clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("reviewer"), 2);
    assert_eq!(f.count("architect"), 1);
    assert_eq!(p["stages"][0]["reviews"].as_array().unwrap().len(), 3);
    assert_eq!(
        p["stages"][0]["reviews"][0]["policy"]["scope"],
        "ordinary_documentation"
    );
    assert_eq!(p["stages"][0]["dual_promoted"], true);
}

#[test]
fn scope_changing_fix_promotes_and_promotion_is_sticky() {
    let f = Fixture::new("Fix prose spelling", 2);
    f.setting("mock_edits", json!([{"README.md":"A friendly greeting.\n"},{"README.md":"The API must greet.\n"},{"README.md":"A friendly greeting again.\n"}]));
    f.setting(
        "mock_verdicts",
        json!([
            reject("Improve explanation"),
            reject("Correct regression"),
            clean()
        ]),
    );
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("reviewer"), 3);
    assert_eq!(f.count("architect"), 2);
    assert_eq!(
        p["stages"][0]["review_gate"]["policy"]["scope"],
        "code_or_contract"
    );
}

#[test]
fn full_diff_classification_includes_index_content_overwritten_in_worktree() {
    let f = Fixture::new("Fix prose spelling", 0);
    fs::write(f.root.join("README.md"), "The API must remain stable.\n").unwrap();
    f.ctx.git(&["add", "README.md"]).unwrap();
    f.docs();
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("architect"), 1);
}

#[test]
fn runtime_artifacts_do_not_change_snapshot_and_verdict_identity_fields_are_exact() {
    let f = Fixture::new("Implement feature", 0);
    let p = f.reviewed();
    let identity: Value = serde_json::from_slice(&fs::read(f.ctx.forge_path("review-identity.json")).unwrap()).unwrap();
    assert_eq!(identity, p["stages"][0]["reviews"].as_array().unwrap().last().unwrap()["identity"]);
    for role in ["reviewer", "architect"] {
        let settings = f.ctx.app.settings.lock().unwrap();
        let prompt = settings[format!("mock_{role}_prompts")][0].as_str().unwrap();
        assert!(prompt.contains(".forge/review-identity.json"));
        assert!(prompt.contains("Do not manually transcribe hashes"));
    }
    let before = review_snapshot(f.ctx.project()).unwrap();
    fs::write(f.ctx.forge_path("unrelated-runtime.json"), "{}").unwrap();
    assert_eq!(review_snapshot(f.ctx.project()).unwrap(), before);
    let record = &p["stages"][0]["reviews"][0];
    for field in [
        "plan_id",
        "revision",
        "stage_id",
        "attempt_id",
        "round",
        "role",
        "policy",
        "snapshot",
    ] {
        let mut bad = record.clone();
        bad["identity"][field] = json!("wrong");
        assert!(
            normalize_review_verdict(
                &bad.to_string(),
                &record["identity"],
                "The requested change works."
            )
            .is_err()
        );
    }
}

#[test]
fn documentation_exhaustion_and_restart_keep_architect_not_required() {
    let f = Fixture::new("Fix prose spelling", 0);
    f.docs();
    f.setting("mock_verdicts", json!([reject("Unresolved prose issue")]));
    f.run();
    let p = f.run();
    f.assert_no_commit();
    assert_eq!(
        p["stages"][0]["review_gate"]["roles"]["architect"],
        "not_required"
    );
    assert_eq!(f.count("architect"), 0);
}

#[test]
fn documentation_with_deferred_reviewer_has_no_stage_required_roles() {
    let f = Fixture::new("Fix prose spelling", 0);
    f.docs();
    f.setting("review_cadence", json!({"architect":"per_stage","reviewer":"per_plan"}));
    let p = f.run();
    let stage = &p["stages"][0];
    assert_eq!(stage["status"], "committed", "{p}");
    assert_eq!(stage["review_policy"]["required_roles"], json!(["reviewer"]));
    assert_eq!(stage["review_policy"]["stage_required_roles"], json!([]));
    assert_eq!(stage["review_policy"]["deferred_roles"], json!(["reviewer"]));
    assert_eq!(stage["review_gate"]["status"], "deferred");
    assert_eq!(stage["review_gate"]["roles"], json!({"architect":"not_required","reviewer":"deferred"}));
    assert_eq!(stage["reviews"], json!([]));
    assert_eq!(f.count("reviewer"), 0);
    assert_eq!(f.count("architect"), 0);
}

