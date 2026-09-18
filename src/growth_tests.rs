//! Regression: routing/reassessment bookkeeping must not grow back into the
//! saved plan or the architect prompt as a multi-stage plan completes.
use crate::app::{App, Ctx};
use crate::test_support::api_request;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

const STAGES: i64 = 6;

struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-growth-test-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        settings["planner"] = json!("mock");
        settings["architect"] = json!("mock");
        settings["test_fake_providers"] = json!(true);
        settings["auto_push"] = json!(false);
        settings["model_catalogue"]["entries"] = json!([
            {"provider":"codex","model":"small","tier":"standard","relative_cost_preference":1},
            {"provider":"codex","model":"large","tier":"strong","relative_cost_preference":5},
            {"provider":"claude","model":"other","tier":"strong","relative_cost_preference":5},
        ]);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        for args in [vec!["init", "-q"], vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"], vec!["config", "commit.gpgsign", "false"]] {
            ctx.git(&args).unwrap();
        }
        fs::write(root.join("README.md"), "The service responds with a greeting.\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }

    /// A placeholder stage: non-documentation intent (so review always
    /// requires both roles, keeping the architect the deterministic last
    /// call of each round), independent of its siblings (no dependency
    /// cascade), with real design text supplied later via `finalize`.
    fn placeholder_stage(id: i64) -> Value {
        json!({"id":id,"title":format!("Implement handler {id}"),
            "instructions":format!("Placeholder for handler {id}."),
            "acceptance":"Placeholder acceptance.","commit":format!("feat: handler {id}"),
            "depends_on":[],"status":"pending","rounds":0})
    }

    fn finalize(id: i64) -> (String, String) {
        (format!("Implement handler {id} for the greeting service: cover the primary success path and one edge case, matching the style of the other handlers."),
         format!("Handler {id} responds correctly.\nExisting checks still pass."))
    }

    fn plan_json_len(&self) -> usize {
        fs::read(self.ctx.forge_path("plan.json")).unwrap().len()
    }

    fn last_architect_prompt_len(&self) -> usize {
        let settings = self.ctx.app.settings.lock().unwrap();
        settings["mock_architect_requests"].as_array().and_then(|a| a.last())
            .and_then(|r| r["prompt"].as_str()).map_or(0, str::len)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); }
}

