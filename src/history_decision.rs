//! Git history changes during editing agents' turns: the engine only detects
//! them; the persistent architect decides whether to uncommit, continue or block.
use super::*;
use crate::agent::AgentResult;
use crate::util::{fill_template, json_payload_with_keys, unix_timestamp};

const ACTIONS: [&str; 3] = ["uncommit", "continue", "block"];

/// Paths of uncommitted work (staged, unstaged and untracked), excluding Forge state.
fn uncommitted_paths(ctx: &Ctx) -> Result<Vec<String>, String> {
    let mut paths: Vec<String> = ctx.git(&["diff", "--name-only", "HEAD", "--", ".", ":(exclude).forge"])?
        .lines().map(str::to_string).collect();
    paths.extend(ctx.git(&["ls-files", "--others", "--exclude-standard", "--", ".", ":(exclude).forge"])?
        .lines().map(str::to_string));
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Validate the architect's answer against what the engine can safely do.
pub(super) fn parse_history_decision(text: &str, evidence: &Value) -> Result<Value, String> {
    let answer: Value = crate::response::parse_json(json_payload_with_keys(text, &["action", "reason"]))
        .map_err(|e| format!("invalid history decision JSON: {e}"))?;
    crate::response::object_fields(&answer, &["action", "reason"])?;
    let action = answer["action"].as_str().filter(|a| ACTIONS.contains(a))
        .ok_or("action must be one of uncommit, continue or block")?;
    answer["reason"].as_str().filter(|r| !r.trim().is_empty())
        .ok_or("reason must be a non-empty string")?;
    if evidence["rewritten"] == true && action != "block" {
        return Err("history was rewritten; only block is valid".into());
    }
    Ok(answer)
}

/// The engine only keeps commits in history when they provably leave the
/// uncommitted work alone; the architect's judgment cannot override this.
pub(super) fn continue_rejection(evidence: &Value) -> Option<String> {
    if evidence["rewritten"] == true {
        Some("history was rewritten".into())
    } else if evidence["uncommitted_files"].as_array().is_none_or(Vec::is_empty) {
        Some("there is no uncommitted work, so the new commits may contain it".into())
    } else if evidence["overlap"].as_array().is_some_and(|o| !o.is_empty()) {
        Some(format!("the new commits change files with uncommitted work: {}", evidence["overlap"]))
    } else {
        None
    }
}

impl Ctx {
    /// Describe how HEAD moved away from `base`, or None when it did not.
    pub(super) fn history_change(&self, base: &str) -> Result<Option<Value>, String> {
        let head = self.git(&["rev-parse", "HEAD"])?;
        if head == base {
            return Ok(None);
        }
        let uncommitted = uncommitted_paths(self)?;
        if self.git(&["merge-base", "--is-ancestor", base, &head]).is_err() {
            return Ok(Some(json!({"base":base,"head":head,"rewritten":true,"commits":[],
                "committed_files":[],"uncommitted_files":uncommitted,"overlap":[]})));
        }
        let commits: Vec<Value> = self.git(&["log", "--reverse", "--format=%H%x1f%an%x1f%s", &format!("{base}..{head}")])?
            .lines().map(|line| {
                let mut parts = line.splitn(3, '\u{1f}');
                json!({"sha":parts.next().unwrap_or(""),"author":parts.next().unwrap_or(""),"subject":parts.next().unwrap_or("")})
            }).collect();
        let committed: Vec<String> = self.git(&["diff", "--name-only", base, &head, "--", ".", ":(exclude).forge"])?
            .lines().map(str::to_string).collect();
        let overlap: Vec<&String> = committed.iter().filter(|path| uncommitted.contains(path)).collect();
        Ok(Some(json!({"base":base,"head":head,"rewritten":false,"commits":commits,
            "committed_files":committed,"uncommitted_files":uncommitted,"overlap":overlap})))
    }

    /// Ask the architect about a detected history change, enforce the engine's
    /// limits on the answer and record the outcome. Never changes history itself.
    pub(super) fn decide_history_change(&self, plan: &mut Value, scope: ReviewScope, role: &str, evidence: &Value)
        -> Result<Value, String>
    {
        let count = evidence["commits"].as_array().map_or(0, Vec::len);
        let what = if evidence["rewritten"] == true { "rewrote git history".to_string() }
            else { format!("left {count} new commit(s)") };
        self.log_event("history", &format!("[{role}] {what}; asking the architect what to do"));
        let stage_id = match scope { ReviewScope::Stage(idx) => plan["stages"][idx]["id"].as_i64(), ReviewScope::Plan => None };
        self.set_step(stage_id, "architect deciding on a git history change");
        let mut decision = match self.architect_history_decision(plan, scope, role, evidence) {
            Ok(decision) => decision,
            Err(error) => json!({"action":"block","reason":format!("the architect could not decide: {error}")}),
        };
        if decision["action"] == "continue" {
            if let Some(rejection) = continue_rejection(evidence) {
                decision["reason"] = json!(format!("engine rejected continue: {rejection}. Architect: {}", decision["reason"].as_str().unwrap_or("")));
                decision["action"] = json!("block");
            }
        }
        decision["role"] = json!(role);
        decision["evidence"] = evidence.clone();
        decision["unix"] = json!(unix_timestamp());
        let target = match scope { ReviewScope::Stage(idx) => &mut plan["stages"][idx], ReviewScope::Plan => &mut plan["plan_review"] };
        if !target["history_decisions"].is_array() { target["history_decisions"] = json!([]); }
        target["history_decisions"].as_array_mut().unwrap().push(decision.clone());
        self.save_plan(plan)?;
        self.log_event("history", &format!("architect decided {}: {}",
            decision["action"].as_str().unwrap_or(""), decision["reason"].as_str().unwrap_or("")));
        Ok(decision)
    }

    fn architect_history_decision(&self, plan: &mut Value, scope: ReviewScope, role: &str, evidence: &Value)
        -> Result<Value, String>
    {
        let requirements = self.model_requirements("architect", None)?;
        let (decision, _) = self.with_selected_model(&requirements, None, |choice| {
            *plan = self.load_plan().ok_or("missing current plan")?;
            let cp = self.architecture_store().checkpoint(plan)?;
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
            let changed = crate::catalogue::Provider::parse(&choice.0).is_some_and(|provider| {
                let facts = self.model_facts(&policy, provider, &choice.1, Some(&choice.2));
                !crate::agent::same_model(&choice.0, facts["resolved_id"].as_str().unwrap_or(&choice.1),
                    cp["effective_model"]["model"].as_str().unwrap_or(""))
            });
            if changed || cp["session"]["provider"] != choice.0 || cp["context_status"] != "ready" {
                *plan = self.architect_publish_selected(plan.clone(), Some(plan), "history decision recovery", Some(choice.clone()))?;
            }
            self.invoke_history_decision(plan, scope, role, evidence, &choice.0, &choice.1)
        })?;
        Ok(decision)
    }

    fn invoke_history_decision(&self, plan: &mut Value, scope: ReviewScope, role: &str, evidence: &Value,
        provider: &str, model: &str) -> Result<Value, String>
    {
        let _guard = self.session.architect_lock.lock().unwrap();
        *plan = self.load_plan().ok_or("missing current plan")?;
        let mut cp = self.architecture_store().checkpoint(plan)?;
        if cp["session"]["provider"] != provider || cp["context_status"] != "ready" {
            return Err("architect session requires recovery".into());
        }
        let session = cp["session"]["reference"].as_str().map(str::to_owned);
        let (subject, stage_id) = match scope {
            ReviewScope::Stage(idx) => {
                let s = &plan["stages"][idx];
                (json!({"stage":{"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"]}}), s["id"].clone())
            }
            ReviewScope::Plan => (json!({"plan_review_fix":{"stages":plan["stages"].as_array().unwrap().iter()
                .map(|s| json!({"id":s["id"],"title":s["title"],"sha":s["sha"]})).collect::<Vec<_>>()}}), Value::Null),
        };
        let prompt = fill_template(crate::prompts::HISTORY_DECISION_PROMPT, &[
            ("{role}", role),
            ("{goal}", plan["goal"].as_str().unwrap_or("")),
            ("{subject}", &subject.to_string()),
            ("{evidence}", &evidence.to_string()),
            ("{base}", evidence["base"].as_str().unwrap_or("")),
            ("{plan_id}", plan["plan_id"].as_str().unwrap_or("")),
        ]);
        let prompt = match crate::feature_context::feature_reference(&plan["feature"]) {
            Some(reference) => format!("{prompt}\n{reference}"),
            None => prompt,
        };
        let turn = crate::architecture::identity();
        crate::durable_json::publish_pretty(
            &self.forge_path("architecture").join(plan["plan_id"].as_str().unwrap_or("")).join("architect-pending.json"),
            &json!({"turn":turn,"previous_session":cp["session"]}),
        )?;
        let snapshot = review_snapshot(self.project())?;
        let mock = provider == "mock";
        #[cfg(test)] let mock = mock || self.app.settings.lock().unwrap()["test_fake_providers"] == true;
        let invoke = |prompt: &str| -> Result<AgentResult, String> {
            let result = if mock { self.mock_history_decision(prompt, session.clone())? } else {
                let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let facts = self.model_facts(&policy, crate::catalogue::Provider::parse(provider).ok_or("invalid architect provider")?, model, None);
                let effort = facts["effort"].as_str().unwrap_or("provider_default").to_string();
                let result = self.run_agent(&AgentRequest { role: "architect_review", provider, model, effort: &effort,
                    session: session.as_deref(), prompt })?;
                let expected = facts["resolved_id"].as_str().unwrap_or(model);
                if !result.model_reported || facts["eligible"] != true || !crate::agent::same_model(provider, expected, &result.effective_model) {
                    return Err(format!("model routing blocked: architect effective model or eligibility changed (expected {expected}, reported {})", result.effective_model));
                }
                result
            };
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Err("history decision stopped".into());
            }
            if review_snapshot(self.project())? != snapshot {
                return Err("implementation or HEAD changed during the history decision".into());
            }
            if result.session.as_deref() != session.as_deref() || result.session.as_deref().is_none_or(|s| !crate::agent::session_id(s)) {
                return Err("architect session identity mismatch".into());
            }
            Ok(result)
        };
        let initial = invoke(&prompt)?;
        let mut usage: Vec<_> = initial.usage.iter().cloned().collect();
        let (_, answer) = self.repair_response("architect history decision", initial,
            |response| parse_history_decision(&response.output, evidence),
            |response, error| {
                let result = invoke(&crate::response::correction_prompt(&prompt, &response.output, error))?;
                if let Some(u) = &result.usage { usage.push(u.clone()); }
                Ok(result)
            })?;
        let action = answer["action"].as_str().unwrap();
        let reason = answer["reason"].as_str().unwrap();
        let now = unix_timestamp();
        let id = format!("HISTORY-{turn}");
        let summary = format!("Git history change during {role} work: {action}");
        let record = json!({"version":1,"id":id,"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":stage_id,
            "summary":summary,"rationale":reason,"alternatives":ACTIONS.iter().filter(|a| **a != action)
                .map(|a| json!({"description":a,"tradeoffs":"not chosen"})).collect::<Vec<_>>(),
            "status":"accepted","supersedes":null,"created_unix":now,"updated_unix":now});
        let _: crate::contracts::Decision = serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
        cp["last_turn"] = json!(turn);
        cp["session"]["checkpoint_reference"] = json!(turn);
        let recent = cp["recent_decisions"].as_array_mut().ok_or("invalid recent decisions")?;
        recent.push(json!({"id":id,"summary":summary,"status":"accepted","rationale":reason,"alternatives":record["alternatives"],"supersedes":null}));
        if recent.len() > 8 { recent.drain(..recent.len() - 8); }
        {
            let _lock = self.session.persistence_lock.lock().unwrap();
            *plan = self.architecture_store().publish(plan.clone(), cp,
                json!({"kind":"history_decision","decisions":[record],"evidence":evidence,"action":action,"turn":turn}))?;
        }
        for u in usage {
            match scope {
                ReviewScope::Stage(idx) => self.record_stage_usage(plan, idx, "architect", provider, Some(u))?,
                ReviewScope::Plan => self.record_plan_usage(plan, "architect", provider, Some(u))?,
            }
        }
        Ok(json!({"action":action,"reason":reason,"decision_id":id}))
    }

    fn mock_history_decision(&self, prompt: &str, session: Option<String>) -> Result<AgentResult, String> {
        let mut settings = self.app.settings.lock().unwrap();
        settings.as_object_mut().unwrap().entry("mock_history_prompts").or_insert(json!([]))
            .as_array_mut().unwrap().push(json!(prompt));
        let answer = settings["mock_history_decisions"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0))
            .unwrap_or(json!({"action":"block","reason":"mock architect blocks unexplained history changes"}));
        if let Some(error) = answer["error"].as_str() {
            return Err(error.into());
        }
        Ok(AgentResult { output: answer.as_str().map(String::from).unwrap_or_else(|| answer.to_string()),
            session, completed: true, ..AgentResult::default() })
    }
}
