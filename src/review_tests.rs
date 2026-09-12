use super::*;
use crate::app::App;
use std::sync::Arc;

struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}
impl Fixture {
    fn new(intent: &str, budget: u64) -> Self {
        Self::with_ignored_runtime(intent, budget, false)
    }
    fn with_ignored_runtime(intent: &str, budget: u64, ignored: bool) -> Self {
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        Self::with_settings(intent, budget, ignored, settings)
    }
    fn with_settings(intent: &str, budget: u64, ignored: bool, mut settings: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "forge-gate-test-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(&root).unwrap();
        for role in ["planner", "architect", "implementer", "reviewer"] {
            settings[role] = json!("mock");
        }
        settings["auto_push"] = json!(false);
        settings["max_fix_rounds"] = json!(budget);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        ctx.git(&["init", "-q"]).unwrap();
        ctx.git(&["config", "user.name", "Test"]).unwrap();
        ctx.git(&["config", "user.email", "test@example.invalid"])
            .unwrap();
        ctx.git(&["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(root.join("README.md"), "The program prints a greeting.\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        if ignored {
            fs::write(root.join(".gitignore"), ".forge/\n").unwrap();
            ctx.git(&["add", ".gitignore"]).unwrap();
        }
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        ctx.publish_plan(&json!({"goal":"Improve the project","status":"ready","stages":[{"id":1,"title":intent,"instructions":intent,"acceptance":"The requested change works.","commit":"feat: stage","status":"pending","rounds":0}]}), true).unwrap();
        Self { root, ctx }
    }
    fn setting(&self, key: &str, v: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = v;
    }
    fn plan(&self) -> Value {
        self.ctx.load_plan().unwrap()
    }
    fn run(&self) -> Value {
        self.ctx.run_worker();
        self.plan()
    }
    fn count(&self, role: &str) -> usize {
        self.ctx.app.settings.lock().unwrap()[format!("mock_{role}_prompts")]
            .as_array()
            .map_or(0, |prompts| prompts.iter().filter(|p| !p.as_str().unwrap_or("").contains("\"scope\":\"plan\"")).count())
    }
    fn docs(&self) {
        self.setting(
            "mock_edits",
            json!([{"README.md":"The program prints a friendly greeting.\n"}]),
        );
    }
    fn reviewed(&self) -> Value {
        let p = self.plan();
        let mut p = self
            .ctx
            .architect_publish(p.clone(), Some(&p), "test")
            .unwrap();
        p["stages"][0]["attempt_id"] = json!(crate::architecture::identity());
        p["stages"][0]["review_budget"] = json!(0);
        self.ctx.save_plan(&p).unwrap();
        assert_eq!(self.ctx.run_review_stage(&mut p, 0).unwrap(), "approved");
        p
    }
    fn assert_no_commit(&self) {
        assert_eq!(self.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn reject(text: &str) -> Value {
    json!({"approved":false,"issues":[text]})
}
fn clean() -> Value {
    json!({"approved":true,"issues":[]})
}

#[test]
fn review_response_corrections_do_not_repeat_implementation_or_spend_fix_rounds() {
    let f = Fixture::new("Implement feature", 0);
    f.setting("mock_verdicts", json!(["malformed", "malformed", "malformed", clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(p["stages"][0]["rounds"], 1);
    assert_eq!(f.count("reviewer"), 4);
    assert_eq!(f.count("architect"), 1);
    assert_eq!(fs::read_to_string(f.root.join("mock.txt")).unwrap().lines().count(), 1);
    let sessions = f.ctx.app.settings.lock().unwrap()["test_review_sessions"].clone();
    assert!(sessions.as_array().unwrap().iter().filter(|s| s["role"] == "reviewer").all(|s| s["session"].is_null()));
    assert_eq!(p["stages"][0]["reviews"].as_array().unwrap().len(), 2);
}

#[test]
fn mutation_or_stop_during_a_correction_prevents_further_calls_and_publication() {
    for action in [json!({"write":{"mock.txt":"changed during correction"}}), json!({"stop":true})] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("mock_verdicts", json!(["malformed", clean()]));
        f.setting("mock_reviewer_actions", json!([{}, action]));
        f.run();
        f.assert_no_commit();
        assert_eq!(f.count("reviewer"), 2);
        assert_eq!(f.count("architect"), 0);
        assert!(!f.ctx.forge_path("verdict.json").exists());
    }
}

#[test]
fn malformed_implementer_report_is_corrected_readonly_without_repeating_work() {
    let f = Fixture::new("Implement feature", 0);
    f.setting("mock_implementation_outputs", json!([{}, {}, {}, {}]));
    let p = f.run();
    f.assert_no_commit();
    assert_eq!(p["stages"][0]["status"], "blocked");
    assert_eq!(p["stages"][0]["rounds"], 1);
    assert_eq!(fs::read_to_string(f.root.join("mock.txt")).unwrap().lines().count(), 1);
    let settings = f.ctx.app.settings.lock().unwrap();
    let calls = settings["mock_agent_requests"].as_array().unwrap();
    assert_eq!(calls.iter().filter(|r| r["role"] == "implementer").count(), 1);
    assert_eq!(calls.iter().filter(|r| r["role"] == "response_correction").count(), 3);
}

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
fn either_role_can_block_and_all_required_roles_repeat_after_fixes() {
    for role in ["architect", "reviewer"] {
        let f = Fixture::new("Implement feature", 1);
        f.setting(
            if role == "architect" {
                "mock_architect_verdicts"
            } else {
                "mock_verdicts"
            },
            json!([reject("Fix defect"), clean()]),
        );
        let p = f.run();
        assert_eq!(p["stages"][0]["status"], "committed");
        assert_eq!(f.count("architect"), 2);
        assert_eq!(f.count("reviewer"), 2);
        assert_eq!(f.count("fixer"), 1);
        assert!(
            f.ctx.app.settings.lock().unwrap()["mock_fixer_prompts"][0]
                .as_str()
                .unwrap()
                .contains(&format!("[{role}] Fix defect"))
        );
    }
}
#[test]
fn combined_conflicting_requests_get_clarification_without_losing_authority() {
    let f = Fixture::new("Implement feature", 1);
    f.setting("mock_verdicts", json!([reject("Use format A"), clean()]));
    f.setting("mock_architect_verdicts", json!([{"approved":false,"issues":["Use format B"],"architecture_context_gap":"A and B conflict; establish compatibility"}, clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    let settings = f.ctx.app.settings.lock().unwrap();
    let prompt = settings["mock_fixer_prompts"][0].as_str().unwrap();
    assert!(prompt.contains("[architect] Use format B"));
    assert!(prompt.contains("[reviewer] Use format A"));
    let turns = settings["mock_architect_requests"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert!(
        turns[1]["prompt"]
            .as_str()
            .unwrap()
            .contains("A and B conflict")
    );
    assert!(
        settings["mock_architect_prompts"][0]
            .as_str()
            .unwrap()
            .contains("Use format A")
    );
}
#[test]
fn legacy_notes_are_requests_and_new_defects_on_rereview_block() {
    let f = Fixture::new("Implement feature", 2);
    f.setting("mock_verdicts", json!([{"approved":true,"issues":[],"notes":["Legacy edit"]},reject("New regression"),clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("fixer"), 2);
    let settings = f.ctx.app.settings.lock().unwrap();
    assert!(
        settings["mock_fixer_prompts"][0]
            .as_str()
            .unwrap()
            .contains("Legacy edit")
    );
    assert!(
        settings["mock_fixer_prompts"][1]
            .as_str()
            .unwrap()
            .contains("New regression")
    );
    assert!(
        !settings["mock_fixer_prompts"][1]
            .as_str()
            .unwrap()
            .contains("Legacy edit")
    );
}
#[test]
fn stale_malformed_contradictory_or_unevidenced_verdicts_never_commit() {
    for bad in [
        json!("garbage"),
        json!({"identity":{},"approved":true,"issues":[]}),
        json!({"approved":true,"issues":["fix"]}),
        json!({"approved":true,"issues":[],"checks":[null]}),
        json!({"approved":true,"issues":[],"checks":[]}),
        json!({"approved":true,"issues":[],"project_checks":[]}),
        json!({"approved":true,"issues":[],"acceptance_evidence":{"verified":true}}),
        json!({"approved":true,"issues":[],"summary":""}),
    ] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("mock_verdicts", json!(vec![bad; 4]));
        fs::write(f.ctx.forge_path("verdict.json"), clean().to_string()).unwrap();
        let p = f.run();
        f.assert_no_commit();
        assert_eq!(p["stages"][0]["review_gate"]["status"], "error");
        assert_eq!(p["stages"][0]["status"], "blocked");
        assert!(p["stages"][0]["finished_unix"].as_i64().is_some());
        assert!(p["stages"][0]["duration_secs"].as_i64().is_some());
        assert!(!f.ctx.forge_path("verdict.json").exists());
    }
}
#[test]
fn partial_pair_failure_retains_history_but_never_reuses_approval_or_budget() {
    let f = Fixture::new("Implement feature", 1);
    f.setting("mock_architect_verdicts", json!(["malformed", "malformed", "malformed", "malformed", clean()]));
    let first = f.run();
    f.assert_no_commit();
    assert_eq!(first["stages"][0]["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(first["stages"][0]["review_gate"]["status"], "error");
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("reviewer"), 2);
    assert_eq!(f.count("architect"), 5);
    assert_eq!(
        p["stages"][0]["attempt_id"],
        first["stages"][0]["attempt_id"]
    );
    let f = Fixture::new("Implement feature", 0);
    f.setting("mock_architect_verdicts", json!(vec!["malformed"; 4]));
    f.run();
    let p = f.run();
    f.assert_no_commit();
    assert_eq!(p["stages"][0]["status"], "blocked");
    assert_eq!(f.count("reviewer"), 1);
}
#[test]
fn zero_and_nonzero_budgets_stay_exhausted_across_process_reconstruction() {
    for budget in [0, 1, 2] {
        let f = Fixture::new("Implement feature", budget);
        // This test isolates the fix budget; reassessment budgets have separate coverage.
        f.ctx.app.settings.lock().unwrap()["reassessment_limits"]["repeat_threshold"] = json!(10);
        f.setting(
            "mock_verdicts",
            json!(vec![reject("Unresolved"); budget as usize + 1]),
        );
        let p = f.run();
        assert_eq!(p["stages"][0]["rounds"], budget + 1);
        f.assert_no_commit();
        let settings = f.ctx.app.settings.lock().unwrap().clone();
        let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
        let ctx = app.context(f.root.to_str().unwrap());
        ctx.run_worker();
        assert_eq!(ctx.load_plan().unwrap()["stages"][0]["status"], "blocked");
        f.assert_no_commit();
        assert_eq!(
            ctx.app.settings.lock().unwrap()["mock_reviewer_prompts"]
                .as_array()
                .unwrap()
                .len(),
            budget as usize + 1
        );
    }
}
#[test]
fn stop_during_review_consumes_round_and_invalidates_current_gate() {
    let f = Fixture::new("Implement feature", 1);
    f.setting("mock_reviewer_actions", json!([{"stop":true}]));
    f.run();
    f.assert_no_commit();
    assert_eq!(f.count("architect"), 0);
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(p["stages"][0]["rounds"], 2);
    assert_eq!(f.count("reviewer"), 2);
}
#[test]
fn mutations_during_review_or_before_commit_invalidate_snapshot() {
    for during in [true, false] {
        for kind in ["unstaged", "staged", "untracked", "head", "delete"] {
            let f = Fixture::new("Implement feature", 0);
            let action = match kind {
                "unstaged" => json!({"write":{"README.md":"Externally changed\n"}}),
                "staged" => json!({"git":["add","mock.txt"]}),
                "untracked" => json!({"write":{"external.txt":"unexpected"}}),
                "head" => json!({"git":["commit","--allow-empty","-qm","external"]}),
                _ => json!({"git":["rm","README.md"]}),
            };
            if during {
                f.setting("mock_reviewer_actions", json!([action]));
                let p = f.run();
                assert_ne!(p["stages"][0]["status"], "committed");
                assert_eq!(f.count("architect"), 0);
            } else {
                let p = f.reviewed();
                if let Some(writes) = action["write"].as_object() {
                    for (path, content) in writes {
                        fs::write(f.root.join(path), content.as_str().unwrap()).unwrap();
                    }
                }
                if let Some(args) = action["git"].as_array() {
                    f.ctx
                        .git(&args.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>())
                        .unwrap();
                }
                assert!(
                    f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err(),
                    "{kind}"
                );
            }
        }
    }
}
#[test]
fn final_staging_preserves_partial_index_untracked_and_deletion_content() {
    let f = Fixture::new("Implement feature", 0);
    fs::write(f.root.join("README.md"), "staged interim\n").unwrap();
    f.ctx.git(&["add", "README.md"]).unwrap();
    fs::write(f.root.join("README.md"), "reviewed final\n").unwrap();
    let p = f.reviewed();
    assert!(
        f.ctx
            .commit_reviewed(&p, 0, "feat: stage")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        f.ctx.git(&["show", "HEAD:README.md"]).unwrap(),
        "reviewed final"
    );
    assert_eq!(
        f.ctx.git(&["show", "HEAD:mock.txt"]).unwrap(),
        "work by implementer"
    );
    let f = Fixture::new("Implement feature", 0);
    fs::remove_file(f.root.join("README.md")).unwrap();
    let p = f.reviewed();
    assert!(
        f.ctx
            .commit_reviewed(&p, 0, "feat: stage")
            .unwrap()
            .is_some()
    );
}
#[test]
fn missing_current_role_or_changed_identity_cannot_commit() {
    for field in ["revision", "attempt_id", "round", "policy", "snapshot"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.reviewed();
        p["stages"][0]["reviews"][1]["identity"][field] = json!("wrong");
        assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err());
        f.assert_no_commit();
    }
}
#[test]
fn reviewer_provider_is_opposite_and_known_unavailability_blocks() {
    let f = Fixture::new("Implement feature", 0);
    f.setting("automatic_routing",json!(false));
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
        {"provider":"codex","model":"review-codex","tier":"strong"},
        {"provider":"claude","model":"review-claude","tier":"strong"}]);
    for (implementer, other) in [("codex", "claude"), ("claude", "codex")] {
        f.setting("reviewer", json!(implementer));
        assert!(f.ctx.reviewer_config(implementer).is_err());
        f.setting("reviewer", json!(other));
        assert_eq!(f.ctx.reviewer_config(implementer).unwrap().0, other);
        let policy =
            crate::catalogue::Policy::from_settings(&f.ctx.app.settings.lock().unwrap()).unwrap();
        f.ctx.app.catalogue.observe(
            &policy,
            crate::catalogue::Provider::parse(other).unwrap(),
            "",
            Err(crate::catalogue::Failure::new(
                crate::catalogue::FailureKind::MissingExecutable,
            )),
        );
        assert!(f.ctx.reviewer_config(implementer).is_err());
    }
}
#[test]
fn reviewers_verify_project_checks_without_engine_command_name_matching() {
    for intent in ["Fix prose spelling", "Implement feature"] {
        for commands in [
            vec!["CARGO_TARGET_DIR=/tmp/forge-review3-l77ap2fa/target cargo build --offline",
                 "CARGO_TARGET_DIR=/tmp/forge-review3-l77ap2fa/target cargo test --offline"],
            vec!["cd /tmp/review && ./scripts/verify-project.sh"],
        ] {
            let f = Fixture::new(intent,0);
            fs::write(f.root.join("Cargo.toml"),"[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
            f.ctx.git(&["add","Cargo.toml"]).unwrap();
            f.ctx.git(&["commit","-qm","manifest"]).unwrap();
            f.docs();
            let checks: Vec<_> = commands.iter().map(|command| json!({
                "command":command,"status":"passed",
                "evidence":"Fixture reviewer verified the required project build and tests; processes exited 0 and expected output matched."
            })).collect();
            let verdict = json!({"approved":true,"issues":[],"project_checks":checks});
            f.setting("mock_verdicts",json!([verdict]));
            f.setting("mock_architect_verdicts",json!([verdict]));
            let plan = f.run();
            let stage = &plan["stages"][0];
            assert_eq!(stage["status"],"committed");
            assert_eq!(stage["review_gate"]["status"],"approved");
            assert_eq!(f.count("architect"),usize::from(intent == "Implement feature"));
            for review in stage["reviews"].as_array().unwrap() {
                assert_eq!(review["project_checks"],json!(checks));
            }
        }
    }
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
fn partial_rejection_failure_retains_requests_for_the_next_fixer() {
    let f = Fixture::new("Implement feature", 1);
    f.setting(
        "mock_verdicts",
        json!([reject("Keep this unresolved finding"), clean()]),
    );
    f.setting("mock_architect_verdicts", json!(["bad", clean()]));
    f.run();
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert!(
        f.ctx.app.settings.lock().unwrap()["mock_fixer_prompts"][0]
            .as_str()
            .unwrap()
            .contains("[reviewer] Keep this unresolved finding")
    );
}
#[test]
fn criterion_evidence_is_complete_and_prompts_preserve_literal_inputs() {
    let f = Fixture::new("Implement {goal} feature", 0);
    let mut p = f.plan();
    p["stages"][0]["acceptance"] = json!("First {verdict_path}\nSecond {review_context}");
    f.ctx.save_plan(&p).unwrap();
    f.setting("mock_verdicts",json!(vec![json!({"approved":true,"issues":[],"criteria":[{"criterion":"First {verdict_path}","status":"passed","evidence":"One check"}]}); 4]));
    f.run();
    f.assert_no_commit();
    let settings = f.ctx.app.settings.lock().unwrap();
    let prompt = settings["mock_reviewer_prompts"][0].as_str().unwrap();
    assert!(prompt.contains("Implement {goal} feature"));
    assert!(prompt.contains("First {verdict_path}"));
    assert!(prompt.contains("Second {review_context}"));
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
    let before = snapshot(f.ctx.project()).unwrap();
    fs::write(f.ctx.forge_path("unrelated-runtime.json"), "{}").unwrap();
    assert_eq!(snapshot(f.ctx.project()).unwrap(), before);
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
            normalize(
                &bad.to_string(),
                &record["identity"],
                "The requested change works."
            )
            .is_err()
        );
    }
}
#[test]
fn reviewer_cli_is_fresh_and_architect_review_resumes_with_scoped_tools() {
    for provider in ["codex", "claude"] {
        let r = AgentRequest {
            role: "reviewer",
            provider,
            model: "",
            effort: "provider_default",
            session: None,
            prompt: "review",
        };
        let cmd = crate::agent::command(&r).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args
                .iter()
                .any(|s| s == "resume" || s == "--resume" || s == "--continue")
        );
        assert!(!args.iter().any(|s| s.contains("dangerously")));
        if provider == "claude" {
            assert!(args.iter().any(|s| s == "Read,Glob,Grep,Bash"));
        } else {
            assert!(args.iter().any(|s| s == "sandbox_workspace_write.network_access=true"));
        }
        let r = AgentRequest {
            role: "architect_review",
            session: Some("11111111-2222-4333-8444-555555555555"),
            ..r
        };
        let resumed = crate::agent::command(&r).unwrap();
        assert!(resumed.get_args().any(|s| s == "11111111-2222-4333-8444-555555555555"));
        if provider == "codex" {
            assert!(resumed.get_args().any(|s| s == "sandbox_workspace_write.network_access=true"));
        }
    }
}
#[test]
fn review_sandbox_denies_repository_git_and_runtime_writes_but_allows_scratch_checks() {
    let f = Fixture::new("Implement feature", 0);
    let mut command = Command::new("sh");
    command.args(["-c", "test ! -w README.md && test ! -w .git && test ! -w .forge && mkdir /tmp/check && echo verified >/tmp/check/result && cat /tmp/check/result"]);
    let sandbox = crate::agent::review_sandbox(&command, f.ctx.project(), "codex");
    // Missing runtime prerequisites must fail closed, never fall back to the raw command.
    if let Ok(mut sandbox) = sandbox {
        match sandbox.output() {
            Ok(out) if out.status.success() => {
                assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "verified")
            }
            Ok(out) => assert!(!out.stderr.is_empty()),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        }
    }
    assert_eq!(
        fs::read_to_string(f.root.join("README.md")).unwrap(),
        "The program prints a greeting.\n"
    );
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
fn duplicate_keys_and_tampered_evidence_cannot_authorize_commit() {
    let f = Fixture::new("Implement feature", 0);
    let mut p = f.reviewed();
    let record = &p["stages"][0]["reviews"][0];
    let duplicate = record.to_string().replacen(
        "\"approved\":true",
        "\"approved\":false,\"approved\":true",
        1,
    );
    assert!(
        normalize(
            &duplicate,
            &record["identity"],
            "The requested change works."
        )
        .is_err()
    );
    p["stages"][0]["reviews"][1]["criteria"] = json!([]);
    assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err());
    f.assert_no_commit();
}

#[test]
fn verdict_presentation_wrappers_preserve_validation() {
    let f = Fixture::new("Implement feature", 0);
    let p = f.reviewed();
    let record = &p["stages"][0]["reviews"][0];
    let raw = record.to_string();
    let preamble = "All checks pass. Shared fixtures use `crate::test_support` as intended.";
    let parse = |output: &str| normalize(output, &record["identity"], "The requested change works.");
    for wrapped in [
        format!("  {raw}\n"),
        format!("{preamble}\n\n{raw}"),
        format!("```json\n{raw}\n```"),
        format!("{preamble}\n\n```\n{raw}\n```"),
        format!("```json\r\n{raw}\r\n```"),
    ] {
        assert_eq!(parse(&wrapped).unwrap(), parse(&raw).unwrap());
    }
    let duplicate = raw.replacen("\"approved\":true", "\"approved\":false,\"approved\":true", 1);
    let nested_duplicate = raw.replacen("\"role\":\"reviewer\"", "\"role\":\"architect\",\"role\":\"reviewer\"", 1);
    for invalid in [
        format!("{preamble}\n{duplicate}"),
        format!("```json\n{nested_duplicate}\n```"),
        format!("{preamble}\n{{}}\n{raw}"),
        format!("{preamble}\n{raw}\n{raw}"),
        format!("{preamble}\n{{broken\n{raw}"),
        format!("{preamble}\n[{raw}]"),
        format!("{preamble}\n{raw}\nActually, changes are needed."),
        format!("```json\n{raw}"),
        format!("```json\n{raw}\n```\nMore text"),
        format!("```text\n{raw}\n```"),
        format!("{}\n{raw}", "x".repeat(128 * 1024)),
    ] {
        assert!(parse(&invalid).is_err(), "unexpectedly accepted: {invalid}");
    }
    for field in ["identity", "criteria", "project_checks", "acceptance_evidence"] {
        let mut invalid = record.clone();
        invalid[field] = Value::Null;
        assert!(parse(&format!("{preamble}\n```json\n{invalid}\n```")).is_err(), "{field}");
    }
}

#[test]
fn idle_state_displays_legacy_gate_errors_as_blocked_without_mutating_plan() {
    let f = Fixture::new("Implement feature", 1);
    let mut p = f.plan();
    p["stages"][0]["status"] = json!("in_progress");
    p["stages"][0]["review_gate"] = json!({"status":"error","error":"malformed verdict"});
    p["stages"][0]["rounds"] = json!(1);
    f.ctx.save_plan(&p).unwrap();
    let before = fs::read(f.ctx.forge_path("plan.json")).unwrap();
    for (busy, expected) in [(false, "blocked"), (true, "in_progress"), (false, "blocked")] {
        f.ctx.session.busy.store(busy, Ordering::SeqCst);
        let (code, state) = crate::test_support::api_request(&f.ctx.app, "GET", "/api/state", json!({}));
        assert_eq!(code, 200);
        assert_eq!(state["plan"]["stages"][0]["status"], expected);
        assert_eq!(state["plan"]["stages"][0]["review_gate"]["status"], "error");
        assert_eq!(state["plan"]["stages"][0]["rounds"], 1);
        assert_eq!(fs::read(f.ctx.forge_path("plan.json")).unwrap(), before);
    }
}

#[test]
fn reviewed_git_commit_is_recovered_after_completion_checkpoint_failure() {
    let f = Fixture::new("Implement feature", 0);
    let p = f.reviewed();
    let sha = f.ctx.commit_reviewed(&p,0,"feat: stage").unwrap().unwrap();
    // Simulate the process ending after update-ref but before finish_stage/save.
    assert_ne!(f.plan()["stages"][0]["status"],"committed");
    assert_eq!(f.ctx.recover_committed_stages().unwrap(),vec![1]);
    let saved=f.plan();
    assert_eq!(saved["stages"][0]["status"],"committed");
    assert_eq!(saved["stages"][0]["sha"],sha);
    assert_eq!(saved["stages"][0]["review_gate"],p["stages"][0]["review_gate"]);
    assert_eq!(saved["stages"][0]["rounds"],p["stages"][0]["rounds"]);
    assert!(f.ctx.recover_committed_stages().unwrap().is_empty());
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"2");
}

#[test]
fn commit_recovery_requires_clean_exact_tree_parent_message_and_current_approvals() {
    for case in ["dirty", "index", "other-commit", "wrong-message", "invalid-approval"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p=f.reviewed();
        if case=="wrong-message" {
            f.ctx.commit_reviewed(&p,0,"other message").unwrap();
        } else { f.ctx.commit_reviewed(&p,0,"feat: stage").unwrap(); }
        match case {
            "dirty" => fs::write(f.root.join("README.md"),"unreviewed change").unwrap(),
            "index" => {
                fs::write(f.root.join("README.md"),"staged change").unwrap();
                f.ctx.git(&["add","README.md"]).unwrap();
                f.ctx.git(&["restore","--source=HEAD","--worktree","README.md"]).unwrap();
            },
            "other-commit" => { f.ctx.git(&["commit","--allow-empty","-qm","other commit"]).unwrap(); },
            "invalid-approval" => {
                p["stages"][0]["reviews"][0]["approved"]=json!(false);
                f.ctx.save_plan(&p).unwrap();
            },
            _ => {},
        }
        assert!(f.ctx.recover_committed_stages().is_err(),"accepted {case}");
        assert_ne!(f.plan()["stages"][0]["status"],"committed");
    }
}

#[test]
fn cli_limit_fallback_keeps_review_identity_budget_and_fresh_independent_session() {
    for blocked in [false, true] {
        let f = Fixture::new("Fix prose spelling", 0);
        f.docs();
        let mut plan = f.reviewed();
        plan["stages"][0]["implementer_provider"] = json!("codex");
        f.ctx.save_plan(&plan).unwrap();
        let base = plan["stages"][0]["review_gate"]["identity"].clone();
        let rounds = plan["stages"][0]["rounds"].clone();
        let retry_count = plan["stages"][0]["reassessment"]["operational_retries"].clone();
        f.setting("test_fake_providers", json!(true));
        f.setting("reviewer", json!("claude"));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
            {"provider":"claude","model":"opus[1m]","tier":"strong"}]);
        f.setting("test_review_sessions", json!([]));
        let refusal = |family: &str| json!({"error":format!("claude exited with exit status: 1: You've reached your {family} limit. Switch to another model.")});
        f.setting("mock_reviewer_actions", if blocked { json!([refusal("Fable"),refusal("Opus")]) } else { json!([refusal("Fable")]) });
        // No usage reading: this reproduces a bridge unsupported after a CLI update.
        assert_eq!(f.ctx.app.quota.snapshot()["status"], "pending");
        let result = f.ctx.review_with_retry(&mut plan, 0, &base, "reviewer", "claude", "claude-fable-5-1[1m]");
        if blocked {
            assert!(result.unwrap_err().contains("no eligible strong independent reviewer"));
        } else {
            let verdict = result.unwrap();
            assert_eq!(verdict["model"], "opus[1m]");
            assert_eq!(verdict["provider"], "claude");
            let mut expected = base.clone(); expected["role"] = json!("reviewer");
            assert_eq!(verdict["identity"], expected);
        }
        let settings = f.ctx.app.settings.lock().unwrap();
        let sessions = settings["test_review_sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["model"], "claude-fable-5-1[1m]");
        assert_eq!(sessions[1]["model"], "opus[1m]");
        assert!(sessions.iter().all(|s| s["provider"] == "claude" && s["session"].is_null()));
        assert_eq!(plan["stages"][0]["rounds"], rounds);
        assert_eq!(plan["stages"][0]["reassessment"]["operational_retries"], retry_count);
    }
}

#[test]
fn cli_model_limit_requires_a_failed_process_and_the_selected_family() {
    let error = "claude exited with exit status: 1: You've reached your Fable limit. Switch to another model.";
    assert!(claude_model_limit(error, "claude-fable-5-1[1m]").is_some());
    for model in ["opus[1m]", "", "claude-notfable-5"] {
        assert!(claude_model_limit(error, model).is_none());
    }
    for unrelated in ["You've reached your Fable limit.", "claude exited with exit status: 1: authentication failed", "claude exited with exit status: 1: rate limit exceeded", "invalid verdict: You've reached your Fable limit."] {
        assert!(claude_model_limit(unrelated, "claude-fable-5-1[1m]").is_none());
    }
}

#[test]
fn cli_limit_fallback_preserves_explicit_choices_and_capability_floor() {
    let f = Fixture::new("Fix prose spelling", 0);
    f.setting("reviewer", json!("claude"));
    f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
        {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
        {"provider":"claude","model":"opus[1m]","tier":"standard"}]);
    let plan = json!({"stages":[{"implementer_provider":"codex"}]});
    let current = ("claude".into(), "claude-fable-5-1[1m]".into());
    let visited = vec![current.clone()];
    let error = "claude exited with exit status: 1: You've reached your Fable limit.";
    assert!(f.ctx.reviewer_quota_fallback(&plan,0,&current,error,&visited).is_err());
    f.setting("reviewer_model",json!("claude-fable-5-1[1m]"));
    assert!(f.ctx.reviewer_quota_fallback(&plan,0,&current,error,&visited).unwrap().is_none());
    f.setting("reviewer_model",json!(""));
    f.setting("automatic_routing",json!(false));
    assert!(f.ctx.reviewer_quota_fallback(&plan,0,&current,error,&visited).unwrap().is_none());
}

#[test]
fn default_cadence_defers_stage_roles_until_plan_review() {
    let f = Fixture::with_settings("Implement feature", 0, false, crate::plan::default_settings());
    f.setting("mock_edits", json!([{"feature.rs":"fn feature() {}\n"}]));
    let base = f.ctx.git(&["rev-parse", "HEAD"]).unwrap();
    let p = f.run();
    let stage = &p["stages"][0];
    assert_eq!(stage["status"], "committed", "{p}");
    assert_eq!(stage["review_cadence"], json!({"architect":"per_plan","reviewer":"per_plan"}));
    assert_eq!(stage["review_policy"]["deferred_roles"], json!(["architect","reviewer"]));
    assert_eq!(stage["review_gate"]["status"], "deferred");
    assert_eq!(stage["review_gate"]["roles"], json!({"architect":"deferred","reviewer":"deferred"}));
    assert_eq!(stage["reviews"], json!([]));
    for role in ["architect", "reviewer", "fixer"] {
        assert_eq!(f.count(role), 0, "unexpected stage {role} invocation");
    }
    assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
    assert_eq!(f.ctx.git(&["rev-parse", "HEAD^"]).unwrap(), base);
    assert_eq!(f.ctx.git(&["show", "HEAD:feature.rs"]).unwrap(), "fn feature() {}");
    assert_eq!(f.ctx.git(&["rev-parse", "--short", "HEAD"]).unwrap(), stage["sha"]);

    let review = &p["plan_review"];
    assert_eq!(review["required_roles"], stage["review_policy"]["deferred_roles"]);
    assert_eq!(review["gate"]["status"], "approved");
    assert_eq!(review["gate"]["roles"], json!({"architect":"approved","reviewer":"approved"}));
    assert_eq!(review["base"], base);
    assert_eq!(review["head"], f.ctx.git(&["rev-parse", "HEAD"]).unwrap());
    let calls = f.plan_calls();
    assert_eq!(calls.len(), 2);
    for role in ["architect", "reviewer"] {
        assert_eq!(calls.iter().filter(|call| call["role"] == role).count(), 1);
    }
    f.ctx.validate_plan_approval(&p).unwrap();
    assert_eq!(p["status"], "done", "{p}");
}

#[test]
fn deferred_architect_commits_with_only_independent_verdicts_including_promotion() {
    for promote in [false, true] {
        let f = Fixture::new(if promote { "Fix prose spelling" } else { "Implement feature" }, 0);
        f.setting("review_cadence", json!({"architect":"per_plan","reviewer":"per_stage"}));
        if promote {
            f.docs();
            f.setting("mock_verdicts", json!([{"approved":true,"issues":[],"requires_dual":true,"scope_reason":"Architectural impact"},clean()]));
        }
        let p = f.run();
        let stage = &p["stages"][0];
        assert_eq!(stage["status"], "committed", "{p}");
        assert_eq!(stage["review_policy"]["required_roles"], json!(["architect","reviewer"]));
        assert_eq!(stage["review_policy"]["stage_required_roles"], json!(["reviewer"]));
        assert_eq!(stage["review_policy"]["deferred_roles"], json!(["architect"]));
        assert_eq!(stage["review_gate"]["status"], "approved");
        assert_eq!(stage["review_gate"]["roles"], json!({"architect":"deferred","reviewer":"approved"}));
        assert_eq!(stage["review_gate"]["identity"]["policy"], stage["review_policy"]);
        assert_eq!(f.count("reviewer"), if promote { 2 } else { 1 });
        assert_eq!(f.count("architect"), 0);
        assert_eq!(f.count("fixer"), 0);
        assert!(stage["reviews"].as_array().unwrap().iter().all(|r| r["role"] == "reviewer"));
        assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
    }
}

#[test]
fn deferred_reviewer_needs_no_reviewer_configuration_and_all_deferred_skips_fixes() {
    for architect in ["per_stage", "per_plan"] {
        for budget in [0, 2] {
            let f = Fixture::new("Implement feature", budget);
            f.setting("review_cadence", json!({"architect":architect,"reviewer":"per_plan"}));
            f.setting("reviewer", json!("unavailable"));
            assert!(f.ctx.reviewer_config("mock").is_err());
            let p = f.run();
            let stage = &p["stages"][0];
            assert_eq!(stage["status"], "committed", "{p}");
            // Round zero is durably reserved as rounds/identity.round == 1.
            assert_eq!(stage["rounds"], 1);
            assert_eq!(stage["review_gate"]["identity"]["round"], 1);
            assert_eq!(stage["review_gate"]["roles"]["reviewer"], "deferred");
            let fully_deferred = architect == "per_plan";
            assert_eq!(stage["review_gate"]["status"], if fully_deferred { "deferred" } else { "approved" });
            assert_eq!(stage["review_gate"]["roles"]["architect"], if fully_deferred { "deferred" } else { "approved" });
            assert_eq!(f.count("reviewer"), 0);
            assert_eq!(f.count("architect"), usize::from(!fully_deferred));
            assert_eq!(f.count("fixer"), 0);
            assert_eq!(stage["reviews"].as_array().unwrap().len(), usize::from(!fully_deferred));
            assert_eq!(f.ctx.git(&["rev-parse", "HEAD^{tree}"]).unwrap(), stage["review_gate"]["identity"]["snapshot"]["tree"]);
            assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
        }
    }
}

#[test]
fn deferred_reviewer_constraints_allow_codex_assignment_reassessment_and_local_commit() {
    for pinned in [false, true] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("implementer", json!("codex"));
        f.setting("reviewer", json!("codex"));
        f.setting("automatic_routing", json!(pinned));
        f.setting("reviewer_model", json!(if pinned { "stage-model" } else { "" }));
        f.setting("test_fake_providers", json!(true));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"stage-model","tier":"strong"},
            {"provider":"claude","model":"other-model","tier":"strong"}
        ]);
        let proposal = json!({"risk":"standard","complexity":"standard","task":"functionality",
            "provider":"codex","model":"stage-model","native_effort":"provider_default",
            "rationale":"Configured strong capability is adequate for this feature."});
        f.setting("mock_routing_planner_outputs", json!([{"proposals":[{"stage_id":1,"proposal":proposal}]}]));
        let cadence = json!({"architect":"per_plan","reviewer":"per_plan"});
        f.setting("review_cadence", cadence.clone());
        let initial = f.plan();
        let mut p = f.ctx.architect_publish(initial.clone(), Some(&initial), "test").unwrap();
        assert_eq!(p["stages"][0]["model_proposal"], proposal);
        assert_eq!(p["stages"][0]["model_agreement"]["reviewer"]["status"], "deferred");
        p["stages"][0]["attempt_id"] = json!(crate::architecture::identity());
        p["stages"][0]["attempt_revision"] = p["revision"].clone();
        p["stages"][0]["review_cadence"] = cadence.clone();
        for legacy in [false, true] {
            let mut required = p.clone();
            if legacy {
                required["stages"][0].as_object_mut().unwrap().remove("review_cadence");
            } else {
                required["stages"][0]["review_cadence"]["reviewer"] = json!("per_stage");
            }
            f.ctx.save_plan(&required).unwrap();
            let error = f.ctx.validated_assignment(&required, 0).unwrap_err();
            assert!(error.contains("cross-provider-review conflict"), "{error}");
            assert!(f.ctx.reviewer_config("codex").is_err());
        }
        f.ctx.save_plan(&p).unwrap();
        // Settings and explicit reviewer changes cannot invalidate a captured deferral.
        f.setting("review_cadence", json!({"architect":"per_stage","reviewer":"per_stage"}));
        f.setting("reviewer", json!("unavailable"));
        assert!(f.ctx.reviewer_config("codex").is_err());
        assert!(f.ctx.validated_assignment(&p, 0).is_ok());
        assert!(f.ctx.restored_assignment(&p, 0).is_ok());
        assert!(f.ctx.has_operational_alternative(&p, 0).unwrap());
        f.setting("mock_routing_planner_outputs", json!([{"proposals":[{"stage_id":1,"proposal":proposal}]}]));
        f.ctx.reassess(&mut p, 0, "material_assignment_change", json!({"test":"revalidate captured cadence"})).unwrap();
        assert_eq!(p["stages"][0]["model_proposal"], proposal);
        let p = f.run();
        let stage = &p["stages"][0];
        assert_eq!(stage["status"], "committed", "{p}");
        assert_eq!(stage["implementer_provider"], "codex");
        assert_eq!(stage["review_cadence"], cadence);
        assert_eq!(stage["review_gate"]["status"], "deferred");
        assert_eq!(stage["reviews"], json!([]));
        assert_eq!(stage["rounds"], 1);
        assert_eq!(f.count("reviewer"), 0);
        assert_eq!(f.count("architect"), 0);
        assert_eq!(f.count("fixer"), 0);
        assert_eq!(f.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
    }
}

#[test]
fn attempt_cadence_survives_settings_changes_and_legacy_absence_requires_stage_reviews() {
    for legacy in [false, true] {
        for architect in ["per_stage", "per_plan"] {
            let f = Fixture::new("Implement feature", 1);
            let cadence = json!({"architect":architect,"reviewer":"per_stage"});
            f.setting("review_cadence", cadence.clone());
            f.setting("mock_reviewer_actions", json!([{"stop":true}]));
            let first = f.run();
            assert_eq!(first["stages"][0]["review_cadence"], cadence);
            if legacy {
                let mut p = first.clone();
                p["stages"][0].as_object_mut().unwrap().remove("review_cadence");
                f.ctx.save_plan(&p).unwrap();
            }
            f.setting("review_cadence", json!({"architect":if architect == "per_stage" {"per_plan"} else {"per_stage"},"reviewer":"per_plan"}));
            let p = f.run();
            let stage = &p["stages"][0];
            assert_eq!(stage["status"], "committed", "{p}");
            assert_eq!(stage["attempt_id"], first["stages"][0]["attempt_id"]);
            assert_eq!(stage["review_cadence"], if legacy {Value::Null} else {cadence});
            assert_eq!(f.count("reviewer"), 2);
            assert_eq!(f.count("architect"), usize::from(legacy || architect == "per_stage"));
            assert_eq!(stage["review_gate"]["roles"]["architect"], if legacy || architect == "per_stage" {"approved"} else {"deferred"});
        }
    }
}

#[test]
fn exhausted_attempt_retains_deferred_role_outcomes() {
    for deferred in ["architect", "reviewer"] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("review_cadence", json!({"architect":if deferred == "architect" {"per_plan"} else {"per_stage"},"reviewer":if deferred == "reviewer" {"per_plan"} else {"per_stage"}}));
        f.setting(if deferred == "architect" {"mock_verdicts"} else {"mock_architect_verdicts"}, json!([reject("Fix the change")]));
        f.run();
        let p = f.run();
        assert_eq!(p["stages"][0]["review_gate"]["status"], "exhausted");
        assert_eq!(p["stages"][0]["review_gate"]["roles"][deferred], "deferred");
        assert_eq!(f.count(deferred), 0);
        f.assert_no_commit();
    }
}

impl Fixture {
    fn deferred_gate(&self) -> Value {
        let p = self.plan();
        let mut p = self.ctx.architect_publish(p.clone(), Some(&p), "test").unwrap();
        p["stages"][0]["attempt_id"] = json!(crate::architecture::identity());
        p["stages"][0]["attempt_revision"] = p["revision"].clone();
        p["stages"][0]["review_budget"] = json!(0);
        p["stages"][0]["review_cadence"] = json!({"architect":"per_plan","reviewer":"per_plan"});
        self.ctx.save_plan(&p).unwrap();
        assert_eq!(self.ctx.run_review_stage(&mut p, 0).unwrap(), "deferred");
        p
    }
}

#[test]
fn deferred_commit_rejects_forged_partitions_identity_outcomes_and_attempt_cadence() {
    for case in ["empty", "overlap", "missing", "unknown", "scope", "stage", "attempt", "round", "cadence", "persisted_cadence", "approved", "roles", "policy"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.deferred_gate();
        let stage = &mut p["stages"][0];
        let mut policy = stage["review_policy"].clone();
        match case {
            "empty" => policy["deferred_roles"] = json!([]),
            "overlap" => policy["stage_required_roles"] = json!(["architect"]),
            "missing" => { policy.as_object_mut().unwrap().remove("stage_required_roles"); },
            "unknown" => policy["deferred_roles"] = json!(["architect","unknown"]),
            "scope" => policy["required_roles"] = json!(["reviewer"]),
            "stage" => stage["review_gate"]["identity"]["stage_id"] = json!(2),
            "attempt" => stage["review_gate"]["identity"]["attempt_id"] = json!("other"),
            "round" => stage["review_gate"]["identity"]["round"] = json!(2),
            "cadence" | "persisted_cadence" => stage["review_cadence"]["architect"] = json!("per_stage"),
            "approved" => stage["review_gate"]["status"] = json!("approved"),
            "roles" => stage["review_gate"]["roles"]["architect"] = json!("approved"),
            "policy" => stage["review_policy"]["rationale"] = json!("tampered"),
            _ => unreachable!(),
        }
        if ["empty", "overlap", "missing", "unknown", "scope"].contains(&case) {
            stage["review_policy"] = policy.clone();
            stage["review_gate"]["policy"] = policy.clone();
            stage["review_gate"]["identity"]["policy"] = policy;
        }
        f.ctx.save_plan(&p).unwrap();
        if case == "persisted_cadence" {
            p["stages"][0]["review_cadence"]["architect"] = json!("per_plan");
        }
        assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err(), "accepted {case}");
        f.assert_no_commit();
    }
}

