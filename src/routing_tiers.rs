//! Provider-independent planning and local, attempt-scoped execution binding.
use super::*;

pub(super) fn policy_inputs(stage: &Value, p: &Proposal) -> Result<Value, String> {
    validate_proposal(p)?;
    validate_constraint(&stage["model_constraint"])?;
    if stage["reassessment"]["pending"].is_object() {
        return Err("execution reassessment requires a concrete replacement and its native effort".into());
    }
    let tier = p.tier.as_ref().ok_or("missing capability tier")?;
    let floor = minimum(stage, p).max(stage["routing_scope_floor"].as_u64().unwrap_or(0));
    if rank(&json!(tier)) < floor {
        return Err(format!("proposed tier {tier} is below the engine capability floor {floor}"));
    }
    Ok(json!({"policy":"stage-tier-2","minimum_tier":rank(&json!(tier)),
        "tier":tier,"tier_provenance":"joint_stage_requirement",
        "constraint":stage["model_constraint"]}))
}

pub(super) fn valid(plan: &Value, idx: usize, a: &Value) -> bool {
    a["valid"] == true
        && a["relevant_inputs"] == crate::plan::stage_inputs(plan, idx)
        && serde_json::from_value::<Proposal>(a["validated_proposal"].clone())
            .ok().filter(|p| p.tier.is_some()).is_some_and(|p|
                policy_inputs(&plan["stages"][idx], &p).is_ok_and(|inputs| inputs == a["policy_inputs"]))
}

pub(super) fn publish(plan: &mut Value, cp: &mut Value, idx: usize,
    p: &Proposal, e: &Evaluation, turns: &[Value]) -> Result<(), String>
{
    let stage = &plan["stages"][idx];
    let inputs = policy_inputs(stage, p)?;
    let id = crate::architecture::identity();
    let last = turns.last().ok_or("missing tier agreement dialogue")?;
    let record = json!({"version":2,"id":id,"kind":"agreement","agreement_id":id,
        "valid":true,"agreed":true,"binding":"at_implementation_start",
        "proposal_ids":[last["planner_proposal_id"],last["architect_evaluation_id"]],
        "dialogue":turns,"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":stage["id"],
        "architectural_constraints":cp["constraints"],"relevant_inputs":crate::plan::stage_inputs(plan,idx),
        "policy_inputs":inputs,"validated_proposal":p,"effective":null,"reviewer":{"status":"deferred"},
        "planner_reason":p.rationale,"architect_reason":e.rationale,
        "planner_bootstrap":stage["model_proposer"],"architect_bootstrap":cp["routing_evaluator"],
        "provenance":{"capability_policy_version":"stage-tier-2","catalogue_revision":"deferred",
            "official_sources":[],"checked_unix":crate::util::unix_timestamp()},
        "availability":"unverified","verification_state":"deferred",
        "trigger":"joint_tier_requirement","superseded_agreement":cp["agreements"][stage["id"].to_string()]["id"],
        "unix":crate::util::unix_timestamp()});
    cp["agreements"][stage["id"].to_string()] = record.clone();
    plan["stages"][idx]["model_agreement"] = record;
    for key in ["model_selection", "model_block"] {
        plan["stages"][idx].as_object_mut().unwrap().remove(key);
    }
    Ok(())
}

