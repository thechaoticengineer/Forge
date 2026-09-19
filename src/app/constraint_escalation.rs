//! Stage-level constraint-conflict escalation to the planner.
//!
//! One entry point serves every trigger: an implementer or fixer outcome of
//! kind `constraint_conflict`, a reviewer or architect verdict field, and the
//! engine fallback when a stage review exhausts its budget. Each escalation is
//! a record on the stage that moves through one persisted sequence: pending
//! (saved before the planner is asked), answered (the validated answer saved
//! before any plan change), then settled in the same save or publication that
//! applies its correction. Recovery resumes an answered record and never asks
//! the planner again; a record left unanswered by a crash settles as failed.
use super::Ctx;
use crate::constraint_conflict::{self as conflict, Decision, RECORDS};
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::process::Command;

#[derive(Debug, PartialEq)]
pub(super) enum ConflictResolution {
    /// The corrected stage restarts as a fresh attempt with replenished rounds.
    Restart,
    /// The correction returned the plan to the user for approval.
    AwaitingApproval,
    /// The planner explained how to meet the stage; the next round gets it.
    Clarified,
    /// The stage blocks with the escalation record attached to its gate.
    Blocked,
    /// A user stop interrupted the planner pass.
    Stopped,
}

/// Stage outcome of an escalation that blocked. The stage blocks like an
/// exhausted review, but without a second (fallback) planner pass.
pub(super) const CONFLICT_BLOCKED: &str = "conflict_blocked";

/// Who escalated, and the conflict statements the signature is built from.
pub(super) struct Trigger<'a> {
    pub source: &'a str,
    pub kind: &'a str,
    pub reason: String,
    pub statements: Vec<String>,
}

fn stage_index(plan: &Value, sid: &Value) -> Result<usize, String> {
    plan["stages"].as_array().ok_or("invalid stages")?.iter()
        .position(|s| s["id"] == *sid).ok_or_else(|| "stage disappeared during constraint escalation".into())
}

/// Rounds remain while the reserved round count has not passed the budget.
fn rounds_left(stage: &Value) -> bool {
    stage["rounds"].as_u64().unwrap_or(0) <= stage["review_budget"].as_u64().unwrap_or(0)
}

/// The full edit body for `edit_plan`: every stage as saved, with the
/// planner's corrections applied and inserted stages placed before `idx`.
fn correction_body(plan: &Value, idx: usize, decision: &Decision) -> Value {
    let mut stages = vec![];
    for (i, s) in plan["stages"].as_array().into_iter().flatten().enumerate() {
        if i == idx && let Decision::Revise { insert_before, .. } = decision {
            for new in insert_before {
                stages.push(json!({"title":new.title,"instructions":new.instructions,
                    "acceptance":new.acceptance,"commit":new.commit}));
            }
        }
        let mut edited = json!({"id": s["id"], "title": s["title"],
            "instructions": s["instructions"], "acceptance": s["acceptance"],
            "commit": s["commit"].as_str().unwrap_or("")});
        if let Some(depends) = s.get("depends_on") { edited["depends_on"] = depends.clone(); }
        if !s["model_constraint"].is_null() { edited["model_constraint"] = s["model_constraint"].clone(); }
        match decision {
            Decision::Revise { stages: edits, .. } => {
                if let Some(edit) = edits.iter().find(|e| s["id"] == e.id) {
                    if let Some(title) = &edit.title { edited["title"] = json!(title); }
                    edited["instructions"] = json!(edit.instructions);
                    edited["acceptance"] = json!(edit.acceptance);
                }
            }
            Decision::ConstraintWrong { instructions, acceptance, .. } if i == idx => {
                edited["instructions"] = json!(instructions);
                edited["acceptance"] = json!(acceptance);
            }
            _ => {}
        }
        stages.push(edited);
    }
    json!({"plan": {"goal": plan["goal"], "stages": stages}})
}

/// A short, human-readable account of what a correction changes.
fn correction_summary(decision: &Decision, sid: &Value) -> String {
    match decision {
        Decision::Revise { stages, insert_before, .. } => {
            let mut parts = vec![];
            if !stages.is_empty() {
                let ids: Vec<String> = stages.iter().map(|s| s.id.to_string()).collect();
                parts.push(format!("revised stage(s) {}", ids.join(", ")));
            }
            if !insert_before.is_empty() {
                let titles: Vec<&str> = insert_before.iter().map(|s| s.title.as_str()).collect();
                parts.push(format!("inserted {} stage(s) before stage {sid}: {}", titles.len(), titles.join("; ")));
            }
            parts.join("; ")
        }
        Decision::ConstraintWrong { constraint, .. } =>
            format!("corrected the wrong constraint of stage {sid} ({constraint}); the delivered work is reviewed again"),
        Decision::Refused(how) => format!("kept stage {sid} unchanged: {how}"),
    }
}

