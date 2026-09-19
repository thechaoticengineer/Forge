//! The architect synthesis turn: one short architect turn that replaces the
//! stored project synthesis (`architecture_synthesis`) from a finished plan.
//!
//! It runs when a plan is saved as done, before the queue moves on, and once
//! more at the first architect turn of the next plan for a previous plan that
//! never began one (a missed turn, or a blocked, reset or abandoned plan).
//! Plans without committed stages are never synthesized. A source plan gets at
//! most one attempt: `synthesis-state.json` records `last_attempted_plan_id`
//! durably before the provider is invoked, and a stored synthesis whose
//! `source.plan_id` is that plan also counts. The turn is advisory: every
//! failure keeps the previous synthesis byte for byte, is logged, and never
//! changes the plan, the phase, the run report or the queue.
use crate::agent::{AgentRequest, AgentResult};
use crate::app::Ctx;
use crate::architecture_synthesis as synthesis;
use crate::durable_json::publish_pretty;
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) const STATE_FILE: &str = "synthesis-state.json";
/// The visible step while the completion turn runs.
pub(crate) const STEP: &str = "synthesizing architecture";
/// Corrections of an invalid synthesis output within one turn.
const MAX_CORRECTIONS: usize = 1;

pub(crate) fn state_path(root: &Path) -> PathBuf {
    root.join("architecture").join(STATE_FILE)
}

fn last_attempted(root: &Path) -> Option<String> {
    let bytes = std::fs::read(state_path(root)).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?["last_attempted_plan_id"].as_str().map(str::to_owned)
}

/// Whether a plan has at least one committed stage.
pub(crate) fn has_committed_stage(plan: &Value) -> bool {
    plan["stages"].as_array().is_some_and(|stages| stages.iter().any(|s| s["status"] == "committed"))
}

/// The SHA the synthesis describes: the plan's last committed stage, so a
/// catch-up that runs later still points at the source plan's own work.
fn final_stage_sha(plan: &Value) -> Option<String> {
    plan["stages"].as_array()?.iter().rev()
        .filter(|s| s["status"] == "committed")
        .find_map(|s| s["sha"].as_str().filter(|sha| !sha.is_empty()).map(str::to_owned))
}

fn cut(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) { end -= 1; }
    text[..end].to_string()
}

impl Ctx {
    /// The completion trigger: runs the turn for a plan just saved as done.
    /// The caller holds neither the architect nor the persistence lock.
    pub(crate) fn synthesize_completed_plan(&self) {
        let _turn = self.session.architect_lock.lock().unwrap();
        let source = {
            let _guard = self.session.persistence_lock.lock().unwrap();
            let store = self.architecture_store();
            store.load().and_then(|plan| match plan {
                Some(plan) => store.checkpoint(&plan).map(|cp| Some((plan, cp))),
                None => Ok(None),
            })
        };
        match source {
            Ok(Some((plan, cp))) if has_committed_stage(&plan) => {
                let previous = self.session.state.lock().unwrap().current_stage;
                self.set_step(None, STEP);
                self.synthesize_once(&plan, &cp, "plan completion");
                self.set_step(previous, "");
            }
            Ok(_) => {}
            Err(error) => self.log_event("architecture", &format!("synthesis failed: {error}; previous synthesis kept")),
        }
    }

    /// The catch-up at the first architect turn of a new plan identity: the
    /// previous plan is the saved plan when it has another identity, otherwise
    /// the plan the last reset archived. The caller holds the architect lock.
    pub(crate) fn catch_up_synthesis(&self, saved: Option<&Value>, candidate: &Value) {
        let source = {
            let _guard = self.session.persistence_lock.lock().unwrap();
            let store = self.architecture_store();
            match saved {
                Some(plan) if plan["plan_id"] == candidate["plan_id"] => Ok(None),
                Some(plan) if plan.get("architecture").is_some() => store.checkpoint(plan).map(|cp| Some((plan.clone(), cp))),
                Some(_) => Ok(None),
                None => store.previous_plan(),
            }
        };
        let (plan, cp) = match source {
            Ok(Some(source)) => source,
            Ok(None) => return,
            Err(error) => {
                self.log_event("architecture", &format!("synthesis catch-up failed: {error}; previous synthesis kept"));
                return;
            }
        };
        if !has_committed_stage(&plan) || plan["plan_id"] == candidate["plan_id"] { return; }
        let (stage, step) = {
            let state = self.session.state.lock().unwrap();
            (state.current_stage, state.current_step.clone())
        };
        self.set_step(stage, STEP);
        self.synthesize_once(&plan, &cp, "catch-up before the next plan");
        self.set_step(stage, &step);
    }

