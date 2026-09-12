//! Joint, bounded stage routing. Global catalogue clocks are never validity inputs.
use crate::{
    agent::AgentRequest,
    app::Ctx,
    catalogue::{Policy, Provider},
};
use crate::usage::accumulate_invocation_usage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const POLICY: &str = "stage-routing-1";
const EXECUTION_CONTRACT: &str = r#"MODEL SELECTION CONTRACT: Include model_proposal on each new or materially changed pending stage:
{"risk":"simple|standard|critical","complexity":"simple|standard|complex","task":"documentation|functionality|concurrency|persistence|security","provider":"exact provider","model":"exact registry ID","native_effort":"provider_default or supported native effort","rationale":"stage-specific adequacy, failure impact and cost reasoning"}.
Use exactly those seven proposal fields; put all explanation, including effort changes, in rationale. Do not add fields.
Use only eligible catalogue/registry options. Copy the option's model field exactly; resolved_id is execution evidence, not an alternative registry key. Configured tiers are explicit adequacy policy, not official evidence. Critical or complex work requires strong; never infer quality from price or model name. Simple work and documentation at any adequate tier prefer a cheaper adequate option only with comparable published billing data or configured relative preferences. Unknown price remains unknown. Constraints narrow choices first, but never waive capability or independent-review requirements. With per_plan reviewer cadence, stage reviewer selection is deferred: the configured reviewer provider is not a stage implementer constraint. With reviewer_provider_mode=configured, respect the selected reviewer provider in a fresh session, including same-provider review. Otherwise with per_stage cadence, automatic routing and no reviewer model pin, Forge chooses the other reviewer provider automatically; the configured reviewer selector alone does not prohibit an implementer provider. Preserve acceptance and committed stages. Unchanged agreements need no new proposal."#;

pub(crate) const CONTRACT: &str = r#"STAGE CAPABILITY CONTRACT: Include model_proposal on each new or materially changed pending stage:
{"risk":"simple|standard|critical","complexity":"simple|standard|complex","task":"documentation|functionality|concurrency|persistence|security","tier":"basic|standard|strong","rationale":"stage-specific capability and failure-impact reasoning"}.
Use exactly these five fields. Select the weakest adequate capability tier, never a provider, model or native effort. Basic suits simple low-impact work; standard suits ordinary implementation; strong is required for critical, complex or sensitive implementation. The architect independently checks the classification. Forge resolves a concrete model locally at implementation start using the implementer provider selected THEN and its current model catalogue. The user can change that provider after planning without replanning. Model prices, provider quota and reviewer settings are launch concerns, not planning inputs. Preserve acceptance and committed stages. Unchanged agreements need no new proposal."#;

