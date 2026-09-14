use super::*;

pub(super) fn validate_checkpoint(cp: &Value, plan: &Value) -> Result<(), String> {
    if cp["version"] != VERSION
        || !cp["summary"].is_string()
        || !cp["guidance"].is_object()
        || !cp["agreements"].is_object()
        || !cp["recent_decisions"]
            .as_array()
            .is_some_and(|a| a.len() <= 8)
        || !matches!(
            cp["context_status"].as_str(),
            Some("inactive" | "ready" | "needs_recovery")
        )
    {
        return Err("invalid architectural checkpoint structure".into());
    }
    storage::pack(cp, CHECKPOINT_LIMIT, "architectural checkpoint")?;
    let policy: crate::contracts::ReviewPolicy =
        serde_json::from_value(cp["review_policy"].clone()).map_err(|e| e.to_string())?;
    if policy.version != VERSION
        || policy.rationale.trim().is_empty()
        || policy.scope.trim().is_empty()
        || policy.required_roles.is_empty()
    {
        return Err("invalid review policy".into());
    }
    for d in cp["recent_decisions"].as_array().unwrap() {
        if !d["id"].as_str().is_some_and(safe_id)
            || !d["summary"].is_string()
            || !matches!(
                d["status"].as_str(),
                Some("proposed" | "accepted" | "rejected" | "superseded")
            )
        {
            return Err("invalid decision summary".into());
        }
    }
    for key in ["guidance", "agreements"] {
        for (stage_key, record) in cp[key].as_object().unwrap() {
            let stage_id = record["stage_id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or("invalid checkpoint stage identity")?;
            let revision = record["revision"]
                .as_u64()
                .filter(|r| *r > 0 && *r <= plan["revision"].as_u64().unwrap_or(0))
                .ok_or("invalid checkpoint record revision")?;
            if record
                .get("plan_id")
                .is_some_and(|id| *id != plan["plan_id"])
                || stage_key != &stage_id.to_string()
                || (record["version"] != VERSION && !(key == "agreements" && record["version"] == 2))
                || !record["id"].as_str().is_some_and(safe_id)
                || !record["valid"].is_boolean()
                || !record["unix"].as_i64().is_some_and(|t| t >= 0)
            {
                return Err("invalid checkpoint record contract".into());
            }
            let inputs = &record["relevant_inputs"];
            if !inputs.is_object()
                || !inputs["dependencies"]
                    .as_array()
                    .is_some_and(|deps| deps.iter().all(|id| id.as_i64().is_some_and(|id| id > 0)))
                || ["goal", "title", "instructions", "acceptance", "commit"]
                    .iter()
                    .any(|key| !inputs[*key].is_string())
            {
                return Err("invalid checkpoint record inputs".into());
            }
            let index = plan["stages"]
                .as_array()
                .ok_or("invalid stages")?
                .iter()
                .position(|stage| stage["id"] == stage_id);
            if record["valid"] == true {
                let index = index.ok_or("active checkpoint record for missing stage")?;
                if plan["stages"][index]["status"] != "committed" && *inputs != crate::plan::stage_inputs(plan, index) {
                    return Err("active checkpoint record inputs are stale".into());
                }
            } else if index.is_none()
                && (revision >= plan["revision"].as_u64().unwrap()
                    || record["invalidation_trigger"] != "stage_removed")
            {
                return Err(
                    "missing stage requires an explicitly invalid historical record".into(),
                );
            }
            if key == "agreements" {
                let model = validate_model(record)?;
                if model.plan_id != plan["plan_id"].as_str().unwrap_or("")
                    || !matches!(model.kind, crate::contracts::SelectionKind::Agreement)
                {
                    return Err("checkpoint agreement identity/kind mismatch".into());
                }
            } else {
                let guidance: crate::contracts::Guidance =
                    serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
                if guidance.text.trim().is_empty() {
                    return Err("empty checkpoint guidance".into());
                }
            }
        }
    }
    if !cp["session"].is_null() {
        let _: crate::contracts::SessionReference =
            serde_json::from_value(cp["session"].clone()).map_err(|e| e.to_string())?;
        let s = &cp["session"];
        for key in ["provider", "reference", "checkpoint_reference"] {
            if !s[key].as_str().is_some_and(|v| !v.is_empty()) {
                return Err("invalid exact session reference".into());
            }
        }
        if s["resume_policy"] == "exact_if_committed" {
            if !s["reference"].as_str().is_some_and(crate::agent::session_id)
                || !cp["last_turn"].as_str().is_some_and(safe_id)
                || cp["last_turn"] != s["checkpoint_reference"]
                || cp["effective_model"]["provider"] != s["provider"]
                || !cp["effective_model"]["model"].as_str().is_some_and(|s| !s.is_empty()) {
                return Err("invalid committed architect session".into());
            }
            for key in ["constraints", "completed_interfaces"] {
                if !cp[key].as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 1000))) {
                    return Err("invalid saved architect constraints/interfaces".into());
                }
            }
            let mut ids = std::collections::BTreeSet::new();
            if !cp["unresolved_risks"].as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(|r|
                r["id"].as_str().is_some_and(|id| safe_id(id) && ids.insert(id.to_string())) &&
                r["text"].as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 1000))) {
                return Err("invalid saved architect risks".into());
            }
        }
        if s["resume_policy"] != "fork_from_checkpoint" && s["resume_policy"] != "exact_if_committed" {
            return Err("unsafe session resume policy".into());
        }
    }
    Ok(())
}