impl Ctx {
    /// Hand a constraint conflict to the planner once per stage and signature.
    /// `rounds_left` says whether a clarification can still reach a new round.
    pub(super) fn escalate_stage_conflict(&self, plan: &mut Value, idx: usize, trigger: Trigger<'_>, rounds_left: bool)
        -> Result<ConflictResolution, String>
    {
        let sid = plan["stages"][idx]["id"].clone();
        let requests = conflict::outstanding_requests(&plan["stages"][idx]);
        let signature = conflict::signature(&trigger.statements, &requests);
        let records = plan["stages"][idx][RECORDS].as_array().cloned().unwrap_or_default();
        // The stage keeps its records across attempts and corrections, so this
        // consults the whole lineage: one planner pass per conflict signature.
        if let Some(prior) = records.iter().rposition(|r| r["signature"] == signature) {
            if records[prior]["outcome"] == "pending" && !records[prior]["decision"].is_null() {
                return self.apply_conflict_answer(plan, idx, prior, rounds_left);
            }
            let reason = format!("the same constraint conflict already had its planner pass (outcome {})",
                records[prior]["outcome"].as_str().unwrap_or("unknown"));
            let at = if prior + 1 == records.len() && records[prior]["outcome"] == "blocked" { prior } else {
                let at = self.begin_constraint_escalation(plan, idx, trigger.source, trigger.kind, &trigger.reason, &trigger.statements)?;
                plan["stages"][idx][RECORDS][at]["repeats"] = json!(prior);
                self.save_plan(plan)?;
                self.settle_constraint_escalation(plan, idx, at, "blocked", Some(&reason))?;
                at
            };
            self.attach_conflict_block(plan, idx, at, &reason)?;
            return Ok(ConflictResolution::Blocked);
        }
        let at = self.begin_constraint_escalation(plan, idx, trigger.source, trigger.kind, &trigger.reason, &trigger.statements)?;
        self.log_event("plan", &format!("stage {sid}: {} reported by {}; one planner pass for this conflict",
            if trigger.kind == conflict::EXHAUSTED_KIND { "review budget exhausted" } else { "constraint conflict" }, trigger.source));
        if let Err(error) = self.consult_conflict_planner(plan, idx, at) {
            // A stop leaves the record unanswered; recovery settles it as failed.
            if self.session.stop_requested.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(ConflictResolution::Stopped);
            }
            let reason = format!("the planner pass failed: {error}");
            self.settle_constraint_escalation(plan, idx, at, "failed", Some(&reason))?;
            self.attach_conflict_block(plan, idx, at, &reason)?;
            return Ok(ConflictResolution::Blocked);
        }
        self.apply_conflict_answer(plan, idx, at, rounds_left)
    }