#[path = "routing_tiers.rs"]
mod tiers;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proposal {
    risk: String,
    complexity: String,
    task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    // Older saved agreements and execution reassessments carry concrete choices.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    native_effort: String,
    rationale: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evaluation {
    stage_id: i64,
    agree: bool,
    rationale: String,
    // Independent classification prevents a planner's underclassification granting adequacy.
    risk: String,
    complexity: String,
    task: String,
}

/// Validate planner-owned proposal fields before paying for an architect turn.
/// Missing proposals can still be supplied by the separate selection operation.
pub(crate) fn validate_candidate_proposals(plan: &Value) -> Result<(), String> {
    for stage in plan["stages"].as_array().ok_or("invalid stages")? {
        if stage["status"] == "committed" || stage["model_proposal"].is_null() { continue; }
        let proposal: Proposal = serde_json::from_value(stage["model_proposal"].clone())
            .map_err(|e| format!("stage {} model_proposal: {e}", stage["id"]))?;
        validate_proposal(&proposal)?;
    }
    Ok(())
}

fn validate_proposal(p: &Proposal) -> Result<(), String> {
    classification(&p.risk, &p.complexity, &p.task)?;
    if let Some(tier) = &p.tier {
        if rank(&json!(tier)) == 0 || !p.provider.is_empty() || !p.model.is_empty()
            || !p.native_effort.is_empty() || p.rationale.trim().is_empty() || p.rationale.len() > 4000 {
            return Err("tier proposal requires basic/standard/strong and rationale, without a provider, model or effort".into());
        }
        return Ok(());
    }
    if (p.provider != "mock" && (Provider::parse(&p.provider).is_none()
        || !crate::catalogue::identifier(&p.model)
        || !crate::catalogue::identifier(&p.native_effort)))
        || p.rationale.trim().is_empty() || p.rationale.len() > 4000 {
        return Err("invalid model proposal provider, model, effort or rationale".into());
    }
    Ok(())
}

pub(crate) fn valid_tier_record(record: &Value) -> bool {
    serde_json::from_value::<Proposal>(record["validated_proposal"].clone()).ok().is_some_and(|p|
        p.tier.as_ref().is_some_and(|tier| validate_proposal(&p).is_ok()
            && record["policy_inputs"]["minimum_tier"] == rank(&json!(tier))
            && record["policy_inputs"]["tier"] == *tier
            && record["binding"] == "at_implementation_start"))
}

pub(crate) fn validate_evaluations(value: &Value, ids: &[i64]) -> Result<(), String> {
    if ids.is_empty() && value.is_null() { return Ok(()); }
    let rows: Vec<Evaluation> = serde_json::from_value(value.clone())
        .map_err(|e| format!("invalid architect model evaluations: {e}"))?;
    if rows.len() != ids.len() { return Err("architect evaluation count mismatch".into()); }
    for id in ids {
        let matched: Vec<_> = rows.iter().filter(|r| r.stage_id == *id).collect();
        if matched.len() != 1 { return Err("duplicate/missing architect model evaluation".into()); }
        let e = matched[0];
        classification(&e.risk, &e.complexity, &e.task)?;
        if e.rationale.trim().is_empty() || e.rationale.len() > 4000 {
            return Err("missing architect rationale".into());
        }
    }
    Ok(())
}

fn parse_proposals(output: &Value, ids: &[i64]) -> Result<Vec<(i64, Proposal)>, String> {
    let rows = output["proposals"].as_array().ok_or("planner omitted routing proposals")?;
    if rows.len() != ids.len() {
        return Err("planner proposal count mismatch".into());
    }
    ids.iter().map(|id| {
        let matches: Vec<_> = rows.iter().filter(|r| r["stage_id"] == *id).collect();
        if matches.len() != 1 {
            return Err("duplicate or missing planner proposal".into());
        }
        let proposal = serde_json::from_value(matches[0]["proposal"].clone())
            .map_err(|e| format!("stage {id}: {e}"))?;
        validate_proposal(&proposal)?;
        Ok((*id, proposal))
    }).collect()
}

pub(crate) fn validate_constraint(value: &Value) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let obj = value
        .as_object()
        .ok_or("model_constraint must be an object or null")?;
    if obj.is_empty() {
        return Err("empty constraint: use null to restore automatic selection".into());
    }
    for (k, v) in obj {
        if !["provider", "model", "native_effort"].contains(&k.as_str())
            || !v.as_str().is_some_and(crate::catalogue::identifier)
        {
            return Err(
                "constraint accepts only exact provider, model and native_effort strings".into(),
            );
        }
    }
    if obj
        .get("provider")
        .is_some_and(|v| Provider::parse(v.as_str().unwrap()).is_none())
    {
        return Err("constraint provider must be codex or claude".into());
    }
    Ok(())
}
fn constraint(settings: &Value, stage: &Value) -> Value {
    if stage["model_constraint"].is_object() {
        return stage["model_constraint"].clone();
    }
    if settings["implementer_model"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        let mut pin = json!({"provider":settings["implementer"],"model":settings["implementer_model"]});
        // Only explicitly configured effort narrows the joint proposal. Catalogue
        // validation still owns eligibility; a stage constraint replaces this pin.
        if let Ok(policy) = crate::catalogue::Policy::from_settings(settings)
            && let Some(effort) = policy.entries.iter()
                .find(|entry| entry.provider.name() == pin["provider"] && entry.model == pin["model"])
                .and_then(|entry| entry.effort.as_deref())
        {
            pin["native_effort"] = json!(effort);
        }
        return pin;
    }
    if settings["automatic_routing"] == false {
        return json!({"provider":settings["implementer"]});
    }
    json!({})
}
fn classification(risk: &str, complexity: &str, task: &str) -> Result<(), String> {
    if !["simple", "standard", "critical"].contains(&risk)
        || !["simple", "standard", "complex"].contains(&complexity)
        || ![
            "documentation",
            "functionality",
            "concurrency",
            "persistence",
            "security",
        ]
        .contains(&task)
    {
        return Err("invalid risk, complexity or task classification".into());
    }
    Ok(())
}
fn rank(tier: &Value) -> u64 {
    match tier.as_str() {
        Some("basic") => 1,
        Some("standard") => 2,
        Some("strong") => 3,
        _ => 0,
    }
}
fn capability_intent(stage: &Value) -> String {
    // A ban on inventing requirements is not a request to implement them.
    // Only omit explicit no-new-scope clauses; keep ambiguous wording and
    // split contrast clauses so a subsequent positive instruction still counts.
    ["title", "instructions", "acceptance"].into_iter()
        .map(|key| stage[key].as_str().unwrap_or("").to_lowercase())
        .flat_map(|text| text.split(['.', ';', '\n']).flat_map(|s| s.split(" but "))
            .flat_map(|s| s.split(" and "))
            .filter_map(|clause| {
                let clause = clause.trim();
                let excluded = ["do not invent ", "do not introduce ", "do not add new ",
                    "do not modify ", "do not change ", "do not edit ", "do not touch "]
                    .into_iter().find_map(|prefix| clause.strip_prefix(prefix));
                // Mixed instructions must retain their conservative floor.
                let mixed = |tail: &str| ["implement", "change", "define", "update", "rewrite",
                    "replace", "remove", "create", "enforce", "introduce", "add", "then",
                    "instead", "however", "persist", "migrate", "encrypt", "authenticate", "authorize"]
                    .iter().any(|verb| tail.split(|c: char| !c.is_alphanumeric()).any(|word| word == *verb));
                if excluded.is_some_and(|tail| !mixed(tail)) { return None; }
                // Preserve any positive work before a no-new-behavior boundary,
                // e.g. "implement persistence, no new requirements".
                if let Some((before, tail)) = clause.split_once(" no ") {
                    if !mixed(tail) { return Some(before.to_owned()); }
                }
                Some(clause.to_owned())
            }).collect::<Vec<_>>())
        .collect::<Vec<_>>().join(" ")
}
fn minimum(stage: &Value, p: &Proposal) -> u64 {
    let text = capability_intent(stage);
    // Conservative engine floor for high failure-impact implementation domains.
    let sensitive = [
        "concurren",
        "persist",
        "security",
        "authentication",
        "authorization",
        "race condition",
        "deadlock",
        "encryption",
        "atomic",
        "migration",
    ]
    .iter()
    .any(|word| text.contains(word));
    let explanatory_docs = p.task == "documentation"
        && ["document", "explain", "readme", "prose", "typo", "spelling"]
            .iter()
            .any(|word| text.contains(word))
        && ![
            "implement",
            "contract",
            "normative",
            "security policy",
            "requirement",
            "schema",
            "design decision",
        ]
        .iter()
        .any(|word| text.contains(word));
    if p.risk == "critical"
        || p.complexity == "complex"
        || (sensitive && !explanatory_docs)
        || ["concurrency", "persistence", "security"].contains(&p.task.as_str())
    {
        3
    } else if p.risk == "simple" && p.complexity == "simple" {
        1
    } else {
        2
    }
}
fn matches_constraint(option: &Value, c: &Value) -> bool {
    c.as_object().unwrap().iter().all(|(key, value)| {
        option[if key == "native_effort" {
            "effort"
        } else {
            key
        }] == *value
    })
}
fn cheaper(a: &Value, b: &Value, basis: &Value) -> bool {
    if let (Some(a), Some(b)) = (
        a["relative_cost_preference"].as_u64(),
        b["relative_cost_preference"].as_u64(),
    ) {
        return a < b;
    }
    let (a, b) = (&a["pricing"], &b["pricing"]);
    if !basis.is_string()
        || a["basis"] != *basis
        || !a.is_object()
        || !b.is_object()
        || ["currency", "unit", "basis"]
            .iter()
            .any(|key| a[*key] != b[*key])
    {
        return false;
    }
    match (
        a["input"].as_f64(),
        a["output"].as_f64(),
        b["input"].as_f64(),
        b["output"].as_f64(),
    ) {
        (Some(ai), Some(ao), Some(bi), Some(bo)) => ai <= bi && ao <= bo && (ai < bi || ao < bo),
        _ => false,
    }
}
fn effort_rank(e: &str) -> u64 {
    match e { "minimal" => 1, "low" => 2, "medium" => 3, "high" => 4, "xhigh" => 5, "max" => 6, _ => 0 }
}
fn automatic_reviewer(settings: &Value) -> bool {
    settings["reviewer_provider_mode"] != "configured" && settings["automatic_routing"] != false
        && settings["reviewer_model"].as_str().is_none_or(str::is_empty)
}
fn material_inputs(v: &Value, settings: &Value) -> Value {
    let mut v = v.clone();
    if let Some(obj) = v.as_object_mut() { for k in ["pricing", "relative_cost_preference", "billing_basis", "tier_provenance"] { obj.remove(k); } }
    // Automatic review resolves the opposite provider at execution. The legacy
    // global selector is unused and may change back to its default on restart.
    if automatic_reviewer(settings) || crate::plan::review_cadence(settings, "reviewer") == "per_plan" {
        if let Some(obj) = v.as_object_mut() { obj.remove("reviewer"); }
    }
    v
}
fn billing_facts(price: &Value) -> Value {
    if !price.is_object() {
        return Value::Null;
    }
    json!({"currency":price["currency"],"unit":price["unit"],"basis":price["basis"],"input":price["input"],"output":price["output"]})
}