    /// Runs at most one attempt for `plan` and never fails: a duplicate is
    /// skipped, and any error is logged with the previous synthesis kept.
    /// The caller holds the architect lock.
    fn synthesize_once(&self, plan: &Value, cp: &Value, reason: &str) {
        let Some(plan_id) = plan["plan_id"].as_str() else { return };
        let root = self.forge_path("");
        let prepared = {
            let _guard = self.session.persistence_lock.lock().unwrap();
            synthesis::load(&root).and_then(|old| {
                if old.as_ref().is_some_and(|doc| doc["source"]["plan_id"] == plan_id)
                    || last_attempted(&root).as_deref() == Some(plan_id) {
                    return Ok(None);
                }
                // The attempt is recorded before the provider runs, so a
                // failed or interrupted turn is never repeated.
                publish_pretty(&state_path(&root), &json!({"version": 1, "last_attempted_plan_id": plan_id, "unix": unix_timestamp()}))?;
                Ok(Some(old))
            })
        };
        let result = match prepared {
            Ok(None) => return,
            Ok(Some(old)) => {
                self.log_event("architecture", &format!("synthesizing architecture from plan {plan_id} ({reason})"));
                self.synthesis_turn(plan, cp, old.as_ref())
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(doc) => self.log_event("architecture", &format!(
                "synthesis written from plan {plan_id}: {} constraints, {} interfaces, {} decisions, {} retired",
                doc["constraints"].as_array().map_or(0, Vec::len), doc["interfaces"].as_array().map_or(0, Vec::len),
                doc["decisions"].as_array().map_or(0, Vec::len), doc["retired"].as_array().map_or(0, Vec::len))),
            Err(error) => self.log_event("architecture", &format!(
                "synthesis failed: {}; previous synthesis kept", crate::util::last_chars(&error, 1000))),
        }
    }

    /// The compact input of the turn: the old synthesis, the source plan's
    /// prompt view of its checkpoint, its goal and compact stage list, the
    /// limits and the keep-or-retire rule. Resume and fresh sessions get the
    /// same prompt.
    fn synthesis_prompt(&self, plan: &Value, cp: &Value, old: Option<&Value>) -> String {
        let stages: Vec<Value> = plan["stages"].as_array().into_iter().flatten()
            .map(|s| json!({"id": s["id"], "title": s["title"], "status": s["status"], "sha": s["sha"]})).collect();
        let events = plan["plan_id"].as_str()
            .map(|id| self.forge_path("architecture").join(id).join("events.jsonl"));
        let previous = old.map(|doc| json!({"source": doc["source"], "constraints": doc["constraints"],
            "interfaces": doc["interfaces"], "decisions": doc["decisions"]}));
        let context = json!({
            "plan": {"plan_id": plan["plan_id"], "goal": plan["goal"], "status": plan["status"], "stages": stages},
            "checkpoint": crate::prompt_view::checkpoint_for(cp, plan),
            "previous_synthesis": previous,
            "limits": {"max_bytes": synthesis::MAX_BYTES, "constraints": synthesis::MAX_CONSTRAINTS,
                "interfaces": synthesis::MAX_INTERFACES, "decisions": synthesis::MAX_DECISIONS,
                "entry_bytes": synthesis::MAX_ENTRY_BYTES},
            "paths": {"architecture_events": events, "repository": self.project()},
        });
        format!("You are this project's architect, taking one short synthesis turn after plan {} finished. \
            You MUST NOT implement, write files, commit or push. Record what holds across plans: the constraints, \
            interfaces and decisions later plans must respect, not a history of this plan. Context carries only \
            a compact view; whenever you need detail, inspect paths.repository yourself (for example `git show <sha>` \
            for a stage) and read paths.architecture_events for the full decisions and records. \
            Keep-or-retire rule: every entry of context.previous_synthesis MUST either be restated with byte-identical \
            text in the same array, or be retired by its id with a reason; a changed text counts as a new entry and \
            its old id must be retired. Merge or retire entries so the result fits context.limits: at most \
            {} constraints, {} interfaces and {} decisions, each at most {} bytes, and at most {} bytes in total. \
            The engine derives every ID from the text; never add IDs to new entries. \
            Return ONLY JSON, no fences, with this exact shape:\n\
            {{\"constraints\":[\"...\"],\"interfaces\":[\"...\"],\"decisions\":[\"...\"],\"retired\":[{{\"id\":\"c-...\",\"reason\":\"why it no longer holds\"}}]}}\n\
            Context:\n{context}",
            plan["plan_id"], synthesis::MAX_CONSTRAINTS, synthesis::MAX_INTERFACES, synthesis::MAX_DECISIONS,
            synthesis::MAX_ENTRY_BYTES, synthesis::MAX_BYTES)
    }

    /// Whether the source plan's architect session can be resumed with the
    /// architect bootstrap choice.
    fn resumable_session(&self, cp: &Value, choice: &crate::model_selection::ModelChoice) -> Option<String> {
        let (provider, model, effort) = choice;
        let session = &cp["session"];
        if session["provider"] != *provider || cp["context_status"] != "ready"
            || session["resume_policy"] == "fork_from_checkpoint" {
            return None;
        }
        if let (Some(id), Some(old)) = (crate::catalogue::Provider::parse(provider), cp["effective_model"]["model"].as_str()) {
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap()).ok()?;
            let facts = self.model_facts(&policy, id, model, Some(effort));
            if !crate::agent::same_model(provider, facts["resolved_id"].as_str().unwrap_or(model), old) { return None; }
        }
        session["reference"].as_str().filter(|s| crate::agent::session_id(s)).map(str::to_owned)
    }

