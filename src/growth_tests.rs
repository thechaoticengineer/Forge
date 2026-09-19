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
    fn new() -> Self { Self::with_cadence("per_stage", "per_stage") }

    fn with_cadence(architect: &str, reviewer: &str) -> Self {
        let root = std::env::temp_dir().join(format!("forge-growth-test-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":architect,"reviewer":reviewer});
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

// ---------------------------------------------------------------------------
// Prompt-size guards for every role, across an eight-stage plan with
// dependencies. Every provider prompt is recorded by `Ctx::record_prompt`
// (src/prompt_capture.rs); these tests only read that log.
// ---------------------------------------------------------------------------

const LINKED_STAGES: i64 = 8;

/// The dependencies of the eight-stage fixture: chains 1<-2<-3<-4 and 5<-6<-7,
/// and a join on stage 8, so `relevant_inputs` would grow transitively.
fn depends_on(id: i64) -> Vec<i64> {
    match id { 2 => vec![1], 3 => vec![2], 4 => vec![3], 6 => vec![5], 7 => vec![6], 8 => vec![4, 7], _ => vec![] }
}

// Every recorded prompt of every role stays under the codex input limit of
// 1,048,576 characters: a string never has fewer bytes than characters, so a
// byte cap of 256 KiB guarantees it with a wide margin. Measured on the
// fixtures below, the largest prompts (the first architect publish, planner
// chat and routing reconciliation) are about 64-68 KB.
const PROMPT_MAX_BYTES: usize = 256 * 1024;
// Roles that repeat across stages (architect publish, stage review,
// implementer, fixer, planner chat, routing reconciliation) may grow by at
// most this many bytes per committed stage. Measured on the eight-stage
// fixtures: about 0.6 KB per committed stage for architect publish and the
// implementer, 0.5 KB for planner chat (one compact stage entry and outcome
// record, plus one Q&A pair in the chat history), 0.6 KB for the fixer and
// about 1 KB for routing reconciliation (64 KB at draft, 71 KB after seven
// committed stages), with stage and plan review flat. The default of 4 KiB leaves headroom for
// legitimate additions without letting per-stage bookkeeping (a stage's
// reviews alone are about 3.5 KB) or copied records back into the prompts.
const PROMPT_MAX_BYTES_PER_COMMITTED_STAGE: usize = 4 * 1024;
// Plan review and PLAN_FIX run once per plan and carry every stage's design
// (about 6 KB each here): a fixed base plus an allowance per stage.
const ONCE_PER_PLAN_BASE_BYTES: usize = 32 * 1024;
const ONCE_PER_PLAN_BYTES_PER_STAGE: usize = 16 * 1024;

/// One recorded prompt with the number of stages committed when it was sent.
struct Sample { kind: String, role: String, stage_id: Option<i64>, committed: i64, bytes: usize }

/// Drives a linked plan one stage at a time so every prompt is tagged with the
/// number of committed stages, and asks the planner chat a question after each
/// stage.
struct Run { f: Fixture, samples: Vec<Sample>, seen: usize }

impl Run {
    fn new(f: Fixture, verdicts: Vec<Value>, architect_verdicts: Vec<Value>) -> Self {
        {
            let mut settings = f.ctx.app.settings.lock().unwrap();
            settings["mock_verdicts"] = json!(verdicts);
            if !architect_verdicts.is_empty() { settings["mock_architect_verdicts"] = json!(architect_verdicts); }
            // The architect disagrees with the planner on stage 3's
            // classification, so the reconciliation exchange runs; every other
            // evaluation agrees.
            let evaluation = |id: i64, agree: bool| json!({"stage_id":id,"agree":agree,"risk":"standard","complexity":"standard",
                "task":"functionality","rationale":"Architect checked downstream interface compatibility and failure impact independently."});
            settings["mock_model_evaluations"] = json!([(1..=LINKED_STAGES).map(|id| evaluation(id, id != 3)).collect::<Vec<_>>()]);
        }
        let stages: Vec<Value> = (1..=LINKED_STAGES).map(|id| {
            let mut stage = Fixture::stage(id);
            stage["depends_on"] = json!(depends_on(id));
            stage
        }).collect();
        f.ctx.architect_publish(json!({"goal":"Ship the greeting service","status":"ready","stages":stages}), None, "draft").unwrap();
        let mut run = Self { f, samples: vec![], seen: 0 };
        run.collect(0);
        run
    }

    fn collect(&mut self, committed: i64) {
        let log = self.f.ctx.all_recorded_prompts();
        for record in &log[self.seen..] {
            self.samples.push(Sample { kind: record["kind"].as_str().unwrap().into(), role: record["role"].as_str().unwrap().into(),
                stage_id: record["stage_id"].as_i64(), committed, bytes: record["bytes"].as_u64().unwrap() as usize });
        }
        self.seen = log.len();
    }

    fn ask(&mut self, question: &str) {
        let committed = self.f.ctx.load_plan().unwrap()["stages"].as_array().unwrap().iter().filter(|s| s["status"] == "committed").count() as i64;
        let plan = self.f.ctx.load_plan().unwrap();
        self.f.ctx.chat_worker(&plan, question);
        self.collect(committed);
    }

    /// Edits the last stage, which nothing depends on, so its input change
    /// makes the architect's next turn evaluate its routing again; the architect
    /// disagrees once, which runs a second reconciliation late in the plan.
    fn edit_last_stage_with_routing_disagreement(&mut self) {
        let plan = self.f.ctx.load_plan().unwrap();
        let idx = plan["stages"].as_array().unwrap().iter().position(|s| s["id"] == LINKED_STAGES).unwrap();
        let mut candidate = plan.clone();
        candidate["stages"][idx]["instructions"] = json!(Fixture::edited_instructions(LINKED_STAGES));
        let mut edited = crate::plan::edit_plan(&plan, &json!({"plan": candidate})).unwrap();
        edited["status"] = json!("ready");
        self.f.ctx.save_plan(&edited).unwrap();
        self.f.ctx.app.settings.lock().unwrap()["mock_model_evaluations"] = json!([[
            {"stage_id":LINKED_STAGES,"agree":false,"risk":"standard","complexity":"standard","task":"functionality",
             "rationale":"Architect checked downstream interface compatibility and failure impact independently."}]]);
    }

    /// Runs stage `id` (the last one through the worker, which also runs plan
    /// review), then asks a question.
    fn stage(&mut self, id: i64) {
        let plan = self.f.ctx.load_plan().unwrap();
        let idx = plan["stages"].as_array().unwrap().iter().position(|s| s["id"] == id).unwrap();
        let committed = plan["stages"].as_array().unwrap().iter().filter(|s| s["status"] == "committed").count() as i64;
        if id < LINKED_STAGES { self.f.ctx.run_single_stage_for_test(idx).unwrap(); } else { self.f.ctx.run_worker(); }
        let after = self.f.ctx.load_plan().unwrap();
        assert_eq!(after["stages"][idx]["status"], "committed", "stage {id} did not commit: {}", self.f.ctx.read_history());
        self.collect(committed);
        self.ask(&format!("What does stage {id} change?"));
    }

    fn of(&self, kind: &str) -> Vec<&Sample> { self.samples.iter().filter(|s| s.kind == kind).collect() }

    fn assert_captured(&self, kind: &str, role: Option<&str>) {
        assert!(self.samples.iter().any(|s| s.kind == kind && role.is_none_or(|r| s.role == r)),
            "no {kind} prompt{} was captured; samples {:?}", role.map(|r| format!(" for role {r}")).unwrap_or_default(),
            self.samples.iter().map(|s| (&s.kind[..], &s.role[..], s.committed)).collect::<Vec<_>>());
    }

    /// Every prompt is under the absolute cap, whatever its role.
    fn assert_absolute_cap(&self) {
        for s in &self.samples {
            assert!(s.bytes <= PROMPT_MAX_BYTES, "{} prompt of role {} (stage {:?}, {} committed) is {} bytes, over the {PROMPT_MAX_BYTES}-byte cap",
                s.kind, s.role, s.stage_id, s.committed, s.bytes);
        }
    }

    /// Like-for-like growth: the first prompt of a role/kind at each committed
    /// count, compared across counts against the fixed per-stage limit.
    fn assert_growth(&self, kind: &str, role: Option<&str>) {
        let mut series: Vec<(i64, usize, Option<i64>)> = vec![];
        for s in self.of(kind).into_iter().filter(|s| role.is_none_or(|r| s.role == r)) {
            if series.last().is_none_or(|last| last.0 != s.committed) { series.push((s.committed, s.bytes, s.stage_id)); }
        }
        for pair in series.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            let allowed = PROMPT_MAX_BYTES_PER_COMMITTED_STAGE as i64 * (after.0 - before.0);
            assert!(after.1 as i64 - before.1 as i64 <= allowed,
                "{kind} prompt{} grew from {} to {} bytes between {} and {} committed stages (stage {:?}), over {PROMPT_MAX_BYTES_PER_COMMITTED_STAGE} bytes per stage (samples {series:?})",
                role.map(|r| format!(" of role {r}")).unwrap_or_default(), before.1, after.1, before.0, after.0, after.2);
        }
    }

    fn assert_once_per_plan_bound(&self, kind: &str) {
        let bound = ONCE_PER_PLAN_BASE_BYTES + ONCE_PER_PLAN_BYTES_PER_STAGE * LINKED_STAGES as usize;
        for s in self.of(kind) {
            assert!(s.bytes <= bound, "{kind} prompt of role {} is {} bytes, over the {bound}-byte bound ({ONCE_PER_PLAN_BASE_BYTES} + \
                {ONCE_PER_PLAN_BYTES_PER_STAGE} per stage × {LINKED_STAGES} stages)", s.role, s.bytes);
        }
    }
}