#[test]
fn deferred_commit_keeps_snapshot_checks_and_exact_commit_recovery() {
    for case in ["recover", "dirty", "wrong_message", "invalid_policy", "before_commit"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.deferred_gate();
        if case == "before_commit" {
            fs::write(f.root.join("README.md"), "Unreviewed content").unwrap();
            assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err());
            f.assert_no_commit();
            continue;
        }
        let sha = f.ctx.commit_reviewed(&p, 0, if case == "wrong_message" {"other"} else {"feat: stage"}).unwrap().unwrap();
        if case == "dirty" { fs::write(f.root.join("README.md"), "Unreviewed content").unwrap(); }
        if case == "invalid_policy" {
            p["stages"][0]["review_cadence"]["architect"] = json!("per_stage");
            f.ctx.save_plan(&p).unwrap();
        }
        let result = f.ctx.recover_committed_stages();
        if case == "recover" {
            assert_eq!(result.unwrap(), vec![1]);
            let saved = f.plan();
            assert_eq!(saved["stages"][0]["sha"], sha);
            assert_eq!(saved["stages"][0]["review_gate"], p["stages"][0]["review_gate"]);
            assert_eq!(saved["stages"][0]["reviews"], json!([]));
            assert!(f.ctx.recover_committed_stages().unwrap().is_empty());
        } else {
            assert!(result.is_err(), "accepted {case}");
            assert_ne!(f.plan()["stages"][0]["status"], "committed");
        }
    }
}

