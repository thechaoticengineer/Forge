//! Plan-review constraint-conflict escalation to the planner.
//!
//! The plan-review adapter of the shared protocol in `constraint_conflict`: a
//! plan reviewer, the architect or the plan fixer can report a constraint
//! conflict, and the engine hands a stalled or exhausted plan review to the
//! planner once per conflict signature. The records live on
//! `plan_review.constraint_escalations` and survive fresh attempts, so the same
//! conflict after an approved correction blocks without another planner pass.
//!
//! Every stage is committed when plan review runs, so a correction only moves
//! forward: new pending stages after the committed ones, or corrected deferred
//! criteria for a committed stage. Every correction returns the plan to the
//! user in draft; the engine never approves it and never rewrites history.
//!
//! A record moves through the same persisted sequence as at stage level:
//! pending (saved with `next_action = conflict_pending` before the planner is
//! asked), answered (the validated answer saved before any plan change), then
//! settled in the same save or publication that applies its outcome.
use super::*;
use crate::constraint_conflict::{self as conflict, CRITERIA, Decision, RECORDS, Scope};
use crate::util::unix_timestamp;

#[derive(Debug, PartialEq)]
pub(super) enum PlanConflict {
    /// The correction returned the plan to the user for approval.
    AwaitingApproval,
    /// The planner explained how the requests can be met; fixing continues.
    Clarified,
    /// The plan review blocks with the escalation record attached to its gate.
    Blocked,
}

/// Who escalated, and the conflict statements the signature is built from.
pub(super) struct PlanTrigger {
    pub source: String,
    pub kind: &'static str,
    pub reason: String,
    pub statements: Vec<String>,
}

/// Where the plan review goes on after a refusal, and how it blocks otherwise.
pub(super) struct PlanFollowUp {
    /// `stalled` or `exhausted`: the existing gate the review blocks with.
    pub blocks_as: &'static str,
    /// Whether a fix round remains to receive a clarification.
    pub rounds_left: bool,
    /// The next action after a clarification.
    pub resume_action: &'static str,
}

/// The fixer's reply to a plan fix round, kept as planner input. Only the
/// latest replies are kept, bounded like stage outcome history.
pub(super) fn record_fixer_reply(review: &mut Value, output: &str) {
    let reported = conflict::fixer_reported(output);
    let reply: String = output.chars().take(conflict::MAX_TEXT).collect();
    let entry = json!({"attempt_id":review["attempt_id"],"round":review["rounds"],
        "reply":reply,"constraint_conflict":reported,"unix":unix_timestamp()});
    if !review["fixer_replies"].is_array() { review["fixer_replies"] = json!([]); }
    let replies = review["fixer_replies"].as_array_mut().unwrap();
    replies.push(entry);
    let excess = replies.len().saturating_sub(conflict::OUTCOME_HISTORY_KEEP);
    replies.drain(..excess);
}

/// The conflict the plan fixer reported in the current round, if any.
pub(super) fn fixer_conflict(plan: &Value) -> Option<String> {
    let review = &plan["plan_review"];
    review["fixer_replies"].as_array()?.last()
        .filter(|r| r["attempt_id"] == review["attempt_id"] && r["round"] == review["rounds"])
        .and_then(|r| r["constraint_conflict"].as_str().map(str::to_owned))
}