    /// One provider turn, validated through the synthesis builder and saved
    /// under the persistence lock. Resumes the source plan's architect session
    /// when it is valid; a resume that fails falls back to exactly one fresh
    /// session with the same input.
    fn synthesis_turn(&self, plan: &Value, cp: &Value, old: Option<&Value>) -> Result<Value, String> {
        let prompt = self.synthesis_prompt(plan, cp, old);
        let head = self.git(&["rev-parse", "HEAD"]);
        let sha = final_stage_sha(plan).map_or(head, Ok)?;
        let source = json!({"plan_id": plan["plan_id"], "checkpoint": plan["architecture"]["checkpoint"], "sha": sha,
            "goal": cut(plan["goal"].as_str().unwrap_or(""), synthesis::MAX_ENTRY_BYTES), "unix": unix_timestamp()});
        let initial = self.bootstrap("architect")?;
        let invoke = |choice: &crate::model_selection::ModelChoice, session: Option<&str>, prompt: &str| -> Result<AgentResult, String> {
            let (provider, model, effort) = choice;
            let result = if provider == "mock" {
                self.mock_synthesis(plan, old, &source, session, prompt)?
            } else {
                self.run_agent(&AgentRequest { role: "architect", provider, model, effort, session, prompt })?
            };
            if !result.completed { return Err("architect synthesis turn did not complete".into()); }
            Ok(result)
        };
        let resumed = self.resumable_session(cp, &initial).and_then(|session| {
            match invoke(&initial, Some(&session), &prompt) {
                Ok(result) => Some((result, initial.clone())),
                Err(error) => {
                    self.log_event("architecture", &format!("synthesis could not resume the architect session: {}; using a fresh session",
                        crate::util::last_chars(&error, 500)));
                    None
                }
            }
        });
        let (result, choice) = match resumed {
            Some(resumed) => resumed,
            None => {
                let requirements = self.model_requirements("architect", None)?;
                self.with_selected_model(&requirements, Some(initial), |choice| invoke(choice, None, &prompt))?
            }
        };
        let mut usage: Vec<_> = result.usage.iter().cloned().collect();
        let mut corrections = 0;
        let (_, doc) = self.repair_response("architecture synthesis", result,
            |reply| {
                let output: Value = crate::response::parse_json(&reply.output).map_err(|e| format!("invalid synthesis output: {e}"))?;
                synthesis::build(&output, old, source.clone())
            },
            |reply, error| {
                corrections += 1;
                if corrections > MAX_CORRECTIONS { return Err(error.to_string()); }
                let correction = crate::response::correction_prompt(&prompt, &reply.output, error);
                let repaired = invoke(&choice, reply.session.as_deref(), &correction)?;
                if let Some(u) = &repaired.usage { usage.push(u.clone()); }
                Ok(repaired)
            })?;
        self.save_project_synthesis(&doc)?;
        let (input, output) = usage.iter().fold((0, 0), |(i, o), u| (i + u.input_tokens, o + u.output_tokens));
        if !usage.is_empty() {
            self.log_event("architecture", &format!("synthesis usage: {} calls, {input} input and {output} output tokens", usage.len()));
        }
        Ok(doc)
    }