#[test]
fn rejected_deferred_obligations_block_completion_reports_and_push_even_after_restart() {
    for reviewer in ["per_stage", "per_plan"] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("review_cadence", json!({"architect":"per_plan","reviewer":reviewer}));
        f.setting("mock_architect_verdicts", json!([reject("Resolve the cross-stage defect")]));
        f.setting("auto_push", json!(true));
        let remote = f.root.join(".git/test-remote");
        f.ctx.git(&["init", "--bare", remote.to_str().unwrap()]).unwrap();
        f.ctx.git(&["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
        let mut approved = f.plan();
        approved["status"] = json!("approved");
        f.ctx.save_plan(&approved).unwrap();
        let (code, response) = crate::test_support::api_request(&f.ctx.app, "POST", "/api/run", json!({}));
        assert_eq!(code, 200, "{response}");
        crate::test_support::wait_for_worker(&f.ctx);
        let p = f.plan();
        assert_eq!(p["status"], "approved");
        assert_eq!(p["stages"][0]["status"], "committed", "{p}");
        assert_eq!(f.ctx.session.state.lock().unwrap().phase, "blocked");
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        assert!(f.ctx.git(&["ls-remote", "origin"]).unwrap().is_empty());
        let settings = {
            let mut settings = f.ctx.app.settings.lock().unwrap().clone();
            settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
            settings
        };
        let app = Arc::new(App::new(f.root.to_str().unwrap(), settings));
        let ctx = app.context(f.root.to_str().unwrap());
        let (code, response) = crate::test_support::api_request(&ctx.app, "POST", "/api/run", json!({}));
        assert_eq!(code, 200, "{response}");
        crate::test_support::wait_for_worker(&ctx);
        let resumed = ctx.load_plan().unwrap();
        assert_eq!(resumed["status"], "approved");
        assert_eq!(ctx.session.state.lock().unwrap().phase, "blocked");
        assert_eq!(resumed["stages"][0]["sha"], p["stages"][0]["sha"]);
        assert!(!ctx.forge_path("reports.jsonl").exists());
        assert!(ctx.git(&["ls-remote", "origin"]).unwrap().is_empty());
        assert_eq!(ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "2");
    }
}

#[test]
fn a_new_attempt_captures_new_cadence_while_preserving_the_model_proposal() {
    let f = Fixture::new("Implement feature", 1);
    f.setting("mock_reviewer_actions", json!([{"stop":true}]));
    let first = f.run();
    let mut revised = first.clone();
    revised["revision"] = json!(first["revision"].as_u64().unwrap() + 1);
    f.ctx.save_plan(&revised).unwrap();
    let cadence = json!({"architect":"per_plan","reviewer":"per_plan"});
    f.setting("review_cadence", cadence.clone());
    let p = f.run();
    let stage = &p["stages"][0];
    assert_eq!(stage["status"], "committed", "{p}");
    assert_ne!(stage["attempt_id"], first["stages"][0]["attempt_id"]);
    assert_eq!(stage["review_cadence"], cadence);
    assert_eq!(stage["model_proposal"], first["stages"][0]["model_proposal"]);
    assert_eq!(stage["review_gate"]["status"], "deferred");
    assert_eq!(stage["rounds"], 1);
    assert_eq!(f.count("reviewer"), 1);
    assert_eq!(f.count("architect"), 0);
    assert_eq!(f.count("fixer"), 0);
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

#[test]
fn legacy_policy_without_partition_still_requires_evidenced_role_approvals() {
    for invalid in [false, true] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.reviewed();
        let stage = &mut p["stages"][0];
        let mut policy = stage["review_policy"].clone();
        policy.as_object_mut().unwrap().remove("stage_required_roles");
        policy.as_object_mut().unwrap().remove("deferred_roles");
        stage["review_policy"] = policy.clone();
        let mut base = stage["review_gate"]["identity"].clone();
        base["policy"] = policy.clone();
        let records = stage["reviews"].as_array_mut().unwrap();
        for record in records.iter_mut() {
            record["policy"] = policy.clone();
            record["identity"]["policy"] = policy.clone();
        }
        if invalid { records[0]["criteria"] = json!([]); }
        stage["review_gate"] = aggregate(&base, records);
        f.ctx.save_plan(&p).unwrap();
        let result = f.ctx.commit_reviewed(&p, 0, "feat: stage");
        assert_eq!(result.is_err(), invalid, "{result:?}");
    }
}

impl Fixture {
    fn two_deferred_stages(&self) -> String {
        let mut p = self.plan();
        let mut second = p["stages"][0].clone();
        second["id"] = json!(2);
        second["title"] = json!("Integrate feature");
        second["instructions"] = json!("Integrate the second feature with the first.");
        second["acceptance"] = json!("Second feature works.\n\n Both features integrate. ");
        second["commit"] = json!("feat: second stage");
        p["stages"].as_array_mut().unwrap().push(second);
        self.ctx.save_plan(&p).unwrap();
        self.setting("review_cadence", json!({"architect":"per_plan","reviewer":"per_plan"}));
        self.setting("mock_edits", json!([{"first.rs":"fn first() {}\n"},{"second.rs":"fn second() {}\n"}]));
        self.ctx.git(&["rev-parse","HEAD"]).unwrap()
    }
    fn plan_calls(&self) -> Vec<Value> {
        self.ctx.app.settings.lock().unwrap()["test_review_sessions"].as_array().into_iter().flatten()
            .filter(|r| r["prompt"].as_str().unwrap().contains("\"scope\":\"plan\"" )).cloned().collect()
    }
}

#[test]
fn plan_review_captures_range_evidence_storage_sessions_and_gates_publication() {
    for push in [false,true] {
        let f = Fixture::new("Implement feature",0);
        let base = f.two_deferred_stages();
        f.setting("auto_push",json!(push));
        f.setting("mock_usage",json!({"input":2,"output":3,"total":5}));
        let remote = f.root.join(".git/test-remote");
        f.ctx.git(&["init","--bare",remote.to_str().unwrap()]).unwrap();
        f.ctx.git(&["remote","add","origin",remote.to_str().unwrap()]).unwrap();
        let p = f.run();
        assert_eq!(p["status"],"done","{p}");
        assert_eq!(f.ctx.session.state.lock().unwrap().phase,"done");
        for stage in p["stages"].as_array().unwrap() {
            assert_eq!(stage["status"],"committed");
            assert_eq!(stage["review_gate"]["status"],"deferred");
            assert_eq!(stage["reviews"],json!([]));
        }
        let r = &p["plan_review"];
        assert_eq!(r["base"],base);
        assert!(r["attempt_id"].is_string());
        assert_eq!(r["head"],f.ctx.git(&["rev-parse","HEAD"]).unwrap());
        assert_eq!(r["budget"],0);
        assert_eq!(r["rounds"],1);
        assert_eq!(r["gate"]["status"],"approved");
        assert!(r["fix_sha"].is_null());
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"3");
        assert_eq!(r["next_action"],"complete");
        assert_eq!(r["required_roles"],json!(["architect","reviewer"]));
        assert_eq!(r["acceptance"],"stage 1 (Implement feature): The requested change works.\nstage 2 (Integrate feature): Second feature works.\nstage 2 (Integrate feature): Both features integrate.");
        let calls = f.plan_calls();
        assert_eq!(calls.len(),2);
        assert_eq!(calls[0]["role"],"reviewer");
        assert!(calls[0]["session"].is_null());
        assert_eq!(calls[1]["role"],"architect");
        assert!(calls[1]["session"].is_string());
        for (i,call) in calls.iter().enumerate() {
            let prompt = call["prompt"].as_str().unwrap();
            assert!(prompt.contains(&format!("git log --stat {base}..HEAD")));
            assert!(prompt.contains(&format!("git diff {base}..HEAD")));
            for stage in r["subject"]["stages"].as_array().unwrap() {
                for key in ["title","instructions","commit","sha"] { assert!(prompt.contains(stage[key].as_str().unwrap())); }
            }
            for rule in ["/tmp", "Do not change source", "Return ONLY JSON", "8734", "18734", "CRITERIA TO EVIDENCE"] { assert!(prompt.contains(rule)); }
            let verdict = &r["reviews"][i];
            assert_eq!(verdict["identity"]["scope"],"plan");
            assert!(verdict["identity"]["stage_id"].is_null());
            assert_eq!(verdict["identity"]["round"],1);
            assert_eq!(verdict["criteria"].as_array().unwrap().len(),3);
            normalize(&verdict.to_string(),&verdict["identity"],r["acceptance"].as_str().unwrap()).unwrap();
        }
        let raw = f.ctx.architecture_store().load_raw().unwrap().unwrap();
        assert!(raw["plan_review"]["reviews"]["$forge_reviews"].is_string());
        assert!(raw["architecture"]["review_history"].as_object().unwrap().keys().all(|k| k == "1" || k == "2"));
        let state = f.ctx.architecture_store().state_plan(raw);
        assert_eq!(state["plan_review"]["review_count"],2);
        for preview in state["plan_review"]["reviews"].as_array().unwrap() {
            for key in ["identity","acceptance_evidence","criteria","project_checks"] { assert!(preview[key].is_null()); }
        }
        let log = fs::read_to_string(f.ctx.forge_path("architecture").join(p["plan_id"].as_str().unwrap()).join("events.jsonl")).unwrap();
        let events: Vec<Value> = log.lines().map(|line| serde_json::from_str(line).unwrap()).filter(|e: &Value| e["payload"]["kind"] == "plan_review").collect();
        assert_eq!(events.len(),2);
        for (i,event) in events.iter().enumerate() { assert_eq!(event["payload"]["reviews"][0],r["reviews"][i]); }
        assert_eq!(p["role_usage"]["reviewer"]["mock"]["total_tokens"],5);
        assert!(p["usage"]["mock"]["total_tokens"].as_i64().unwrap() >= 10);
        assert_eq!(fs::read_to_string(f.ctx.forge_path("reports.jsonl")).unwrap().lines().count(),1);
        assert_eq!(!f.ctx.git(&["ls-remote","origin"]).unwrap().is_empty(),push);
        f.ctx.validate_plan_approval(&p).unwrap();
        let before = calls.len();
        let mut resumed = f.plan();
        assert!(f.ctx.run_plan_review(&mut resumed).unwrap());
        assert_eq!(f.plan_calls().len(),before);
    }
}