impl Ctx {
    pub(super) fn resolve_tier_assignment(&self, plan: &Value, idx: usize, agreement: &Value)
        -> Result<Value, String>
    {
        let stage = &plan["stages"][idx];
        let mut settings = self.stage_routing_settings(plan, idx);
        let options = self.routing_candidates()?;
        let saved = &stage["model_selection"];
        // Once work starts, retries retain the captured choice. Changing the
        // selector affects the next stage/attempt, never an in-flight invocation.
        if stage["attempt_id"].is_string() && saved["attempt_id"] == stage["attempt_id"]
            && saved["agreement_id"] == agreement["id"] {
            let p: Proposal = serde_json::from_value(saved["validated_proposal"].clone()).map_err(|e| e.to_string())?;
            let mut check_stage = stage.clone();
            check_stage["model_constraint"] = saved["policy_inputs"]["constraint"].clone();
            check_stage["routing_scope_floor"] = agreement["policy_inputs"]["minimum_tier"].clone();
            check_stage["routing_validity_only"] = json!(true);
            let facts = policy_inputs_concrete(&settings, &check_stage, &p, &options)?;
            if material_inputs(&facts, &settings) != material_inputs(&saved["policy_inputs"], &settings) {
                return Err("captured execution model facts changed; saved work retained".into());
            }
            return Ok(saved.clone());
        }

        let mut c = constraint(&settings, stage);
        if c["provider"].is_null() { c["provider"] = settings["implementer"].clone(); }
        let provider = c["provider"].as_str().ok_or("choose an implementer provider before starting")?;
        let floor = agreement["policy_inputs"]["minimum_tier"].as_u64().ok_or("missing agreed tier")?;
        let task = agreement["validated_proposal"]["task"].as_str().ok_or("missing agreed task")?;
        let policy = Policy::from_settings(&settings)?;
        let mut candidates: Vec<_> = options.iter().filter(|o|
            o["eligible"] == true && matches_constraint(o, &c) && rank(&o["tier"]) >= floor
            && o["suitability"].as_array().is_none_or(|a| a.is_empty() || a.iter().any(|v| v == task || v == "general"))
            // Native effort belongs to the selected provider's entry, not to a
            // provider-independent plan. An explicit stage effort can override it.
            && (c["native_effort"].is_string() || o["effort"] == policy.entries.iter()
                .find(|e| e.provider.name() == provider && o["model"] == e.model)
                .map(|e| e.execution_effort()).unwrap_or("provider_default")))
            .collect();
        candidates.sort_by_key(|o| rank(&o["tier"]));
        let weakest = candidates.first().map(|o| rank(&o["tier"])).ok_or_else(|| format!(
            "No eligible {provider} model meets the agreed {} tier and task/constraints. Update Model settings & options or choose another implementer provider, then retry; the plan remains valid.",
            agreement["policy_inputs"]["tier"].as_str().unwrap_or("required")))?;
        candidates.retain(|o| rank(&o["tier"]) == weakest);
        if candidates.iter().all(|o| o["relative_cost_preference"].as_u64().is_some()) {
            candidates.sort_by_key(|o| o["relative_cost_preference"].as_u64().unwrap());
        }
        let option = candidates[0];
        let mut p: Proposal = serde_json::from_value(agreement["validated_proposal"].clone()).map_err(|e| e.to_string())?;
        p.tier = None;
        p.provider = provider.into();
        p.model = option["model"].as_str().ok_or("missing registry model")?.into();
        p.native_effort = option["effort"].as_str().ok_or("missing registry effort")?.into();
        let mut check_stage = stage.clone();
        check_stage["model_constraint"] = c;
        check_stage["routing_scope_floor"] = json!(floor);
        check_stage["routing_validity_only"] = json!(true);
        settings["implementer"] = json!(p.provider);
        let inputs = policy_inputs_concrete(&settings, &check_stage, &p, &options)?;
        let reviewer = if crate::plan::review_cadence(&settings, "reviewer") == "per_plan" {
            json!({"status":"deferred"})
        } else {
            let (provider, model) = self.reviewer_config(&p.provider)?;
            json!({"provider":provider,"model":model})
        };
        let mut selection = agreement.clone();
        selection["version"] = json!(1);
        selection["kind"] = json!("selection");
        selection["id"] = json!(crate::architecture::identity());
        selection["attempt_id"] = stage["attempt_id"].clone();
        selection["binding"] = json!("implementation_start");
        selection["policy_inputs"] = inputs;
        selection["validated_proposal"] = json!(p);
        selection["effective"] = json!({"provider":p.provider,"model":option["resolved_id"].as_str().unwrap_or(&p.model),"native_effort":p.native_effort});
        selection["reviewer"] = reviewer;
        selection["availability"] = json!(if option["availability_unverified"] == true { "unverified" } else { "verified" });
        selection["verification_state"] = option["availability"].clone();
        selection["provenance"]["catalogue_revision"] = json!(option["policy_revision"].as_str().unwrap_or("configured"));
        selection["provenance"]["official_sources"] = json!(option["official_source"].as_str().into_iter().collect::<Vec<_>>());
        selection["unix"] = json!(crate::util::unix_timestamp());
        Ok(selection)
    }
}

// Avoid shadowing the tier-only policy function above.
use super::policy_inputs as policy_inputs_concrete;
