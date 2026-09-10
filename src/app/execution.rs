//! Stage execution, durable attempt recovery, and completed-run publication.
use super::{Ctx, FORGE_DIR, WorkerGuard};
use crate::agent::AgentUsage;
use crate::prompts::REVIEW_PROMPT;
use crate::usage::accumulate_invocation_usage;
use crate::util::{fill_template, fmt_duration, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;
use std::sync::atomic::Ordering;

impl Ctx {
    pub(super) fn record_stage_usage(&self, plan: &mut Value, idx: usize, role: &str, tool: &str, usage: Option<AgentUsage>) -> Result<(), String> {
        if let Some(usage) = usage.filter(|usage| !usage.is_empty()) {
            accumulate_invocation_usage(&mut plan["stages"][idx], "usage", tool, &usage);
            accumulate_invocation_usage(plan, "usage", tool, &usage);
            accumulate_invocation_usage(&mut plan["role_usage"], role, tool, &usage);
            self.save_plan(plan)?;
        }
        Ok(())
    }

    pub(super) fn finish_stage(&self, plan: &mut Value, idx: usize, status: &str) -> Result<i64, String> {
        let stage = &mut plan["stages"][idx];
        let finished = unix_timestamp();
        let started = stage["started_unix"].as_i64().unwrap_or(finished);
        stage["status"] = json!(status);
        stage["finished_unix"] = json!(finished);
        stage["duration_secs"] = json!(finished - started);
        self.save_plan(plan)?;
        Ok(finished - started)
    }

    /// The saved verdict keeps legacy fields; both agents get the same effective requests.
    pub(super) fn review_requests(verdict: &Value) -> Vec<String> {
        let mut requests = Vec::new();
        for field in ["issues", "notes"] {
            for request in verdict[field].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !requests.iter().any(|existing| existing == request) {
                    requests.push(request.to_string());
                }
            }
        }
        requests
    }

    fn stage_review_context(stage: &Value) -> String {
        let round = stage["rounds"].as_i64().unwrap_or(1);
        let kind = if round == 1 { "Initial review" } else { "Re-review" };
        let mut context = format!("REVIEW ROUND: {round} (current attempt) — {kind}.\n");
        let Some(verdict) = stage.get("last_verdict").filter(|v| v.is_object()
            && stage["context_valid"] != false && stage["last_verdict_valid"] != false) else {
            context.push_str("No previous review findings for this stage.\n");
            return context;
        };
        if round == 1 {
            context.push_str("The feedback below is from this stage's previous attempt; current-attempt numbering restarts at 1.\n");
        } else {
            context.push_str("The feedback below is from the immediately preceding review in this attempt.\n");
        }
        let requests = Self::review_requests(verdict);
        let decision = if verdict["approved"] == true && requests.is_empty() {
            "approved"
        } else {
            "changes requested"
        };
        context.push_str(&format!(
            "BEGIN PREVIOUS REVIEW CONTEXT\nEffective decision: {decision}\nSummary:\n{}\nOutstanding change requests (including legacy notes):\n",
            verdict["summary"].as_str().unwrap_or(""),
        ));
        for request in requests {
            context.push_str(&format!("- {request}\n"));
        }
        context.push_str("Previous checks (must be verified again):\n");
        for check in verdict["checks"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            context.push_str(&format!("- {check}\n"));
        }
        context.push_str("END PREVIOUS REVIEW CONTEXT\n");
        context
    }

    pub(super) fn stage_prompt(&self, template: &str, plan: &Value, stage: &Value) -> Result<String, String> {
        let overview: String = plan["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                format!(
                    "  {}. {} -> {}\n",
                    s["id"], s["title"].as_str().unwrap_or(""), s["commit"].as_str().unwrap_or("")
                )
            })
            .collect();
        let mut prompt = fill_template(template, &[
            ("{goal}", plan["goal"].as_str().unwrap_or("")),
            ("{plan_overview}", &overview),
            ("{sid}", &stage["id"].to_string()),
            ("{title}", stage["title"].as_str().unwrap_or("")),
            ("{instructions}", stage["instructions"].as_str().unwrap_or("")),
            ("{acceptance}", stage["acceptance"].as_str().unwrap_or("")),
            ("{forge_dir}", FORGE_DIR),
            ("{verdict_path}", &format!("{FORGE_DIR}/verdict.json")),
            ("{review_context}", &Self::stage_review_context(stage)),
        ]);
        if template != REVIEW_PROMPT {
            let current = self.load_plan().ok_or("missing architectural plan")?;
            let cp = self.architecture_store().checkpoint(&current)?;
            let guidance = &cp["guidance"][stage["id"].to_string()];
            let idx = current["stages"].as_array().unwrap().iter().position(|s| s["id"] == stage["id"]).ok_or("missing guidance stage")?;
            if cp["context_status"] != "ready" || guidance["valid"] != true || guidance["relevant_inputs"] != crate::plan::stage_inputs(&current, idx) {
                return Err("required architectural guidance is missing or stale".into());
            }
            prompt.push_str(&format!("\nARCHITECT GUIDANCE:\n{}\nSaved constraints: {}\nCompleted interfaces: {}\nOutstanding risks: {}\nDecision history: .forge/architecture/{}/events.jsonl\n", guidance["text"].as_str().unwrap_or(""), cp["constraints"], cp["completed_interfaces"], cp["unresolved_risks"], current["plan_id"].as_str().unwrap_or("")));
            let completed: Vec<Value> = current["stages"].as_array().unwrap().iter().filter(|s| s["status"] == "committed")
                .map(|s| json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"],"sha":s["sha"]})).collect();
            prompt.push_str(&format!("\nCompleted stage interfaces and verified outcomes: {}\nRecent execution outcomes: {}\nRead the decision history for relevant decisions omitted from the recent preview; inspect completed interfaces in code before changing them.\n", json!(completed), cp["execution_outcomes"]));
            prompt.push_str(&format!("\n[implementer] Last validated outcome/escalation request: {}\n[engine] Latest routing handoff: {}\n", stage["implementer_outcome"], stage["reassessment"]["history"].as_array().and_then(|h| h.last()).map(|h| json!({"kind":h["kind"],"evidence":h["evidence"]})).unwrap_or(Value::Null)));
            let worktree = self.git(&["status", "--short"])?;
            let diff = self.git(&["diff", "HEAD", "--", ".", ":(exclude).forge"])?;
            prompt.push_str(&format!("\nSAVED ARCHITECTURAL SUMMARY: {}\nRelevant decisions: {}\nWORKTREE: {}\nDIFF (bounded preview; inspect full staged/unstaged diff and all untracked contents yourself):\n{}\nOUTSTANDING FINDINGS:\n{}\nYou may be inheriting partial work from another agent. Inspect and preserve all existing changes, completed interfaces and accepted decisions before editing. Saved findings remain authoritative until resolved with evidence.\n", cp["summary"], cp["recent_decisions"], worktree, diff.chars().take(16000).collect::<String>(), Self::stage_review_context(stage)));

        }
        Ok(prompt)
    }

    /// Implement + independent review + bounded fix loop for one stage.
    fn run_one_stage(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let result = self.run_review_stage(plan, idx);
        if let Err(error) = &result {
            plan["stages"][idx]["review_gate"]["status"] = json!("error");
            plan["stages"][idx]["review_gate"]["error"] = json!(error);
            if error.contains("model routing blocked:") { plan["stages"][idx]["reassessment"]["status"] = json!("blocked"); plan["stages"][idx]["reassessment"]["error"] = json!(error); }
            plan["stages"][idx]["last_verdict_valid"] = json!(false);
            let stage = &plan["stages"][idx];
            let mut requests = Self::review_requests(&stage["previous_requests"]);
            for record in stage["reviews"].as_array().into_iter().flatten().filter(|r|
                r["attempt_id"] == stage["attempt_id"] && r["round"] == stage["rounds"]) {
                for request in Self::review_requests(record) {
                    requests.push(format!("[{}] {request}", record["role"].as_str().unwrap_or("reviewer")));
                }
            }
            requests.push(format!("[engine] {error}; re-verify the entire stage and all required checks"));
            plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Review interrupted or invalid; prior unresolved findings retain authority","issues":requests,"notes":[],"checks":[]});
            if self.session.stop_requested.load(Ordering::SeqCst) {
                plan["stages"][idx]["review_gate"]["status"] = json!("interrupted");
                self.save_plan(plan)?;
                return Ok("stopped");
            }
            self.finish_stage(plan, idx, "blocked")?;
        }
        if matches!(result, Ok("stopped")) {
            plan["stages"][idx]["review_gate"]["status"] = json!("interrupted");
            plan["stages"][idx]["last_verdict_valid"] = json!(false);
            self.save_plan(plan)?;
        }
        result
    }

    pub(crate) fn run_worker(&self) {
        let _worker = WorkerGuard(&self.session);
        self.run_with_busy_claim();
    }

    /// Run and record failures while the caller owns the busy claim and WorkerGuard.
    /// This method must not construct WorkerGuard or release the busy claim.
    pub(super) fn run_with_busy_claim(&self) {
        let result = self.run_worker_inner();
        if let Err(e) = result {
            self.set_phase(if e.contains("model routing blocked:") { "blocked" } else { "failed" });
            self.log_event("error", &format!("run failed: {e}"));
        }
        self.set_step(None, "");
        self.session.state.lock().unwrap().run_started_unix = 0;
    }

    fn run_worker_inner(&self) -> Result<(), String> {
        let mut plan = self.recover_execution_plan()?;
        let count = plan["stages"].as_array().unwrap().len();
        for idx in 0..count {
            if plan["stages"][idx]["status"] == json!("committed") {
                continue;
            }
            if self.session.stop_requested.load(Ordering::SeqCst) {
                self.set_phase("plan_ready");
                self.log_event("run", "stopped by user; progress is saved, run again to continue");
                return Ok(());
            }
            let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
            // A renegotiated stage carries revised text and replenished rounds,
            // so it starts over as a fresh attempt rather than ending the run.
            let outcome = loop {
                let title = plan["stages"][idx]["title"].as_str().unwrap_or("").to_string();
                self.initialize_stage_attempt(&mut plan, idx)?;
                self.set_step(Some(sid), "implementing");
                self.log_event("stage", &format!("stage {sid} started: {title}"));
                match self.run_one_stage(&mut plan, idx)? {
                    "renegotiated" => plan = self.load_plan()
                        .ok_or("missing plan after scope renegotiation")?,
                    settled => break settled,
                }
            };

            match outcome {
                "stopped" => {
                    self.set_phase("plan_ready");
                    self.log_event("run", "stopped by user; progress is saved, run again to continue");
                    return Ok(());
                }
                "configuration_blocked" => {
                    self.finish_stage(&mut plan, idx, "blocked")?;
                    self.set_phase("blocked");
                    self.log_event("stage", &format!("stage {sid} blocked: independent reviewer configuration requires correction"));
                    return Ok(());
                }
                "exhausted" => {
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "blocked")?);
                    self.set_phase("blocked");
                    self.log_event("stage", &format!(
                        "stage {sid} blocked after {duration}: required review gate is not clean after max fix rounds — needs a human"));
                    return Ok(());
                }
                "scope_blocked" => {
                    let reason = plan["stages"][idx]["review_gate"]["reason"]
                        .as_str().unwrap_or("the stage cannot be built as written").to_string();
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "blocked")?);
                    self.set_phase("blocked");
                    self.log_event("stage", &format!(
                        "stage {sid} blocked after {duration}: {reason} — needs a human"));
                    return Ok(());
                }
                // Both approved and fully deferred gates may authorize a local commit.
                _settled => {
                    let msg = plan["stages"][idx]["commit"].as_str().unwrap_or("forge: stage").to_string();
                    let sha = match self.commit_reviewed(&plan, idx, &msg) {
                        Ok(sha) => sha,
                        Err(error) => {
                            plan["stages"][idx]["review_gate"]["status"] = json!("invalidated");
                            plan["stages"][idx]["review_gate"]["error"] = json!(error);
                            plan["stages"][idx]["last_verdict_valid"] = json!(false);
                            self.finish_stage(&mut plan, idx, "blocked")?;
                            return Err(error);
                        }
                    };
                    if let Some(sha) = sha {
                        plan["stages"][idx]["sha"] = json!(sha);
                    }
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "committed")?);
                    self.log_event("stage", &format!("stage {sid} committed in {duration}"));
                }
            }
        }

        self.publish_completed_run(&mut plan, count)
    }

    /// Recover committed work and reuse architectural guidance only for matching saved inputs.
    fn recover_execution_plan(&self) -> Result<Value, String> {
        self.recover_committed_stages()?;
        let loaded = self.load_plan().ok_or("no plan")?;
        let cp = if loaded["architecture"].is_object() { self.architecture_store().checkpoint(&loaded)? } else { Value::Null };
        let pending_turn = loaded["plan_id"].as_str().and_then(|id| fs::read(self.forge_path("architecture").join(id).join("architect-pending.json")).ok())
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap_or(json!({"turn":"invalid"})));
        let reusable = cp["context_status"] == "ready" && cp["session"]["resume_policy"] != "fork_from_checkpoint"
            && pending_turn.as_ref().is_none_or(|p| p["turn"] == cp["last_turn"])
            && loaded["stages"].as_array().into_iter().flatten().enumerate().all(|(idx,s)| s["status"] == "committed"
                || (cp["guidance"][s["id"].to_string()]["valid"] == true && cp["guidance"][s["id"].to_string()]["relevant_inputs"] == crate::plan::stage_inputs(&loaded,idx)));
        let plan = if reusable { loaded } else {
            self.architect_publish(loaded.clone(), Some(&loaded), "stage run").map_err(|e| format!("model routing blocked: {e}"))?
        };
        Ok(plan)
    }

    /// Persist attempt state before implementation; restart alone does not replenish budgets.
    fn initialize_stage_attempt(&self, plan: &mut Value, idx: usize) -> Result<(), String> {
        plan["stages"][idx]["status"] = json!("in_progress");
        plan["stages"][idx]["started_unix"] = json!(unix_timestamp());
        // Completion timing is no longer current; reuse the attempt and budgets unless identity or revision changes.
        let plan_revision = plan["revision"].clone();
        let stage = plan["stages"][idx].as_object_mut().unwrap();
        if stage.get("attempt_id").is_none() || stage.get("attempt_revision") != Some(&plan_revision) {
            stage.insert("attempt_id".into(), json!(crate::architecture::identity()));
            stage.insert("attempt_revision".into(), plan_revision);
            stage.insert("rounds".into(), json!(0));
            let settings = self.app.settings.lock().unwrap();
            stage.insert("review_budget".into(), json!(settings["max_fix_rounds"].as_i64().unwrap_or(3).max(0)));
            stage.insert("review_cadence".into(), json!({
                "architect": crate::plan::review_cadence(&settings, "architect"),
                "reviewer": crate::plan::review_cadence(&settings, "reviewer"),
            }));
            stage.insert("dual_promoted".into(), json!(false));
            stage.remove("attempt_head");
            stage.remove("reassessment");
            stage.remove("previous_requests");
        }
        stage.insert("review_gate".into(), json!({"status":"pending"}));
        stage.insert("last_verdict_valid".into(), json!(false));
        stage.remove("finished_unix");
        stage.remove("duration_secs");
        self.save_plan(plan)?;
        Ok(())
    }

    /// Publish completion durably; report errors propagate so queue goals remain recoverable.
    fn publish_completed_run(&self, plan: &mut Value, count: usize) -> Result<(), String> {
        let persisted = self.load_plan().ok_or("missing plan before completion")?;
        if persisted["stages"].as_array().ok_or("invalid stages")?.iter().any(|stage| {
            stage["review_policy"]["deferred_roles"].as_array().is_some_and(|roles| !roles.is_empty())
        }) {
            self.set_phase("blocked");
            self.log_event("run", "local stage commits retained; deferred reviews require the plan review phase before completion or push");
            return Ok(());
        }
        plan["status"] = json!("done");
        self.save_plan(plan)?;
        if self.app.settings.lock().unwrap()["auto_push"].as_bool() == Some(true) {
            self.set_step(None, "pushing");
            match self.git(&["push", "-u", "origin", "HEAD"]) {
                Ok(out) => self.log_event("git", &format!("pushed to origin: {}",
                    if out.is_empty() { "ok" } else { &out })),
                Err(e) => self.log_event("error",
                    &format!("push failed (commits are safe locally): {e}")),
            }
        }
        self.set_phase("done");
        let started = self.session.state.lock().unwrap().run_started_unix;
        let now = unix_timestamp();
        let project = self.project();
        let published = self.architecture_store().load_raw()?.ok_or("missing completed plan")?;
        let architecture = self.architecture_store().summary(Some(&published))?;
        let report = crate::reports::completed_run_report(plan, project, count, started, now, architecture);
        self.ensure_forge_dir();
        let mut f = fs::OpenOptions::new().create(true).append(true)
            .open(self.forge_path("reports.jsonl")).map_err(|e| format!("run report: {e}"))?;
        writeln!(f, "{report}").and_then(|_| f.sync_all())
            .and_then(|_| fs::File::open(self.forge_path("")).and_then(|dir| dir.sync_all()))
            .map_err(|e| format!("run report: {e}"))?;
        let text = crate::reports::completion_message(&report);
        self.log_event("run", &text);
        Ok(())
    }
}