#[test]
fn plan_review_errors_stops_and_budget_never_publish_partial_approval() {
    for kind in ["reject","evidence","stop","mutation","error"] {
        let f = Fixture::new("Implement feature",0);
        f.two_deferred_stages();
        match kind {
            "reject" => f.setting("mock_architect_verdicts",json!([reject("Fix cross-stage integration")])),
            "evidence" => f.setting("mock_verdicts",json!(vec![json!({"approved":true,"issues":[],"criteria":[]}); 4])),
            "stop" => f.setting("mock_architect_actions",json!([{"stop":true}])),
            "mutation" => f.setting("mock_architect_actions",json!([{"write":{"first.rs":"changed during review"}}])),
            _ => f.setting("mock_architect_actions",json!([{"error":"review provider unavailable"}])),
        }
        let p = f.run();
        assert_eq!(p["plan_review"]["rounds"],1,"{kind}: {p}");
        assert_eq!(p["plan_review"]["status"],if kind == "stop" {"interrupted"} else {"blocked"});
        assert_eq!(f.ctx.session.state.lock().unwrap().phase,if kind == "stop" {"plan_ready"} else {"blocked"});
        assert_eq!(p["plan_review"]["gate"]["status"],if kind == "stop" {"interrupted"} else if kind == "reject" {"exhausted"} else {"error"});
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"3");
        let calls = f.plan_calls().len();
        f.ctx.session.stop_requested.store(false,Ordering::SeqCst);
        f.setting("max_fix_rounds",json!(10));
        let resumed = f.run();
        assert_eq!(resumed["plan_review"]["rounds"],1);
        assert_eq!(resumed["plan_review"]["budget"],0);
        assert_eq!(f.plan_calls().len(),calls);
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
    }
}