/// Conflicts reported in this attempt's latest review round by a plan reviewer
/// or the architect, and by the plan fixer after it, with their roles.
fn reported_conflicts(plan: &Value) -> Vec<(String, String)> {
    let review = &plan["plan_review"];
    let attempt = &review["attempt_id"];
    let reviews: Vec<&Value> = review["reviews"].as_array().into_iter().flatten()
        .filter(|r| r["identity"]["attempt_id"] == *attempt).collect();
    let latest = reviews.iter().filter_map(|r| r["identity"]["round"].as_u64()).max().unwrap_or(0);
    let mut reports: Vec<(String, String)> = reviews.iter()
        .filter(|r| r["identity"]["round"].as_u64() == Some(latest))
        .filter_map(|r| conflict::verdict_field(r).ok().flatten()
            .map(|text| (r["identity"]["role"].as_str().unwrap_or("reviewer").to_owned(), text)))
        .collect();
    reports.extend(review["fixer_replies"].as_array().into_iter().flatten()
        // A fix round and the review after it share their round number.
        .filter(|r| r["attempt_id"] == *attempt && r["round"].as_u64().is_some_and(|round| round > latest))
        .filter_map(|r| r["constraint_conflict"].as_str().map(|text| ("fixer".to_owned(), text.to_owned()))));
    reports
}

/// A trigger from role-reported conflict statements.
pub(super) fn role_trigger(reports: &[(String, String)]) -> Option<PlanTrigger> {
    let (source, _) = reports.first()?;
    let statements: Vec<String> = reports.iter().map(|(_, text)| text.clone()).collect();
    Some(PlanTrigger { source: source.clone(), kind: conflict::KIND, reason: statements.join("\n"), statements })
}

/// A short, human-readable account of what a plan-review correction changes.
fn correction_summary(decision: &Decision) -> String {
    match decision {
        Decision::Revise { append, criteria, .. } => {
            let mut parts = vec![];
            if !append.is_empty() {
                let titles: Vec<&str> = append.iter().map(|s| s.title.as_str()).collect();
                parts.push(format!("appended {} stage(s) after the committed ones: {}", titles.len(), titles.join("; ")));
            }
            if !criteria.is_empty() {
                let ids: Vec<String> = criteria.iter().map(|c| c.id.to_string()).collect();
                parts.push(format!("corrected the plan review criteria of stage(s) {}", ids.join(", ")));
            }
            parts.join("; ")
        }
        Decision::ConstraintWrong { stage, constraint, .. } => format!(
            "corrected the wrong constraint of stage {} ({constraint}) in the plan review criteria",
            stage.map(|s| s.to_string()).unwrap_or_default()),
        Decision::Refused(how) => format!("kept the plan unchanged: {how}"),
    }
}

/// The `edit_plan` body: every stage as saved (committed ones unchanged, as
/// edit_plan enforces) followed by the appended stages.
fn correction_body(plan: &Value, decision: &Decision) -> Value {
    let mut stages: Vec<Value> = plan["stages"].as_array().into_iter().flatten().map(|s| {
        let mut kept = json!({"id": s["id"], "title": s["title"], "instructions": s["instructions"],
            "acceptance": s["acceptance"], "commit": s["commit"].as_str().unwrap_or("")});
        if let Some(depends) = s.get("depends_on") { kept["depends_on"] = depends.clone(); }
        if !s["model_constraint"].is_null() { kept["model_constraint"] = s["model_constraint"].clone(); }
        kept
    }).collect();
    if let Decision::Revise { append, .. } = decision {
        stages.extend(append.iter().map(|new| json!({"title":new.title,"instructions":new.instructions,
            "acceptance":new.acceptance,"commit":new.commit})));
    }
    json!({"plan": {"goal": plan["goal"], "stages": stages}})
}

