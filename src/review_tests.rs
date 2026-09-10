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
        let root = std::env::temp_dir().join(format!(
            "forge-gate-test-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
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
            .map_or(0, Vec::len)
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
        f.setting("mock_verdicts", json!([bad]));
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
    f.setting("mock_architect_verdicts", json!(["malformed", clean()]));
    let first = f.run();
    f.assert_no_commit();
    assert_eq!(first["stages"][0]["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(first["stages"][0]["review_gate"]["status"], "error");
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("reviewer"), 2);
    assert_eq!(f.count("architect"), 2);
    assert_eq!(
        p["stages"][0]["attempt_id"],
        first["stages"][0]["attempt_id"]
    );
    let f = Fixture::new("Implement feature", 0);
    f.setting("mock_architect_verdicts", json!(["malformed"]));
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
    f.setting("mock_verdicts",json!([{"approved":true,"issues":[],"criteria":[{"criterion":"First {verdict_path}","status":"passed","evidence":"One check"}]}]));
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
        f.setting("review_cadence", crate::plan::default_settings()["review_cadence"].clone());
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
fn deferred_obligations_block_completion_reports_and_push_even_after_restart() {
    for reviewer in ["per_stage", "per_plan"] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("review_cadence", json!({"architect":"per_plan","reviewer":reviewer}));
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
