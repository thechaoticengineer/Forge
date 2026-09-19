//! Attempt-owned reservations survive cancellation, failure and process restart.
use super::Ctx;
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Duration;
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

pub(crate) use crate::model_selection::failure_kind;
pub(crate) fn signature(kind: &str, evidence: &Value) -> String {
    crate::metadata::fingerprint(format!("{kind}:{evidence}").as_bytes())
}

pub(super) fn structured_outcome(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with(['{', '[']) || text.starts_with("```")
}

fn parse_outcome(plan: &Value, idx: usize, turn: &str, text: &str) -> Result<Option<Outcome>, String> {
    if text.trim().is_empty() { return Err("missing implementer outcome".into()); }
    // Legacy completion prose is allowed, but never interpreted as an escalation.
    if !structured_outcome(text) {
        return Ok(None);
    }
    let text = text.trim();
    let text = if let Some(fenced) = text.strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```\n"))
        .or_else(|| text.strip_prefix("```json\r\n"))
        .or_else(|| text.strip_prefix("```\r\n")) {
        fenced.trim_end().strip_suffix("```")
            .ok_or("malformed implementer outcome: unclosed JSON fence")?.trim()
    } else { text };
    let o: Outcome = crate::response::parse_json(text)
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
        if !["reasoning", "scope", "capability", crate::constraint_conflict::KIND].contains(&r.kind.as_str())
            || [&r.reason, &r.required_capability]
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 2000)
        {
            return Err("invalid implementer escalation request".into());
        }
    }
    Ok(Some(o))
}

/// Stage reassessment history entries kept in the saved plan. Older entries
/// move, complete, to the architecture event log when the plan is published.
pub(crate) const HISTORY_KEEP: usize = 4;
/// Per-publication archive batch, well inside the 512 KiB event limit.
pub(crate) const ARCHIVE_MAX_ENTRIES: usize = 32;
pub(crate) const ARCHIVE_MAX_BYTES: usize = 128 * 1024;

/// Give legacy or partially migrated stage history strictly increasing `seq`
/// values, a true `history_count` and a watermark, without reordering entries.
pub(crate) fn normalize_history(state: &mut Value) {
    if !state.is_object() {
        return;
    }
    if !state["history"].is_array() {
        state["history"] = json!([]);
    }
    let watermark = state["history_archived_through"].as_u64().unwrap_or(0);
    let len = state["history"].as_array().unwrap().len() as u64;
    let count = state["history_count"].as_u64().unwrap_or(len).max(len);
    let history = state["history"].as_array_mut().unwrap();
    // Valid positions are kept. A missing, duplicate or decreasing one follows
    // its predecessor; a leading one follows the archived or dropped prefix.
    let mut previous: Option<u64> = None;
    for entry in history.iter_mut() {
        let floor = previous.map_or(0, |p| p + 1);
        let seq = match entry["seq"].as_u64() {
            Some(seq) if seq >= floor => seq,
            _ => previous.map_or(watermark.max(count - len), |p| p + 1),
        };
        if entry.is_object() {
            entry["seq"] = json!(seq);
        }
        previous = Some(seq);
    }
    state["history_count"] = json!(count.max(previous.map_or(0, |p| p + 1)));
    state["history_archived_through"] = json!(watermark);
}

/// Append a stage reassessment history entry with the next monotonic position.
pub(crate) fn push_history(state: &mut Value, entry: Value) {
    normalize_history(state);
    let seq = state["history_count"].as_u64().unwrap();
    let mut entry = entry;
    entry["seq"] = json!(seq);
    state["history"].as_array_mut().unwrap().push(entry);
    state["history_count"] = json!(seq + 1);
}

