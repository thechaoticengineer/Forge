//! Plan-owned architect turns. Provider session tails are usable only after publication.
use crate::agent::{AgentRequest, AgentResult};
use crate::app::Ctx;
use crate::architecture::{checkpoint_default, identity};
use crate::catalogue::{Policy, Provider};
use crate::durable_json::publish_pretty;
use crate::util::unix_timestamp;
use crate::usage::accumulate_invocation_usage;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    summary: String,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    completed_interfaces: Vec<String>,
}
/// An explicit retirement is the only way a saved constraint may disappear. A
/// revision that reorders stages can make a stage-numbered constraint false, and
/// without this the architect can neither drop it nor stop reporting the conflict.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retirement {
    text: String,
    reason: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Risk {
    id: String,
    text: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Guidance {
    stage_id: i64,
    text: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    id: String,
    stage_id: Option<i64>,
    summary: String,
    rationale: String,
    alternatives: Vec<crate::contracts::Alternative>,
    supersedes: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    // The engine knows the contract version and the prompt asks the agent to
    // echo it; an omitted echo is a slip, not a different contract. Collection
    // fields are documented as allowed to be empty, so an omitted key means
    // empty rather than a discarded turn. plan_id and revision stay required:
    // they are the proof that the turn addressed the current plan.
    #[serde(default = "contract_version")]
    version: u64,
    plan_id: String,
    revision: u64,
    checkpoint: Checkpoint,
    #[serde(default)]
    decisions: Vec<Decision>,
    #[serde(default)]
    guidance: Vec<Guidance>,
    #[serde(default)]
    unresolved_risks: Vec<Risk>,
    #[serde(default)]
    resolved_risks: Vec<String>,
    #[serde(default)]
    retired_constraints: Vec<Retirement>,
    #[serde(default)]
    model_evaluations: Value,
}
fn contract_version() -> u64 { crate::architecture::VERSION }
fn text_ok(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max
}
fn safe_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Validate the entire turn before preparing any persistent mutation.
fn apply_turn(
    plan: &Value,
    cp: &Value,
    output: &str,
    decisions: &BTreeMap<String, Value>,
    required: &[i64],
) -> Result<(Value, Vec<Value>), String> {
    if output.len() > 48 * 1024 {
        return Err("architect output exceeds 48 KiB".into());
    }
    let t: Turn =
        crate::response::parse_json(output).map_err(|e| format!("invalid architect output: {e}"))?;
    let _ = &t.model_evaluations;
    if t.version != 1 || t.plan_id != plan["plan_id"] || t.revision != plan["revision"] {
        return Err("stale architect plan/revision".into());
    }
    if !text_ok(&t.checkpoint.summary, 8000)
        || t.checkpoint.constraints.len() > 64
        || t.checkpoint.completed_interfaces.len() > 64
        || t.decisions.len() > 16
        || t.guidance.len() > 64
        || t.unresolved_risks.len() > 64
        || t.resolved_risks.len() > 64
        || t.retired_constraints.len() > 64
        || t.checkpoint
            .constraints
            .iter()
            .chain(&t.checkpoint.completed_interfaces)
            .any(|s| !text_ok(s, 1000))
    {
        return Err("invalid or oversized architect checkpoint".into());
    }
    // A retirement must name a saved constraint, give a reason, be unique, and
    // actually remove it: retiring a constraint the turn still proposes would
    // record a decision that changed nothing.
    let saved_constraints = strings(&cp["constraints"]);
    let mut retired = BTreeSet::new();
    for r in &t.retired_constraints {
        if !text_ok(&r.text, 1000)
            || !text_ok(&r.reason, 1000)
            || !saved_constraints.contains(&r.text)
            || t.checkpoint.constraints.contains(&r.text)
            || !retired.insert(r.text.clone())
        {
            return Err("invalid retired constraint".into());
        }
    }
    if saved_constraints.iter().any(|s| !t.checkpoint.constraints.contains(s) && !retired.contains(s)) {
        return Err("architect dropped saved constraints without explicit retirement".into());
    }
    if strings(&cp["completed_interfaces"]).iter().any(|s| !t.checkpoint.completed_interfaces.contains(s)) {
        return Err("architect dropped saved completed_interfaces".into());
    }
    let mut seen = BTreeSet::new();
    for r in &t.unresolved_risks {
        if !safe_key(&r.id) || !text_ok(&r.text, 1000) || !seen.insert(r.id.clone()) {
            return Err("invalid unresolved risk".into());
        }
    }
    let old_risks = cp["unresolved_risks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut resolved = BTreeSet::new();
    for id in &t.resolved_risks {
        if !resolved.insert(id) || seen.contains(id) || !old_risks.iter().any(|r| r["id"] == *id) {
            return Err("invalid resolved risk".into());
        }
    }
    for r in &old_risks {
        let id = r["id"].as_str().ok_or("invalid saved risk")?;
        if !seen.contains(id) && !t.resolved_risks.iter().any(|r| r == id) {
            return Err("architect dropped unresolved risk without explicit resolution".into());
        }
    }
    let mut next = cp.clone();
    next["summary"] = json!(t.checkpoint.summary);
    next["constraints"] = json!(t.checkpoint.constraints);
    next["completed_interfaces"] = json!(t.checkpoint.completed_interfaces);
    next["unresolved_risks"] = json!(
        t.unresolved_risks
            .iter()
            .map(|r| json!({"id":r.id,"text":r.text}))
            .collect::<Vec<_>>()
    );
    let mut records = vec![];
    let mut ids = BTreeSet::new();
    let mut superseded = BTreeSet::new();
    for d in t.decisions {
        if !safe_key(&d.id)
            || decisions.contains_key(&d.id)
            || !ids.insert(d.id.clone())
            || !text_ok(&d.summary, 1000)
            || !text_ok(&d.rationale, 4000)
            || d.alternatives.len() > 8
            || d.alternatives
                .iter()
                .any(|a| !text_ok(&a.description, 1000) || !text_ok(&a.tradeoffs, 1000))
            || d.stage_id.is_some_and(|id| {
                !plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["id"] == id)
            })
        {
            return Err("invalid proposed architect decision".into());
        }
        if let Some(id) = &d.supersedes {
            if !decisions.contains_key(id)
                || decisions[id]["status"] == "superseded"
                || !superseded.insert(id.clone())
            {
                return Err("invalid decision supersession".into());
            }
        }
        let now = unix_timestamp();
        let record = json!({"version":1,"id":d.id,"plan_id":plan["plan_id"],"revision":plan["revision"],
            "stage_id":d.stage_id,"summary":d.summary,"rationale":d.rationale,"alternatives":d.alternatives,
            "status":"accepted","supersedes":d.supersedes,"created_unix":now,"updated_unix":now});
        let _: crate::contracts::Decision =
            serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
        records.push(record);
    }
    let recent = next["recent_decisions"].as_array_mut().unwrap();
    for d in recent.iter_mut() {
        if superseded.contains(d["id"].as_str().unwrap_or("")) {
            d["status"] = json!("superseded");
        }
    }
    for d in &records {
        recent.push(json!({"id":d["id"],"summary":d["summary"],"status":d["status"],"rationale":d["rationale"],"alternatives":d["alternatives"],"supersedes":d["supersedes"]}));
    }
    if recent.len() > 8 {
        recent.drain(..recent.len() - 8);
    }
    let mut seen = BTreeSet::new();
    for g in t.guidance {
        let idx = plan["stages"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["id"] == g.stage_id)
            .ok_or("guidance for unknown stage")?;
        if !seen.insert(g.stage_id) || !text_ok(&g.text, 4000) {
            return Err("invalid stage guidance".into());
        }
        if plan["stages"][idx]["status"] == "committed" { continue; }
        next["guidance"][g.stage_id.to_string()] = json!({"version":1,"id":identity(),"stage_id":g.stage_id,
            "revision":plan["revision"],"relevant_inputs":crate::plan::stage_inputs(plan, idx),"text":g.text,"valid":true,"unix":unix_timestamp()});
    }
    if required.iter().any(|id| !seen.contains(id)) {
        return Err("architect omitted required stage guidance".into());
    }
    // Keep the audit trail of what stopped applying and why, bounded like decisions.
    if !t.retired_constraints.is_empty() {
        let history = next["retired_constraints"].as_array().cloned().unwrap_or_default();
        let mut history: Vec<Value> = history;
        for r in &t.retired_constraints {
            history.push(json!({"text":r.text,"reason":r.reason,
                "revision":plan["revision"],"unix":unix_timestamp()}));
        }
        if history.len() > 16 { history.drain(..history.len() - 16); }
        next["retired_constraints"] = json!(history);
    }
    Ok((next, records))
}

/// A bounded textual summary of a stage's outcome for the architect's compact
/// per-stage entry: the committed sha, the review status, and an excerpt
/// of the implementer outcome (or, once a stage's own record ages out, the
/// matching checkpoint execution outcome) -- never the record itself.
fn stage_outcome(stage: &Value, execution_outcomes: &[Value]) -> Option<String> {
    let sha = stage["sha"].as_str().filter(|s| !s.is_empty());
    let review_status = stage["review_gate"]["status"].as_str().filter(|s| !s.is_empty());
    let detail = if !stage["implementer_outcome"].is_null() {
        Some(stage["implementer_outcome"].to_string())
    } else {
        execution_outcomes
            .iter()
            .find(|o| o["stage_id"] == stage["id"])
            .map(|o| o.to_string())
    };
    if sha.is_none() && review_status.is_none() && detail.is_none() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(sha) = sha {
        parts.push(format!("sha={sha}"));
    }
    if let Some(status) = review_status {
        parts.push(format!("review={status}"));
    }
    if let Some(detail) = detail {
        let bounded: String = detail.chars().take(500).collect();
        parts.push(bounded);
    }
    Some(parts.join(" "))
}

impl Ctx {
    fn decision_context(
        &self,
        previous: Option<&Value>,
    ) -> Result<BTreeMap<String, Value>, String> {
        let mut decisions = BTreeMap::new();
        if previous.is_none_or(|p| p.get("architecture").is_none()) {
            return Ok(decisions);
        }
        let mut cursor = 0;
        loop {
            let page = self.architecture_store().history(
                previous.unwrap()["plan_id"].as_str(),
                cursor,
                100,
            )?;
            for event in page["items"].as_array().unwrap() {
                let p = &event["payload"];
                let records = if p["kind"] == "decision" {
                    vec![p["record"].clone()]
                } else {
                    p["decisions"].as_array().cloned().unwrap_or_default()
                };
                for d in records {
                    if let Some(id) = d["supersedes"].as_str() {
                        if let Some(old) = decisions.get_mut(id) {
                            let old: &mut Value = old;
                            old["status"] = json!("superseded");
                        }
                    }
                    if let Some(id) = d["id"].as_str() {
                        decisions.insert(id.into(), d);
                    }
                }
            }
            match page["next_cursor"].as_u64() {
                Some(n) => cursor = n,
                None => break,
            }
        }
        Ok(decisions)
    }

    /// Candidate and architectural output cross the same publication boundary.
    pub(crate) fn architect_publish(
        &self,
        candidate: Value,
        previous: Option<&Value>,
        reason: &str,
    ) -> Result<Value, String> {
        self.architect_publish_selected(candidate, previous, reason, None)
    }

    pub(crate) fn architect_publish_selected(
        &self,
        mut candidate: Value,
        previous: Option<&Value>,
        reason: &str,
        selected: Option<crate::model_selection::ModelChoice>,
    ) -> Result<Value, String> {
        let _turn_guard = self.session.architect_lock.lock().unwrap();
        let result = (|| {
            let store = self.architecture_store();
            let old = {
                let _guard = self.session.persistence_lock.lock().unwrap();
                store.load()?
            };
            if let Some(previous) = previous {
                if old.as_ref() != Some(previous) {
                    return Err("stale architect candidate".into());
                }
            }
            let mut cp = if let Some(p) = previous.filter(|p| p.get("architecture").is_some()) {
                store.checkpoint(p)?
            } else {
                checkpoint_default()
            };
            if candidate["plan_id"].is_null() {
                candidate["plan_id"] = json!(identity());
                candidate["revision"] = json!(1);
            }
            if !candidate["plan_id"].as_str().is_some_and(safe_key)
                || candidate["revision"].as_u64().unwrap_or(0) == 0
            {
                return Err("invalid architect plan identity".into());
            }
            let routing_ids = self.routing_required(&candidate, &cp)?;
            let missing: Vec<i64> = routing_ids.iter().copied().filter(|id| {
                candidate["stages"].as_array().unwrap().iter().find(|s| s["id"] == *id)
                    .is_none_or(|s| s["reassessment"]["pending"].is_object() || !s["model_proposal"].is_object() || !self.proposal_inputs_current(&candidate, candidate["stages"].as_array().unwrap().iter().position(|s| s["id"] == *id).unwrap()))
            }).collect();
            self.propose_routing(&mut candidate, &missing, &Value::Null)?;
            let mut required = vec![];
            for (idx, stage) in candidate["stages"]
                .as_array()
                .ok_or("invalid architect plan")?
                .iter()
                .enumerate()
            {
                if stage["status"] == "committed" {
                    continue;
                }
                let g = &cp["guidance"][stage["id"].to_string()];
                if g["valid"] != true
                    || g["relevant_inputs"] != crate::plan::stage_inputs(&candidate, idx)
                {
                    required.push(stage["id"].as_i64().ok_or("invalid stage id")?);
                }
            }
            // Invalidate historical inputs before a candidate revision is published.
            for key in ["guidance", "agreements"] {
                for (_, g) in cp[key].as_object_mut().unwrap() {
                    let index = candidate["stages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .position(|s| s["id"] == g["stage_id"]);
                    if index.is_some_and(|idx| candidate["stages"][idx]["status"] == "committed") { continue; }
                    if index.is_none_or(|idx| {
                        g["relevant_inputs"] != crate::plan::stage_inputs(&candidate, idx)
                    }) {
                        g["valid"] = json!(false);
                        g["invalidation_trigger"] = json!(if index.is_none() {
                            "stage_removed"
                        } else {
                            "stage_or_dependency_changed"
                        });
                    }
                }
            }
            let pinned = selected.is_some();
            let (provider, model, effort) = match selected {
                Some(choice) => choice,
                None => self.bootstrap("architect")?,
            };
            let dir = self
                .forge_path("architecture")
                .join(candidate["plan_id"].as_str().ok_or("missing plan id")?);
            let pending_path = dir.join("architect-pending.json");
            let pending = match fs::read(&pending_path) {
                Ok(bytes) => Some(
                    serde_json::from_slice::<Value>(&bytes)
                        .unwrap_or(json!({"turn":"unknown-uncommitted-turn"})),
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.to_string()),
            };
            let contaminated = pending
                .as_ref()
                .is_some_and(|p| p["turn"] != cp["last_turn"]);
            let changed_model = if let (Some(provider_id),Some(old)) = (Provider::parse(&provider),cp["effective_model"]["model"].as_str()) {
                let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let facts = self.model_facts(&policy,provider_id,&model,Some(&effort));
                !crate::agent::same_model(&provider,facts["resolved_id"].as_str().unwrap_or(&model),old)
            } else { false };
            let recovery = if contaminated {
                Some("uncommitted or failed architect turn")
            } else if !cp["session"].is_null() && cp["session"]["provider"] != provider {
                Some("provider changed")
            } else if changed_model {
                Some("model changed; reconstruct from saved checkpoint")
            } else if cp["context_status"] == "needs_recovery"
                || cp["session"]["resume_policy"] == "fork_from_checkpoint"
            {
                Some("checkpoint reconstruction required")
            } else {
                None
            };
            // A revision is the one event that can falsify plan-wide saved context,
            // such as a constraint naming a stage number the reorder moved. Take a
            // turn for it even when no stage needs guidance, so the architect can
            // retire what no longer applies instead of reporting it forever.
            let revised = previous.is_some_and(|p| p["revision"] != candidate["revision"]);
            if required.is_empty() && routing_ids.is_empty() && cp["context_status"] == "ready"
                && recovery.is_none() && !revised {
                if previous == Some(&candidate) {
                    return Ok(candidate);
                }
                let _guard = self.session.persistence_lock.lock().unwrap();
                return store.publish(
                    candidate,
                    cp,
                    json!({"kind":"architect_boundary_reused","reason":reason}),
                );
            }
            let mut session = if recovery.is_none() {
                cp["session"]["reference"].as_str().map(str::to_owned)
            } else {
                None
            };
            let mut recovery_reason = recovery.map(str::to_owned);
            let decisions = self.decision_context(previous)?;
            let mut included = Vec::new();
            let mut size = 0;
            for d in decisions.values().filter(|d| d["status"] != "superseded") {
                let n = d.to_string().len();
                if size + n <= 24 * 1024 {
                    included.push(d.clone());
                    size += n;
                }
            }
            let observations = self.git(&["status", "--short"]).unwrap_or_else(|e| e);
            // A work-status context, not a plan clone: bookkeeping (selection,
            // agreement, proposal inputs, reassessment, invocations, reviews,
            // usage) never enters the turn, and only the stages that actually
            // need guidance or routing contribute their design fields, so the
            // turn stays flat as the plan grows.
            let execution_outcomes: Vec<Value> = cp["execution_outcomes"].as_array().into_iter().flatten()
                .map(|o| crate::prompt_view::outcome_view(o, &candidate)).collect();
            let stages: Vec<Value> = candidate["stages"].as_array().unwrap().iter().map(|stage| {
                let mut entry = json!({"id":stage["id"],"title":stage["title"],"status":stage["status"]});
                if let Some(depends_on) = stage.get("depends_on") { entry["depends_on"] = depends_on.clone(); }
                if let Some(outcome) = stage_outcome(stage, &execution_outcomes) { entry["outcome"] = json!(outcome); }
                entry
            }).collect();
            let mut needs_detail: BTreeSet<i64> = required.iter().copied().collect();
            needs_detail.extend(routing_ids.iter().copied());
            let stage_details: Vec<Value> = candidate["stages"].as_array().unwrap().iter()
                .filter(|stage| stage["id"].as_i64().is_some_and(|id| needs_detail.contains(&id)))
                .map(|stage| {
                    let mut detail = json!({"id":stage["id"],"title":stage["title"],"instructions":stage["instructions"],
                        "acceptance":stage["acceptance"],"commit":stage["commit"]});
                    if let Some(depends_on) = stage.get("depends_on") { detail["depends_on"] = depends_on.clone(); }
                    if !stage["model_constraint"].is_null() { detail["model_constraint"] = stage["model_constraint"].clone(); }
                    if !stage["model_proposal"].is_null() { detail["model_proposal"] = stage["model_proposal"].clone(); }
                    detail
                }).collect();
            let mut paths = json!({
                "architecture_events": dir.join("events.jsonl"),
                "plan": format!("{}/plan.json", crate::app::FORGE_DIR),
                "repository": self.project(),
            });
            if self.forge_path("plan-candidate.json").exists() {
                paths["candidate"] = json!(format!("{}/plan-candidate.json", crate::app::FORGE_DIR));
            }
            let context = json!({"goal":candidate["goal"],"plan_id":candidate["plan_id"],"revision":candidate["revision"],
                "checkpoint":crate::prompt_view::checkpoint_for(&cp, &candidate),"decisions":included,
                "stages":stages,"stage_details":stage_details,"paths":paths,"repository_observations":observations,
                "unfinished_diff_preview":self.git(&["diff","HEAD","--",".",":(exclude).forge"]).unwrap_or_else(|e| crate::util::last_chars(&e,500)).chars().take(16000).collect::<String>(),
                "required_stage_ids":required,"required_model_stage_ids":routing_ids,"reason":reason});
            let mut context = context;
            let feature = crate::feature_context::feature_summary(&candidate["feature"]);
            let context_has_feature = feature.is_some();
            if let Some(feature) = feature {
                context["feature"] = feature;
            }
            let mut prompt = format!(
                "You are this plan's persistent architect. Inspect repository code as needed. You MUST NOT implement, write files, commit, push, or alter acceptance criteria. The engine alone publishes validated output. Preserve saved completed interfaces exactly, and preserve every saved constraint unless you explicitly retire it; explicitly resolve risks by ID. Propose decisions with unique alphanumeric/hyphen IDs and supersessions of existing IDs. Context is a work status, not the plan document: stages carries one compact entry per stage and stage_details carries full design only for required_stage_ids/required_model_stage_ids. Whenever you need detail this prompt omits, such as a non-required stage's instructions, past decisions, or what changed, you MUST inspect it yourself: read paths.architecture_events for decision/history detail, paths.plan (and paths.candidate when present) for the authoritative plan, and paths.repository with `git log` and `git diff` for code and history; never assume omitted detail does not exist. In checkpoint.agreements, constraints are referenced by the IDs in checkpoint.constraint_ids (constraint_refs; retired_constraint_refs name constraints retired since the agreement); full agreement records are in paths.architecture_events. Return ONLY JSON, no fences, with this exact shape:\n{{\"version\":1,\"plan_id\":{},\"revision\":{},\"checkpoint\":{{\"summary\":\"bounded architectural context\",\"constraints\":[],\"completed_interfaces\":[]}},\"decisions\":[{{\"id\":\"unique-id\",\"stage_id\":null,\"summary\":\"decision\",\"rationale\":\"why\",\"alternatives\":[{{\"description\":\"alternative\",\"tradeoffs\":\"tradeoffs\"}}],\"supersedes\":null}}],\"guidance\":[{{\"stage_id\":1,\"text\":\"concrete guidance\"}}],\"unresolved_risks\":[{{\"id\":\"risk-id\",\"text\":\"risk\"}}],\"resolved_risks\":[],\"retired_constraints\":[]}}\nSupply guidance for every required_stage_id. checkpoint.constraints and checkpoint.completed_interfaces are arrays of strings, not objects. resolved_risks is an array of exact existing unresolved-risk IDs from the supplied checkpoint/history, never descriptions or newly invented IDs; use [] when no existing risk can be resolved. New findings belong in unresolved_risks as objects with id and text. retired_constraints is how a saved constraint stops applying: each entry is {{\"text\":\"the exact saved constraint\",\"reason\":\"why it no longer applies\"}}, the text must be omitted from checkpoint.constraints in the same turn, and it must match a saved constraint exactly. Retire a constraint that a later revision made false, such as one naming a stage number that now holds different work, instead of reporting the contradiction and leaving it in place; use [] when every saved constraint still applies. Empty decisions/risks are allowed. Keep summary <=8000 bytes, each constraint/interface/risk <=1000 bytes and guidance <=4000 bytes; total output <=48 KiB. Context:\n{context}",
                candidate["plan_id"], candidate["revision"]
            );
            if context_has_feature {
                prompt.push_str("\nThis plan implements one milestone of a feature spec. context.feature names its folder, milestone and covered scenario IDs only: the feature's files (scenarios.md, decisions.md, milestones.md) are in the repository and must be read there.");
            }
            prompt.push_str(&format!("\n{}\n{}\nEvaluate exactly required_model_stage_ids: {:?}. Include model_evaluations in the complete architect output.", self.stage_selection_prompt(&candidate, &routing_ids)?, crate::routing::EVALUATION_CONTRACT, routing_ids));
            let turn = identity();
            publish_pretty(
                &pending_path,
                &json!({"version":1,"plan_id":candidate["plan_id"],"revision":candidate["revision"],"turn":turn,"previous_session":cp["session"],"reason":reason}),
            )?;
            self.session.state.lock().unwrap().architect_activity = json!({"status":if recovery_reason.is_some() {"recovering"} else {"working"},"reason":recovery_reason,"plan_id":candidate["plan_id"]});
            self.log_event(
                "architect",
                &format!(
                    "{reason}: {}",
                    recovery_reason.as_deref().unwrap_or("guidance turn")
                ),
            );
            let requirements = self.model_requirements("architect",None)?;
            let requirements = if pinned { requirements.pinned() } else { requirements };
            let initial = (provider,model,effort);
            let invoke = |choice: &crate::model_selection::ModelChoice, session: Option<&str>| -> Result<AgentResult, String> {
                let (provider,model,effort) = choice;
                if provider == "mock" {
                    return self.mock_architect(&candidate, &cp, &required, &routing_ids, &prompt, session);
                }
                self.run_agent(&AgentRequest {
                    role: "architect",
                    provider,
                    model,
                    effort,
                    session,
                    prompt: &prompt,
                })
            };
            let (output,(provider,model,effort)) = self.with_selected_model(&requirements,Some(initial.clone()), |choice| {
                if choice != &initial {
                    session = None;
                    recovery_reason = Some("model quota fallback; reconstruct from saved checkpoint".into());
                    self.session.state.lock().unwrap().architect_activity = json!({"status":"recovering","reason":recovery_reason});
                }
                let mut output = invoke(choice,session.as_deref());
                if let Err(error) = &output {
                    let lower = error.to_lowercase();
                    if session.is_some()
                        && [
                            "session not found",
                            "no conversation found",
                            "session expired",
                            "thread not found",
                            "no rollout found",
                            "no saved session found",
                            "could not find session",
                        ]
                        .iter()
                        .any(|s| lower.contains(s))
                    {
                        recovery_reason = Some(format!(
                            "missing/expired session: {}",
                            crate::util::last_chars(error, 500)
                        ));
                        session = None;
                        self.session.state.lock().unwrap().architect_activity =
                            json!({"status":"recovering","reason":recovery_reason});
                        output = invoke(choice,None);
                    }
                }
                output
            })?;
            if provider != "mock" {
                let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let selected = self.model_facts(&policy, Provider::parse(&provider).ok_or("invalid architect provider")?, &model, None);
                if !output.model_reported {
                    return Err("architect effective model missing: provider did not report a model in the stream or current session metadata; see model log".into());
                }
                if selected["eligible"] != true {
                    return Err(format!("architect model is no longer eligible: {}", selected["error"].as_str().unwrap_or(&model)));
                }
                let expected = selected["resolved_id"].as_str().unwrap_or(&model);
                if !crate::agent::same_model(&provider, expected, &output.effective_model) {
                    return Err(format!("architect effective model changed: expected {expected}, reported {}", output.effective_model));
                }
            }
            if self
                .session
                .stop_requested
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("architect stopped; saved checkpoint retained".into());
            }
            let reference = output
                .session
                .as_deref()
                .filter(|s| crate::agent::session_id(s))
                .ok_or("architect omitted exact session identity")?.to_owned();
            if session.as_deref().is_some_and(|id| id != reference) {
                return Err("architect session identity changed unexpectedly".into());
            }
            if recovery_reason.is_some() && cp["session"]["reference"] == reference {
                return Err("recovery did not create a replacement session identity".into());
            }
            let mut turn_usage: Vec<_> = output.usage.iter().cloned().collect();
            let expected_model = output.effective_model.clone();
            let (output, (mut next, records)) = self.repair_response("architect", output,
                |response| {
                    let applied = apply_turn(&candidate, &cp, &response.output, &decisions, &required)?;
                    let parsed: Value = crate::response::parse_json(&response.output).map_err(|e| e.to_string())?;
                    crate::routing::validate_evaluations(&parsed["model_evaluations"], &routing_ids, &candidate)?;
                    Ok(applied)
                },
                |response, error| {
                    let correction = crate::response::correction_prompt(&prompt, &response.output, error);
                    let repaired = if provider == "mock" {
                        self.mock_architect(&candidate, &cp, &required, &routing_ids, &correction, Some(&reference))?
                    } else {
                        self.run_agent(&AgentRequest { role:"architect", provider:&provider, model:&model,
                            effort:&effort, session:Some(&reference), prompt:&correction })?
                    };
                    if repaired.session.as_deref() != Some(reference.as_str())
                        || !repaired.completed || (provider != "mock" && (!repaired.model_reported
                            || !crate::agent::same_model(&provider, &expected_model, &repaired.effective_model))) {
                        return Err("architect correction changed session/model or did not complete".into());
                    }
                    if let Some(usage) = &repaired.usage { turn_usage.push(usage.clone()); }
                    Ok(repaired)
                })?;
            let evaluation_output: Value = crate::response::parse_json(&output.output).map_err(|e| e.to_string())?;
            next["routing_evaluator"] = json!({"provider":provider,"model":output.effective_model,"native_effort":effort});
            self.agree_routing(&mut candidate, &mut next, &routing_ids, &evaluation_output["model_evaluations"])?;
            next["session"] = json!({"provider":provider,"reference":reference,"checkpoint_reference":turn,"resume_policy":"exact_if_committed"});
            next["context_status"] = json!("ready");
            next["last_turn"] = json!(turn);
            next.as_object_mut().unwrap().remove("context_gap");
            next["effective_model"] =
                json!({"provider":provider,"model":output.effective_model,"native_effort":effort});
            next["recovery"] = json!({"reason":recovery_reason,"previous_session":cp["session"],"replacement_session":reference});
            for usage in &turn_usage {
                accumulate_invocation_usage(&mut next, "role_usage", "architect", usage);
                accumulate_invocation_usage(&mut candidate, "usage", &provider, usage);
                accumulate_invocation_usage(&mut candidate["role_usage"], "architect", &provider, usage);
            }
            let _guard = self.session.persistence_lock.lock().unwrap();
            if store.load()? != old {
                return Err("plan changed during architect turn".into());
            }
            let published = store.publish(candidate, next.clone(), json!({"kind":"architect_turn","turn":turn,"reason":reason,
                "model_agreements":next["agreements"],"decisions":records,"resolved_risks":evaluation_output["resolved_risks"],
                "recovery_reason":recovery_reason,"previous_session":cp["session"],"session":reference}))?;
            self.session.state.lock().unwrap().architect_activity =
                json!({"status":"ready","reason":recovery_reason,"session":reference});
            Ok(published)
        })();
        if let Err(error) = &result {
            self.session.state.lock().unwrap().architect_activity =
                json!({"status":"failed","recoverable":true,"error":error});
            self.log_event("architect", &format!("recoverable failure: {error}"));
        }
        result
    }

    fn mock_architect(
        &self,
        plan: &Value,
        cp: &Value,
        required: &[i64],
        routing_ids: &[i64],
        prompt: &str,
        session: Option<&str>,
    ) -> Result<AgentResult, String> {
        #[cfg(test)]
        self.record_prompt("architect", crate::prompt_capture::ARCHITECT_PUBLISH, None, prompt);
        let mut settings = self.app.settings.lock().unwrap();
        let requests = settings
            .as_object_mut()
            .unwrap()
            .entry("mock_architect_requests")
            .or_insert(json!([]));
        requests.as_array_mut().unwrap().push(json!({"plan_id":plan["plan_id"],"revision":plan["revision"],"session":session,"prompt":prompt}));
        if let Some(errors) = settings["mock_architect_errors"].as_array_mut() {
            if !errors.is_empty() {
                let error = errors.remove(0);
                if let Some(error) = error.as_str() {
                    return Err(error.into());
                }
            }
        }
        if !routing_ids.is_empty() {
            if !settings["mock_routing_architect_requests"].is_array() { settings["mock_routing_architect_requests"] = json!([]); }
            settings["mock_routing_architect_requests"].as_array_mut().unwrap().push(json!({"ids":routing_ids,"prompt":prompt}));
        }
        let evaluations = settings["mock_model_evaluations"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0))
            .unwrap_or_else(|| crate::routing::mock_evaluations(plan, routing_ids));
        let id = mock_session_uuid(&identity());
        // An array scripts successive turns, so a rejection followed by a
        // correction can be exercised.
        let scripted = match settings.get_mut("mock_architect_output").and_then(Value::as_array_mut) {
            Some(queue) if queue.len() > 1 => Some(queue.remove(0)),
            Some(queue) => queue.first().cloned(),
            None => settings.get("mock_architect_output").cloned(),
        };
        // Null selects the built-in valid turn, so a script can mix rejected and
        // accepted outputs without restating the whole contract.
        let output = scripted.filter(|v| !v.is_null())
            .map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| json!({
            "version":1,"plan_id":plan["plan_id"],"revision":plan["revision"],"checkpoint":{"summary":"Keep stage interfaces and acceptance constraints.",
            "constraints":strings(&cp["constraints"]),"completed_interfaces":strings(&cp["completed_interfaces"])},"decisions":[],
            "guidance":required.iter().map(|id| json!({"stage_id":id,"text":"Preserve existing interfaces and verify the stage acceptance checks."})).collect::<Vec<_>>(),
            "model_evaluations":evaluations,
            "unresolved_risks":cp["unresolved_risks"].as_array().cloned().unwrap_or_default(),"resolved_risks":[]}).to_string());
        Ok(AgentResult {
            output,
            session: Some(session.unwrap_or(&id).into()),
            effective_model: "mock-architect".into(),
            completed: true,
            ..AgentResult::default()
        })
    }
}

/// Formats an identity as a UUID-shaped mock session id. Identities have
/// variable-length hex parts, so pad (and truncate) to 32 digits before slicing.
pub(crate) fn mock_session_uuid(identity: &str) -> String {
    let raw: String = format!("{:0<32}", identity.replace('-', "")).chars().take(32).collect();
    format!("{}-{}-{}-{}-{}", &raw[..8], &raw[8..12], &raw[12..16], &raw[16..20], &raw[20..32])
}

#[cfg(test)]
#[path = "architect_tests.rs"]
mod tests;