fn proposal_option<'a>(options: &'a [Value], p: &Proposal) -> Result<&'a Value, String> {
    options.iter().find(|o| o["provider"] == p.provider && o["model"] == p.model && o["effort"] == p.native_effort)
        .ok_or_else(|| format!("unknown model ID or unsupported native effort: {}/{} with {}; copy an exact option's model and effort fields, not its resolved_id", p.provider, p.model, p.native_effort))
}

fn policy_inputs(
    settings: &Value,
    stage: &Value,
    p: &Proposal,
    options: &[Value],
) -> Result<Value, String> {
    if p.tier.is_some() { return tiers::policy_inputs(stage, p); }
    classification(&p.risk, &p.complexity, &p.task)?;
    if p.rationale.trim().is_empty() || p.rationale.len() > 4000 {
        return Err("missing/oversized planner rationale".into());
    }
    validate_constraint(&stage["model_constraint"])?;
    let c = constraint(settings, stage);
    let selected = proposal_option(options, p)?;
    if selected["eligible"] != true {
        return Err(format!("model unavailable: {}", selected["error"]));
    }
    if !matches_constraint(selected, &c) {
        return Err(format!(
            "model conflicts with explicit stage/global constraint {c}: proposed {}/{} with native_effort {}; edit the constraint or proposal",
            p.provider, p.model, p.native_effort
        ));
    }
    let pending = &stage["reassessment"]["pending"];
    let old = &pending["old_agreement"];
    let min = minimum(stage, p).max(stage["routing_scope_floor"].as_u64().unwrap_or(0)).max(if pending.is_object() { old["policy_inputs"]["minimum_tier"].as_u64().unwrap_or(0) } else { 0 });
    let cost_sensitive = min == 1 || p.task == "documentation";
    let adequate = |o: &&Value| {
        o["eligible"] == true
            && rank(&o["tier"]) >= min
            && (o["suitability"]
                .as_array()
                .is_none_or(|a| a.is_empty() || a.iter().any(|v| v == &p.task || v == "general")))
    };
    if !adequate(&selected) {
        return Err(format!(
            "no adequate selection: stage requires at least the configured {} capability tier and task suitability; {}/{} is configured {}; correct the registry, the constraint or the proposal",
            match min { 1 => "basic", 2 => "standard", _ => "strong" },
            p.provider, p.model, selected["tier"].as_str().unwrap_or("unclassified")
        ));
    }
    let reviewer_deferred = crate::plan::review_cadence(settings, "reviewer") == "per_plan";
    if settings["reviewer_provider_mode"] != "configured" && !reviewer_deferred && p.provider != "mock" && settings["reviewer"] != if p.provider == "codex" { "claude" } else { "codex" }
        && (settings["automatic_routing"] == false || settings["reviewer_model"].as_str().is_some_and(|s| !s.is_empty())) {
        return Err("cross-provider-review conflict: explicit reviewer constraint prevents switch".into());
    }
    if pending.is_object() {
        if minimum(stage,p).max(stage["routing_scope_floor"].as_u64().unwrap_or(0)) < old["policy_inputs"]["minimum_tier"].as_u64().unwrap_or(0) { return Err("reassessment cannot lower the agreed risk/capability floor".into()); }
        let effective = json!({"provider":p.provider,"model":selected["resolved_id"].as_str().unwrap_or(&p.model),"native_effort":p.native_effort});
        let same = effective == old["effective"];
        let material = pending["kind"] == "material_assignment_change" || pending["kind"] == "material_scope_change";
        if stage["reassessment"]["visited"].as_array().is_some_and(|v| v.contains(&effective)) && !(same && material) {
            return Err("model oscillation or unchanged escalation rejected".into());
        }
        if rank(&selected["tier"]) < rank(&old["policy_inputs"]["tier"]) { return Err("reassessment cannot weaken capability".into()); }
        let reasoning = pending["kind"] == "repeated_reasoning_failure" || pending["kind"] == "implementer_escalation";
        if reasoning {
            let higher_efforts: Vec<_> = options.iter().filter(|o| o["eligible"] == true && o["provider"] == old["effective"]["provider"]
                && o["model"] == old["validated_proposal"]["model"] && rank(&o["tier"]) >= min && matches_constraint(o,&c)
                && effort_rank(old["effective"]["native_effort"].as_str().unwrap_or("")) > 0
                && effort_rank(o["effort"].as_str().unwrap_or("")) > effort_rank(old["effective"]["native_effort"].as_str().unwrap_or("")))
                .collect();
            if !higher_efforts.is_empty() && !higher_efforts.contains(&selected) { return Err("prefer supported effort-only escalation while capability remains adequate".into()); }
            if higher_efforts.is_empty() && rank(&selected["tier"]) <= rank(&old["policy_inputs"]["tier"]) { return Err("reasoning escalation requires a stronger suitable capability tier".into()); }
        }
        if pending["kind"] == "provider_operational_failure" && selected["provider"] == old["effective"]["provider"] {
            return Err("operational provider failure requires an eligible other provider".into());
        }
        if pending["kind"] == "context_pressure" && selected["limits"]["context_window"].as_u64().unwrap_or(0) <= old["policy_inputs"]["limits"]["context_window"].as_u64().unwrap_or(0) {
            return Err("context pressure requires a measured larger context window".into());
        }
    }
    if cost_sensitive
        && !pending.is_object() && stage["routing_validity_only"] != true
        && options.iter().filter(adequate).any(|o| {
            matches_constraint(o, &c)
                && (settings["reviewer_provider_mode"] == "configured" || reviewer_deferred || o["provider"] == "mock"
                    || settings["reviewer"]
                        == if o["provider"] == "codex" {
                            "claude"
                        } else {
                            "codex"
                        })
                && cheaper(o, selected, &settings["routing_billing_basis"])
        })
    {
        return Err("simple or documentation stage must prefer a cheaper adequate eligible option under comparable billing or configured preferences".into());
    }
    // Record only facts used about the chosen option. Re-running the policy above
    // detects new cheaper alternatives without invalidating on unrelated catalogue updates.
    Ok(
        json!({"policy":POLICY,"constraint":c,"reviewer":settings["reviewer"],"minimum_tier":min,
        "provider":selected["provider"],"model":selected["model"],"resolved_id":selected["resolved_id"].as_str().unwrap_or(&p.model),
        "effort":selected["effort"],"tier":selected["tier"],"suitability":selected["suitability"],"limits":selected["limits"],
        "tier_provenance":selected["provenance"],"relative_cost_preference":if cost_sensitive {selected["relative_cost_preference"].clone()} else {Value::Null},
        "pricing":if cost_sensitive && settings["routing_billing_basis"].is_string() {billing_facts(&selected["pricing"])} else {Value::Null},"billing_basis":if cost_sensitive {settings["routing_billing_basis"].clone()} else {Value::Null}}),
    )
}