#[test]
fn plan_review_approved_revision_starts_new_attempt_and_preserves_evidence() {
    use crate::test_support::api_request;

    for rejected in [false,true] {
        for add_stage in [false,true] {
            let f = Fixture::new("Implement feature",0);
            let base = f.two_deferred_stages();
            if rejected {
                f.setting("mock_architect_verdicts",json!([reject("Fix cross-stage integration")]));
            }
            let previous = f.run();
            let old = &previous["plan_review"];
            assert_eq!(old["status"],if rejected {"blocked"} else {"approved"});
            assert_eq!(old["rounds"],1);
            assert_eq!(old["budget"],0);
            let old_reviews = old["reviews"].as_array().unwrap();
            assert_eq!(old_reviews.len(),2);

            let mut incoming = previous.clone();
            incoming["goal"] = json!("Improve the project and verify integration");
            if add_stage {
                incoming["stages"].as_array_mut().unwrap().push(json!({"id":3,
                    "title":"Complete integration","instructions":"Integrate the completed features.",
                    "acceptance":"All three features integrate.\n Existing behavior works.",
                    "commit":"feat: complete integration"}));
                f.setting("mock_edits",json!([{"third.rs":"fn third() {}\n"}]));
            }
            let response = api_request(&f.ctx.app,"POST","/api/plan/edit",json!({"plan":incoming}));
            assert_eq!(response.0,200,"{response:?}");
            let draft = f.plan();
            assert_eq!(draft["status"],"draft");
            assert_eq!(draft["revision"].as_u64().unwrap(),previous["revision"].as_u64().unwrap()+1);
            let mut pending = old.clone();
            pending["pending_revision"] = draft["revision"].clone();
            assert_eq!(draft["plan_review"],pending);
            assert!(f.ctx.validate_plan_approval(&draft).is_err());
            assert_eq!(api_request(&f.ctx.app,"POST","/api/run",json!({})).0,400);
            f.setting("max_fix_rounds",json!(2));
            let response = api_request(&f.ctx.app,"POST","/api/approve",json!({}));
            assert_eq!(response.0,200,"{response:?}");
            assert_eq!(f.plan()["status"],"approved");

            let completed = f.run();
            assert_eq!(completed["status"],"done","rejected={rejected}, add_stage={add_stage}: {completed}");
            assert_eq!(f.ctx.session.state.lock().unwrap().phase,"done");
            let new = &completed["plan_review"];
            assert_ne!(new["attempt_id"],old["attempt_id"]);
            assert_eq!(new["status"],"approved");
            assert_eq!(new["rounds"],1);
            assert_eq!(new["budget"],2);
            assert!(new["pending_revision"].is_null());
            assert_eq!(new["base"],base);
            assert_eq!(new["head"],f.ctx.git(&["rev-parse","HEAD"]).unwrap());
            assert_eq!(new["head"] != old["head"],add_stage);
            assert_eq!(new["subject"]["revision"],completed["revision"]);
            assert_eq!(new["subject"]["goal"],incoming["goal"]);
            assert_eq!(new["subject"]["stages"].as_array().unwrap().len(),if add_stage {3} else {2});
            assert_eq!(new["required_roles"],json!(["architect","reviewer"]));
            let records = new["reviews"].as_array().unwrap();
            assert_eq!(&records[..2],old_reviews);
            assert_eq!(records.len(),4);
            for record in &records[2..] {
                assert_eq!(record["attempt_id"],new["attempt_id"]);
                assert_eq!(record["identity"]["revision"],completed["revision"]);
                assert_eq!(record["criteria"].as_array().unwrap().len(),if add_stage {5} else {3});
                assert_eq!(record["acceptance_evidence"]["acceptance"],new["acceptance"]);
            }
            assert_eq!(f.plan_calls().len(),4);
            assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),if add_stage {"4"} else {"3"});
            for i in 0..2 {
                assert_eq!(completed["stages"][i]["sha"],previous["stages"][i]["sha"]);
                assert_eq!(completed["stages"][i]["model_invocations"],previous["stages"][i]["model_invocations"]);
            }
            assert_eq!(fs::read_to_string(f.ctx.forge_path("reports.jsonl")).unwrap().lines().count(),if rejected {1} else {2});
            f.ctx.validate_plan_approval(&completed).unwrap();
            let raw = f.ctx.architecture_store().load_raw().unwrap().unwrap();
            assert_eq!(raw["architecture"]["plan_review_history"]["count"],4);
            let log = fs::read_to_string(f.ctx.forge_path("architecture").join(completed["plan_id"].as_str().unwrap()).join("events.jsonl")).unwrap();
            let verdicts: Vec<Value> = log.lines().map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|e| e["payload"]["kind"] == "plan_review")
                .map(|e| e["payload"]["reviews"][0].clone()).collect();
            assert_eq!(&verdicts,records);
        }
    }
}