impl Ctx {
    /// Files changed since the plan review base: its commits plus uncommitted
    /// and untracked work, excluding Forge runtime data.
    fn plan_review_changed_files(&self, plan: &Value) -> Result<Vec<String>, String> {
        let Some(base) = plan["plan_review"]["base"].as_str() else { return Ok(vec![]); };
        let mut files = std::collections::BTreeSet::new();
        for args in [vec!["diff", "--name-only", base, "--", ".", ":(exclude).forge"],
            vec!["ls-files", "--others", "--exclude-standard", "--", ".", ":(exclude).forge"]] {
            files.extend(self.git(&args)?.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned));
        }
        Ok(files.into_iter().collect())
    }

    /// A stalled fix round or a spent budget: one planner pass for the conflict
    /// the roles reported, or the engine fallback when none did. A round whose
    /// escalation already ran blocks without another pass.
    pub(super) fn plan_review_nonconvergent(&self, plan: &mut Value, follow: PlanFollowUp) -> Result<PlanConflict, String> {
        let review = &plan["plan_review"];
        let this_round = review[RECORDS].as_array().and_then(|records| records.iter()
            .rposition(|r| r["attempt_id"] == review["attempt_id"] && r["round"] == review["rounds"]));
        if let Some(at) = this_round {
            let reason = "this round already had its constraint escalation";
            self.block_plan_review(plan, at, None, follow.blocks_as, reason)?;
            return Ok(PlanConflict::Blocked);
        }
        let trigger = role_trigger(&reported_conflicts(plan)).unwrap_or_else(|| {
            let (kind, reason) = if follow.blocks_as == "stalled" {
                (conflict::PLAN_STALLED_KIND, "two plan fix rounds changed nothing against the same outstanding requests".to_owned())
            } else {
                (conflict::PLAN_EXHAUSTED_KIND, format!("the plan review used all {} fix rounds without a clean review gate",
                    review["budget"].as_u64().unwrap_or(0)))
            };
            PlanTrigger { source: "engine".into(), kind, reason, statements: vec![conflict::plan_fallback_statement()] }
        });
        self.escalate_plan_conflict(plan, trigger, follow)
    }

    /// Hand a plan-review constraint conflict to the planner once per
    /// signature across the whole plan-review lineage.
    pub(super) fn escalate_plan_conflict(&self, plan: &mut Value, trigger: PlanTrigger, follow: PlanFollowUp)
        -> Result<PlanConflict, String>
    {
        let requests = conflict::plan_review_requests(plan);
        let signature = conflict::signature(&trigger.statements, &requests);
        let records = plan["plan_review"][RECORDS].as_array().cloned().unwrap_or_default();
        if let Some(prior) = records.iter().rposition(|r| r["signature"] == signature) {
            if records[prior]["outcome"] == "pending" && !records[prior]["decision"].is_null() {
                return self.apply_plan_conflict_answer(plan, prior);
            }
            let reason = format!("the same constraint conflict already had its planner pass (outcome {})",
                records[prior]["outcome"].as_str().unwrap_or("unknown"));
            let at = self.begin_plan_escalation(plan, &trigger, &signature, &follow)?;
            plan["plan_review"][RECORDS][at]["repeats"] = json!(prior);
            self.block_plan_review(plan, at, Some("blocked"), follow.blocks_as, &reason)?;
            return Ok(PlanConflict::Blocked);
        }
        let at = self.begin_plan_escalation(plan, &trigger, &signature, &follow)?;
        self.log_event("plan_review", &format!("plan review: {} reported by {}; one planner pass for this conflict",
            if trigger.source == "engine" { "no convergence" } else { "constraint conflict" }, trigger.source));
        if let Err(error) = self.consult_plan_planner(plan, at) {
            // A stop leaves the record unanswered; recovery settles it as failed.
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Err("plan review stopped; resume to continue".into());
            }
            let reason = format!("the planner pass failed: {error}");
            self.block_plan_review(plan, at, Some("failed"), follow.blocks_as, &reason)?;
            return Ok(PlanConflict::Blocked);
        }
        #[cfg(test)]
        if self.app.settings.lock().unwrap()["test_plan_conflict_crash"] == "answered" {
            return Err("simulated crash after the planner answered".into());
        }
        self.apply_plan_conflict_answer(plan, at)
    }

    /// Persist a pending record, and the action that resumes it, before the
    /// planner is asked, so a restart finds it. Returns the record's index.
    fn begin_plan_escalation(&self, plan: &mut Value, trigger: &PlanTrigger, signature: &str, follow: &PlanFollowUp)
        -> Result<usize, String>
    {
        let changed = self.plan_review_changed_files(plan)?;
        let inputs = conflict::plan_review_inputs(plan, &trigger.statements, &changed);
        let mut record = conflict::new_record(&trigger.source, trigger.kind, &trigger.reason, signature, inputs, unix_timestamp())?;
        let review = &plan["plan_review"];
        record["scope"] = json!("plan");
        record["attempt_id"] = review["attempt_id"].clone();
        record["round"] = review["rounds"].clone();
        record["blocks_as"] = json!(follow.blocks_as);
        record["rounds_left"] = json!(follow.rounds_left);
        record["resume_action"] = json!(follow.resume_action);
        let at = conflict::push_record(&mut plan["plan_review"], record);
        plan["plan_review"]["next_action"] = json!("conflict_pending");
        self.save_plan(plan)?;
        #[cfg(test)]
        if self.app.settings.lock().unwrap()["test_plan_conflict_crash"] == "pending" {
            return Err("simulated crash before the planner pass".into());
        }
        Ok(at)
    }

    /// Ask the planner about a pending record and save its validated answer,
    /// with the plan revision it answers, before anything applies it.
    fn consult_plan_planner(&self, plan: &mut Value, at: usize) -> Result<(), String> {
        let record = plan["plan_review"][RECORDS][at].clone();
        self.set_step(None, "asking the planner about the plan review");
        self.log_event("plan_review", "plan review: constraint conflict handed to the planner");
        let prompt = conflict::plan_prompt(plan, &record);
        let snapshot = plan.clone();
        let answer = self.planner_answer(plan, None, "plan review constraint conflict", &prompt,
            |text| conflict::validate_answer_for(text, &snapshot, Scope::Plan))?;
        let revision = plan["revision"].clone();
        let record = &mut plan["plan_review"][RECORDS][at];
        conflict::record_answer(record, &answer);
        record["answer_revision"] = revision;
        self.save_plan(plan)
    }

    /// Apply the saved, validated planner answer of record `at`. The record is
    /// settled in the same save or publication that applies its outcome.
    fn apply_plan_conflict_answer(&self, plan: &mut Value, at: usize) -> Result<PlanConflict, String> {
        let current = self.load_plan().ok_or("missing plan")?;
        let record = current["plan_review"][RECORDS][at].clone();
        let decision: Decision = serde_json::from_value(record["correction"].clone())
            .map_err(|e| format!("invalid saved constraint escalation answer: {e}"))?;
        let blocks_as = if record["blocks_as"] == "exhausted" { "exhausted" } else { "stalled" };
        *plan = current.clone();
        if record["answer_revision"] != current["revision"] {
            let reason = "the plan changed after the planner answered; its correction was not applied";
            self.block_plan_review(plan, at, Some("failed"), blocks_as, reason)?;
            return Ok(PlanConflict::Blocked);
        }
        let summary = correction_summary(&decision);
        if let Decision::Refused(how) = &decision {
            if record["rounds_left"] != true {
                let reason = format!("{summary}; no fix round remains");
                plan["plan_review"][RECORDS][at]["correction_summary"] = json!(summary);
                self.block_plan_review(plan, at, Some("blocked"), blocks_as, &reason)?;
                return Ok(PlanConflict::Blocked);
            }
            let review = &mut plan["plan_review"];
            review["conflict_clarification"] = json!({"attempt_id":review["attempt_id"],"record":at,
                "message":how,"reason":record["trigger"]["reason"],"unix":unix_timestamp()});
            review["next_action"] = record["resume_action"].clone();
            let settled = &mut review[RECORDS][at];
            settled["correction_summary"] = json!(summary);
            conflict::set_outcome(settled, "refused", Some(&summary), Some(&current["revision"]), unix_timestamp())?;
            self.save_plan(plan)?;
            self.log_event("plan_review", "plan review: the planner explained how the outstanding requests can be met as written; its explanation goes to the next fix round");
            return Ok(PlanConflict::Clarified);
        }
        let body = correction_body(&current, &decision);
        let mut edited = crate::plan::edit_plan(&current, &body).map_err(str::to_string)?;
        let revision = edited["revision"].clone();
        let correction = match &decision {
            Decision::Revise { criteria, .. } => criteria.iter()
                .map(|c| (c.id, json!({"acceptance":c.acceptance}))).collect::<Vec<_>>(),
            Decision::ConstraintWrong { stage, constraint, instructions, acceptance, justification } =>
                vec![(stage.unwrap_or_default(), json!({"acceptance":acceptance,"instructions":instructions,
                    "constraint":constraint,"justification":justification}))],
            Decision::Refused(_) => vec![],
        };
        for (id, mut entry) in correction {
            entry["record"] = json!(at);
            entry["revision"] = revision.clone();
            if !edited[CRITERIA].is_object() { edited[CRITERIA] = json!({}); }
            edited[CRITERIA][id.to_string()] = entry;
        }
        let review = &mut edited["plan_review"];
        let settled = &mut review[RECORDS][at];
        settled["correction_summary"] = json!(summary);
        conflict::set_outcome(settled, "awaiting_approval", Some(&summary), Some(&revision), unix_timestamp())?;
        let settled = settled.clone();
        review.as_object_mut().ok_or("invalid plan review")?.remove("conflict_clarification");
        review["status"] = json!("awaiting_approval");
        review["next_action"] = json!("awaiting_approval");
        review["gate"]["status"] = json!("awaiting_approval");
        review["gate"]["constraint_escalation"] = settled;
        review["gate"]["reason"] = json!(format!("the planner's correction awaits approval: {summary}"));
        *plan = self.architect_publish(edited, Some(&current), "plan review constraint conflict correction")?;
        self.log_event("plan_review", &format!(
            "plan review: the planner's correction ({summary}) awaits your approval; the plan is a draft again, committed stages are unchanged, and approving it starts a fresh plan review attempt (escalation record {at}); commits remain local"));
        Ok(PlanConflict::AwaitingApproval)
    }

    /// Block the plan review with the existing stalled or exhausted gate and the
    /// escalation record attached, settling the record in the same save.
    fn block_plan_review(&self, plan: &mut Value, at: usize, outcome: Option<&str>, blocks_as: &str, reason: &str)
        -> Result<(), String>
    {
        let mut current = self.load_plan().ok_or("missing plan")?;
        // The caller's copy may hold record changes the saved plan lacks.
        if plan["plan_review"][RECORDS][at].is_object() {
            current["plan_review"][RECORDS][at] = plan["plan_review"][RECORDS][at].clone();
        }
        let revision = current["revision"].clone();
        let review = &mut current["plan_review"];
        let record = &mut review[RECORDS][at];
        if !record.is_object() { return Err("missing constraint escalation record".into()); }
        if let Some(outcome) = outcome {
            conflict::set_outcome(record, outcome, Some(reason), Some(&revision), unix_timestamp())?;
        }
        let record = record.clone();
        if !review["gate"].is_object() { review["gate"] = json!({}); }
        review["gate"]["status"] = json!(blocks_as);
        review["gate"]["constraint_escalation"] = record.clone();
        review["gate"]["reason"] = json!(reason);
        review["status"] = json!("blocked");
        review["next_action"] = json!(blocks_as);
        self.save_plan(&current)?;
        *plan = current;
        let attached = format!("the constraint escalation record {at} (outcome {}) is attached to the gate",
            record["outcome"].as_str().unwrap_or("unknown"));
        self.log_event("plan_review", &if blocks_as == "exhausted" {
            format!("plan review budget exhausted: {reason}; {attached}; inspect outstanding requests and edit and approve a revised plan; commits remain local")
        } else {
            format!("plan review stalled: {reason}; {attached}; resolve the outstanding requests outside the working tree, then edit and approve a revised plan; commits remain local")
        });
        Ok(())
    }

    /// Resume an escalation a restart interrupted: apply an answered record's
    /// correction; a record the planner never answered fails and blocks, so the
    /// conflict is never handed back a second time.
    pub(super) fn resume_plan_conflict(&self, plan: &mut Value) -> Result<PlanConflict, String> {
        let records = plan["plan_review"][RECORDS].as_array().cloned().unwrap_or_default();
        let Some(at) = records.iter().rposition(|r| r["outcome"] == "pending") else {
            let at = records.len().checked_sub(1).ok_or("conflict_pending without an escalation record")?;
            let blocks_as = if records[at]["blocks_as"] == "exhausted" { "exhausted" } else { "stalled" };
            self.block_plan_review(plan, at, None, blocks_as, "the escalation was already settled before the restart")?;
            return Ok(PlanConflict::Blocked);
        };
        if !records[at]["decision"].is_null() {
            self.log_event("plan_review", "plan review: resuming the planner's saved constraint-conflict answer");
            return self.apply_plan_conflict_answer(plan, at);
        }
        let blocks_as = if records[at]["blocks_as"] == "exhausted" { "exhausted" } else { "stalled" };
        let reason = "interrupted before the planner's answer was saved; the conflict is not handed back again";
        self.block_plan_review(plan, at, Some("failed"), blocks_as, reason)?;
        Ok(PlanConflict::Blocked)
    }

    /// Once the user approved a plan-review correction, its records count as
    /// applied and the old attempt no longer awaits approval. The approved
    /// revision starts a fresh plan review attempt once every stage is committed.
    pub(in crate::app) fn start_approved_plan_correction(&self, plan: &mut Value) -> Result<(), String> {
        let review = &plan["plan_review"];
        if plan["status"] != "approved" || review["next_action"] != "awaiting_approval"
            || review["pending_revision"] != plan["revision"] {
            return Ok(());
        }
        let revision = plan["revision"].clone();
        mark_approved(&mut plan["plan_review"], &revision);
        let review = &mut plan["plan_review"];
        review["status"] = json!("pending");
        review["next_action"] = json!("reserve_review");
        review["gate"]["status"] = json!("pending");
        self.save_plan(plan)?;
        self.log_event("plan_review", "plan review: the planner's correction was approved; a fresh plan review attempt starts once every stage is committed");
        Ok(())
    }
}