#[test]
fn saved_plan_and_architect_context_stay_flat_across_a_six_stage_plan() {
    let f = Fixture::new();
    let stages: Vec<Value> = (1..=STAGES).map(Fixture::placeholder_stage).collect();
    f.ctx.architect_publish(json!({"goal":"Ship the greeting service","status":"ready","stages":stages}), None, "draft").unwrap();

    let mut plan_bytes = Vec::new();
    let mut architect_prompt_lens = Vec::new();

    for id in 1..=STAGES {
        // Finalize this stage's real design just before it runs. Every earlier
        // stage is already committed and carries its own reviews, invocations
        // and model agreement, so this is the point where the pre-refactor
        // engine would have re-embedded all of that bookkeeping into the next
        // architect turn's plan clone.
        let current = f.ctx.load_plan().unwrap();
        let idx = current["stages"].as_array().unwrap().iter().position(|s| s["id"] == id).unwrap();
        let (instructions, acceptance) = Fixture::finalize(id);
        let mut candidate = current.clone();
        candidate["stages"][idx]["instructions"] = json!(instructions);
        candidate["stages"][idx]["acceptance"] = json!(acceptance);

        // Stage input-change detection still invalidates an edited stage: the
        // stored fingerprint no longer matches freshly computed material.
        assert!(!f.ctx.proposal_inputs_current(&candidate, idx), "stage {id} edit must invalidate its stored proposal fingerprint");

        let mut edited = crate::plan::edit_plan(&current, &json!({"plan": candidate})).unwrap();
        edited["status"] = json!("ready");
        f.ctx.save_plan(&edited).unwrap();

        if id < STAGES {
            // Settle exactly this stage so the sample below reflects exactly
            // one committed stage boundary, without running the rest of the
            // plan (and its own further architect turns) first.
            f.ctx.run_single_stage_for_test(idx).unwrap();
        } else {
            // The last stage runs through the normal worker, which also
            // completes plan-level review and publishes the run report.
            f.ctx.run_worker();
        }

        let after = f.ctx.load_plan().unwrap();
        let stage = after["stages"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap();
        assert_eq!(stage["status"], "committed", "stage {id} did not commit: {}", f.ctx.read_history());
        assert_eq!(stage["instructions"], json!(instructions));
        assert_eq!(stage["acceptance"], json!(acceptance));

        // Architect guidance is still validated against (and recorded against)
        // the current stage inputs.
        let cp = f.ctx.architecture_store().checkpoint(&after).unwrap();
        assert_eq!(cp["guidance"][id.to_string()]["relevant_inputs"], crate::plan::stage_inputs(&after, idx));

        plan_bytes.push(f.plan_json_len());
        architect_prompt_lens.push(f.last_architect_prompt_len());
    }

    assert_eq!(plan_bytes.len(), STAGES as usize);
    assert_eq!(architect_prompt_lens.len(), STAGES as usize);

    // Each additional committed stage necessarily adds real content (its own
    // design, sha, review pointer, one more compact architect entry), so the
    // saved plan and the architect prompt both grow roughly linearly with the
    // stage count — that is the "roughly flat" this stage protects: a
    // constant marginal cost per stage, never one that grows with how many
    // stages already exist (the original quadratic bug: a stage's stored
    // `model_proposal_inputs`/context re-embedded every earlier stage's own
    // bookkeeping, so later stages cost dramatically more than earlier ones).
    // Measured on this fixture: plan.json costs ~13.8 KB/stage and the
    // architect prompt ~2 KB/stage, both essentially constant across all 5
    // transitions. The bounds below assert that constancy with headroom,
    // rather than a fixed total-size ratio that would depend on how much
    // upfront content (here, all 6 stages' initial proposals) a given run
    // happens to start with.
    fn assert_flat_growth(label: &str, samples: &[usize], per_transition_headroom: usize, total_headroom: usize) {
        let deltas: Vec<i64> = samples.windows(2).map(|w| w[1] as i64 - w[0] as i64).collect();
        let (min_delta, max_delta) = (*deltas.iter().min().unwrap(), *deltas.iter().max().unwrap());
        assert!(max_delta <= min_delta * 2 + per_transition_headroom as i64,
            "{label}: a later stage cost {max_delta} bytes to commit vs {min_delta} for an earlier one \
             (samples {samples:?}, deltas {deltas:?}); a stage's marginal cost must stay roughly constant, \
             not grow with how many stages already exist");
        let bound = samples[0] + (samples.len() - 1) * max_delta.max(0) as usize * 2 + total_headroom;
        let last = *samples.last().unwrap();
        assert!(last <= bound,
            "{label}: grew from {} to {last} bytes across {} stages, beyond the {bound}-byte linear-growth budget \
             (samples {samples:?}); routing/reassessment bookkeeping may have leaked back into per-stage content",
            samples[0], samples.len());
    }
    assert_flat_growth("plan.json size", &plan_bytes, 2048, 4096);
    assert_flat_growth("architect prompt length", &architect_prompt_lens, 512, 1024);

    // The completed plan still carries the full design and only a compact
    // proposal-input fingerprint per stage.
    let done = f.ctx.load_plan().unwrap();
    assert_eq!(done["status"], "done", "{}", f.ctx.read_history());
    for id in 1..=STAGES {
        let stage = done["stages"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap();
        let (instructions, acceptance) = Fixture::finalize(id);
        assert_eq!(stage["instructions"], json!(instructions));
        assert_eq!(stage["acceptance"], json!(acceptance));
        let inputs = &stage["model_proposal_inputs"];
        assert!(inputs.get("stage").is_none(), "stage {id} still stores an expanded model_proposal_inputs.stage copy: {inputs}");
        assert!(inputs["digest"].is_string() && inputs["bytes"].is_u64(), "stage {id} model_proposal_inputs is not a fingerprint: {inputs}");
        assert!(inputs.to_string().len() < 512);
    }

    // Reassessment history stays within the keep limit with an accurate total,
    // even for a stage whose history was seeded well past that limit.
    let mut seeded = done.clone();
    let seed_idx = 0;
    {
        let state = &mut seeded["stages"][seed_idx]["reassessment"];
        assert!(state.is_object(), "committed stage must already carry an initialized reassessment state");
        for n in 0..12u64 {
            crate::app::reassessment::push_history(state, json!({"kind":"operational_retry","role":"implementer",
                "failure_kind":"transient","error":format!("synthetic failure {n}"),"retry":n}));
        }
    }
    f.ctx.save_plan(&seeded).unwrap();
    let bounded = f.ctx.load_plan().unwrap();
    let state = &bounded["stages"][seed_idx]["reassessment"];
    assert_eq!(state["history"].as_array().unwrap().len(), crate::app::reassessment::HISTORY_KEEP);
    assert_eq!(state["history_count"], 12);

    // Review evidence for a committed stage is still retrievable through the
    // reviews endpoint.
    let plan_id = bounded["plan_id"].as_str().unwrap();
    let (code, reviews) = api_request(&f.ctx.app, "GET",
        &format!("/api/architecture/reviews?plan_id={plan_id}&stage_id=1&limit=10"), json!({}));
    assert_eq!(code, 200);
    assert_eq!(reviews["plan_id"], json!(plan_id));
    assert!(!reviews["items"].as_array().unwrap().is_empty());

    // The run report is still produced with its stage outcomes and
    // reassessment counts for every stage.
    let report = f.ctx.read_reports()[0].clone();
    assert_eq!(report["plan_id"], json!(plan_id));
    let outcomes = report["stage_outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), STAGES as usize);
    for outcome in outcomes {
        assert!(outcome["reassessment"]["count"].is_u64());
        assert_eq!(outcome["status"], "committed");
    }
}