#[test]
fn plan_review_no_obligations_skips_even_with_unrelated_plan_review_state() {
    let f = Fixture::new("Implement feature",0);
    let mut p = f.plan();
    p["plan_review"] = json!({"version":1,"status":"blocked","reviews":[]});
    f.ctx.save_plan(&p).unwrap();
    let p = f.run();
    assert_eq!(p["status"],"done");
    assert_eq!(p["plan_review"]["status"],"blocked");
    assert!(f.plan_calls().is_empty());
    assert_eq!(f.count("architect"),1);
    assert_eq!(f.count("reviewer"),1);
}

#[test]
fn plan_review_architect_only_marks_independent_reviewer_not_required() {
    let f = Fixture::new("Implement feature",0);
    f.setting("review_cadence",json!({"architect":"per_plan","reviewer":"per_stage"}));
    let p = f.run();
    assert_eq!(p["plan_review"]["gate"]["roles"],json!({"architect":"approved","reviewer":"not_required"}));
    assert_eq!(f.plan_calls().len(),1);
}

impl Fixture {
    fn pending_plan_review(&self) -> Value {
        self.two_deferred_stages();
        self.setting("reviewer",json!("unavailable"));
        let mut p = self.run();
        assert!(p["stages"].as_array().unwrap().iter().all(|s| s["status"] == "committed"));
        p.as_object_mut().unwrap().remove("plan_review");
        self.ctx.save_plan(&p).unwrap();
        self.setting("reviewer",json!("mock"));
        self.plan()
    }
}

#[test]
fn plan_review_provider_exclusion_covers_all_stages_prior_fixers_and_fallback() {
    for kind in ["single","mixed","fixer","unknown","fallback","exhausted"] {
        let f = Fixture::new("Implement feature",0);
        let mut p = f.pending_plan_review();
        for stage in p["stages"].as_array_mut().unwrap() {
            stage["implementer_provider"] = json!("codex");
            stage["model_invocations"] = json!([]);
        }
        if kind == "mixed" { p["stages"][1]["implementer_provider"] = json!("claude"); }
        if kind == "fixer" { p["stages"][0]["model_invocations"] = json!([{"role":"fixer","requested":{"provider":"claude"},"status":"failed"}]); }
        if kind == "unknown" { p["stages"][1]["implementer_provider"] = Value::Null; }
        f.ctx.save_plan(&p).unwrap();
        f.setting("test_fake_providers",json!(true));
        f.setting("reviewer",json!("claude"));
        f.setting("test_review_sessions",json!([]));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"claude","model":"claude-fable-5-1[1m]","tier":"strong"},
            {"provider":"claude","model":"opus[1m]","tier":"strong"},
            {"provider":"codex","model":"stage-model","tier":"strong"}]);
        let refusal = |family: &str| json!({"error":format!("claude exited with exit status: 1: You've reached your {family} limit.")});
        if kind == "fallback" { f.setting("mock_reviewer_actions",json!([refusal("Fable")])); }
        if kind == "exhausted" { f.setting("mock_reviewer_actions",json!([refusal("Fable"),refusal("Opus")])); }
        let p = f.run();
        let approved = matches!(kind,"single"|"fallback");
        assert_eq!(p["plan_review"]["status"],if approved {"approved"} else {"blocked"},"{kind}: {p}");
        let calls = f.plan_calls();
        let reviewers: Vec<_> = calls.iter().filter(|c| c["role"] == "reviewer").collect();
        assert!(reviewers.iter().all(|c| c["provider"] == "claude" && c["session"].is_null()));
        assert_eq!(reviewers.len(),if matches!(kind,"fallback"|"exhausted") {2} else {usize::from(approved)});
        if matches!(kind,"mixed"|"fixer") { assert!(p["plan_review"]["gate"]["error"].as_str().unwrap().contains("provider no stage implementer used")); }
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),approved);
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"3");
    }
}