/// Tokens are estimated from length at four bytes each, which errs towards
/// letting a borderline prompt through to be answered by the provider. An
/// unknown context window means no engine-side bound at all.
fn context_budget_error(bytes: usize, window: u64, percent: u64, provider: &str, model: &str)
    -> Option<String>
{
    let estimate = bytes as u64 / 4;
    (window > 0 && estimate > window.saturating_mul(percent) / 100).then(|| format!(
        "routing input is roughly {estimate} tokens, over {percent}% of {provider}/{model}'s \
         {window}-token context; raise reassessment_limits.context_percent, choose a model with \
         a larger context, or shorten the pending plan"))
}

fn selection_plan(plan: &Value) -> Value {
    json!({"goal":plan["goal"],"plan_id":plan["plan_id"],"revision":plan["revision"],
        "stages":plan["stages"].as_array().into_iter().flatten().map(|s| json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"],"depends_on":s["depends_on"],"status":s["status"],"model_constraint":s["model_constraint"],"model_proposal":s["model_proposal"],"reassessment":{"pending":s["reassessment"]["pending"],"visited":s["reassessment"]["visited"]},"previous_requests":s["previous_requests"]})).collect::<Vec<_>>()})
}

impl Ctx {
    fn stage_routing_settings(&self, plan: &Value, idx: usize) -> Value {
        let mut settings = self.app.settings.lock().unwrap().clone();
        settings["review_cadence"] = json!({
            "architect": crate::plan::stage_review_cadence(&settings, plan, idx, "architect"),
            "reviewer": crate::plan::stage_review_cadence(&settings, plan, idx, "reviewer"),
        });
        settings
    }
    fn routing_handoff(&self, plan: &Value) -> Result<String, String> {
        let mut cp = self.load_plan().filter(|p| p["plan_id"] == plan["plan_id"] && p["architecture"].is_object())
            .map(|p| self.architecture_store().checkpoint(&p)).transpose()?.unwrap_or(Value::Null);
        if let Some(obj) = cp.as_object_mut() { obj.remove("agreements"); }
        let cp = crate::architecture::prompt_checkpoint(&cp);
        Ok(format!("\nArchitecture checkpoint and referenced decisions: {cp}\nWorktree: {}\nUnfinished diff preview: {}\nInspect and preserve staged, unstaged and untracked partial work before advising a replacement. During execution reassessment only, higher effort cannot supply missing capability. For reasoning escalation prefer a supported higher effort on the same adequate model; otherwise propose a stronger suitable tier. Never revisit retired assignments. Operational provider failure requires another provider and a fresh independent other-provider reviewer.\n",
            self.git(&["status","--short"]).unwrap_or_else(|e| e) , self.git(&["diff","HEAD","--",".",":(exclude).forge"] ).unwrap_or_else(|e| crate::util::last_chars(&e,500)).chars().take(16000).collect::<String>()))
    }
    pub(crate) fn has_operational_alternative(&self, plan: &Value, idx: usize) -> Result<bool,String> {
        let stage = &plan["stages"][idx];
        let old_agreement = if stage["reassessment"]["pending"]["old_agreement"].is_object() {
            &stage["reassessment"]["pending"]["old_agreement"]
        } else if stage["model_selection"].is_object() { &stage["model_selection"] }
        else { &stage["model_agreement"] };
        let old: Proposal = serde_json::from_value(old_agreement["validated_proposal"].clone()).map_err(|e| e.to_string())?;
        let settings = self.stage_routing_settings(plan, idx);
        let options = self.routing_candidates()?;
        for o in &options {
            let mut p = old.clone();
            p.provider = o["provider"].as_str().unwrap_or("").into(); p.model = o["model"].as_str().unwrap_or("").into(); p.native_effort = o["effort"].as_str().unwrap_or("").into();
            if policy_inputs(&settings,stage,&p,&options).is_ok()
                && (crate::plan::review_cadence(&settings, "reviewer") == "per_plan"
                    || self.reviewer_config(&p.provider).is_ok()) { return Ok(true); }
        }
        Ok(false)
    }
    pub(crate) fn proposal_inputs(&self, plan: &Value, idx: usize) -> Value {
        if plan["stages"][idx]["model_proposal"]["tier"].is_string() {
            return json!({"stage":crate::plan::stage_inputs(plan, idx)});
        }
        let settings = self.app.settings.lock().unwrap();
        json!({"stage":crate::plan::stage_inputs(plan, idx),"constraint":constraint(&settings, &plan["stages"][idx])})
    }
    pub(crate) fn routing_prompt(&self) -> Result<String, String> {
        Ok(CONTRACT.into())
    }
    pub(crate) fn stage_selection_prompt(&self, plan: &Value, ids: &[i64]) -> Result<String, String> {
        if plan["stages"].as_array().unwrap().iter().any(|s|
            ids.contains(&s["id"].as_i64().unwrap_or(0)) && s["reassessment"]["pending"].is_object()) {
            let reviewer_mode = self.app.settings.lock().unwrap()["reviewer_provider_mode"].clone();
            Ok(format!("{EXECUTION_CONTRACT}\nReviewer settings: {}\nOptions: {}", json!({"reviewer_provider_mode":reviewer_mode}), json!(self.routing_candidates()?)))
        } else { self.routing_prompt() }
    }
    pub(crate) fn routing_required(&self, plan: &Value, cp: &Value) -> Result<Vec<i64>, String> {
        let mut ids = vec![];
        for (idx, stage) in plan["stages"]
            .as_array()
            .ok_or("invalid stages")?
            .iter()
            .enumerate()
        {
            if stage["status"] == "committed" {
                continue;
            }
            let settings = self.stage_routing_settings(plan, idx);
            let a = &cp["agreements"][stage["id"].to_string()];
            let same_inputs = a["relevant_inputs"] == crate::plan::stage_inputs(plan, idx);
            if a["version"] == 2 {
                if stage["reassessment"]["pending"].is_object() || !tiers::valid(plan, idx, a) {
                    ids.push(stage["id"].as_i64().ok_or("invalid stage id")?);
                }
                continue;
            }
            let options = self.routing_options()?;
            let mut validity_stage = stage.clone();
            validity_stage.as_object_mut().unwrap().remove("reassessment");
            validity_stage["routing_validity_only"] = json!(true);
            let check = serde_json::from_value::<Proposal>(a["validated_proposal"].clone())
                .map_err(|e| e.to_string())
                .and_then(|p| policy_inputs(&settings, &validity_stage, &p, &options));
            let valid = a["valid"] == true
                && same_inputs
                && check.as_ref().is_ok_and(|p| material_inputs(p, &settings) == material_inputs(&a["policy_inputs"], &settings));
            if stage["reassessment"]["pending"].is_object() {
                ids.push(stage["id"].as_i64().ok_or("invalid stage id")?);
                continue;
            }
            if !valid {
                // Stage 7 owns reassessment of an existing assignment after operational/material failure.
                if a["valid"] == true
                    && same_inputs
                    && a["policy_inputs"]["constraint"] == constraint(&settings, stage)
                    && a["validated_proposal"].is_object()
                {
                    return Err(format!(
                        "stage {} assignment materially invalid: {}; saved work retained. Correct model policy/availability or explicitly revise constraints before retrying",
                        stage["id"],
                        check.err().unwrap_or_else(|| {
                            "capability, resolution or cost facts changed".into()
                        })
                    ));
                }
                ids.push(stage["id"].as_i64().ok_or("invalid stage id")?);
            }
        }
        Ok(ids)
    }
    /// Only actual planner output may populate proposals; manual fields are never trusted.
    pub(crate) fn propose_routing(
        &self,
        plan: &mut Value,
        ids: &[i64],
        feedback: &Value,
    ) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let context_plan = selection_plan(plan);
        let prompt = format!(
            "{}\nRead-only planner selection turn. Return ONLY {{\"proposals\":[{{\"stage_id\":1,\"proposal\":<model_proposal>}}]}} for exactly these IDs: {ids:?}. Plan: {context_plan}\nArchitect/engine feedback: {feedback}\nWhen engine_reason rejects an otherwise agreed proposal, preserve its agreed risk, complexity and task. For planning correct only the capability tier. Concrete model/provider/effort changes belong exclusively to execution reassessment. Do not reclassify work to evade the capability policy.",
            self.stage_selection_prompt(plan, ids)?
        );
        let prompt = prompt + &self.routing_handoff(plan)?;
        let reply = self.validated_routing_response("planner", &prompt, plan, ids)?;
        for usage in &reply.usage {
            accumulate_invocation_usage(plan, "usage", &reply.choice.0, usage);
            accumulate_invocation_usage(&mut plan["role_usage"], "planner", &reply.choice.0, usage);
        }
        for (id, p) in parse_proposals(&reply.value, ids)? {
            let idx = plan["stages"].as_array().unwrap().iter().position(|s| s["id"] == id).unwrap();
            plan["stages"][idx]["model_proposal"] = json!(p);
            let inputs = self.proposal_inputs(plan, idx);
            plan["stages"][idx]["model_proposal_inputs"] = inputs;
            plan["stages"][idx]["model_proposer"] = json!({"provider":reply.choice.0,"model":reply.choice.1,"native_effort":reply.choice.2});
        }
        Ok(())
    }

    fn validated_routing_response(&self, role: &str, prompt: &str, plan: &Value, ids: &[i64])
        -> Result<crate::response::ValidatedReply<Value>, String>
    {
        let requirements = self.model_requirements(role, None)?;
        let (initial, choice) = self.with_selected_model(&requirements, None,
            |choice| self.routing_dialogue(role, &choice.0, &choice.1, &choice.2, prompt, plan, ids))?;
        let mut usage: Vec<_> = initial.usage.iter().cloned().collect();
        let (result, value) = self.repair_response(&format!("{role} routing"), initial,
            |reply| {
                if reply.output.len() > 48 * 1024 { return Err("routing output exceeds 48 KiB".into()); }
                let mut value: Value = serde_json::from_str(&reply.output)
                    .map_err(|e| format!("invalid selection output: {e}"))?;
                value.as_object_mut().ok_or("selection output must be an object")?.remove("_engine_usage");
                if role == "planner" {
                    let proposals = parse_proposals(&value, ids)?;
                    let options = if proposals.iter().any(|(_, p)| p.tier.is_none()) { self.routing_candidates()? } else { vec![] };
                    for (id, proposal) in proposals {
                        // Correct catalogue identity errors in the planner's own
                        // response loop before paying for another architect turn.
                        if proposal.tier.is_none() && plan["stages"].as_array().unwrap().iter().any(|s| s["id"] == id && s["model_proposal"].is_object()) {
                            proposal_option(&options, &proposal)?;
                        }
                    }
                }
                else { validate_evaluations(&value["model_evaluations"], ids)?; }
                Ok(value)
            },
            |reply, error| {
                let correction = crate::response::correction_prompt(prompt, &reply.output, error)
                    + "\nPut explanatory notes in rationale.";
                let result = self.routing_dialogue(role, &choice.0, &choice.1, &choice.2, &correction, plan, ids)?;
                if let Some(u) = &result.usage { usage.push(u.clone()); }
                Ok(result)
            })?;
        Ok(crate::response::ValidatedReply { value, result, choice, usage })
    }

    pub(crate) fn routing_dialogue(
        &self,
        role: &str,
        provider: &str,
        model: &str,
        effort: &str,
        prompt: &str,
        plan: &Value,
        ids: &[i64],
    ) -> Result<crate::agent::AgentResult, String> {
        if self.session.stop_requested.load(std::sync::atomic::Ordering::SeqCst) { return Err("selection stopped".into()); }
        if provider == "mock" {
            let mut settings = self.app.settings.lock().unwrap();
            let key = format!("mock_routing_{role}_requests");
            if !settings[&key].is_array() {
                settings[&key] = json!([]);
            }
            settings[&key]
                .as_array_mut()
                .unwrap()
                .push(json!({"ids":ids,"prompt":prompt}));
            if let Some(outputs) = settings[format!("mock_routing_{role}_outputs")]
                .as_array_mut()
                .filter(|a| !a.is_empty())
            {
                let output = outputs.remove(0);
                return Ok(crate::agent::AgentResult { output:output.as_str().map(str::to_owned).unwrap_or_else(|| output.to_string()), completed:true, ..Default::default() });
            }
            drop(settings);
            if role == "architect" {
                return Ok(crate::agent::AgentResult { output:json!({"model_evaluations":mock_evaluations(plan, ids)}).to_string(), completed:true, ..Default::default() });
            }
            let option = self
                .routing_candidates()?
                .into_iter()
                .find(|o| o["eligible"] == true)
                .ok_or("no eligible model")?;
            return Ok(crate::agent::AgentResult { output:json!({"proposals":ids.iter().map(|id| json!({"stage_id":id,"proposal":{"risk":"standard","complexity":"standard","task":"functionality","provider":option["provider"],"model":option["model"],"native_effort":option["effort"],"rationale":"Planner: configured adequacy for this stage."}})).collect::<Vec<_>>()}).to_string(), completed:true, ..Default::default() });
        }
        // The selected model's own context window is the real bound, so the
        // budget follows the catalogue rather than a fixed ceiling. Tokens are
        // estimated from length at four bytes each, which errs towards letting
        // a borderline prompt through and being answered by the provider.
        let (window, percent) = {
            let settings = self.app.settings.lock().unwrap();
            let policy = Policy::from_settings(&settings)?;
            let percent = settings["reassessment_limits"]["context_percent"].as_u64().unwrap_or(85);
            drop(settings);
            let window = Provider::parse(provider)
                .map(|parsed| self.model_facts(&policy, parsed, model, Some(effort)))
                .and_then(|facts| facts["limits"]["context_window"].as_u64())
                .unwrap_or(0);
            (window, percent)
        };
        if let Some(error) = context_budget_error(prompt.len(), window, percent, provider, model) {
            return Err(error);
        }
        let output = self.run_agent(&AgentRequest {
            role,
            provider,
            model,
            effort,
            session: None,
            prompt,
        })?;
        if self.session.stop_requested.load(std::sync::atomic::Ordering::SeqCst) { return Err("selection stopped".into()); }
        let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
        let selected = self.model_facts(&policy, Provider::parse(provider).ok_or("invalid selection provider")?, model, Some(effort));
        if !output.model_reported || selected["eligible"] != true || !crate::agent::same_model(provider, selected["resolved_id"].as_str().unwrap_or(model), &output.effective_model) { return Err("selection effective-model mismatch or missing report".into()); }
        if !output.completed { return Err("incomplete selection dialogue".into()); }
        Ok(output)
    }
    pub(crate) fn agree_routing(
        &self,
        plan: &mut Value,
        cp: &mut Value,
        ids: &[i64],
        evaluations: &Value,
    ) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut evaluations = evaluations.clone();
        let mut dialogue: std::collections::BTreeMap<i64, Vec<Value>> =
            std::collections::BTreeMap::new();
        let mut engine_corrections: std::collections::BTreeMap<i64, (Proposal, String)> =
            std::collections::BTreeMap::new();
        for exchange in 0..=1 {
            let options = if ids.iter().any(|id| plan["stages"].as_array().unwrap().iter().any(|s| s["id"] == *id && !s["model_proposal"]["tier"].is_string())) { self.routing_candidates()? } else { vec![] };
            let rows: Vec<Evaluation> = serde_json::from_value(evaluations.clone())
                .map_err(|e| format!("invalid architect model evaluations: {e}"))?;
            if rows.len() != ids.len() {
                return Err("architect evaluation count mismatch".into());
            }
            let mut disagreements = vec![];
            for id in ids {
                let matched: Vec<_> = rows.iter().filter(|r| r.stage_id == *id).collect();
                if matched.len() != 1 {
                    return Err("duplicate/missing architect model evaluation".into());
                }
                let e = matched[0];
                classification(&e.risk, &e.complexity, &e.task)?;
                if e.rationale.trim().is_empty() || e.rationale.len() > 4000 {
                    return Err("missing architect rationale".into());
                }
                let stage = plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["id"] == *id)
                    .unwrap();
                let p: Proposal = serde_json::from_value(stage["model_proposal"].clone())
                    .map_err(|e| e.to_string())?;
                if let Some((agreed, error)) = engine_corrections.get(id) {
                    if p.risk != agreed.risk || p.complexity != agreed.complexity || p.task != agreed.task {
                        return Err(format!("{error}; model-selection correction must preserve the agreed risk, complexity and task; revise stage scope separately"));
                    }
                }
                let turns = dialogue.entry(*id).or_default();
                if turns.last().is_none_or(|last| {
                    last["proposal"] != json!(p) || last["evaluation"] != json!(e)
                }) {
                    turns.push(json!({"exchange":exchange,"planner_proposal_id":crate::architecture::identity(),
                        "architect_evaluation_id":crate::architecture::identity(),"proposal":p,"evaluation":e}));
                }
                if !e.agree || e.risk != p.risk || e.complexity != p.complexity || e.task != p.task
                {
                    disagreements.push(json!({"stage_id":id,"architect_reason":e.rationale,"risk":e.risk,"complexity":e.complexity,"task":e.task}));
                } else {
                    let idx = plan["stages"].as_array().unwrap().iter().position(|s| s["id"] == *id).unwrap();
                    let settings = self.stage_routing_settings(plan, idx);
                    if let Err(error) = policy_inputs(&settings, stage, &p, &options) {
                        // Agreement between models does not waive engine policy.
                        // Return the exact rejection through the same bounded
                        // exchange, before publishing any stage's agreement.
                        engine_corrections.entry(*id).or_insert_with(|| (p.clone(), error.clone()));
                        turns.last_mut().unwrap()["engine_reason"] = json!(error);
                        disagreements.push(json!({"stage_id":id,"architect_reason":e.rationale,
                            "engine_reason":error,"risk":e.risk,"complexity":e.complexity,"task":e.task}));
                    }
                }
            }
            if !disagreements.is_empty() {
                if exchange == 1 {
                    return Err(format!(
                        "planner/architect disagreement after one reconciliation exchange: {}. Initial engine rejections: {}. Revise stage constraints or scope and retry; previous plan retained",
                        json!(disagreements), json!(engine_corrections.iter().map(|(id,(_,error))| json!({"stage_id":id,"error":error})).collect::<Vec<_>>())
                    ));
                }
                let affected: Vec<i64> = disagreements
                    .iter()
                    .map(|d| d["stage_id"].as_i64().unwrap())
                    .collect();
                self.propose_routing(plan, &affected, &json!(disagreements))?;
                let context_plan = selection_plan(plan);
                let prompt = format!(
                    "{}\n{}\nPlan: {context_plan}\nSaved architecture: {}\nEvaluate exactly stage IDs {affected:?}. Previous disagreement: {}",
                    self.stage_selection_prompt(plan, &affected)?,
                    EVALUATION_CONTRACT,
                    crate::architecture::prompt_checkpoint(cp),
                    json!(disagreements)
                );
                let prompt = prompt + &self.routing_handoff(plan)?;
                let reply = self.validated_routing_response("architect", &prompt, plan, &affected)?;
                for usage in &reply.usage {
                    accumulate_invocation_usage(plan,"usage",&reply.choice.0,usage);
                    accumulate_invocation_usage(&mut plan["role_usage"],"architect",&reply.choice.0,usage);
                }
                let replacements = reply.value["model_evaluations"].as_array().unwrap();
                let all = evaluations.as_array_mut().unwrap();
                all.retain(|r| !affected.contains(&r["stage_id"].as_i64().unwrap_or(0)));
                all.extend(replacements.iter().cloned());
                continue;
            }
            for id in ids {
                let idx = plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|s| s["id"] == *id)
                    .unwrap();
                let stage = &plan["stages"][idx];
                let settings = self.stage_routing_settings(plan, idx);
                let p: Proposal = serde_json::from_value(stage["model_proposal"].clone())
                    .map_err(|e| e.to_string())?;
                if p.tier.is_some() {
                    let e = rows.iter().find(|e| e.stage_id == *id).unwrap();
                    tiers::publish(plan, cp, idx, &p, e, &dialogue[id])?;
                    continue;
                }
                let inputs = policy_inputs(&settings, stage, &p, &options)?;
                let reviewer = if crate::plan::review_cadence(&settings, "reviewer") == "per_plan" {
                    json!({"status":"deferred"})
                } else {
                    let (provider, model) = self.reviewer_config(&p.provider)?;
                    json!({"provider":provider,"model":model})
                };
                let option = options
                    .iter()
                    .find(|o| {
                        o["provider"] == p.provider
                            && o["model"] == p.model
                            && o["effort"] == p.native_effort
                    })
                    .unwrap();
                let e = rows.iter().find(|e| e.stage_id == *id).unwrap();
                let agreement_id = crate::architecture::identity();
                let old = &cp["agreements"][id.to_string()];
                let turns = &dialogue[id];
                let last = turns.last().unwrap();
                let record = json!({"version":1,"id":agreement_id,"kind":"agreement","agreement_id":agreement_id,"valid":true,"agreed":true,
                    "proposal_ids":[last["planner_proposal_id"],last["architect_evaluation_id"]],"dialogue":turns,"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":id,
                    "architectural_constraints":cp["constraints"],"relevant_inputs":crate::plan::stage_inputs(plan, idx),"input_fingerprint":crate::metadata::fingerprint(json!({"stage":crate::plan::stage_inputs(plan,idx),"policy":inputs}).to_string().as_bytes()),"policy_inputs":inputs,
                    "reviewer":reviewer,"effective":{"provider":p.provider,"model":option["resolved_id"].as_str().unwrap_or(&p.model),"native_effort":p.native_effort},
                    "validated_proposal":p,"planner_reason":p.rationale,"architect_reason":e.rationale,"planner_bootstrap":stage["model_proposer"],
                    "architect_bootstrap":cp["routing_evaluator"],
                    "provenance":{"capability_policy_version":POLICY,"catalogue_revision":option["policy_revision"].as_str().unwrap_or("configured"),"official_sources":option["official_source"].as_str().into_iter().collect::<Vec<_>>(),"checked_unix":crate::util::unix_timestamp()},
                    "availability":if option["availability_unverified"] == true {"unverified"} else {"verified"},"verification_state":option["availability"],
                    "trigger":stage["reassessment"]["pending"]["kind"].as_str().unwrap_or("joint_assignment"),"trigger_evidence":stage["reassessment"]["pending"]["evidence"],"superseded_agreement":old["id"],"unix":crate::util::unix_timestamp()});
                cp["agreements"][id.to_string()] = record.clone();
                plan["stages"][idx]["model_agreement"] = record.clone();
                plan["stages"][idx].as_object_mut().unwrap().remove("model_selection");
                if plan["stages"][idx]["reassessment"]["pending"].is_object() {
                    let state = &mut plan["stages"][idx]["reassessment"];
                    let pending = state["pending"].clone();
                    state["history"].as_array_mut().unwrap().push(json!({"kind":pending["kind"],"evidence":pending["evidence"],"old_agreement":pending["old_agreement"],"new_agreement":record,"planner_reason":record["planner_reason"],"architect_reason":record["architect_reason"]}));
                    state.as_object_mut().unwrap().remove("pending");
                    state["status"] = json!("reusing");
                }
                plan["stages"][idx]
                    .as_object_mut()
                    .unwrap()
                    .remove("model_block");
            }
            return Ok(());
        }
        unreachable!()
    }
    pub(crate) fn validated_assignment(&self, plan: &Value, idx: usize) -> Result<Value, String> {
        self.check_assignment(plan, idx, false)
    }
    pub(crate) fn restored_assignment(&self, plan: &Value, idx: usize) -> Result<Value, String> {
        self.check_assignment(plan, idx, true)
    }
    fn check_assignment(&self, plan: &Value, idx: usize, ignore_reservation: bool) -> Result<Value, String> {
        let current = self.load_plan().ok_or("missing saved plan")?;
        if let Some(error) = current["stages"][idx]["model_block"].as_str() {
            return Err(error.into());
        }
        if current["plan_id"] != plan["plan_id"]
            || crate::plan::stage_inputs(&current, idx) != crate::plan::stage_inputs(plan, idx)
        {
            return Err("launch plan changed; reconcile before implementation".into());
        }
        let cp = self.architecture_store().checkpoint(&current)?;
        // Only the launching stage is checked; unrelated pending stages have their own boundary.
        let mut local = current;
        if ignore_reservation {
            local["stages"][idx].as_object_mut().unwrap().remove("reassessment");
        }
        for (i, s) in local["stages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            if i != idx {
                s["status"] = json!("committed");
            }
        }
        if !self.routing_required(&local, &cp)?.is_empty() {
            return Err("stage assignment missing/stale; reconcile before implementation".into());
        }
        let a = &cp["agreements"][plan["stages"][idx]["id"].to_string()];
        if a["architectural_constraints"] != cp["constraints"] { return Err("material architectural constraints changed".into()); }
        if a["version"] == 2 { return self.resolve_tier_assignment(&local, idx, a); }
        Ok(a.clone())
    }
}
pub(crate) const EVALUATION_CONTRACT: &str = r#"Independently evaluate each required capability proposal against cross-stage constraints and failure impact. Planning agrees a tier without a provider or model; only active execution reassessment evaluates a concrete replacement and cost policy. A planner proposal is not your endorsement. Include model_evaluations:[{"stage_id":1,"agree":true,"rationale":"independent architectural reasons","risk":"simple|standard|critical","complexity":"simple|standard|complex","task":"documentation|functionality|concurrency|persistence|security"}]. Explicit agreement requires both classifications to match. On disagreement, explain corrections; only one planner/architect reconciliation exchange is allowed. For a selection-only turn return ONLY {"model_evaluations":[...]} and do not write files."#;
pub(crate) fn mock_evaluations(plan: &Value, ids: &[i64]) -> Value {
    json!(ids.iter().map(|id| { let p = &plan["stages"].as_array().unwrap().iter().find(|s| s["id"] == *id).unwrap()["model_proposal"];
        json!({"stage_id":id,"agree":true,"rationale":"Architect: independently checked cross-stage interfaces and failure impact.","risk":p["risk"],"complexity":p["complexity"],"task":p["task"]}) }).collect::<Vec<_>>())
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