#[test]
fn every_role_prompt_stays_bounded_across_an_eight_stage_plan() {
    let approve = || json!({"approved":true,"issues":[]});
    // Even stages need a fix round; the reviewer's architecture_context_gap
    // also sends the architect a follow-up turn, so architect publish, fixer
    // and both review roles repeat across the plan.
    let mut verdicts = vec![];
    for id in 1..=LINKED_STAGES {
        if id % 2 == 0 {
            verdicts.push(json!({"approved":false,"issues":[format!("Clarify handler {id} error mapping")],
                "architecture_context_gap":format!("Which error type must handler {id} keep stable?")}));
        }
        verdicts.push(approve());
    }
    let mut run = Run::new(Fixture::new(), verdicts, vec![]);
    run.ask("What is planned?");
    for id in 1..=LINKED_STAGES {
        if id == LINKED_STAGES { run.edit_last_stage_with_routing_disagreement(); }
        run.stage(id);
    }

    // Coverage: every role reached its provider at least once.
    for kind in ["architect_publish", "implementer", "fixer", "planner_chat", "routing_reconciliation"] { run.assert_captured(kind, None); }
    run.assert_captured("stage_review", Some("reviewer"));
    run.assert_captured("stage_review", Some("architect"));
    assert_eq!(run.of("planner_chat").len(), LINKED_STAGES as usize + 1);

    run.assert_absolute_cap();
    assert_eq!(run.of("routing_reconciliation").len(), 2, "one reconciliation at draft and one after the late edit");
    for kind in ["architect_publish", "implementer", "fixer", "planner_chat", "routing_reconciliation"] { run.assert_growth(kind, None); }
    run.assert_growth("stage_review", Some("reviewer"));
    run.assert_growth("stage_review", Some("architect"));
}