    /// Apply the saved, validated planner answer of record `at`. The record is
    /// settled in the same save or publication that applies the correction, so
    /// a crash either leaves it answered (and recovery applies it) or done.
    pub(super) fn apply_conflict_answer(&self, plan: &mut Value, idx: usize, at: usize, rounds_left: bool)
        -> Result<ConflictResolution, String>
    {
        let sid = plan["stages"][idx]["id"].clone();
        let current = self.load_plan().ok_or("missing plan")?;
        let target = stage_index(&current, &sid)?;
        let record = current["stages"][target][RECORDS][at].clone();
        let decision: Decision = serde_json::from_value(record["correction"].clone())
            .map_err(|e| format!("invalid saved constraint escalation answer: {e}"))?;
        if record["answer_revision"] != current["revision"] {
            *plan = current;
            let reason = "the plan changed after the planner answered; its correction was not applied";
            self.settle_constraint_escalation(plan, target, at, "failed", Some(reason))?;
            self.attach_conflict_block(plan, target, at, reason)?;
            return Ok(ConflictResolution::Blocked);
        }
        let summary = correction_summary(&decision, &sid);
        if let Decision::Refused(how) = &decision {
            let mut next = current.clone();
            let inputs = crate::plan::stage_inputs(&next, target);
            let stage = &mut next["stages"][target];
            stage["scope_clarification"] = json!({"source":conflict::KIND,"source_inputs":inputs,
                "message":how,"reason":record["trigger"]["reason"],"pending":rounds_left,"unix":unix_timestamp()});
            let record = &mut stage[RECORDS][at];
            record["correction_summary"] = json!(summary);
            let (outcome, detail) = if rounds_left {
                ("refused", summary.clone())
            } else {
                ("blocked", format!("{summary}; no review rounds remain"))
            };
            conflict::set_outcome(record, outcome, Some(&detail), Some(&current["revision"]), unix_timestamp())?;
            *plan = self.publish_plan(&next, false)?;
            if !rounds_left {
                self.attach_conflict_block(plan, target, at, "the planner kept the stage as written but no review rounds remain")?;
                return Ok(ConflictResolution::Blocked);
            }
            self.log_event("plan", &format!("stage {sid}: the planner kept the stage as written; its explanation goes to the next round"));
            return Ok(ConflictResolution::Clarified);
        }
        let inserted = match &decision { Decision::Revise { insert_before, .. } => insert_before.len(), _ => 0 };
        let current_only = match &decision {
            Decision::Revise { stages, insert_before, .. } => insert_before.is_empty() && stages.iter().all(|s| sid == s.id),
            _ => true,
        };
        let body = correction_body(&current, target, &decision);
        let mut edited = crate::plan::edit_plan(&current, &body).map_err(str::to_string)?;
        let at_idx = target + inserted;
        let revision = edited["revision"].clone();
        let stage = &mut edited["stages"][at_idx];
        let record = &mut stage[RECORDS][at];
        record["correction_summary"] = json!(summary);
        conflict::set_outcome(record, if current_only { "applied" } else { "awaiting_approval" },
            Some(&summary), Some(&revision), unix_timestamp())?;
        if current_only {
            // Correcting only the stage the user already approved keeps the
            // run's approval, as a scope renegotiation does.
            edited["status"] = current["status"].clone();
        } else {
            // Changed later stages or new stages are a new plan to approve;
            // edit_plan already left it in draft.
            stage["status"] = json!("pending");
            if inserted > 0 {
                stage["suspended_work"] = json!({"status":"held","record":at,"unix":unix_timestamp()});
            }
        }
        if matches!(decision, Decision::ConstraintWrong { .. }) {
            let inputs = crate::plan::stage_inputs(&edited, at_idx);
            edited["stages"][at_idx]["conflict_rereview"] = json!({"record":at,"source_inputs":inputs});
        }
        *plan = self.architect_publish(edited, Some(&current), "constraint conflict correction")?;
        if current_only {
            self.log_event("plan", &format!("stage {sid} corrected by the planner: {summary}; the stage restarts with fresh rounds"));
            Ok(ConflictResolution::Restart)
        } else {
            self.log_event("plan", &format!(
                "stage {sid}: the planner's correction ({summary}) changes the plan beyond this stage and awaits your approval; the stage is not committed and its uncommitted work is kept"));
            Ok(ConflictResolution::AwaitingApproval)
        }
    }

    /// Keep the escalation record on the blocked stage's gate and name why.
    fn attach_conflict_block(&self, plan: &mut Value, idx: usize, at: usize, reason: &str) -> Result<(), String> {
        let sid = plan["stages"][idx]["id"].clone();
        let mut current = self.load_plan().ok_or("missing plan")?;
        let target = stage_index(&current, &sid)?;
        let record = current["stages"][target][RECORDS][at].clone();
        let gate = &mut current["stages"][target]["review_gate"];
        if !gate.is_object() { *gate = json!({"status":"exhausted"}); }
        gate["constraint_escalation"] = record;
        gate["reason"] = json!(reason);
        *plan = self.publish_plan(&current, false)?;
        self.log_event("stage", &format!("stage {sid}: constraint escalation blocked the stage: {reason} — needs a human"));
        Ok(())
    }