/// Bound every stage's history for publication. Entries at or above the
/// effective watermark (the maximum of this copy's and the saved same-attempt
/// stage's) stay; entries below it were archived already and are dropped, so a
/// stale copy republishes idempotently. At most `max_entries` of the oldest
/// excess entries, within `max_bytes` (the first is always admitted so one large
/// entry cannot stall draining), are removed and returned grouped by stage.
pub(crate) fn archive_history(
    plan: &mut Value,
    saved: Option<&Value>,
    max_entries: usize,
    max_bytes: usize,
) -> Vec<Value> {
    let mut archived = Vec::new();
    let (mut entries_left, mut bytes) = (max_entries, 0usize);
    let Some(stages) = plan["stages"].as_array_mut() else {
        return archived;
    };
    for stage in stages {
        if !stage["reassessment"]["history"].is_array() {
            continue;
        }
        let (id, attempt) = (stage["id"].clone(), stage["reassessment"]["attempt_id"].clone());
        let state = &mut stage["reassessment"];
        normalize_history(state);
        // A reset attempt restarts its positions; only the same attempt's saved
        // watermark describes these entries.
        let saved_state = saved
            .and_then(|p| p["stages"].as_array())
            .and_then(|s| s.iter().find(|s| s["id"] == id))
            .map(|s| &s["reassessment"])
            .filter(|s| s["attempt_id"] == attempt);
        let mut watermark = state["history_archived_through"].as_u64().unwrap_or(0);
        if let Some(saved_state) = saved_state {
            watermark = watermark.max(saved_state["history_archived_through"].as_u64().unwrap_or(0));
            let count = state["history_count"].as_u64().unwrap_or(0)
                .max(saved_state["history_count"].as_u64().unwrap_or(0));
            state["history_count"] = json!(count);
        }
        let history = state["history"].as_array_mut().unwrap();
        history.retain(|h| h["seq"].as_u64().is_none_or(|seq| seq >= watermark));
        let mut moved = Vec::new();
        while history.len() > HISTORY_KEEP && entries_left > 0 {
            let size = serde_json::to_vec(&history[0]).map_or(usize::MAX, |b| b.len());
            let first = max_entries - entries_left == 0;
            if !first && bytes.saturating_add(size) > max_bytes {
                entries_left = 0;
                break;
            }
            let entry = history.remove(0);
            watermark = entry["seq"].as_u64().map_or(watermark, |seq| seq + 1);
            bytes = bytes.saturating_add(size);
            entries_left -= 1;
            moved.push(entry);
        }
        state["history_archived_through"] = json!(watermark);
        if !moved.is_empty() {
            archived.push(json!({"stage_id": id, "attempt_id": attempt, "entries": moved}));
        }
    }
    archived
}

impl Ctx {
    pub(super) fn validate_implementer_response(&self, plan: &Value, idx: usize, turn: &str, text: &str) -> Result<(), String> {
        parse_outcome(plan, idx, turn, text).map(|_| ())
    }

