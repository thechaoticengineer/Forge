//! Attempt-owned reservations survive cancellation, failure and process restart.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    version: u8,
    plan_id: String,
    stage_id: i64,
    attempt_id: String,
    turn_id: String,
    status: String,
    evidence: Vec<String>,
    request: Option<Escalation>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Escalation {
    kind: String,
    reason: String,
    required_capability: String,
}

pub(crate) fn failure_kind(error: &str) -> &'static str {
    let e = error.to_lowercase();
    if crate::catalogue::auth_error(&e) {
        "auth"
    } else if [
        "rate limit",
        "rate_limit",
        "429",
        "overloaded",
        "503",
        "timeout",
        "temporarily unavailable",
    ]
    .iter()
    .any(|s| e.contains(s))
    {
        "transient"
    } else {
        "tool_process"
    }
}
fn signature(kind: &str, evidence: &Value) -> String {
    crate::metadata::fingerprint(format!("{kind}:{evidence}").as_bytes())
}

impl Ctx {
    fn reassessment_init(&self, plan: &mut Value, idx: usize) {
        if plan["stages"][idx]["reassessment"]["attempt_id"] == plan["stages"][idx]["attempt_id"]
            && plan["stages"][idx]["reassessment"].is_object()
        {
            return;
        }
        let limits = self.app.settings.lock().unwrap()["reassessment_limits"].clone();
        plan["stages"][idx]["reassessment"] = json!({"attempt_id":plan["stages"][idx]["attempt_id"],"limits":limits,
            "status":"reusing","count":0,"operational_retries":0,"signatures":[],"visited":[],"history":[],"failures":{}});
    }
    fn boundary_guard(&self, plan: &Value, idx: usize) -> Result<(), String> {
        if self.session.stop_requested.load(Ordering::SeqCst) {
            return Err("stopped at model boundary; saved work retained".into());
        }
        let current = self.load_plan().ok_or("missing plan at model boundary")?;
        if current["plan_id"] != plan["plan_id"]
            || current["revision"] != plan["revision"]
            || current["stages"][idx]["attempt_id"] != plan["stages"][idx]["attempt_id"]
        {
            return Err("project/plan/attempt changed at model boundary".into());
        }
        Ok(())
    }
    /// Only cheap local checks on unchanged boundaries. No catalogue refresh or paid selection.
    pub(super) fn assignment_boundary(
        &self,
        plan: &mut Value,
        idx: usize,
    ) -> Result<Value, String> {
        self.boundary_guard(plan, idx)?;
        self.reassessment_init(plan, idx);
        if let Some(error) = plan["stages"][idx]["model_block"].as_str() {
            return Err(error.into());
        }
        if plan["stages"][idx]["reassessment"]["pending"].is_object() {
            let pending = plan["stages"][idx]["reassessment"]["pending"].clone();
            let restored = (pending["kind"] == "material_assignment_change")
                .then(|| self.restored_assignment(plan, idx)).transpose()?;
            if let Some(agreement) = restored.filter(|a| *a == pending["old_agreement"]) {
                self.reviewer_config(agreement["effective"]["provider"].as_str().ok_or("missing agreed provider")?)?;
                let state = &mut plan["stages"][idx]["reassessment"];
                state["history"].as_array_mut().unwrap().push(json!({
                    "kind":"material_assignment_restored", "reservation":pending,
                    "agreement_id":agreement["id"], "unix":unix_timestamp()
                }));
                state.as_object_mut().unwrap().remove("pending");
                state.as_object_mut().unwrap().remove("error");
                state["status"] = json!("reusing");
                // Keep count, signatures and review rounds: recovery refunds no budget.
                self.save_plan(plan)?;
            } else {
                return Err("model routing blocked: interrupted selection reservation; revise scope/constraints to recover or inspect saved evidence".into());
            }
        }
        match self.validated_assignment(plan, idx) {
            Ok(a) => Ok(a),
            Err(error) => {
                self.reassess(
                    plan,
                    idx,
                    "material_assignment_change",
                    json!({"local_validity":error}),
                )?;
                self.validated_assignment(plan, idx)
            }
        }
    }
    pub(super) fn reassess(
        &self,
        plan: &mut Value,
        idx: usize,
        kind: &str,
        evidence: Value,
    ) -> Result<(), String> {
        self.boundary_guard(plan, idx)?;
        self.reassessment_init(plan, idx);
        let old = plan["stages"][idx]["model_agreement"].clone();
        let sig = signature(kind, &evidence);
        let state = &mut plan["stages"][idx]["reassessment"];
        let count = state["count"].as_u64().unwrap_or(0);
        if count >= state["limits"]["max_reassessments"].as_u64().unwrap_or(3)
            || state["signatures"]
                .as_array()
                .unwrap()
                .contains(&json!(sig))
        {
            return Err("model routing blocked: reassessment/signature budget exhausted; partial work and checkpoint retained".into());
        }
        state["count"] = json!(count + 1);
        state["signatures"].as_array_mut().unwrap().push(json!(sig));
        if !state["visited"]
            .as_array()
            .unwrap()
            .contains(&old["effective"])
        {
            state["visited"]
                .as_array_mut()
                .unwrap()
                .push(old["effective"].clone());
        }
        state["status"] = json!("escalating");
        state["pending"] = json!({"kind":kind,"evidence":evidence,"signature":sig,"old_agreement":old,"unix":unix_timestamp()});
        // Reserve before any selection turn, including unsuccessful dialogues.
        self.save_plan(plan)?;
        if kind == "provider_operational_failure" && !self.has_operational_alternative(plan, idx)? {
            return Err(format!(
                "model routing blocked: no adequate constrained alternative with an eligible independent reviewer; repair provider authentication/availability or revise constraints. Evidence: {evidence}"
            ));
        }
        let mut current = self.load_plan().ok_or("missing selection reservation")?;
        if kind == "material_scope_change" {
            let mut cp = self.architecture_store().checkpoint(&current)?;
            cp["guidance"][current["stages"][idx]["id"].to_string()]["valid"] = json!(false);
            cp["context_gap"] = json!({"role":"implementer","evidence":evidence});
            current = self.architecture_store().publish(
                current,
                cp,
                json!({"kind":"material_scope_change","evidence":evidence}),
            )?;
        }
        match self.architect_publish(
            current.clone(),
            Some(&current),
            "concrete model reassessment",
        ) {
            Ok(next) => {
                *plan = next;
                self.boundary_guard(plan, idx)?;
                Ok(())
            }
            Err(error) => {
                *plan = self.load_plan().unwrap_or(current);
                plan["stages"][idx]["reassessment"]["status"] = json!("blocked");
                plan["stages"][idx]["reassessment"]["error"] = json!(error);
                self.save_plan(plan)?;
                Err(format!("model routing blocked: {error}"))
            }
        }
    }
    /// Reserve retries before waiting; auth never incurs another paid retry.
    pub(super) fn operational_retry(
        &self,
        plan: &mut Value,
        idx: usize,
        error: &str,
        role: &str,
    ) -> Result<bool, String> {
        self.boundary_guard(plan, idx)?;
        self.reassessment_init(plan, idx);
        let kind = failure_kind(error);
        let state = &mut plan["stages"][idx]["reassessment"];
        let n = state["operational_retries"].as_u64().unwrap_or(0);
        if kind != "transient"
            || n >= state["limits"]["max_operational_retries"]
                .as_u64()
                .unwrap_or(2)
        {
            return Ok(false);
        }
        state["operational_retries"] = json!(n + 1);
        state["status"] = json!("retrying");
        state["history"].as_array_mut().unwrap().push(json!({"kind":"operational_retry","role":role,"failure_kind":kind,"error":crate::util::last_chars(error,1000),"retry":n+1}));
        self.save_plan(plan)?;
        let delay = 1000u64.saturating_mul(1 << n.min(4));
        #[cfg(test)]
        let delay = delay.min(5);
        for _ in 0..delay.div_ceil(25) {
            self.boundary_guard(plan, idx)?;
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(true)
    }
    pub(super) fn repeated_findings(
        &self,
        plan: &mut Value,
        idx: usize,
        requests: &Value,
    ) -> Result<bool, String> {
        self.reassessment_init(plan, idx);
        let mut trigger = vec![];
        let state = &mut plan["stages"][idx]["reassessment"];
        let mut next = json!({});
        for r in requests.as_array().into_iter().flatten() {
            let text = r
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let sig = signature("finding", &json!(text));
            let count = state["failures"][&sig].as_u64().unwrap_or(0) + 1;
            next[&sig] = json!(count);
            if count >= state["limits"]["repeat_threshold"].as_u64().unwrap_or(2) {
                trigger.push(r.clone());
            }
        }
        state["failures"] = next;
        self.save_plan(plan)?;
        if trigger.is_empty() {
            return Ok(false);
        }
        self.reassess(plan, idx, "repeated_reasoning_failure", json!(trigger))?;
        Ok(true)
    }
    pub(super) fn outcome_prompt(&self, plan: &Value, idx: usize, turn: &str) -> String {
        format!(
            "\nENGINE OUTCOME CHANNEL: Never edit .forge state. Finish with ONLY JSON: {}. status is completed, test_failure, or escalation. evidence contains concrete checks/findings (max 16 strings, 2000 bytes each). request is null unless status=escalation, then {{\"kind\":\"reasoning|scope|capability\",\"reason\":\"specific limitation\",\"required_capability\":\"specific need\"}}. The engine validates identity and decides whether selection is warranted; you cannot assign models. Preserve inherited partial work.",
            json!({"version":1,"plan_id":plan["plan_id"],"stage_id":plan["stages"][idx]["id"],"attempt_id":plan["stages"][idx]["attempt_id"],"turn_id":turn,"status":"completed","evidence":[],"request":null})
        )
    }
    pub(super) fn implementer_outcome(
        &self,
        plan: &mut Value,
        idx: usize,
        turn: &str,
        output: &crate::agent::AgentResult,
    ) -> Result<Option<(String, Value)>, String> {
        // Legacy completion prose is allowed, but never interpreted as an escalation.
        if !output.output.trim_start().starts_with('{') {
            return Ok(None);
        }
        let o: Outcome = serde_json::from_str(&output.output)
            .map_err(|e| format!("invalid implementer outcome: {e}"))?;
        if o.version != 1
            || json!(o.plan_id) != plan["plan_id"]
            || json!(o.stage_id) != plan["stages"][idx]["id"]
            || json!(o.attempt_id) != plan["stages"][idx]["attempt_id"]
            || o.turn_id != turn
            || !["completed", "test_failure", "escalation"].contains(&o.status.as_str())
            || o.evidence.len() > 16
            || o.evidence
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 2000)
            || (o.status == "escalation") != o.request.is_some()
        {
            return Err("invalid implementer outcome identity or fields".into());
        }
        if o.status != "completed" && o.evidence.is_empty() {
            return Err("implementer failure/escalation requires evidence".into());
        }
        if let Some(r) = &o.request {
            if !["reasoning", "scope", "capability"].contains(&r.kind.as_str())
                || [&r.reason, &r.required_capability]
                    .iter()
                    .any(|s| s.trim().is_empty() || s.len() > 2000)
            {
                return Err("invalid implementer escalation request".into());
            }
        }
        plan["stages"][idx]["implementer_outcome"] = json!(o);
        self.save_plan(plan)?;
        if o.status == "test_failure" {
            self.reassessment_init(plan, idx);
            let sig = signature("test_failure", &json!(o.evidence));
            let state = &mut plan["stages"][idx]["reassessment"];
            let count = if state["test_signature"] == sig {
                state["test_count"].as_u64().unwrap_or(0) + 1
            } else {
                1
            };
            state["test_signature"] = json!(sig);
            state["test_count"] = json!(count);
            let triggered = count >= state["limits"]["repeat_threshold"].as_u64().unwrap_or(2);
            self.save_plan(plan)?;
            if triggered {
                return Ok(Some((
                    "repeated_reasoning_failure".into(),
                    json!({"role":"implementer","tests":o.evidence}),
                )));
            }
        } else {
            plan["stages"][idx]["reassessment"]["test_count"] = json!(0);
            self.save_plan(plan)?;
        }
        Ok(o.request.as_ref().map(|r| {
            (
                if r.kind == "scope" {
                    "material_scope_change"
                } else {
                    "implementer_escalation"
                }
                .into(),
                json!(o),
            )
        }))
    }
}

#[cfg(test)]
#[path = "reassessment_tests.rs"]
mod tests;