    /// The engine fallback for an exhausted stage review: one planner pass,
    /// even when no role reported a conflict. A round whose escalation already
    /// ran (a role's conflict, or this fallback before a restart) blocks.
    pub(super) fn exhaustion_fallback(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let stage = &plan["stages"][idx];
        let this_round = stage[RECORDS].as_array().and_then(|records| records.iter()
            .rposition(|r| r["attempt_id"] == stage["attempt_id"] && r["round"] == stage["rounds"]));
        if let Some(at) = this_round {
            self.attach_conflict_block(plan, idx, at, "this exhausted round already had its constraint escalation")?;
            return Ok("exhausted");
        }
        let sid = stage["id"].clone();
        let trigger = Trigger {
            source: "engine",
            kind: conflict::EXHAUSTED_KIND,
            reason: format!("stage {sid} used all {} fix rounds without a clean review gate",
                stage["review_budget"].as_u64().unwrap_or(0)),
            statements: vec![conflict::exhausted_statement(&sid)],
        };
        Ok(match self.escalate_stage_conflict(plan, idx, trigger, false)? {
            ConflictResolution::Restart => "renegotiated",
            ConflictResolution::AwaitingApproval => "awaiting_approval",
            ConflictResolution::Stopped => "stopped",
            ConflictResolution::Clarified | ConflictResolution::Blocked => "exhausted",
        })
    }

    /// The draft plan carries a planner correction the user has not approved,
    /// from a stage or from the plan review.
    pub(super) fn correction_awaits_approval(plan: &Value) -> bool {
        let awaiting = |container: &Value| container[RECORDS].as_array().into_iter().flatten().any(|r|
            r["outcome"] == "awaiting_approval" && r["plan_revision"] == plan["revision"]);
        plan["status"] == "draft" && (awaiting(&plan["plan_review"])
            || plan["stages"].as_array().into_iter().flatten().any(awaiting))
    }

    /// Resume escalations a restart interrupted: apply an answered record's
    /// correction; settle a record the planner never answered as failed.
    pub(super) fn recover_constraint_escalations(&self, plan: &mut Value) -> Result<(), String> {
        let count = plan["stages"].as_array().ok_or("invalid stages")?.len();
        for idx in 0..count {
            if plan["stages"][idx]["status"] == "committed" { continue; }
            let sid = plan["stages"][idx]["id"].clone();
            let pending: Vec<(usize, bool)> = plan["stages"][idx][RECORDS].as_array().into_iter().flatten()
                .enumerate().filter(|(_, r)| r["outcome"] == "pending")
                .map(|(at, r)| (at, !r["decision"].is_null())).collect();
            for (at, answered) in pending {
                let idx = stage_index(plan, &sid)?;
                if answered {
                    let left = rounds_left(&plan["stages"][idx]);
                    self.log_event("plan", &format!("stage {sid}: resuming the planner's saved constraint-conflict correction"));
                    self.apply_conflict_answer(plan, idx, at, left)?;
                } else {
                    let reason = "interrupted before the planner's answer was saved; the conflict is not handed back again";
                    self.settle_constraint_escalation(plan, idx, at, "failed", Some(reason))?;
                    self.log_event("plan", &format!("stage {sid}: constraint escalation {reason}"));
                }
            }
        }
        // Settling writes the saved plan; continue from exactly that plan.
        *plan = self.load_plan().ok_or("missing plan after constraint escalation recovery")?;
        Ok(())
    }

    /// A constraint_wrong correction reviews the delivered work again under
    /// the corrected text. Returns true once, for the first round after it.
    pub(super) fn take_conflict_rereview(&self, plan: &mut Value, idx: usize) -> Result<bool, String> {
        let marker = &plan["stages"][idx]["conflict_rereview"];
        if marker.is_null() { return Ok(false); }
        let due = marker["source_inputs"] == crate::plan::stage_inputs(plan, idx);
        plan["stages"][idx].as_object_mut().unwrap().remove("conflict_rereview");
        self.save_plan(plan)?;
        if due {
            let sid = plan["stages"][idx]["id"].clone();
            self.log_event("review", &format!("stage {sid}: reviewing the delivered work again under the planner's corrected text"));
        }
        Ok(due)
    }

    /// Before stage `idx` runs: suspend uncommitted work held for a later stage
    /// (an inserted predecessor commits on its own), and restore this stage's
    /// suspended work. Each step is persisted, so a restart resumes it.
    pub(super) fn settle_suspended_work(&self, plan: &mut Value, idx: usize) -> Result<(), String> {
        let count = plan["stages"].as_array().ok_or("invalid stages")?.len();
        for later in idx + 1..count {
            if matches!(plan["stages"][later]["suspended_work"]["status"].as_str(), Some("held" | "suspending")) {
                self.suspend_stage_work(plan, later)?;
            }
        }
        match plan["stages"][idx]["suspended_work"]["status"].as_str() {
            // The work never left the working tree: it already belongs here.
            Some("held") => {
                plan["stages"][idx].as_object_mut().unwrap().remove("suspended_work");
                self.save_plan(plan)?;
            }
            Some("suspended" | "restoring") => self.restore_stage_work(plan, idx)?,
            _ => {}
        }
        Ok(())
    }