pub(super) fn validate_model(record: &Value) -> Result<crate::contracts::ModelRecord, String> {
    use crate::contracts::SelectionKind;
    let m: crate::contracts::ModelRecord =
        serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
    let effective_valid = if m.version == 2 {
        matches!(m.kind, SelectionKind::Agreement) && m.effective.is_none()
            && crate::routing::valid_tier_record(record)
    } else {
        m.version == VERSION && m.effective.as_ref().is_some_and(|e|
            !e.provider.trim().is_empty() && !e.model.trim().is_empty()
            && record["effective"].as_object().is_some_and(|m| m.contains_key("native_effort"))
            && e.native_effort.as_ref().is_none_or(|s| !s.trim().is_empty()))
    };
    if !effective_valid
        || !safe_id(&m.id)
        || !safe_id(&m.plan_id)
        || m.revision == 0
        || m.stage_id <= 0
        || m.unix < 0
        || m.provenance.checked_unix < 0
        || m.provenance.capability_policy_version.trim().is_empty()
        || m.provenance.catalogue_revision.trim().is_empty()
        || m.trigger.trim().is_empty()
        || m.proposal_ids.iter().any(|id| !safe_id(id))
        || m.agreement_id.as_ref().is_some_and(|id| !safe_id(id))
        || m.superseded_agreement
            .as_ref()
            .is_some_and(|id| !safe_id(id) || *id == m.id)
    {
        return Err("incomplete model selection metadata".into());
    }
    if matches!(m.kind, SelectionKind::Agreement)
        && (m.planner_reason.trim().is_empty()
            || m.architect_reason.trim().is_empty()
            || m.proposal_ids.is_empty()
            || m.agreement_id.as_ref() != Some(&m.id))
    {
        return Err("agreement requires proposals, identity and both participants' reasons".into());
    }
    Ok(m)
}

pub(super) fn validate_reviews(store: &Store, plan: &Value) -> Result<(), String> {
    if let Some(reference) = plan["architecture"].get("plan_review_history") {
        crate::review_history::validate(&store.review_dir(plan)?, reference)?;
        if plan["plan_review"]["reviews"]["$forge_reviews"] != reference["file"] {
            return Err("plan review/reference mismatch".into());
        }
    } else if plan["plan_review"]["reviews"]["$forge_reviews"].is_string() {
        return Err("missing plan review reference".into());
    }
    if let Some(refs) = plan["architecture"].get("review_history") {
        let dir = store.review_dir(plan)?;
        for (id, reference) in refs.as_object().ok_or("invalid review manifest")? {
            if !id.parse::<i64>().is_ok_and(|id| id > 0) {
                return Err("invalid review stage".into());
            }
            crate::review_history::validate(&dir, reference)?;
        }
        for stage in plan["stages"].as_array().ok_or("invalid stages")? {
            if stage["reviews"].is_object()
                && stage["reviews"].get("$forge_reviews").is_some()
                && (refs[stage["id"].to_string()].is_null()
                    || stage["reviews"]["$forge_reviews"]
                        != refs[stage["id"].to_string()]["file"])
            {
                return Err("review stage/reference mismatch".into());
            }
        }
    }
    Ok(())
}