    fn reassessment_init(&self, plan: &mut Value, idx: usize) {
        if plan["stages"][idx]["reassessment"]["attempt_id"] == plan["stages"][idx]["attempt_id"]
            && plan["stages"][idx]["reassessment"].is_object()
        {
            return;
        }
        let limits = self.app.settings.lock().unwrap()["reassessment_limits"].clone();
        plan["stages"][idx]["reassessment"] = json!({"attempt_id":plan["stages"][idx]["attempt_id"],"limits":limits,
            "status":"reusing","count":0,"operational_retries":0,"signatures":[],"visited":[],"history":[],"history_count":0,"history_archived_through":0,"failures":{}});
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
    pub(crate) fn assignment_boundary(
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
            let restored = (pending["kind"] == "material_assignment_change"
                || pending["kind"] == "provider_operational_failure")
                .then(|| self.restored_assignment(plan, idx)).transpose()?;
            if let Some(agreement) = restored.filter(|a| *a == pending["old_agreement"]) {
                if crate::plan::review_cadence(&plan["stages"][idx], "reviewer") == "per_stage" {
                    self.reviewer_config(agreement["effective"]["provider"].as_str().ok_or("missing agreed provider")?)?;
                }
                let state = &mut plan["stages"][idx]["reassessment"];
                push_history(state, json!({
                    "kind":if pending["kind"] == "provider_operational_failure" {"provider_operation_resumed"} else {"material_assignment_restored"}, "reservation":pending,
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
        // Draft tier agreements never need a paid planning turn merely because
        // a provider/model is unavailable at launch. Resolve locally and retain
        // the tier contract so changing the selector can recover immediately.
        if plan["stages"][idx]["model_agreement"]["version"] == 2 {
            let selection = match self.validated_assignment(plan, idx) {
                Ok(selection) => selection,
                Err(error) if plan["stages"][idx]["model_selection"]["attempt_id"].is_string()
                    && plan["stages"][idx]["model_selection"]["attempt_id"] == plan["stages"][idx]["attempt_id"] => {
                    self.reassess(plan, idx, "material_assignment_change", json!({"local_validity":error}))?;
                    return self.validated_assignment(plan, idx);
                }
                Err(error) => return Err(error),
            };
            if plan["stages"][idx]["model_selection"] != selection {
                plan["stages"][idx]["model_selection"] = selection.clone();
                self.save_plan(plan)?;
            }
            return Ok(selection);
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
        let old = if plan["stages"][idx]["model_selection"].is_object() {
            plan["stages"][idx]["model_selection"].clone()
        } else { plan["stages"][idx]["model_agreement"].clone() };
        if old["kind"] == "selection" {
            let floor = plan["stages"][idx]["routing_scope_floor"].as_u64().unwrap_or(0)
                .max(old["policy_inputs"]["minimum_tier"].as_u64().unwrap_or(0));
            plan["stages"][idx]["routing_scope_floor"] = json!(floor);
        }
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
        self.operational_retry_for_scope(plan, Some(idx), error, role)
    }

    pub(crate) fn operational_retry_for_scope(
        &self, plan: &mut Value, idx: Option<usize>, error: &str, role: &str,
    ) -> Result<bool, String> {
        let guard = |plan: &Value| match idx {
            Some(idx) => self.boundary_guard(plan, idx),
            None => self.plan_fixer_boundary(plan),
        };
        guard(plan)?;
        let kind = failure_kind(error);
        let state = match idx {
            Some(idx) => &mut plan["stages"][idx]["reassessment"],
            None => &mut plan["plan_review"]["retries"],
        };
        let n = state["operational_retries"].as_u64().unwrap_or(0);
        if kind != "transient"
            || n >= state["limits"]["max_operational_retries"].as_u64().unwrap_or(2)
        {
            return Ok(false);
        }
        state["operational_retries"] = json!(n + 1);
        state["status"] = json!("retrying");
        let entry = json!({"kind":"operational_retry","role":role,"failure_kind":kind,"error":crate::util::last_chars(error,1000),"retry":n+1});
        if idx.is_some() {
            push_history(state, entry);
        } else {
            state["history"].as_array_mut().unwrap().push(entry);
        }
        self.save_plan(plan)?;
        let delay = 1000u64.saturating_mul(1 << n.min(4));
        #[cfg(test)]
        let delay = delay.min(5);
        for _ in 0..delay.div_ceil(25) {
            guard(plan)?;
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
            "\nENGINE OUTCOME CHANNEL: Never edit .forge state. Finish with ONLY JSON: {}. status is completed, test_failure, or escalation. evidence contains concrete checks/findings (max 16 strings, 2000 bytes each). request is null unless status=escalation, then {{\"kind\":\"reasoning|scope|capability|constraint_conflict\",\"reason\":\"specific limitation\",\"required_capability\":\"specific need\"}}. Use kind constraint_conflict when the stage's own constraints contradict each other; its reason states which constraints contradict each other and why. The engine validates identity and decides whether selection is warranted; you cannot assign models. Preserve inherited partial work.",
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
        let Some(o) = parse_outcome(plan, idx, turn, &output.output)? else { return Ok(None); };
        plan["stages"][idx]["implementer_outcome"] = json!(o);
        // Keep each round's reply so a planner hand-back sees how fixes went.
        let stage = &mut plan["stages"][idx];
        if !stage["outcome_history"].is_array() { stage["outcome_history"] = json!([]); }
        let entry = json!({"round":stage["rounds"],"attempt_id":stage["attempt_id"],"turn_id":o.turn_id,
            "status":o.status,"evidence":o.evidence,"request":o.request,"unix":unix_timestamp()});
        let history = stage["outcome_history"].as_array_mut().unwrap();
        history.push(entry);
        let excess = history.len().saturating_sub(crate::constraint_conflict::OUTCOME_HISTORY_KEEP);
        history.drain(..excess);
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
                } else if r.kind == crate::constraint_conflict::KIND {
                    crate::constraint_conflict::KIND
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