    fn worktree_clean(&self) -> Result<bool, String> {
        let snapshot = super::review::review_snapshot(self.project())?;
        Ok(snapshot["tree"] == json!(self.git(&["rev-parse", "HEAD^{tree}"])?))
    }

    fn git_raw(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("git").args(args).current_dir(self.project()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(out.stdout)
    }

    fn suspend_stage_work(&self, plan: &mut Value, idx: usize) -> Result<(), String> {
        let sid = plan["stages"][idx]["id"].clone();
        if plan["stages"][idx]["suspended_work"]["status"] == "held" {
            if self.worktree_clean()? {
                plan["stages"][idx].as_object_mut().unwrap().remove("suspended_work");
                self.save_plan(plan)?;
                return Ok(());
            }
            let snapshot = super::review::review_snapshot(self.project())?;
            let (head, tree) = (snapshot["head"].as_str().unwrap_or("").to_owned(), snapshot["tree"].as_str().unwrap_or("").to_owned());
            let patch = self.git_raw(&["diff", "--binary", "--full-index", "--no-ext-diff", "--no-textconv", &head, &tree])?;
            let name = format!("suspended-work/{}-stage-{sid}-{tree}.patch", plan["plan_id"].as_str().unwrap_or("plan"));
            let path = self.forge_path(&name);
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("suspend stage work: {e}"))?;
            fs::write(&path, &patch).and_then(|_| fs::File::open(&path)?.sync_all())
                .map_err(|e| format!("suspend stage work: {e}"))?;
            let marker = &mut plan["stages"][idx]["suspended_work"];
            marker["status"] = json!("suspending");
            marker["base"] = json!(head);
            marker["tree"] = json!(tree);
            marker["patch"] = json!(format!("{}/{name}", super::FORGE_DIR));
            self.save_plan(plan)?;
        }
        let marker = plan["stages"][idx]["suspended_work"].clone();
        if !self.worktree_clean()? {
            let snapshot = super::review::review_snapshot(self.project())?;
            if snapshot["head"] != marker["base"] || snapshot["tree"] != marker["tree"] {
                return Err(format!("the working tree changed while stage {sid}'s work was being suspended; its patch is kept at {} — needs a human",
                    marker["patch"].as_str().unwrap_or("")));
            }
            self.git(&["reset", "-q", "--hard", "HEAD"])?;
            self.git(&["clean", "-fdq", "--", ".", ":(exclude).forge"])?;
        }
        plan["stages"][idx]["suspended_work"]["status"] = json!("suspended");
        self.save_plan(plan)?;
        self.log_event("git", &format!("stage {sid}: uncommitted work suspended until the stage runs again; saved at {}",
            marker["patch"].as_str().unwrap_or("")));
        Ok(())
    }

    fn restore_stage_work(&self, plan: &mut Value, idx: usize) -> Result<(), String> {
        let sid = plan["stages"][idx]["id"].clone();
        let marker = plan["stages"][idx]["suspended_work"].clone();
        let patch = marker["patch"].as_str().ok_or("suspended stage work has no patch")?.to_owned();
        if marker["status"] == "suspended" {
            plan["stages"][idx]["suspended_work"]["status"] = json!("restoring");
            self.save_plan(plan)?;
        }
        // A dirty tree while restoring means the patch was already applied.
        if self.worktree_clean()? {
            let path = std::path::Path::new(self.project()).join(&patch);
            let path = path.to_str().ok_or("invalid suspended work path")?;
            if self.git(&["apply", "--binary", path]).is_err() {
                self.git(&["apply", "--binary", "--3way", path]).map_err(|e| format!(
                    "could not restore stage {sid}'s suspended work onto the new HEAD: {e}; the patch is kept at {patch} — needs a human"))?;
            }
        }
        plan["stages"][idx].as_object_mut().unwrap().remove("suspended_work");
        self.save_plan(plan)?;
        self.log_event("git", &format!("stage {sid}: suspended uncommitted work restored"));
        Ok(())
    }
}
