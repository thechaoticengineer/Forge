//! Joint, bounded stage routing. Global catalogue clocks are never validity inputs.
use crate::{
    agent::AgentRequest,
    app::Ctx,
    catalogue::{Policy, Provider},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const POLICY: &str = "stage-routing-1";
pub(crate) const CONTRACT: &str = r#"MODEL SELECTION CONTRACT: Include model_proposal on each new or materially changed pending stage:
{"risk":"simple|standard|critical","complexity":"simple|standard|complex","task":"documentation|functionality|concurrency|persistence|security","provider":"exact provider","model":"exact registry ID","native_effort":"provider_default or supported native effort","rationale":"stage-specific adequacy, failure impact and cost reasoning"}.
Use only eligible catalogue/registry options. Configured tiers are explicit adequacy policy, not official evidence. Critical or complex work requires strong; never infer quality from price or model name. Simple work prefers a cheaper adequate option only with comparable published billing data or configured relative preferences. Unknown price remains unknown. Constraints narrow choices first, but never waive capability or independent-review requirements. Preserve acceptance and committed stages. Unchanged agreements need no new proposal."#;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proposal {
    risk: String,
    complexity: String,
    task: String,
    provider: String,
    model: String,
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
        return json!({"provider":settings["implementer"],"model":settings["implementer_model"]});
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
fn minimum(stage: &Value, p: &Proposal) -> u64 {
    let text = format!(
        "{} {} {}",
        stage["title"], stage["instructions"], stage["acceptance"]
    )
    .to_lowercase();
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
fn billing_facts(price: &Value) -> Value {
    if !price.is_object() {
        return Value::Null;
    }
    json!({"currency":price["currency"],"unit":price["unit"],"basis":price["basis"],"input":price["input"],"output":price["output"]})
}

fn policy_inputs(
    settings: &Value,
    stage: &Value,
    p: &Proposal,
    options: &[Value],
) -> Result<Value, String> {
    classification(&p.risk, &p.complexity, &p.task)?;
    if p.rationale.trim().is_empty() || p.rationale.len() > 4000 {
        return Err("missing/oversized planner rationale".into());
    }
    validate_constraint(&stage["model_constraint"])?;
    let c = constraint(settings, stage);
    let selected = options.iter().find(|o| o["provider"] == p.provider && o["model"] == p.model && o["effort"] == p.native_effort)
        .ok_or("unknown model ID or unsupported native effort; choose an exact eligible registry option")?;
    if selected["eligible"] != true {
        return Err(format!("model unavailable: {}", selected["error"]));
    }
    if !matches_constraint(selected, &c) {
        return Err("model conflicts with explicit stage/global constraint; edit the constraint or proposal".into());
    }
    let min = minimum(stage, p);
    let adequate = |o: &&Value| {
        o["eligible"] == true
            && rank(&o["tier"]) >= min
            && (o["suitability"]
                .as_array()
                .is_none_or(|a| a.is_empty() || a.iter().any(|v| v == &p.task || v == "general")))
    };
    if !adequate(&selected) {
        return Err(format!(
            "no adequate selection: stage requires {} configured capability tier and task suitability; correct registry or constraint",
            if min == 3 {
                "strong (strongest suitable)"
            } else {
                "standard/basic"
            }
        ));
    }
    if p.provider != "mock"
        && settings["reviewer"]
            != if p.provider == "codex" {
                "claude"
            } else {
                "codex"
            }
    {
        return Err("cross-provider-review conflict: configure the other reviewer provider or narrow implementation choices".into());
    }
    if min == 1
        && options.iter().filter(adequate).any(|o| {
            matches_constraint(o, &c)
                && (o["provider"] == "mock"
                    || settings["reviewer"]
                        == if o["provider"] == "codex" {
                            "claude"
                        } else {
                            "codex"
                        })
                && cheaper(o, selected, &settings["routing_billing_basis"])
        })
    {
        return Err("simple stage must prefer a cheaper adequate eligible option under comparable billing or configured preferences".into());
    }
    // Record only facts used about the chosen option. Re-running the policy above
    // detects new cheaper alternatives without invalidating on unrelated catalogue updates.
    Ok(
        json!({"policy":POLICY,"constraint":c,"reviewer":settings["reviewer"],"minimum_tier":min,
        "provider":selected["provider"],"model":selected["model"],"resolved_id":selected["resolved_id"].as_str().unwrap_or(&p.model),
        "effort":selected["effort"],"tier":selected["tier"],"suitability":selected["suitability"],"limits":selected["limits"],
        "tier_provenance":selected["provenance"],"relative_cost_preference":if min == 1 {selected["relative_cost_preference"].clone()} else {Value::Null},
        "pricing":if min == 1 && settings["routing_billing_basis"].is_string() {billing_facts(&selected["pricing"])} else {Value::Null},"billing_basis":if min == 1 {settings["routing_billing_basis"].clone()} else {Value::Null}}),
    )
}

fn selection_plan(plan: &Value) -> Value {
    json!({"goal":plan["goal"],"plan_id":plan["plan_id"],"revision":plan["revision"],
        "stages":plan["stages"].as_array().into_iter().flatten().map(|s| json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"],"depends_on":s["depends_on"],"status":s["status"],"model_constraint":s["model_constraint"],"model_proposal":s["model_proposal"]})).collect::<Vec<_>>()})
}

impl Ctx {
    pub(crate) fn proposal_inputs(&self, plan: &Value, idx: usize) -> Value {
        let settings = self.app.settings.lock().unwrap();
        json!({"stage":crate::plan::stage_inputs(plan, idx),"constraint":constraint(&settings, &plan["stages"][idx])})
    }
    pub(crate) fn routing_options(&self) -> Result<Vec<Value>, String> {
        let settings = self.app.settings.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(options) = settings["mock_routing_options"].as_array() {
            return Ok(options.clone());
        }
        if settings["implementer"] == "mock" {
            return Ok(vec![
                json!({"provider":"mock","model":settings["implementer_model"].as_str().filter(|s| !s.is_empty()).unwrap_or("mock-implementer"),"availability":"configured_unverified","effort":"provider_default","eligible":true,"tier":"strong","provenance":"configured","availability_unverified":true}),
            ]);
        }
        let policy = Policy::from_settings(&settings)?;
        let details = self.app.catalogue.details(&policy);
        let mut options = details["options"].as_array().unwrap().clone();
        for entry in &policy.entries {
            let efforts = details["providers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["provider"] == entry.provider.name())
                .flat_map(|p| p["models"].as_array().into_iter().flatten())
                .filter(|m| {
                    m["id"] == entry.model
                        || m["aliases"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!(entry.model)))
                })
                .flat_map(|m| m["supported_efforts"].as_array().into_iter().flatten())
                .filter_map(Value::as_str);
            for effort in std::iter::once("provider_default").chain(efforts) {
                if effort != entry.effort {
                    options.push(self.app.catalogue.select(
                        &policy,
                        entry.provider,
                        &entry.model,
                        effort,
                    ));
                }
            }
        }
        let metadata = self
            .app
            .metadata
            .details(&self.app.catalogue.metadata_snapshot(&policy));
        for option in &mut options {
            if let Some(record) = metadata["records"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| {
                    r["provider"] == option["provider"]
                        && (r["model"] == option["model"] || r["model"] == option["resolved_id"])
                        && r["removed"] != true
                })
            {
                option["pricing"] = record["pricing"].clone();
                option["official_source"] = record["source_url"].clone();
            }
        }
        Ok(options)
    }
    pub(crate) fn routing_prompt(&self) -> Result<String, String> {
        let settings = self.app.settings.lock().unwrap().clone();
        Ok(format!(
            "{CONTRACT}\nOptions: {}\nRouting settings: {}",
            json!(self.routing_options()?),
            json!({"automatic_routing":settings["automatic_routing"],"implementer":settings["implementer"],"implementer_model":settings["implementer_model"],"reviewer":settings["reviewer"],"billing_basis":settings["routing_billing_basis"]})
        ))
    }
    pub(crate) fn routing_required(&self, plan: &Value, cp: &Value) -> Result<Vec<i64>, String> {
        let settings = self.app.settings.lock().unwrap().clone();
        let options = self.routing_options()?;
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
            let a = &cp["agreements"][stage["id"].to_string()];
            let same_inputs = a["relevant_inputs"] == crate::plan::stage_inputs(plan, idx);
            let check = serde_json::from_value::<Proposal>(a["validated_proposal"].clone())
                .map_err(|e| e.to_string())
                .and_then(|p| policy_inputs(&settings, stage, &p, &options));
            let valid = a["valid"] == true
                && same_inputs
                && check.as_ref().is_ok_and(|p| *p == a["policy_inputs"]);
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
            "{}\nRead-only planner selection turn. Return ONLY {{\"proposals\":[{{\"stage_id\":1,\"proposal\":<model_proposal>}}]}} for exactly these IDs: {ids:?}. Plan: {context_plan}\nArchitect disagreement: {feedback}",
            self.routing_prompt()?
        );
        let (provider, model, effort) = self.bootstrap("planner")?;
        let output =
            self.routing_dialogue("planner", &provider, &model, &effort, &prompt, plan, ids)?;
        let rows = output["proposals"]
            .as_array()
            .ok_or("planner omitted routing proposals")?;
        if rows.len() != ids.len() {
            return Err("planner proposal count mismatch".into());
        }
        for id in ids {
            let matches: Vec<_> = rows.iter().filter(|r| r["stage_id"] == *id).collect();
            if matches.len() != 1 {
                return Err("duplicate or missing planner proposal".into());
            }
            let p: Proposal = serde_json::from_value(matches[0]["proposal"].clone())
                .map_err(|e| e.to_string())?;
            let idx = plan["stages"]
                .as_array()
                .unwrap()
                .iter()
                .position(|s| s["id"] == *id)
                .unwrap();
            let inputs = self.proposal_inputs(plan, idx);
            plan["stages"][idx]["model_proposal"] = json!(p);
            plan["stages"][idx]["model_proposal_inputs"] = inputs;
            plan["stages"][idx]["model_proposer"] =
                json!({"provider":provider,"model":model,"native_effort":effort});
        }
        Ok(())
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
    ) -> Result<Value, String> {
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
                return Ok(outputs.remove(0));
            }
            drop(settings);
            if role == "architect" {
                return Ok(json!({"model_evaluations":mock_evaluations(plan, ids)}));
            }
            let option = self
                .routing_options()?
                .into_iter()
                .find(|o| o["eligible"] == true)
                .ok_or("no eligible model")?;
            return Ok(
                json!({"proposals":ids.iter().map(|id| json!({"stage_id":id,"proposal":{"risk":"standard","complexity":"standard","task":"functionality","provider":option["provider"],"model":option["model"],"native_effort":option["effort"],"rationale":"Planner: configured adequacy for this stage."}})).collect::<Vec<_>>()}),
            );
        }
        if prompt.len() > 256 * 1024 {
            return Err("routing input exceeds 256 KiB; shorten the pending plan".into());
        }
        let output = self.run_agent(&AgentRequest {
            role,
            provider,
            model,
            effort,
            session: None,
            prompt,
        })?;
        if output.output.len() > 48 * 1024 {
            return Err("routing output exceeds 48 KiB".into());
        }
        if !output.completed {
            return Err("incomplete selection dialogue".into());
        }
        serde_json::from_str(&output.output).map_err(|e| format!("invalid selection output: {e}"))
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
        for exchange in 0..=1 {
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
                }
            }
            if !disagreements.is_empty() {
                if exchange == 1 {
                    return Err(format!(
                        "planner/architect disagreement after one reconciliation exchange: {}. Revise stage constraints or scope and retry; previous plan retained",
                        json!(disagreements)
                    ));
                }
                let affected: Vec<i64> = disagreements
                    .iter()
                    .map(|d| d["stage_id"].as_i64().unwrap())
                    .collect();
                self.propose_routing(plan, &affected, &json!(disagreements))?;
                let (provider, model, effort) = self.bootstrap("architect")?;
                let context_plan = selection_plan(plan);
                let prompt = format!(
                    "{}\n{}\nPlan: {context_plan}\nSaved architecture: {cp}\nEvaluate exactly stage IDs {affected:?}. Previous disagreement: {}",
                    self.routing_prompt()?,
                    EVALUATION_CONTRACT,
                    json!(disagreements)
                );
                let output = self.routing_dialogue(
                    "architect",
                    &provider,
                    &model,
                    &effort,
                    &prompt,
                    plan,
                    &affected,
                )?;
                let replacements = output["model_evaluations"]
                    .as_array()
                    .ok_or("missing reconciliation evaluations")?;
                if replacements.len() != affected.len()
                    || affected
                        .iter()
                        .any(|id| replacements.iter().filter(|r| r["stage_id"] == *id).count() != 1)
                {
                    return Err("invalid reconciliation evaluation IDs".into());
                }
                let all = evaluations.as_array_mut().unwrap();
                all.retain(|r| !affected.contains(&r["stage_id"].as_i64().unwrap_or(0)));
                all.extend(replacements.iter().cloned());
                continue;
            }
            let settings = self.app.settings.lock().unwrap().clone();
            let options = self.routing_options()?;
            for id in ids {
                let idx = plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|s| s["id"] == *id)
                    .unwrap();
                let stage = &plan["stages"][idx];
                let p: Proposal = serde_json::from_value(stage["model_proposal"].clone())
                    .map_err(|e| e.to_string())?;
                let inputs = policy_inputs(&settings, stage, &p, &options)?;
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
                    "relevant_inputs":crate::plan::stage_inputs(plan, idx),"input_fingerprint":crate::metadata::fingerprint(json!({"stage":crate::plan::stage_inputs(plan,idx),"policy":inputs}).to_string().as_bytes()),"policy_inputs":inputs,
                    "effective":{"provider":p.provider,"model":option["resolved_id"].as_str().unwrap_or(&p.model),"native_effort":p.native_effort},
                    "validated_proposal":p,"planner_reason":p.rationale,"architect_reason":e.rationale,"planner_bootstrap":stage["model_proposer"],
                    "architect_bootstrap":cp["routing_evaluator"],
                    "provenance":{"capability_policy_version":POLICY,"catalogue_revision":option["policy_revision"].as_str().unwrap_or("configured"),"official_sources":option["official_source"].as_str().into_iter().collect::<Vec<_>>(),"checked_unix":crate::util::unix_timestamp()},
                    "availability":if option["availability_unverified"] == true {"unverified"} else {"verified"},"verification_state":option["availability"],
                    "trigger":"joint_assignment","superseded_agreement":old["id"],"unix":crate::util::unix_timestamp()});
                cp["agreements"][id.to_string()] = record.clone();
                plan["stages"][idx]["model_agreement"] = record;
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
        Ok(cp["agreements"][plan["stages"][idx]["id"].to_string()].clone())
    }
}
pub(crate) const EVALUATION_CONTRACT: &str = r#"Independently evaluate each required model proposal against cross-stage constraints, failure impact and capability/cost policy. A planner proposal is not your endorsement. Include model_evaluations:[{"stage_id":1,"agree":true,"rationale":"independent architectural reasons","risk":"simple|standard|critical","complexity":"simple|standard|complex","task":"documentation|functionality|concurrency|persistence|security"}]. Explicit agreement requires both classifications to match. On disagreement, explain corrections; only one planner/architect reconciliation exchange is allowed. For a selection-only turn return ONLY {"model_evaluations":[...]} and do not write files."#;
pub(crate) fn mock_evaluations(plan: &Value, ids: &[i64]) -> Value {
    json!(ids.iter().map(|id| { let p = &plan["stages"].as_array().unwrap().iter().find(|s| s["id"] == *id).unwrap()["model_proposal"];
        json!({"stage_id":id,"agree":true,"rationale":"Architect: independently checked cross-stage interfaces and failure impact.","risk":p["risk"],"complexity":p["complexity"],"task":p["task"]}) }).collect::<Vec<_>>())
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