#[test]
fn plan_review_requires_recoverable_base_and_frozen_subject_on_resume() {
    for kind in ["fallback","missing","nonancestor","inputs","head","scope","evidence"] {
        let f = Fixture::new("Implement feature",1);
        let mut p = f.pending_plan_review();
        let expected = p["stages"][0]["attempt_head"].clone();
        if matches!(kind,"fallback"|"missing") { p["stages"][0].as_object_mut().unwrap().remove("attempt_head"); }
        if kind == "missing" { p["stages"][0].as_object_mut().unwrap().remove("sha"); }
        if kind == "nonancestor" {
            let tree = f.ctx.git(&["rev-parse","HEAD^{tree}"]).unwrap();
            let orphan = f.ctx.git(&["commit-tree",&tree,"-m","unrelated history"]).unwrap();
            p["stages"][0]["attempt_head"] = json!(orphan);
        }
        f.ctx.save_plan(&p).unwrap();
        let p = f.run();
        if matches!(kind,"missing"|"nonancestor") {
            assert_eq!(f.ctx.session.state.lock().unwrap().phase,"blocked");
            assert!(f.plan_calls().is_empty());
            assert!(!f.ctx.forge_path("reports.jsonl").exists());
            continue;
        }
        assert_eq!(p["plan_review"]["base"],expected);
        assert_eq!(p["plan_review"]["status"],"approved","{p}");
        let mut changed = p.clone();
        match kind {
            "inputs" => changed["stages"][0]["acceptance"] = json!("Changed criterion"),
            "head" => { f.ctx.git(&["commit","--allow-empty","-qm","external change"]).unwrap(); },
            "scope" => changed["plan_review"]["reviews"][0]["identity"]["scope"] = json!("stage"),
            "evidence" => changed["plan_review"]["reviews"][0]["criteria"] = json!([]),
            _ => continue,
        }
        assert!(f.ctx.validate_plan_approval(&changed).is_err(),"accepted {kind}");
        f.ctx.save_plan(&changed).unwrap();
        let calls = f.plan_calls().len();
        let resumed = f.run();
        assert_eq!(f.ctx.session.state.lock().unwrap().phase,"blocked");
        assert_eq!(resumed["plan_review"]["status"],"blocked");
        assert_eq!(f.plan_calls().len(),calls);
        assert_eq!(fs::read_to_string(f.ctx.forge_path("reports.jsonl")).unwrap().lines().count(),1);
    }
}

#[test]
fn plan_review_stop_resumes_a_new_round_with_persistent_architect_recovery() {
    let f = Fixture::new("Implement feature",1);
    f.two_deferred_stages();
    f.setting("mock_architect_actions",json!([{"stop":true}]));
    let p = f.run();
    let cp = f.ctx.architecture_store().checkpoint(&p).unwrap();
    assert_eq!(p["plan_review"]["reviews"].as_array().unwrap().len(),1);
    f.ctx.session.stop_requested.store(false,Ordering::SeqCst);
    let resumed = f.run();
    assert_eq!(resumed["status"],"done","{resumed}");
    assert_eq!(resumed["plan_review"]["attempt_id"],p["plan_review"]["attempt_id"]);
    assert_eq!(resumed["plan_review"]["rounds"],2);
    assert_eq!(resumed["plan_review"]["reviews"].as_array().unwrap().len(),3);
    let cp2 = f.ctx.architecture_store().checkpoint(&resumed).unwrap();
    assert_ne!(cp2["session"]["reference"],cp["session"]["reference"]);
    for call in f.ctx.app.settings.lock().unwrap()["mock_architect_requests"].as_array().unwrap() {
        assert!(!call["prompt"].as_str().unwrap().contains("acceptance_evidence"));
    }
    assert_eq!(f.plan_calls().len(),4);
}

#[test]
fn plan_review_head_drift_does_not_replenish_interrupted_attempt() {
    let f = Fixture::new("Implement feature",1);
    f.two_deferred_stages();
    f.setting("mock_architect_actions",json!([{"stop":true}]));
    let interrupted = f.run();
    assert_eq!(interrupted["plan_review"]["status"],"interrupted");
    f.ctx.session.stop_requested.store(false,Ordering::SeqCst);
    f.ctx.git(&["commit","--allow-empty","-qm","unexpected HEAD movement"]).unwrap();
    f.setting("max_fix_rounds",json!(10));
    let blocked = f.run();
    assert_eq!(blocked["plan_review"]["status"],"blocked");
    assert_eq!(blocked["plan_review"]["gate"]["status"],"error");
    assert!(blocked["plan_review"]["gate"]["error"].as_str().unwrap().contains("edit and approve a new plan revision"));
    for key in ["attempt_id","head","subject","rounds","budget","reviews"] {
        assert_eq!(blocked["plan_review"][key],interrupted["plan_review"][key],"{key}");
    }
    assert_eq!(f.plan_calls().len(),2);
    assert!(!f.ctx.forge_path("reports.jsonl").exists());
}

#[test]
fn plan_review_uses_earliest_deferred_base_but_all_stages_as_context() {
    let f = Fixture::new("Implement feature",0);
    let mut p = f.pending_plan_review();
    p["stages"][0]["review_policy"]["deferred_roles"] = json!([]);
    p["stages"][0]["review_policy"]["stage_required_roles"] = json!(["architect","reviewer"]);
    let expected = p["stages"][1]["attempt_head"].clone();
    f.ctx.save_plan(&p).unwrap();
    let p = f.run();
    assert_eq!(p["status"],"done");
    assert_eq!(p["plan_review"]["base"],expected);
    assert_eq!(p["plan_review"]["subject"]["stages"].as_array().unwrap().len(),2);
    let acceptance = p["plan_review"]["acceptance"].as_str().unwrap();
    assert!(acceptance.starts_with("stage 1 (Implement feature):"));
    for call in f.plan_calls() {
        let prompt = call["prompt"].as_str().unwrap();
        assert!(prompt.contains("contextual stages outside this range"));
        assert!(prompt.contains(&format!("CRITERIA TO EVIDENCE:\n{acceptance}\n")));
    }
}

#[test]
fn plan_review_fixes_are_reviewed_then_committed_once() {
    for deleted in [false, true] {
        let f = Fixture::new("Implement feature", 1);
        f.two_deferred_stages();
        f.setting("mock_edits", json!([{"first.rs":"first"},{"second.rs":"second"},{"first.rs":"fixed"}]));
        f.setting("mock_verdicts", json!([reject("Fix feature behavior"),clean()]));
        f.setting("mock_architect_verdicts", json!([reject("Fix integration"),clean()]));
        f.setting("mock_usage", json!({"input":2,"output":3,"total":5}));
        if deleted { f.setting("mock_fixer_actions",json!([{"remove":["second.rs"]}])); }
        let p = f.run();
        let r = &p["plan_review"];
        assert_eq!(p["status"],"done","{p}");
        assert_eq!(r["rounds"],2);
        assert_eq!(r["budget"],1);
        assert_eq!(r["next_action"],"complete");
        assert_eq!(r["fix_sha"],f.ctx.git(&["rev-parse","--short","HEAD"]).unwrap());
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
        assert_eq!(f.ctx.git(&["show","-s","--format=%B","HEAD"]).unwrap(),"fix(review): apply deferred plan review findings");
        assert_eq!(f.ctx.git(&["rev-parse","HEAD^{tree}"]).unwrap(),r["gate"]["identity"]["snapshot"]["tree"]);
        assert_eq!(f.ctx.git(&["rev-parse","HEAD^"]).unwrap(),r["head"]);
        assert_eq!(f.plan_calls().len(),4);
        assert_eq!(r["reviews"].as_array().unwrap().len(),4);
        assert_ne!(r["reviews"][0]["identity"]["snapshot"],r["reviews"][2]["identity"]["snapshot"]);
        assert_eq!(r["model_invocations"].as_array().unwrap().len(),1);
        let call = &r["model_invocations"][0];
        for key in ["turn_id","requested","effective","model_reported","verification_state","usage"] { assert!(!call[key].is_null(),"{key}"); }
        assert_eq!(call["role"],"fixer");
        assert_eq!(call["round"],2);
        assert_eq!(call["verification_state"],"execution_verified");
        assert_eq!(p["role_usage"]["fixer"]["mock"]["total_tokens"],5);
        assert_eq!(r["usage"]["mock"]["total_tokens"],25);
        assert_eq!(r["usage"]["mock"]["calls"],5);
        let reports = f.ctx.read_reports();
        assert_eq!(reports[0]["plan_review"]["usage"],r["usage"]);
        assert_eq!(reports[0]["usage"],p["usage"]);
        let stage_tokens: i64 = p["stages"].as_array().unwrap().iter()
            .map(|s| s["usage"]["mock"]["total_tokens"].as_i64().unwrap_or(0)).sum();
        assert_eq!(p["usage"]["mock"]["total_tokens"].as_i64().unwrap(),stage_tokens + 25);
        let settings = f.ctx.app.settings.lock().unwrap();
        let prompt = settings["mock_fixer_prompts"][0].as_str().unwrap();
        for text in ["Improve the project","[architect] Fix integration","[reviewer] Fix feature behavior","no commit, amend, rebase, reset --hard, cherry-pick, revert or any ref update","8734","18734","ARCHITECT GUIDANCE","SAVED CONSTRAINTS"] { assert!(prompt.contains(text),"{text}"); }
        for stage in r["subject"]["stages"].as_array().unwrap() {
            for key in ["title","instructions","acceptance","sha"] { assert!(prompt.contains(&stage[key].to_string())); }
        }
        assert!(prompt.contains(&format!("{}..HEAD",r["base"].as_str().unwrap())));
        drop(settings);
        f.ctx.validate_plan_approval(&p).unwrap();
        let mut resumed = f.plan();
        assert!(f.ctx.run_plan_review(&mut resumed).unwrap());
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
        assert_eq!(f.plan_calls().len(),4);
    }
}

#[test]
fn plan_review_exhaustion_preserves_corrections_budget_across_restart() {
    for budget in [0, 1, 2] {
        let f = Fixture::new("Implement feature",budget);
        f.two_deferred_stages();
        f.setting("mock_verdicts",json!(vec![reject("Fix behavior");4]));
        f.setting("auto_push",json!(true));
        let remote = f.root.join(".git/test-remote");
        f.ctx.git(&["init","--bare",remote.to_str().unwrap()]).unwrap();
        f.ctx.git(&["remote","add","origin",remote.to_str().unwrap()]).unwrap();
        let p = f.run();
        assert_eq!(p["plan_review"]["rounds"],budget+1);
        assert_eq!(p["plan_review"]["gate"]["status"],"exhausted");
        assert_eq!(p["plan_review"]["status"],"blocked");
        assert_eq!(p["plan_review"]["outstanding_requests"],json!(["[reviewer] Fix behavior"]));
        assert_eq!(f.count("fixer"),budget as usize);
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"3");
        assert!(p["plan_review"]["fix_sha"].is_null());
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        assert!(f.ctx.git(&["ls-remote","origin"]).unwrap().is_empty());
        // A new process/context with increased settings must retain the captured budget.
        let mut settings = f.ctx.app.settings.lock().unwrap().clone();
        settings["max_fix_rounds"] = json!(20);
        let app = Arc::new(App::new(f.root.to_str().unwrap(),settings));
        let ctx = app.context(f.root.to_str().unwrap());
        ctx.run_worker();
        let resumed = ctx.load_plan().unwrap();
        assert_eq!(resumed["plan_review"],p["plan_review"]);
        assert!(!ctx.forge_path("reports.jsonl").exists());
        assert!(ctx.git(&["ls-remote","origin"]).unwrap().is_empty());
    }
}

