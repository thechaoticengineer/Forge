//! Plan generation, revision, and plan-question chat workflows.
use super::{Ctx, FORGE_DIR, PlanMode, WorkerGuard};
use crate::agent::AgentRequest;
use crate::prompts::{CHAT_PROMPT, PLANNER_PROMPT, REFACTOR_PROMPT, REVISE_PROMPT};
use crate::usage::accumulate_invocation_usage;
use crate::util::{fill_template, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;

impl Ctx {
    pub(crate) fn plan_worker(&self, goal: &str, mode: &PlanMode) {
        let _worker = WorkerGuard(&self.session);
        self.plan_with_busy_claim(goal, mode);
    }

    /// The caller owns the session busy claim and its WorkerGuard.
    /// This method must not construct WorkerGuard or release the busy claim.
    pub(super) fn plan_with_busy_claim(&self, goal: &str, mode: &PlanMode) -> bool {
        let prompt = match mode {
            PlanMode::Standard => PLANNER_PROMPT
                .replace("{goal}", goal)
                .replace("{plan_path}", &format!("{FORGE_DIR}/plan-candidate.json")),
            PlanMode::Refactor { focus } => fill_template(REFACTOR_PROMPT, &[
                ("{focus}", focus.as_str()), ("{plan_path}", ".forge/plan-candidate.json"),
            ]),
        };
        match self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, None, "ready"))
        {
            Ok(()) => true,
            Err(e) => {
                let blocked = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
                self.set_phase(if blocked { "blocked" } else { "failed" });
                self.log_event("error", &format!("planning failed: {e}"));
                false
            }
        }
    }

    pub(crate) fn revise_worker(&self, current_plan: &Value, feedback: &str) {
        let _worker = WorkerGuard(&self.session);
        let authoritative = self.load_plan().unwrap_or_else(|| current_plan.clone());
        let current_plan = &authoritative;
        let snapshot = serde_json::to_string_pretty(current_plan).unwrap();
        let goal = current_plan["goal"].as_str().unwrap_or("");
        let prompt = fill_template(REVISE_PROMPT, &[
            ("{current_plan}", snapshot.as_str()), ("{feedback}", feedback),
            ("{goal}", goal), ("{plan_path}", ".forge/plan-candidate.json"),
        ]);
        let result = self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, Some(current_plan), "revised"));
        if let Err(error) = result {
            let architecture_failed = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
            self.set_phase(if architecture_failed { "blocked" } else { "plan_ready" });
            self.log_event("error", &format!("revision failed: {error}"));
        }
    }

    pub(crate) fn chat_worker(&self, current_plan: &Value, question: &str) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "answering plan question");
        let snapshot = serde_json::to_string_pretty(current_plan).unwrap();
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
            let tool = self.setting("planner");
            let model = self.setting("planner_model");
            let context = if current_plan.get("architecture").is_some() { self.architecture_store().checkpoint(current_plan)? } else { crate::architecture::checkpoint_default() };
            let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: read-only Q&A in a fresh conversation. Do not write files or alter plan/decisions. Return ONLY {{\"answer\":\"your answer\"}}. Saved architecture context: {context}");
            let result = self.run_agent(&AgentRequest { role:"chat", provider:&tool, model:&model, effort:"provider_default", session:None, prompt:if tool == "mock" {&prompt} else {&readonly_prompt} })?;
            let text = if tool == "mock" { fs::read_to_string(self.forge_path("answer.json")).map_err(|e| format!("could not read answer file: {e}"))? } else { result.output };
            let output: Value = serde_json::from_str(&text)
                .map_err(|e| format!("invalid answer JSON: {e}"))?;
            let answer = output["answer"].as_str().filter(|answer| !answer.trim().is_empty())
                .ok_or_else(|| "agent did not produce a non-empty answer string".to_string())?;
            crate::architecture::atomic_json(&self.forge_path("answer.json"), &output)?;
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

    fn generate_plan(&self, prompt: &str) -> Result<Value, String> {
        let (tool, model, effort) = self.bootstrap("planner")?;
        self.ensure_forge_dir();
        let _ = fs::remove_file(self.forge_path("chat.jsonl"));
        let path = self.forge_path("plan-candidate.json");
        match fs::remove_file(&path) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not remove previous candidate: {e}")),
        }
        let prompt = format!("{prompt}\n{}", self.routing_prompt()?);
        let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: inspect only; do not write any files, implement, commit or push. Return the complete candidate plan as a JSON object in your final response, without markdown fences. Forge validates and writes the candidate file itself.");
        let result = self.run_agent(&AgentRequest { role:"planner", provider:&tool, model:&model, effort:&effort, session:None, prompt: if tool == "mock" {&prompt} else {&readonly_prompt} })?;
        let mut plan: Value = if tool == "mock" {
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| format!("invalid candidate: {e}"))?
        } else { serde_json::from_str(&result.output).map_err(|e| format!("invalid candidate: {e}"))? };
        plan["planner_selection_actor"] = json!({"provider":tool,"model":result.effective_model,"native_effort":effort});
        let usage = result.usage;
        if !plan["stages"].is_array() { return Err("planner did not produce valid stages".into()); }
        // Agent-authored usage may be an echo of the previous revision. Only
        // the invocation result contributes new planner usage.
        plan.as_object_mut().unwrap().remove("planner_usage");
        plan.as_object_mut().unwrap().remove("role_usage");
        if let Some(usage) = usage { accumulate_invocation_usage(&mut plan, "planner_usage", &tool, &usage); }
        if !plan["planner_usage"].is_null() { plan["role_usage"] = json!({"planner":plan["planner_usage"]}); }
        crate::architecture::atomic_json(&path, &plan)?;
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

    fn finalize_plan(&self, plan: Value, goal: &str, previous: Option<&Value>, action: &str)
        -> Result<(), String>
    {
        let (mut plan, n) = crate::candidate_draft::prepare_candidate_draft(plan, goal, previous)?;
        self.bind_planner_proposals(&mut plan);
        if let Some(previous) = previous {
            self.architect_publish(plan, Some(previous), "draft revision")?;
        } else {
            self.architect_publish(plan, None, "initial plan guidance")?;
        }
        self.set_phase("plan_ready");
        self.log_event("plan", &format!("plan {action} with {n} stages"));
        Ok(())
    }
}
