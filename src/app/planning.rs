//! Plan generation, revision, and plan-question chat workflows.

/// Scope renegotiations allowed per stage before the block belongs to a human.
const SCOPE_RENEGOTIATIONS: u64 = 1;

#[derive(Debug, PartialEq)]
pub(super) enum ScopeResolution {
    Revised(String),
    Clarified(String),
    Blocked(String),
}
use super::{Ctx, FORGE_DIR, PlanMode, WorkerGuard};
use crate::agent::AgentRequest;
use crate::prompts::{CHAT_PROMPT, DISCUSSION_PLANNER_PROMPT, ENHANCE_PROMPT, PLANNER_PROMPT, REFACTOR_PROMPT, REVISE_PROMPT, SCOPE_PROMPT};
use crate::candidate_draft::prepare_candidate_draft;
use crate::usage::accumulate_invocation_usage;
use crate::util::{fill_template, json_payload, json_payload_with_keys, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;

impl Ctx {
    /// If planning finished before the architect became unavailable, retry its
    /// publication without asking the planner to replace the saved candidate.
    pub(super) fn continue_queue_planning(&self, goal: &str) -> bool {
        let candidate = fs::read(self.forge_path("plan-candidate.json")).ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|plan| plan["goal"] == goal && plan["status"] == "draft"
                && plan["planner_selection_actor"].is_object() && plan["stages"].is_array());
        let Some(candidate) = candidate else {
            return self.plan_with_busy_claim(goal, &PlanMode::Standard);
        };
        match self.finalize_plan(candidate, None, "ready") {
            Ok(_) => true,
            Err(error) => {
                self.set_phase("blocked");
                self.log_event("error", &format!("planning continuation failed: {error}"));
                false
            }
        }
    }

    pub(crate) fn plan_worker(&self, goal: &str, mode: &PlanMode) {
        let _worker = WorkerGuard(&self.session);
        self.plan_with_busy_claim(goal, mode);
    }

    /// The caller owns the session busy claim and its WorkerGuard.
    /// This method must not construct WorkerGuard or release the busy claim.
    pub(super) fn plan_with_busy_claim(&self, goal: &str, mode: &PlanMode) -> bool {
        let discussion = matches!(mode, PlanMode::Discussion { .. });
        let prompt = match mode {
            PlanMode::Standard => PLANNER_PROMPT
                .replace("{goal}", goal)
                .replace("{plan_path}", &format!("{FORGE_DIR}/plan-candidate.json")),
            PlanMode::Milestone { feature } => {
                let prompt = PLANNER_PROMPT
                    .replace("{goal}", goal)
                    .replace("{plan_path}", &format!("{FORGE_DIR}/plan-candidate.json"));
                format!("{prompt}\n\n{}", crate::feature_context::feature_reference(feature).unwrap_or_default())
            }
            PlanMode::Refactor { focus } => fill_template(REFACTOR_PROMPT, &[
                ("{focus}", focus.as_str()), ("{plan_path}", ".forge/plan-candidate.json"),
            ]),
            PlanMode::Discussion { transcript, note } => fill_template(DISCUSSION_PLANNER_PROMPT, &[
                ("{note}", note.as_str()), ("{transcript}", transcript.as_str()),
                ("{plan_path}", ".forge/plan-candidate.json"),
            ]),
        };
        let feature = match mode {
            PlanMode::Milestone { feature } => Some(feature),
            _ => None,
        };
        let result = self.generate_plan(&prompt, goal, discussion, None, feature)
            .and_then(|plan| {
                let final_goal = plan["goal"].as_str().unwrap_or("").to_string();
                let published = self.finalize_plan(plan, None, "ready")?;
                if discussion {
                    self.session.state.lock().unwrap().goal = final_goal;
                }
                Ok(published)
            });
        let planned = match &result {
            Ok(_) => true,
            Err(e) => {
                let blocked = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
                self.set_phase(if blocked { "blocked" } else { "failed" });
                self.log_event("error", &format!("planning failed: {e}"));
                false
            }
        };
        if let Some(feature) = feature {
            self.settle_plan_link(feature, &result);
        }
        planned
    }

    /// Moves the milestone's `planning` link to `planned` with the published
    /// plan ID, or to `failed` with the error. The plan itself is already
    /// settled, so a state write failure is logged rather than undoing it.
    fn settle_plan_link(&self, feature: &Value, result: &Result<Value, String>) {
        let slug = feature["slug"].as_str().unwrap_or("");
        let milestone = feature["milestone"].as_str().unwrap_or("");
        let settled = match result {
            Ok(plan) => crate::feature_state::set_plan_link_status(
                self, slug, milestone, "planned", plan["plan_id"].as_str(), None),
            Err(error) => crate::feature_state::set_plan_link_status(
                self, slug, milestone, "failed", None, Some(error)),
        };
        if let Err(error) = settled {
            self.log_event("error", &format!("could not record the plan link of {slug} {milestone}: {error}"));
        }
    }

    pub(crate) fn revise_worker(&self, current_plan: &Value, feedback: &str) {
        let _worker = WorkerGuard(&self.session);
        let authoritative = self.load_plan().unwrap_or_else(|| current_plan.clone());
        let current_plan = &authoritative;
        let snapshot = serde_json::to_string_pretty(&crate::plan::content_view(current_plan)).unwrap();
        let goal = current_plan["goal"].as_str().unwrap_or("");
        let prompt = fill_template(REVISE_PROMPT, &[
            ("{current_plan}", snapshot.as_str()), ("{feedback}", feedback),
            ("{goal}", goal), ("{plan_path}", ".forge/plan-candidate.json"),
        ]);
        let reference = crate::feature_context::feature_reference(&current_plan["feature"]);
        let prompt = match &reference {
            Some(reference) => format!("{prompt}\n\n{reference}"),
            None => prompt,
        };
        let result = self.generate_plan(&prompt, goal, false, Some(current_plan), None)
            .and_then(|plan| self.finalize_plan(plan, Some(current_plan), "revised"));
        if let Err(error) = result {
            let architecture_failed = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
            self.set_phase(if architecture_failed { "blocked" } else { "plan_ready" });
            self.log_event("error", &format!("revision failed: {error}"));
        }
    }

    pub(crate) fn chat_worker(&self, current_plan: &Value, question: &str) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "answering plan question");
        let snapshot = serde_json::to_string_pretty(&crate::plan::content_view(current_plan)).unwrap();
        let history = serde_json::to_string_pretty(&self.read_chat()).unwrap();
        let prompt = fill_template(CHAT_PROMPT, &[
            ("{current_plan}", snapshot.as_str()), ("{history}", history.as_str()),
            ("{question}", question), ("{answer_path}", ".forge/answer.json"),
        ]);
        let result = (|| -> Result<(), String> {
            self.ensure_forge_dir();
            match fs::remove_file(self.forge_path("answer.json")) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("could not remove previous answer: {e}")),
            }
            let context = if current_plan.get("architecture").is_some() { self.architecture_store().checkpoint(current_plan)? } else { crate::architecture::checkpoint_default() };
            let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: read-only Q&A in a fresh conversation. Do not write files or alter plan/decisions. Return ONLY {{\"answer\":\"your answer\"}}. Saved architecture context: {context}");
            let reply = self.readonly_response("chat", &readonly_prompt, Some("answer.json"), |text| {
                let output: Value = crate::response::parse_json(json_payload(text))
                    .map_err(|e| format!("invalid answer JSON: {e}"))?;
                crate::response::object_fields(&output, &["answer"])?;
                output["answer"].as_str().filter(|answer| !answer.trim().is_empty())
                    .ok_or("agent did not produce a non-empty answer string")?;
                Ok(output)
            })?;
            let output = reply.value;
            let answer = output["answer"].as_str().unwrap();
            crate::durable_json::publish_pretty(&self.forge_path("answer.json"), &output)?;
            let user = json!({"role": "user", "text": question, "unix": unix_timestamp()});
            let assistant = json!({"role": "assistant", "text": answer, "unix": unix_timestamp()});
            let mut file = fs::OpenOptions::new().create(true).append(true)
                .open(self.forge_path("chat.jsonl")).map_err(|e| e.to_string())?;
            writeln!(file, "{user}\n{assistant}").map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            self.log_event("error", &format!("chat failed: {error}"));
        }
    }

    pub(crate) fn enhance_goal_worker(&self, goal: &str, request_id: i64) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "enhancing goal description");
        let result = (|| -> Result<String, String> {
            self.ensure_forge_dir();
            match fs::remove_file(self.forge_path("enhanced-goal.json")) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("could not remove previous enhanced goal: {e}")),
            }
            let prompt = fill_template(ENHANCE_PROMPT, &[
                ("{goal}", goal), ("{answer_path}", ".forge/enhanced-goal.json"),
            ]);
            let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: read-only. Do not write files, do not modify the plan or any repository content. Return ONLY {{\"goal\":\"rewritten description\"}} as JSON in your final response.");
            let reply = self.readonly_response("enhance", &readonly_prompt, Some("enhanced-goal.json"), |text| {
                let output: Value = crate::response::parse_json(json_payload(text))
                    .map_err(|e| format!("invalid enhanced goal JSON: {e}"))?;
                crate::response::object_fields(&output, &["goal"])?;
                let enhanced = output["goal"].as_str().map(str::trim).filter(|goal| !goal.is_empty())
                    .ok_or("agent did not produce a non-empty goal string")?;
                if enhanced.chars().count() > 20000 { return Err("enhanced description too long".into()); }
                Ok(enhanced.to_string())
            })?;
            Ok(reply.value)
        })();
        let (enhancement, kind, message) = match result {
            Ok(enhanced) => (json!({"status":"ready","request_id":request_id,"original":goal,"goal":enhanced,"unix":unix_timestamp()}),
                "plan", "goal description enhanced".to_string()),
            Err(error) => (json!({"status":"failed","request_id":request_id,"original":goal,"error":error,"unix":unix_timestamp()}),
                "error", format!("goal enhancement failed: {error}")),
        };
        {
            let mut state = self.session.state.lock().unwrap();
            if state.goal_enhancement_serial == request_id { state.goal_enhancement = enhancement; }
        }
        self.log_event(kind, &message);
    }

    /// For a discussion run the engine has no goal to enforce up front, so the
    /// candidate's own trimmed `goal` field is validated and used instead of
    /// the engine-supplied override every other mode keeps applying.
    ///
    /// `feature` is the milestone plan mode's feature object. Only then does
    /// the plan carry a `feature` record, and it is always the engine's: a
    /// planner-supplied `feature` key is removed from every candidate (S35).
    /// A revision passes None and keeps the previous plan's record, because
    /// the revised draft is built from that plan.
    fn generate_plan(&self, prompt: &str, goal: &str, discussion: bool, previous: Option<&Value>,
        feature: Option<&Value>) -> Result<Value, String>
    {
        self.ensure_forge_dir();
        let _ = fs::remove_file(self.forge_path("chat.jsonl"));
        match fs::remove_file(self.forge_path("plan-candidate.json")) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not remove previous candidate: {e}")),
        }
        let prompt = format!("{prompt}\n{}\nOUTPUT CONTRACT OVERRIDE: inspect only; do not write any files, implement, commit or push. Return the complete candidate plan as a JSON object in your final response, without markdown fences. Forge validates and writes the candidate file itself.", self.routing_prompt()?);
        // A revision keeps its plan's feature record and its first-stage rule.
        let plan_feature = feature.or_else(|| previous.map(|plan| &plan["feature"]))
            .filter(|feature| feature.is_object());
        let reply = self.readonly_response("planner", &prompt, Some("plan-candidate.json"), |text| {
            let mut candidate: Value = crate::response::parse_json(json_payload(text))
                .map_err(|e| format!("invalid candidate: {e}"))?;
            if !candidate["stages"].is_array() { return Err("planner did not produce valid stages".into()); }
            // Only the engine attaches a feature, and only to a milestone plan (S35).
            candidate.as_object_mut().unwrap().remove("feature");
            let resolved_goal: String = if discussion {
                let candidate_goal = candidate["goal"].as_str().map(str::trim).unwrap_or("").to_string();
                if candidate_goal.is_empty() || candidate_goal.chars().count() > 20000 {
                    return Err("planner did not produce a goal from the discussion".into());
                }
                candidate_goal
            } else {
                goal.to_string()
            };
            let (draft, _) = prepare_candidate_draft(candidate, &resolved_goal, previous)?;
            crate::routing::validate_candidate_proposals(&draft)?;
            // A committed first stage is history a revision can no longer change.
            if let Some(feature) = plan_feature.filter(|_| draft["stages"][0]["status"] != "committed") {
                let missing = crate::feature_context::missing_first_stage_ids(feature, &draft["stages"][0]);
                if !missing.is_empty() {
                    return Err(format!("first stage must name every covered scenario ID; missing: {}", missing.join(", ")));
                }
            }
            Ok(draft)
        })?;
        let mut plan = reply.value;
        if let Some(feature) = feature {
            plan["feature"] = json!({
                "slug": feature["slug"], "milestone": feature["milestone"],
                "title": feature["title"], "scenario_ids": feature["scenario_ids"],
            });
        }
        plan["planner_selection_actor"] = json!({"provider":reply.choice.0,"model":reply.result.effective_model,"native_effort":reply.choice.2});
        // Only measured calls contribute usage, including every correction.
        plan.as_object_mut().unwrap().remove("planner_usage");
        plan.as_object_mut().unwrap().remove("role_usage");
        if let Some(previous) = previous {
            plan["planner_usage"] = previous["planner_usage"].clone();
            plan["role_usage"] = previous["role_usage"].clone();
        }
        for usage in &reply.usage { accumulate_invocation_usage(&mut plan, "planner_usage", &reply.choice.0, usage); }
        if !plan["planner_usage"].is_null() {
            if !plan["role_usage"].is_object() { plan["role_usage"] = json!({}); }
            plan["role_usage"]["planner"] = plan["planner_usage"].clone();
        }
        crate::durable_json::publish_pretty(&self.forge_path("plan-candidate.json"), &plan)?;
        Ok(plan)
    }

    fn bind_planner_proposals(&self, plan: &mut Value) {
        for idx in 0..plan["stages"].as_array().unwrap().len() {
            if plan["stages"][idx]["status"] != "committed" && plan["stages"][idx]["model_proposal"].is_object() {
                plan["stages"][idx]["model_proposal_inputs"] = self.proposal_inputs(plan, idx);
                plan["stages"][idx]["model_proposer"] = plan["planner_selection_actor"].clone();
            }
        }
    }

    fn finalize_plan(&self, mut plan: Value, previous: Option<&Value>, action: &str)
        -> Result<Value, String>
    {
        let n = plan["stages"].as_array().unwrap().len();
        self.bind_planner_proposals(&mut plan);
        let published = self.architect_publish(plan, previous, if previous.is_some() { "draft revision" } else { "initial plan guidance" })?;
        self.set_phase("plan_ready");
        self.log_event("plan", &format!("plan {action} with {n} stages"));
        Ok(published)
    }

    /// A scope escalation reports that the stage as written cannot be built. The
    /// planner owns the stage text, so hand the report back to it once instead of
    /// spending the remaining fix rounds re-running the same impossible stage.
    /// A planner may revise the scope or explain how to complete it unchanged.
    pub(super) fn renegotiate_scope(&self, plan: &mut Value, idx: usize, outcome: &Value)
        -> Result<ScopeResolution, String>
    {
        if !plan["stages"][idx]["scope_revision_pending"].is_null() {
            return self.resume_scope_revision(plan, idx).map(ScopeResolution::Revised);
        }
        let sid = plan["stages"][idx]["id"].clone();
        let request = &outcome["request"];
        let reason = request["reason"].as_str().unwrap_or("").to_string();
        if plan["stages"][idx]["scope_clarification"]["source_inputs"] == crate::plan::stage_inputs(plan, idx) {
            let message = format!("the implementer still reports a scope blocker after the planner's clarification: {reason}");
            plan["stages"][idx]["scope_clarification"]["blocked_reason"] = json!(message);
            self.save_plan(plan)?;
            return Ok(ScopeResolution::Blocked(message));
        }
        let done = plan["stages"][idx]["scope_renegotiations"].as_u64().unwrap_or(0);
        if done >= SCOPE_RENEGOTIATIONS {
            return Ok(ScopeResolution::Blocked(format!(
                "the stage was already revised once and the implementer still calls it unbuildable: {reason}")));
        }
        self.set_step(sid.as_i64(), "renegotiating stage scope");
        self.log_event("plan", &format!(
            "stage {sid} reported unbuildable as written; asking the planner to revise it"));
        let escalation = format!("reason: {reason}\nrequired: {}\nevidence:\n- {}",
            request["required_capability"].as_str().unwrap_or(""),
            outcome["evidence"].as_array().into_iter().flatten()
                .filter_map(Value::as_str).collect::<Vec<_>>().join("\n- "));
        let stage = plan["stages"][idx].clone();
        let prompt = fill_template(SCOPE_PROMPT, &[
            ("{goal}", plan["goal"].as_str().unwrap_or("")),
            ("{sid}", &sid.to_string()),
            ("{title}", stage["title"].as_str().unwrap_or("")),
            ("{instructions}", stage["instructions"].as_str().unwrap_or("")),
            ("{acceptance}", stage["acceptance"].as_str().unwrap_or("")),
            ("{escalation}", &escalation),
        ]);
        let answer = self.planner_answer(plan, idx, "scope revision", &prompt, |text| {
            let answer: Value = crate::response::parse_json(json_payload_with_keys(text, &["revised", "refused"]))
                .map_err(|e| format!("invalid scope revision: {e}"))?;
            crate::response::object_fields(&answer, &["revised", "refused", "removed"])?;
            if answer.get("revised").is_some() == answer.get("refused").is_some() {
                return Err("scope response must contain exactly one of revised or refused; preserve your actual decision".into());
            }
            if answer.get("refused").is_some() {
                if answer.get("removed").is_some() {
                    return Err("scope clarification cannot also report removed requirements".into());
                }
                answer["refused"].as_str().filter(|r| !r.trim().is_empty())
                    .ok_or("scope refusal requires a non-empty explanation")?;
                return Ok(answer);
            }
            let revised = &answer["revised"];
            crate::response::object_fields(revised, &["instructions", "acceptance"])?;
            match (revised["instructions"].as_str(), revised["acceptance"].as_str()) {
                (Some(i), Some(_)) if !i.trim().is_empty() => {},
                _ => return Err("planner returned no usable stage revision".into()),
            }
            if let Some(removed) = answer.get("removed") {
                removed.as_str().ok_or("scope removed explanation must be a string")?;
            }
            if revised["instructions"] == stage["instructions"] && revised["acceptance"] == stage["acceptance"] {
                return Err("the planner returned the stage unchanged without clarification; return refused with an explanation or an actual revision".into());
            }
            Ok(answer)
        })?;
        if let Some(refused) = answer["refused"].as_str().filter(|r| !r.trim().is_empty()) {
            plan["stages"][idx]["scope_clarification"] = json!({
                "source_inputs":crate::plan::stage_inputs(plan, idx),
                "message":refused, "reason":reason, "pending":true,
                "unix":unix_timestamp(),
            });
            self.save_plan(plan)?;
            self.log_event("plan", &format!("stage {sid}: planner clarified the existing requirements; returning to implementation"));
            return Ok(ScopeResolution::Clarified(refused.to_owned()));
        }
        let revised = &answer["revised"];
        // The complete response was validated before either decision branch.
        let instructions = revised["instructions"].as_str().unwrap().to_owned();
        let acceptance = revised["acceptance"].as_str().unwrap().to_owned();
        let changed = answer["removed"].as_str().unwrap_or("").to_owned();
        // Record the renegotiation before the edit so it survives reconciliation
        // and stays visible next to the stage it narrowed. It also goes into the
        // caller's plan, so a later failure saving that copy cannot drop the
        // count and hand the stage an unlimited supply of renegotiations.
        let mut current = self.load_plan().ok_or("missing plan")?;
        let target = current["stages"].as_array().ok_or("invalid stages")?.iter()
            .position(|s| s["id"] == sid).ok_or("stage disappeared during renegotiation")?;
        current["stages"][target]["scope_renegotiations"] = json!(done + 1);
        if !current["stages"][target]["scope_history"].is_array() {
            current["stages"][target]["scope_history"] = json!([]);
        }
        let entry = json!({"unix": unix_timestamp(), "revision": current["revision"],
            "reason": reason, "changed": changed});
        current["stages"][target]["scope_history"].as_array_mut().unwrap().push(entry);
        current["stages"][target]["scope_revision_pending"] = json!({
            "source_inputs": crate::plan::stage_inputs(&current, target),
            "instructions": instructions, "acceptance": acceptance, "changed": changed,
        });
        *plan = self.publish_plan(&current, false)?;
        self.resume_scope_revision(plan, target).map(ScopeResolution::Revised)
    }

    /// Ask the stage's planner once and validate the complete answer, spending
    /// the shared correction budget on malformed answers. The mock planner
    /// answers from the `mock_scope_output` setting (a string, object or queue).
    fn planner_answer<T>(&self, plan: &mut Value, idx: usize, operation: &str, prompt: &str,
        mut validate: impl FnMut(&str) -> Result<T, String>) -> Result<T, String>
    {
        let requirements = self.model_requirements("planner", None)?;
        let (result, (tool, model, effort)) = self.with_selected_model(&requirements, None, |(tool, model, effort)|
            self.run_agent(&AgentRequest { role:"planner",provider:tool,model,effort,session:None,prompt }))?;
        let expected_model = result.effective_model.clone();
        let model_reported = result.model_reported;
        self.record_stage_usage(plan, idx, "planner", &tool, result.usage)?;
        // The mock planner answers through settings, the way it answers plan
        // generation through the candidate file.
        let text = if tool == "mock" {
            self.mock_scope_response()
        } else {
            result.output
        };
        let (_, answer) = self.repair_response(operation, text, |text| validate(text), |text, error| {
            let correction = crate::response::correction_prompt(prompt, text, error);
            let result = self.run_agent(&AgentRequest { role:"planner",provider:&tool,model:&model,effort:&effort,session:None,prompt:&correction })?;
            if !result.completed || (tool != "mock" && model_reported && (!result.model_reported
                || !crate::agent::same_model(&tool, &expected_model, &result.effective_model))) {
                return Err(format!("{operation} correction model changed or did not complete"));
            }
            self.record_stage_usage(plan, idx, "planner", &tool, result.usage)?;
            if tool == "mock" {
                Ok(self.mock_scope_response())
            } else { Ok(result.output) }
        })?;
        Ok(answer)
    }

    /// Files changed since the stage attempt began: tracked edits against the
    /// attempt head plus untracked files, excluding Forge runtime data.
    pub(super) fn attempt_changed_files(&self, plan: &Value, idx: usize) -> Result<Vec<String>, String> {
        let Some(base) = plan["stages"][idx]["attempt_head"].as_str() else { return Ok(vec![]); };
        let mut files = std::collections::BTreeSet::new();
        for args in [vec!["diff", "--name-only", base, "--", ".", ":(exclude).forge"],
            vec!["ls-files", "--others", "--exclude-standard", "--", ".", ":(exclude).forge"]] {
            files.extend(self.git(&args)?.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned));
        }
        Ok(files.into_iter().collect())
    }

    /// Persist a pending constraint escalation on the stage before anything asks
    /// the planner, so a restart finds it. Returns the record's index.
    pub(super) fn begin_constraint_escalation(&self, plan: &mut Value, idx: usize, source: &str, kind: &str,
        reason: &str, statements: &[String]) -> Result<usize, String>
    {
        use crate::constraint_conflict as conflict;
        let changed = self.attempt_changed_files(plan, idx)?;
        let requests = plan["stages"][idx]["review_gate"]["requests"].as_array().cloned().unwrap_or_default();
        let signature = conflict::signature(statements, &requests);
        let inputs = conflict::stage_inputs(plan, idx, statements, &changed);
        let record = conflict::new_record(source, kind, reason, &signature, inputs, unix_timestamp())?;
        let at = conflict::push_record(&mut plan["stages"][idx], record);
        self.save_plan(plan)?;
        Ok(at)
    }

    /// Ask the planner about a pending escalation record and store its validated
    /// answer (analysis, decision, correction) on the record.
    // Only tests call this until the stage loop hands conflicts to the planner.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn consult_conflict_planner(&self, plan: &mut Value, idx: usize, at: usize)
        -> Result<crate::constraint_conflict::PlannerAnswer, String>
    {
        use crate::constraint_conflict as conflict;
        let sid = plan["stages"][idx]["id"].clone();
        let record = plan["stages"][idx][conflict::RECORDS][at].clone();
        if !record.is_object() { return Err("missing constraint escalation record".into()); }
        self.set_step(sid.as_i64(), "asking the planner about a constraint conflict");
        self.log_event("plan", &format!("stage {sid}: constraint conflict handed to the planner"));
        let prompt = conflict::prompt(plan, idx, &record);
        let snapshot = plan.clone();
        let answer = self.planner_answer(plan, idx, "constraint conflict", &prompt,
            |text| conflict::validate_answer(text, &snapshot, idx))?;
        conflict::record_answer(&mut plan["stages"][idx][conflict::RECORDS][at], &answer);
        self.save_plan(plan)?;
        Ok(answer)
    }

    /// Settle an escalation record with the given outcome. The saved plan is
    /// authoritative (a failed hand-back may leave the caller's copy stale), so
    /// the record is settled there and mirrored into the caller's copy.
    pub(super) fn settle_constraint_escalation(&self, plan: &mut Value, idx: usize, at: usize, outcome: &str,
        detail: Option<&str>) -> Result<(), String>
    {
        use crate::constraint_conflict as conflict;
        let sid = plan["stages"][idx]["id"].clone();
        let mut current = self.load_plan().ok_or("missing plan")?;
        let target = current["stages"].as_array().ok_or("invalid stages")?.iter()
            .position(|s| s["id"] == sid).ok_or("stage disappeared during constraint escalation")?;
        let revision = current["revision"].clone();
        let record = &mut current["stages"][target][conflict::RECORDS][at];
        if !record.is_object() { return Err("missing constraint escalation record".into()); }
        conflict::set_outcome(record, outcome, detail, Some(&revision), unix_timestamp())?;
        let settled = record.clone();
        self.save_plan(&current)?;
        if plan["stages"][idx][conflict::RECORDS][at].is_object() {
            plan["stages"][idx][conflict::RECORDS][at] = settled;
        }
        Ok(())
    }

    fn mock_scope_response(&self) -> String {
        let mut settings = self.app.settings.lock().unwrap();
        let value = &mut settings["mock_scope_output"];
        let answer = match value.as_array_mut() {
            Some(queue) if queue.len() > 1 => queue.remove(0),
            Some(queue) if !queue.is_empty() => queue[0].clone(),
            _ => value.clone(),
        };
        answer.as_str().map(str::to_owned).unwrap_or_else(|| answer.to_string())
    }

    /// A failed routing/guidance publication must not discard the planner's
    /// accepted revision or spend another scope negotiation after restart.
    pub(super) fn resume_scope_revision(&self, plan: &mut Value, idx: usize) -> Result<String, String> {
        let current = self.load_plan().ok_or("missing plan")?;
        if current["plan_id"] != plan["plan_id"]
            || current["stages"][idx]["id"] != plan["stages"][idx]["id"] {
            return Err("stale pending scope revision".into());
        }
        let sid = current["stages"][idx]["id"].clone();
        let pending = &current["stages"][idx]["scope_revision_pending"];
        if pending["source_inputs"] != crate::plan::stage_inputs(&current, idx) {
            return Err("pending scope revision no longer matches stage inputs; reconcile the edited stage".into());
        }
        let (instructions, acceptance) = match (pending["instructions"].as_str(), pending["acceptance"].as_str()) {
            (Some(i), Some(a)) if !i.trim().is_empty() => (i, a),
            _ => return Err("invalid pending scope revision".into()),
        };
        let changed = pending["changed"].as_str().unwrap_or("").to_owned();
        let stages: Vec<Value> = current["stages"].as_array().unwrap().iter().map(|s| {
            let mut edited = json!({"id": s["id"], "title": s["title"],
                "instructions": s["instructions"], "acceptance": s["acceptance"],
                "commit": s["commit"].as_str().unwrap_or("")});
            if s["id"] == sid {
                edited["instructions"] = json!(instructions);
                edited["acceptance"] = json!(acceptance);
            }
            if let Some(depends) = s.get("depends_on") { edited["depends_on"] = depends.clone(); }
            if !s["model_constraint"].is_null() { edited["model_constraint"] = s["model_constraint"].clone(); }
            edited
        }).collect();
        let body = json!({"plan": {"goal": current["goal"], "stages": stages}});
        let mut edited = crate::plan::edit_plan(&current, &body).map_err(str::to_string)?;
        // The user approved this plan. Narrowing one stage to something buildable
        // is not a new plan to approve, so the run keeps its approval.
        edited["status"] = current["status"].clone();
        edited["stages"][idx].as_object_mut().unwrap().remove("scope_revision_pending");
        *plan = self.architect_publish(edited, Some(&current), "stage scope renegotiation")?;
        self.log_event("plan", &format!("stage {sid} revised by the planner: {changed}"));
        Ok(changed)
    }
}