/// Records whose correction the user approved at `revision` become applied.
pub(super) fn mark_approved(review: &mut Value, revision: &Value) {
    for record in review[RECORDS].as_array_mut().into_iter().flatten() {
        if record["outcome"] == "awaiting_approval" && record["plan_revision"].as_u64() <= revision.as_u64() {
            let _ = conflict::set_outcome(record, "applied",
                Some("approved by the user; a fresh plan review attempt reviews the corrected plan"), None, unix_timestamp());
        }
    }
}

/// Carry the escalation lineage and fixer replies into a fresh attempt.
pub(super) fn carry_lineage(old: &Value, next: &mut Value, revision: &Value) {
    for key in [RECORDS, "fixer_replies"] {
        if let Some(value) = old.get(key).filter(|v| v.is_array()) {
            next[key] = value.clone();
        }
    }
    mark_approved(next, revision);
}

/// The fixer prompt's view of a plan-review clarification for this attempt.
pub(super) fn clarification_text(review: &Value) -> Option<String> {
    let clarification = &review["conflict_clarification"];
    (clarification["attempt_id"] == review["attempt_id"] && !clarification["attempt_id"].is_null()).then(|| format!(
        "\nPLANNER ANSWER TO THE REPORTED CONSTRAINT CONFLICT (the outstanding requests can be met as written):\n{}\nResolve the outstanding requests under the unchanged plan using this explanation. This clarification is not a review approval: verify the fixes and every required check before handing off.\n",
        clarification["message"].as_str().unwrap_or("")))
}