    /// The mock architect's synthesis turn. Requests are recorded in
    /// `mock_synthesis_requests` with the visible step; `mock_synthesis_errors` scripts failures and
    /// `mock_synthesis_output` outputs (one value, or an array consumed in
    /// order; null selects the default). The default keeps every old entry,
    /// adds one constraint from the plan goal, and retires the oldest entries
    /// when a limit would be exceeded.
    fn mock_synthesis(&self, plan: &Value, old: Option<&Value>, source: &Value, session: Option<&str>, prompt: &str)
        -> Result<AgentResult, String>
    {
        #[cfg(test)]
        self.record_prompt("architect", crate::prompt_capture::ARCHITECTURE_SYNTHESIS, None, prompt);
        let scripted = {
            let mut settings = self.app.settings.lock().unwrap();
            if !settings["mock_synthesis_requests"].is_array() { settings["mock_synthesis_requests"] = json!([]); }
            settings["mock_synthesis_requests"].as_array_mut().unwrap()
                .push(json!({"plan_id": plan["plan_id"], "session": session, "prompt": prompt,
                    "step": self.session.state.lock().unwrap().current_step}));
            if let Some(errors) = settings["mock_synthesis_errors"].as_array_mut().filter(|e| !e.is_empty())
                && let Some(error) = errors.remove(0).as_str() {
                return Err(error.into());
            }
            match settings.get_mut("mock_synthesis_output") {
                Some(Value::Array(queue)) => (!queue.is_empty()).then(|| queue.remove(0)),
                other => other.cloned(),
            }
        };
        let output = match scripted.filter(|v| !v.is_null()) {
            Some(Value::String(raw)) => raw,
            Some(value) => value.to_string(),
            None => default_mock_output(plan, old, source).to_string(),
        };
        let reference = session.map(str::to_owned)
            .unwrap_or_else(|| crate::architect::mock_session_uuid(&crate::architecture::identity()));
        Ok(AgentResult { output, session: Some(reference), effective_model: "mock-architect".into(), completed: true,
            ..AgentResult::default() })
    }
}

/// Keeps every old entry and adds one goal constraint, retiring the oldest
/// constraints until the count and byte limits hold.
fn default_mock_output(plan: &Value, old: Option<&Value>, source: &Value) -> Value {
    let texts = |kind: &str| -> Vec<(String, String)> {
        old.and_then(|doc| doc[kind].as_array()).into_iter().flatten()
            .filter_map(|e| Some((e["id"].as_str()?.to_owned(), e["text"].as_str()?.to_owned()))).collect()
    };
    let mut constraints = texts("constraints");
    let added = cut(&format!("Delivered goal: {}", plan["goal"].as_str().unwrap_or("")), synthesis::MAX_ENTRY_BYTES);
    if !constraints.iter().any(|(_, text)| *text == added) { constraints.push((String::new(), added)); }
    let mut retired: Vec<Value> = vec![];
    loop {
        let output = json!({
            "constraints": constraints.iter().map(|(_, t)| t).collect::<Vec<_>>(),
            "interfaces": texts("interfaces").into_iter().map(|(_, t)| t).collect::<Vec<_>>(),
            "decisions": texts("decisions").into_iter().map(|(_, t)| t).collect::<Vec<_>>(),
            "retired": retired,
        });
        let fits = constraints.len() <= synthesis::MAX_CONSTRAINTS
            && synthesis::build(&output, old, source.clone()).is_ok();
        if fits || constraints.len() <= 1 { return output; }
        let (id, _) = constraints.remove(0);
        if !id.is_empty() { retired.push(json!({"id": id, "reason": "merged into newer entries to fit the synthesis limits"})); }
    }
}

#[cfg(test)]
#[path = "synthesis_tests.rs"]
mod tests;
