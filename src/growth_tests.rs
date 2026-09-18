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

    /// A stage with a realistically sized design (about 6 KB, like the
    /// stages of a real plan): non-documentation intent (so review always
    /// requires both roles, keeping the architect the deterministic last
    /// call of each round) and independent of its siblings (no dependency
    /// cascade when one stage is edited).
    fn stage(id: i64) -> Value {
        let (instructions, acceptance) = Self::design(id);
        json!({"id":id,"title":format!("Implement handler {id}"),"instructions":instructions,
            "acceptance":acceptance,"commit":format!("feat: handler {id}"),
            "depends_on":[],"status":"pending","rounds":0})
    }

    fn design(id: i64) -> (String, String) {
        let step = |n: i64| format!("Step {n}: extend handler {id} of the greeting service so it validates its input, \
            maps each failure to the shared error type, logs the request id at debug level, and keeps the response \
            shape identical to the other handlers; add a unit test for the success path and for the failure path.\n");
        ((1..=24).map(step).collect(),
         format!("Handler {id} responds correctly on the success path and on every documented failure path.\n\
            Its unit tests cover each step of the instructions.\nExisting checks still pass."))
    }

    /// The small edit made to stage `id` just before it runs; the final
    /// instructions are the published design plus this line.
    fn edited_instructions(id: i64) -> String {
        format!("{}Also cover the empty-name edge case for handler {id}.\n", Self::design(id).0)
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
    let stages: Vec<Value> = (1..=STAGES).map(Fixture::stage).collect();
    f.ctx.architect_publish(json!({"goal":"Ship the greeting service","status":"ready","stages":stages}), None, "draft").unwrap();

    let mut plan_bytes = Vec::new();
    let mut architect_prompt_lens = Vec::new();

    for id in 1..=STAGES {
        // Edit this stage just before it runs. Every earlier stage is already
        // committed and carries its own reviews, invocations and model
        // agreement, so this is the point where the pre-refactor engine would
        // have re-embedded all of that bookkeeping into the next architect
        // turn's plan clone.
        let current = f.ctx.load_plan().unwrap();
        let idx = current["stages"].as_array().unwrap().iter().position(|s| s["id"] == id).unwrap();
        let (instructions, acceptance) = (Fixture::edited_instructions(id), Fixture::design(id).1);
        let mut candidate = current.clone();
        candidate["stages"][idx]["instructions"] = json!(instructions);

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

    // Fixed bounds, independent of the samples they check. Measured on this
    // fixture: plan.json goes from ~125 KB after the first stage to ~199 KB
    // after the sixth (~14.8 KB per committed stage: that stage's own reviews,
    // verdict, gate, agreement and invocations), and the architect prompt from
    // ~28.8 KB to ~39.3 KB (~2.1 KB per committed stage: one compact stage
    // entry with its outcome summary plus the checkpoint's outcome record).
    //
    // Saved plan: the final size must stay within about twice the size after
    // the first stage. The quadratic bug (a stage storing copies of its
    // dependencies' inputs and its own earlier records) grew the file far
    // faster than that. The per-stage cap names the stage that broke it.
    const PLAN_MAX_GROWTH_FACTOR: usize = 2;
    const PLAN_MAX_BYTES_PER_STAGE: usize = 20 * 1024;
    // Architect prompt: each completed stage may add at most this many bytes.
    // Re-embedding a committed stage's bookkeeping (its reviews alone are
    // ~3.5 KB here) or the plan document in the work-status context exceeds it.
    const ARCHITECT_PROMPT_MAX_BYTES_PER_STAGE: usize = 3 * 1024;
    let transitions = |samples: &[usize]| samples.windows(2).map(|w| w[1] as i64 - w[0] as i64).collect::<Vec<_>>();
    let (first_plan, last_plan) = (plan_bytes[0], *plan_bytes.last().unwrap());
    assert!(last_plan <= first_plan * PLAN_MAX_GROWTH_FACTOR,
        "plan.json grew from {first_plan} to {last_plan} bytes over {STAGES} stages, more than {PLAN_MAX_GROWTH_FACTOR}x \
         (samples {plan_bytes:?}); routing/reassessment bookkeeping may be copied into stages again");
    for (n, delta) in transitions(&plan_bytes).into_iter().enumerate() {
        assert!(delta <= PLAN_MAX_BYTES_PER_STAGE as i64,
            "committing stage {} added {delta} bytes to plan.json, over the {PLAN_MAX_BYTES_PER_STAGE}-byte per-stage cap \
             (samples {plan_bytes:?})", n + 2);
    }
    for (n, delta) in transitions(&architect_prompt_lens).into_iter().enumerate() {
        assert!(delta <= ARCHITECT_PROMPT_MAX_BYTES_PER_STAGE as i64,
            "completing stage {} grew the architect prompt by {delta} bytes, over the {ARCHITECT_PROMPT_MAX_BYTES_PER_STAGE}-byte \
             per-stage cap (samples {architect_prompt_lens:?}); the work-status context may be carrying plan bookkeeping again", n + 2);
    }

    // The completed plan still carries the full design and only a compact
    // proposal-input fingerprint per stage.
    let done = f.ctx.load_plan().unwrap();
    assert_eq!(done["status"], "done", "{}", f.ctx.read_history());
    for id in 1..=STAGES {
        let stage = done["stages"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap();
        let (instructions, acceptance) = (Fixture::edited_instructions(id), Fixture::design(id).1);
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