pub(super) enum ValidatedRecord {
    Decision(crate::contracts::Decision),
    Model(crate::contracts::ModelRecord),
    Guidance(crate::contracts::Guidance),
    Review,
}

pub(super) fn validate_record_identity(plan: &Value, record: &Value) -> Result<(), String> {
    if record["version"] != VERSION
        || !record["id"].as_str().is_some_and(safe_id)
        || record["revision"] != plan["revision"]
    {
        return Err("invalid record identity/version/revision".into());
    }
    if let Some(id) = record.get("plan_id") {
        if *id != plan["plan_id"] {
            return Err("record belongs to another plan".into());
        }
    }
    if !record["stage_id"].is_null()
        && !plan["stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == record["stage_id"])
    {
        return Err("record belongs to an unknown stage".into());
    }
    Ok(())
}

pub(super) fn validate_record(
    plan: &Value,
    checkpoint: &Value,
    kind: &str,
    record: &Value,
) -> Result<ValidatedRecord, String> {
    match kind {
        "decision" => {
            let decision: crate::contracts::Decision =
                serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
            if decision.summary.trim().is_empty()
                || decision.rationale.trim().is_empty()
                || decision.created_unix < 0
                || decision.updated_unix < decision.created_unix
                || decision
                    .supersedes
                    .as_ref()
                    .is_some_and(|id| !safe_id(id) || *id == decision.id)
            {
                return Err("invalid decision rationale/timestamps".into());
            }
            Ok(ValidatedRecord::Decision(decision))
        }
        "model" => {
            let model = validate_model(record)?;
            if matches!(model.kind, crate::contracts::SelectionKind::Agreement) {
                if model.planner_reason.trim().is_empty()
                    || model.architect_reason.trim().is_empty()
                {
                    return Err("agreement requires both participants' reasons".into());
                }
                let index = plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|stage| stage["id"] == model.stage_id)
                    .ok_or("unknown stage")?;
                if model.relevant_inputs != crate::plan::stage_inputs(plan, index) {
                    return Err("agreement inputs are stale".into());
                }
            } else if matches!(model.kind, crate::contracts::SelectionKind::Invalidation) {
                let active = &checkpoint["agreements"][model.stage_id.to_string()];
                if active.is_object()
                    && model
                        .agreement_id
                        .as_ref()
                        .is_none_or(|id| active["id"] != *id)
                {
                    return Err("invalidation agreement mismatch".into());
                }
            }
            Ok(ValidatedRecord::Model(model))
        }
        "guidance" => {
            let guidance: crate::contracts::Guidance =
                serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
            let index = plan["stages"]
                .as_array()
                .unwrap()
                .iter()
                .position(|stage| stage["id"] == guidance.stage_id)
                .ok_or("unknown stage")?;
            if guidance.relevant_inputs != crate::plan::stage_inputs(plan, index) {
                return Err("guidance inputs are stale".into());
            }
            Ok(ValidatedRecord::Guidance(guidance))
        }
        "review" => {
            let review: crate::contracts::ReviewRecord =
                serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
            if review.attempt_id.is_empty()
                || review.policy.version != VERSION
                || review.policy.required_roles.is_empty()
                || review.policy.rationale.trim().is_empty()
                || review.policy.scope.trim().is_empty()
                || review.round == 0
                || !matches!(
                    review.role,
                    crate::contracts::Role::Architect | crate::contracts::Role::Reviewer
                )
            {
                return Err("invalid review role/attempt/round".into());
            }
            Ok(ValidatedRecord::Review)
        }
        _ => Err("unknown architecture record kind".into()),
    }
}
