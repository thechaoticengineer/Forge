use super::*;

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
fn available_cargo_checks_cannot_be_omitted_even_for_documentation() {
    let f = Fixture::new("Fix prose spelling", 0);
    fs::write(
        f.root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    f.ctx.git(&["add", "Cargo.toml"]).unwrap();
    f.ctx.git(&["commit", "-qm", "manifest"]).unwrap();
    f.docs();
    let p = f.run();
    assert_eq!(p["stages"][0]["review_gate"]["status"], "error");
    assert_eq!(f.count("architect"), 0);
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
        }
        let r = AgentRequest {
            role: "architect_review",
            session: Some("11111111-2222-4333-8444-555555555555"),
            ..r
        };
        assert!(
            crate::agent::command(&r)
                .unwrap()
                .get_args()
                .any(|s| s == "11111111-2222-4333-8444-555555555555")
        );
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