#[test]
fn plan_review_interrupted_fixer_consumes_reserved_cycle() {
    for budget in [1,2] {
        let f = Fixture::new("Implement feature",budget);
        f.two_deferred_stages();
        f.setting("mock_verdicts",json!([reject("Fix behavior"),clean()]));
        f.setting("mock_fixer_actions",json!([{"stop":true}]));
        let p = f.run();
        assert_eq!(p["plan_review"]["status"],"interrupted");
        assert_eq!(p["plan_review"]["next_action"],"fixing");
        assert_eq!(p["plan_review"]["rounds"],2);
        assert_eq!(p["plan_review"]["outstanding_requests"],json!(["[reviewer] Fix behavior"]));
        assert_eq!(f.plan_calls().len(),2);
        assert_eq!(f.count("fixer"),1);
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        f.ctx.session.stop_requested.store(false,Ordering::SeqCst);
        f.setting("max_fix_rounds",json!(10));
        let resumed = f.run();
        assert_eq!(resumed["plan_review"]["rounds"],if budget == 1 {2} else {3});
        assert_eq!(resumed["plan_review"]["budget"],budget);
        assert_eq!(f.count("fixer"),if budget == 1 {1} else {2});
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),budget == 2);
    }
}

#[test]
fn plan_review_recovers_only_exact_fix_commit_after_update_ref_crash() {
    for change in ["none","delete","message","content","subject","evidence"] {
        let f = Fixture::new("Implement feature",1);
        f.two_deferred_stages();
        f.setting("mock_verdicts",json!([reject("Fix behavior"),clean()]));
        if change == "delete" { f.setting("mock_fixer_actions",json!([{"remove":["second.rs"]}])); }
        f.setting("test_plan_commit_crash",json!(true));
        let p = f.run();
        assert_eq!(p["plan_review"]["next_action"],"finalize");
        assert_eq!(p["plan_review"]["gate"]["status"],"approved");
        assert!(p["plan_review"]["fix_sha"].is_null());
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
        let head = f.ctx.git(&["rev-parse","HEAD"]).unwrap();
        match change {
            "message" => { f.ctx.git(&["commit","--amend","-m","wrong message"]).unwrap(); }
            "content" => { fs::write(f.root.join("first.rs"),"external change").unwrap(); }
            "subject" => { let mut changed = p.clone(); changed["stages"][0]["acceptance"] = json!("Different criteria"); f.ctx.save_plan(&changed).unwrap(); }
            "evidence" => { let mut changed = p.clone(); changed["plan_review"]["reviews"][2]["criteria"] = json!([]); f.ctx.save_plan(&changed).unwrap(); }
            _ => {}
        }
        f.setting("test_plan_commit_crash",json!(false));
        let resumed = f.run();
        let recovered = matches!(change,"none"|"delete");
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),recovered,"{change}: {resumed}");
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
        if recovered {
            assert_eq!(f.ctx.git(&["rev-parse","HEAD"]).unwrap(),head);
            assert!(resumed["plan_review"]["fix_sha"].is_string());
            assert_eq!(resumed["plan_review"]["next_action"],"complete");
        } else { assert_eq!(f.ctx.session.state.lock().unwrap().phase,"blocked"); }
        assert_eq!(f.plan_calls().len(),4);
    }
}

#[test]
fn plan_review_crash_boundaries_distinguish_unstarted_and_completed_fixers() {
    for action in ["fix_pending","fixing","review_pending","reviewing","finalize"] {
        let f = Fixture::new("Implement feature",1);
        f.two_deferred_stages();
        f.setting("mock_verdicts",json!([reject("Fix behavior"),clean()]));
        f.setting("test_plan_crash_action",json!(action));
        let p = f.run();
        assert_eq!(p["plan_review"]["rounds"],2);
        assert_eq!(p["plan_review"]["next_action"],action);
        assert!(!f.ctx.forge_path("reports.jsonl").exists());
        assert_eq!(f.count("fixer"),usize::from(!matches!(action,"fix_pending"|"fixing")));
        f.setting("test_plan_crash_action",Value::Null);
        let resumed = f.run();
        let completes = matches!(action,"fix_pending"|"review_pending"|"finalize");
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),completes,"{action}: {resumed}");
        assert_eq!(resumed["plan_review"]["rounds"],2);
        assert_eq!(resumed["plan_review"]["attempt_id"],p["plan_review"]["attempt_id"]);
        assert_eq!(f.count("fixer"),usize::from(action != "fixing"));
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),if completes {"4"} else {"3"});
    }
}

#[test]
fn plan_review_fixer_head_changes_block_without_engine_commit_or_report() {
    let f = Fixture::new("Implement feature",2);
    f.two_deferred_stages();
    f.setting("mock_verdicts",json!([reject("Fix behavior")]));
    f.setting("mock_fixer_actions",json!([{"git":["commit","--allow-empty","-m","unauthorized fixer commit"]}]));
    let p = f.run();
    assert_eq!(p["plan_review"]["status"],"blocked");
    assert!(p["plan_review"]["gate"]["error"].as_str().unwrap().contains("HEAD moved"));
    assert_eq!(p["plan_review"]["rounds"],2);
    assert!(p["plan_review"]["fix_sha"].is_null());
    assert_eq!(f.plan_calls().len(),2);
    assert!(!f.ctx.forge_path("reports.jsonl").exists());
    let resumed = f.run();
    assert_eq!(resumed["plan_review"]["rounds"],2);
    assert_eq!(f.count("fixer"),1);
    assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"4");
}

#[test]
fn plan_review_fixer_uses_shared_capability_retry_fallback_and_provenance() {
    for kind in ["transient","retry_exhausted","fallback","weak_pin","other_provider","substitution"] {
        let f = Fixture::new("Implement feature",1);
        let mut p = f.pending_plan_review();
        for stage in p["stages"].as_array_mut().unwrap() {
            stage["implementer_provider"] = json!("claude");
            stage["model_invocations"] = json!([]);
            stage["model_agreement"]["policy_inputs"]["minimum_tier"] = json!(3);
        }
        f.ctx.save_plan(&p).unwrap();
        let stages = f.plan()["stages"].clone();
        f.setting("implementer",json!(if kind == "other_provider" {"codex"} else {"claude"}));
        f.setting("reviewer",json!("codex"));
        f.setting("test_fake_providers",json!(true));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"claude","model":"claude-fable-5-1","tier":"strong"},
            {"provider":"claude","model":"claude-opus-4-6","tier":"strong"},
            {"provider":"claude","model":"weak-model","tier":"basic"},
            {"provider":"codex","model":"stage-model","tier":"strong"}]);
        f.setting("mock_verdicts",json!([reject("Fix behavior"),clean()]));
        match kind {
            "transient" => f.setting("mock_implementation_errors",json!(["429 rate limit",null])),
            "retry_exhausted" => f.setting("mock_implementation_errors",json!(["429 rate limit","429 rate limit","429 rate limit",null])),
            "fallback" => f.setting("mock_implementation_errors",json!(["claude exited with exit status: 1: You've reached your Fable limit.",null])),
            "weak_pin" => f.setting("implementer_model",json!("weak-model")),
            "substitution" => f.setting("mock_effective_models",json!(["unexpected-model"])),
            _ => {}
        }
        let p = f.run();
        let approved = matches!(kind,"transient"|"fallback");
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),approved,"{kind}: {}",p["plan_review"]["gate"]);
        assert_eq!(p["stages"],stages,"fixer must not borrow stage bookkeeping");
        let calls = p["plan_review"]["model_invocations"].as_array().unwrap();
        assert_eq!(calls.len(),match kind {"weak_pin"|"other_provider" => 0,"retry_exhausted" => 3,"substitution" => 1,_ => 2},"{kind}");
        assert!(calls.iter().all(|c| c["requested"]["provider"] == "claude"));
        if kind == "transient" {
            assert_eq!(calls[0]["failure_kind"],"transient");
            assert_eq!(calls[0]["requested"],calls[1]["requested"]);
            assert_eq!(p["plan_review"]["retries"]["operational_retries"],1);
        }
        if kind == "fallback" {
            assert_ne!(calls[0]["requested"]["model"],calls[1]["requested"]["model"]);
            assert_eq!(calls[1]["verification_state"],"execution_verified");
        }
        if kind == "substitution" { assert_eq!(calls[0]["verification_state"],"unexpected_substitution"); }
        assert!(f.plan_calls().iter().filter(|c| c["role"] == "reviewer").all(|c| c["provider"] == "codex"));
        assert_eq!(p["plan_review"]["rounds"],2);
    }
}

#[test]
fn plan_review_recovers_staging_crash_but_rejects_new_content_or_index() {
    for change in ["none","content","index"] {
        let f = Fixture::new("Implement feature",1);
        f.two_deferred_stages();
        f.setting("mock_verdicts",json!([reject("Fix behavior"),clean()]));
        f.setting("test_plan_staging_crash",json!(true));
        let p = f.run();
        assert_eq!(p["plan_review"]["gate"]["status"],"approved");
        assert_eq!(p["plan_review"]["commit_state"],"staging");
        assert_eq!(f.ctx.git(&["write-tree"]).unwrap(),p["plan_review"]["gate"]["identity"]["snapshot"]["tree"]);
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),"3");
        if change == "content" { fs::write(f.root.join("first.rs"),"external change").unwrap(); }
        if change == "index" { f.ctx.git(&["rm","--cached","first.rs"]).unwrap(); }
        f.setting("test_plan_staging_crash",json!(false));
        let resumed = f.run();
        assert_eq!(f.ctx.forge_path("reports.jsonl").exists(),change == "none","{change}: {}",resumed["plan_review"]["gate"]);
        assert_eq!(f.ctx.git(&["rev-list","--count","HEAD"]).unwrap(),if change == "none" {"4"} else {"3"});
        assert_eq!(f.plan_calls().len(),4);
    }
}

#[test]
fn configured_reviewer_runs_fresh_codex_reviews_after_codex_for_both_cadences() {
    for cadence in ["per_stage", "per_plan"] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("implementer", json!("codex"));
        f.setting("reviewer", json!("codex"));
        f.setting("reviewer_provider_mode", json!("configured"));
        f.setting("test_fake_providers", json!(true));
        f.setting("review_cadence", json!({"architect":cadence,"reviewer":cadence}));
        f.ctx.app.settings.lock().unwrap()["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"stage-model","tier":"strong"},
            {"provider":"claude","model":"other-model","tier":"strong"}
        ]);
        let p = f.run();
        assert_eq!(p["status"], "done", "{cadence}: {p}");
        assert_eq!(p["stages"][0]["implementer_provider"], "codex");
        let settings = f.ctx.app.settings.lock().unwrap();
        let reviewers: Vec<_> = settings["test_review_sessions"].as_array().unwrap().iter()
            .filter(|r| r["role"] == "reviewer").collect();
        assert!(!reviewers.is_empty());
        assert!(reviewers.iter().all(|r| r["provider"] == "codex" && r["session"].is_null()));
    }
}