#[test]
fn plan_review_and_plan_fix_prompts_stay_bounded_across_an_eight_stage_plan() {
    let approve = || json!({"approved":true,"issues":[]});
    // Stage reviews are deferred to the plan review, which requests changes
    // once and is then approved after PLAN_FIX.
    let mut verdicts = vec![json!({"approved":false,"issues":["Handler 4 must document its error mapping"]})];
    verdicts.extend((0..4).map(|_| approve()));
    let mut run = Run::new(Fixture::with_cadence("per_plan", "per_plan"), verdicts, vec![]);
    for id in 1..=LINKED_STAGES { run.stage(id); }

    for kind in ["architect_publish", "implementer", "planner_chat", "plan_review", "plan_fix"] { run.assert_captured(kind, None); }
    run.assert_captured("plan_review", Some("reviewer"));
    run.assert_captured("plan_review", Some("architect"));
    assert!(run.of("plan_review").len() >= 2, "plan review must run again after PLAN_FIX; samples {:?}",
        run.samples.iter().map(|s| (&s.kind[..], &s.role[..], s.committed)).collect::<Vec<_>>());

    run.assert_absolute_cap();
    for kind in ["architect_publish", "implementer", "planner_chat"] { run.assert_growth(kind, None); }
    run.assert_once_per_plan_bound("plan_review");
    run.assert_once_per_plan_bound("plan_fix");
}
