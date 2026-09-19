//! Agent invocation, retries, durable review publication, and commit recovery.
use super::{
    Ctx, acceptance_criteria_items, aggregate_review_gate, classify_review_scope,
    dual_review_policy, normalize_review_verdict, partition_review_policy_by_cadence,
    review_snapshot, stage_required_roles,
};
use crate::agent::{AgentRequest, AgentResult, AgentUsage};
use crate::prompts::{FIX_PROMPT, IMPLEMENT_PROMPT, REVIEW_PROMPT};
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy)]
pub(super) enum ReviewScope { Stage(usize), Plan }

/// Where a round goes after the engine exports its changed designs.
pub(super) enum DesignExport { Ready, Blocked, Failed }

impl Ctx {
    pub(super) fn review_with_retry(&self, plan: &mut Value, idx: usize, base: &Value, role: &str, provider: &str, model: &str) -> Result<Value,String> {
        let mut current = (provider.to_string(), model.to_string());
        let mut visited = vec![current.clone()];
        loop {
            match self.invoke_review(plan,ReviewScope::Stage(idx),base,role,&current.0,&current.1) {
                Ok(v) => {
                    plan["stages"][idx]["reassessment"]["status"] = json!("reusing");
                    self.save_plan(plan)?;
                    return Ok(v);
                },
                Err(error) => {
                    // Quota can be learned from the pre-launch probe or a CLI refusal.
                    // Switch only the fresh independent reviewer; keep this exact
                    // snapshot, round and required roles. Never loop over a model twice.
                    if role == "reviewer"
                        && let Some(next) = self.reviewer_quota_fallback(plan, idx, &current, &error, &visited)
                            .map_err(|e| format!("model routing blocked: {e}; work and checkpoint retained"))?
                        && !visited.contains(&next) {
                        visited.push(next.clone());
                        current = next;
                        continue;
                    }
                    if self.operational_retry(plan,idx,&error,role)? { continue; }
                    return Err(format!("model routing blocked: {role}/{}: {error}; restore an eligible independent reviewer; work and checkpoint retained", current.0));
                }
            }
        }
    }

    pub(super) fn reviewer_quota_fallback(&self, plan: &Value, idx: usize, current: &(String, String), error: &str, visited: &[(String, String)]) -> Result<Option<(String, String)>, String> {
        let implementer = plan["stages"][idx]["implementer_provider"].as_str().ok_or("missing implementer provider for reviewer fallback")?;
        let requirements = self.model_requirements("reviewer",Some(implementer))?;
        let choice = (current.0.clone(),current.1.clone(),"provider_default".into());
        Ok(self.model_fallback(&requirements,&choice,error,visited)?.map(|next| (next.0,next.1)))
    }

    fn architect_review_with_selection(&self, plan: &mut Value, idx: usize, base: &Value) -> Result<Value,String> {
        self.architect_review_for_scope(plan, ReviewScope::Stage(idx), base)
    }

    pub(super) fn architect_review_for_scope(&self, plan: &mut Value, scope: ReviewScope, base: &Value) -> Result<Value, String> {
        let requirements = self.model_requirements("architect",None)?;
        let (verdict,_) = self.with_selected_model(&requirements,None, |choice| {
            *plan = self.load_plan().ok_or("missing current review plan")?;
            let cp = self.architecture_store().checkpoint(plan)?;
            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
            let changed = crate::catalogue::Provider::parse(&choice.0).is_some_and(|provider| {
                let facts = self.model_facts(&policy,provider,&choice.1,Some(&choice.2));
                !crate::agent::same_model(&choice.0,facts["resolved_id"].as_str().unwrap_or(&choice.1),
                    cp["effective_model"]["model"].as_str().unwrap_or(""))
            });
            if changed || cp["session"]["provider"] != choice.0 || cp["context_status"] != "ready" {
                *plan = self.architect_publish_selected(plan.clone(),Some(plan),
                    "review model recovery",Some(choice.clone()))?;
            }
            self.invoke_review(plan,scope,base,"architect",&choice.0,&choice.1)
        })?;
        Ok(verdict)
    }
    pub(super) fn invoke_review(
        &self,
        plan: &mut Value,
        scope: ReviewScope,
        base: &Value,
        role: &str,
        provider: &str,
        model: &str,
    ) -> Result<Value, String> {
        let _guard = self.session.architect_lock.lock().unwrap();
        let mut identity = base.clone();
        identity["role"] = json!(role);
        *plan = self.load_plan().ok_or("missing current review plan")?;
        let mut cp = self.architecture_store().checkpoint(plan)?;
        let (mut prompt, acceptance, guidance, step) = match scope {
            ReviewScope::Stage(idx) => {
                let mut context_stage = plan["stages"][idx].clone();
                // Only preceding findings, never the other role's current endorsement.
                context_stage["last_verdict"] = context_stage["previous_requests"].clone();
                context_stage["last_verdict_valid"] = json!(true);
                if plan["stages"][idx]["attempt_id"] != base["attempt_id"] {
                    return Err("plan identity changed before review".into());
                }
                (self.stage_prompt(REVIEW_PROMPT, plan, &context_stage)?,
                    self.stage_review_acceptance(plan, idx)?,
                    cp["guidance"][context_stage["id"].to_string()].clone(), context_stage["id"].as_i64())
            }
            ReviewScope::Plan => {
                self.validate_plan_subject(plan)?;
                if plan["plan_review"]["attempt_id"] != base["attempt_id"] {
                    return Err("plan review attempt changed".into());
                }
                if role == "reviewer" { self.validate_plan_reviewer(plan, provider)?; }
                (self.plan_review_prompt(plan), plan["plan_review"]["acceptance"].as_str().unwrap_or("").to_string(), cp["guidance"].clone(), None)
            }
        };
        // Reviewer and architect prompts share this path: point both at any
        // changed designs' PNGs, never at editing instructions.
        let design_base = match scope {
            ReviewScope::Stage(idx) => plan["stages"][idx]["attempt_head"].as_str().map(str::to_owned),
            ReviewScope::Plan => plan["plan_review"]["base"].as_str().map(str::to_owned),
        };
        if let Some(base_sha) = design_base {
            let root = Path::new(self.project());
            let files = crate::pen::changed_pen_files(root, &base_sha).unwrap_or_default();
            if !files.is_empty() {
                let designs: Vec<(String, Vec<String>)> =
                    files.iter().map(|f| (f.clone(), crate::pen::exported_pngs(root, f))).collect();
                prompt.push_str(&crate::pen::reviewer_instructions(&designs));
            }
        }
        // Business test rules (S31-S33) apply to every plan; each section is
        // absent when it has nothing to say, so other prompts stay unchanged.
        if let Some(reference) = crate::feature_context::feature_reference(&plan["feature"]) {
            prompt.push('\n');
            prompt.push_str(&reference);
        }
        let registered = crate::features::registered_business_tests(Path::new(self.project()));
        if let Some(section) = crate::feature_context::registry_section(&registered) {
            prompt.push_str(&section);
        }
        let diff_base = match scope {
            ReviewScope::Stage(idx) => plan["stages"][idx]["attempt_head"].as_str(),
            ReviewScope::Plan => plan["plan_review"]["base"].as_str(),
        };
        if let Some(diff_base) = diff_base {
            let changed = self.changed_business_tests(diff_base, &registered)?;
            if let Some(section) = crate::feature_context::changed_section(&changed) {
                prompt.push_str(&section);
            }
        }
        if role == "architect" {
            prompt = prompt.replacen(
                "You are an independent reviewer in a fresh session.",
                "You are this plan's persistent architect in the saved session.",
                1,
            );
        }
        if plan["plan_id"] != base["plan_id"]
            || plan["revision"] != base["revision"]
        {
            return Err("plan identity changed before review".into());
        }
        match scope {
            ReviewScope::Stage(_) => prompt.push_str(&format!(
                "\nCRITERIA TO EVIDENCE: {}\n", json!(acceptance_criteria_items(&acceptance)))),
            ReviewScope::Plan => prompt.push_str(&format!(
                "\nCRITERIA TO EVIDENCE:\n{acceptance}\n")),
        }
        prompt.push_str(&format!("\nREVIEW IDENTITY (echo exactly): {identity}\nSaved constraints: {}\nCompleted interfaces: {}\nGuidance: {}\nDecision history: .forge/architecture/{}/events.jsonl\n", cp["constraints"], cp["completed_interfaces"], guidance, plan["plan_id"].as_str().unwrap()));
        crate::durable_json::publish_pretty(&self.forge_path("review-identity.json"), &identity)?;
        prompt.push_str("\nThe engine also wrote the exact identity to .forge/review-identity.json (read-only during this review). Assemble your final verdict in private /tmp using a script: load that file with json.load, assign the resulting object to verdict['identity'], and serialize the verdict with json.dumps. Return that exact serialized JSON. Do not manually transcribe hashes or reconstruct the identity. The engine still validates the complete identity and rejects any mismatch.\n");
        if role == "architect" {
            let records = match scope { ReviewScope::Stage(idx) => &plan["stages"][idx]["reviews"], ReviewScope::Plan => &plan["plan_review"]["reviews"] };
            let requests: Vec<_> = records
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| {
                    r["identity"]["round"] == base["round"]
                        && r["identity"]["attempt_id"] == base["attempt_id"]
                })
                .flat_map(Ctx::review_requests)
                .collect();
            prompt.push_str(&format!(
                "Current actionable independent requests (not an endorsement): {requests:?}\n"
            ));
            prompt.push_str("\nYou are the persistent architect reviewing recorded design, cross-stage interfaces and regressions. Your verdict has independent authority. For conflicting requests, record architectural clarification in architecture_context_gap without dismissing either role's unresolved findings. When the stage's (or plan's) own constraints contradict each other, set constraint_conflict for the planner instead of rejecting round after round.\n");
            prompt.push_str(crate::prompts::REVIEWER_HISTORY_RULE);
            prompt.push('\n');
            prompt.push_str(crate::prompts::REVIEWER_CONFLICT_RULE);
            prompt.push('\n');
        }
        self.set_step(
            step,
            &format!("reviewing ({role})"),
        );
        for name in ["verdict.json", "architect-verdict.json"] {
            match fs::remove_file(self.forge_path(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let turn = crate::architecture::identity();
        let pending = self
            .forge_path("architecture")
            .join(plan["plan_id"].as_str().unwrap())
            .join("architect-pending.json");
        let session = cp["session"]["reference"].as_str().map(str::to_owned);
        if role == "architect" {
            if cp["session"]["provider"] != provider || cp["context_status"] != "ready" {
                return Err("architect session requires recovery".into());
            }
            crate::durable_json::publish_pretty(
                &pending,
                &json!({"turn":turn,"previous_session":cp["session"]}),
            )?;
        }
        let mock = provider == "mock";
        #[cfg(test)] let mock = mock || self.app.settings.lock().unwrap()["test_fake_providers"] == true;
        let invoke = |prompt: &str| -> Result<AgentResult, String> {
            #[cfg(test)] {
                let mut settings = self.app.settings.lock().unwrap();
                if !settings["test_review_sessions"].is_array() { settings["test_review_sessions"] = json!([]); }
                settings["test_review_sessions"].as_array_mut().unwrap().push(json!({"role":role,"provider":provider,"model":model,"session":if role == "architect" {session.clone()} else {None},"prompt":prompt}));
            }
            let result = if mock {
                self.mock_review(&identity, &prompt, &json!({"acceptance":acceptance}))
            } else {
                let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let facts = self.model_facts(&policy,crate::catalogue::Provider::parse(provider).ok_or("invalid review provider")?,model,None);
                let effort = facts["effort"].as_str().unwrap_or("provider_default");
                self.run_agent(&AgentRequest {
                    role: if role == "architect" {
                        "architect_review"
                    } else {
                        "reviewer"
                    },
                    provider,
                    model,
                    effort: &effort,
                    session: if role == "architect" {
                        session.as_deref()
                    } else {
                        None
                    },
                    prompt: &prompt,
                })
            }?;
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Err("review stopped; approval invalid".into());
            }
            if review_snapshot(self.project())? != base["snapshot"] {
                return Err("implementation or HEAD changed during review".into());
            }
            if matches!(scope, ReviewScope::Plan) {
                let current = self.load_plan().ok_or("missing plan after review")?;
                self.validate_plan_subject(&current)?;
                if current["plan_review"] != plan["plan_review"] || current["revision"] != base["revision"] {
                    return Err("plan review changed during invocation".into());
                }
            }
            if !mock {
                let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let selected = self.model_facts(&policy, crate::catalogue::Provider::parse(provider).ok_or("invalid reviewer provider")?, model, None);
                let expected = selected["resolved_id"].as_str().unwrap_or(model);
                if !result.model_reported || selected["eligible"] != true || !crate::agent::same_model(provider, expected, &result.effective_model) {
                    return Err(format!("model routing blocked: {role} effective model or eligibility changed (expected {expected}, reported {}, model_reported {}, eligible {})",
                        result.effective_model, result.model_reported, selected["eligible"]));
                }
            }
            if role == "architect"
                && (result.session.as_deref() != session.as_deref()
                    || result
                        .session
                        .as_deref()
                        .is_none_or(|s| !crate::agent::session_id(s)))
            {
                return Err("architect review session identity mismatch".into());
            }
            Ok(result)
        };
        let initial = invoke(&prompt)?;
        let mut response_usage: Vec<_> = initial.usage.iter().cloned().collect();
        let (result, mut verdict) = self.repair_response(&format!("{role} review"), initial,
            |response| normalize_review_verdict(&response.output, &identity, &acceptance),
            |response, error| {
                let correction = crate::response::correction_prompt(&prompt, &response.output, error);
                let result = invoke(&correction)?;
                if let Some(usage) = &result.usage { response_usage.push(usage.clone()); }
                Ok(result)
            })?;
        verdict["version"] = json!(1);
        verdict["id"] = json!(crate::architecture::identity());
        verdict["plan_id"] = base["plan_id"].clone();
        verdict["stage_id"] = base["stage_id"].clone();
        verdict["provider"] = json!(provider);
        verdict["model"] = json!(if result.effective_model.is_empty() {
            model
        } else {
            &result.effective_model
        });
        verdict["fresh_session"] = json!(role == "reviewer");
        verdict["role"] = json!(role);
        verdict["round"] = base["round"].clone();
        verdict["attempt_id"] = base["attempt_id"].clone();
        verdict["revision"] = base["revision"].clone();
        verdict["policy"] = base["policy"].clone();
        verdict["unix"] = json!(unix_timestamp());
        match scope {
            ReviewScope::Stage(idx) => {
                let stage = &mut plan["stages"][idx];
                stage["context_valid"] = json!(true);
                stage.as_object_mut().unwrap().entry("reviews").or_insert(json!([]))
                    .as_array_mut().unwrap().push(verdict.clone());
                if role == "reviewer" {
                    stage["last_verdict"] = verdict.clone();
                    stage["last_verdict_valid"] = json!(true);
                }
            }
            ReviewScope::Plan => {
                verdict["scope"] = json!("plan");
                plan["plan_review"]["reviews"].as_array_mut().ok_or("invalid plan review records")?.push(verdict.clone());
            }
        }
        if role == "architect" {
            let reference = result
                .session
                .as_deref()
                .filter(|s| crate::agent::session_id(s))
                .ok_or("missing architect session identity")?;
            if Some(reference) != session.as_deref() {
                return Err("architect review changed session identity".into());
            }
            cp["last_turn"] = json!(turn);
            cp["session"]["checkpoint_reference"] = json!(turn);
        }
        if role == "architect" || matches!(scope, ReviewScope::Plan) {
            let _lock = self.session.persistence_lock.lock().unwrap();
            let store = self.architecture_store();
            let publication = store.publish(
                plan.clone(),
                cp,
                json!({"kind":if matches!(scope, ReviewScope::Plan) {"plan_review"} else {"architect_review"},"reviews":[verdict.clone()],"turn":turn}),
            );
            match publication {
                Ok(published) => *plan = published,
                Err(error) => {
                    if matches!(scope, ReviewScope::Plan) {
                        // Never save an unpublished role verdict via a later error checkpoint.
                        *plan = store.load()?.ok_or("missing plan after failed verdict publication")?;
                    }
                    return Err(error);
                }
            }
        } else {
            self.save_plan(plan)?;
        }
        // Only now is the verdict durable, so its scenario results may be recorded.
        self.record_scenario_results(plan, &acceptance, &verdict);
        for usage in response_usage {
            match scope {
                ReviewScope::Stage(idx) => self.record_stage_usage(plan, idx, role, provider, Some(usage))?,
                ReviewScope::Plan => self.record_plan_usage(plan, role, provider, Some(usage))?,
            }
        }
        Ok(verdict)
    }

    /// Appends one scenario result per verdict criterion that is a scenario
    /// criterion of the reviewed acceptance (S38): a plan review's, or a final
    /// stage review's when `stage_review_acceptance` added them. Only the
    /// independent reviewer's verdicts are test results (S38, D18); the
    /// architect's verdict follows it and would otherwise shadow the latest
    /// result. Runtime metadata only; a failed write is logged and never
    /// changes the verdict, and a plan without a feature records nothing and
    /// touches no state.
    fn record_scenario_results(&self, plan: &Value, acceptance: &str, verdict: &Value) {
        let feature = &plan["feature"];
        if !feature.is_object() || verdict["role"] != "reviewer" {
            return;
        }
        let required = acceptance_criteria_items(acceptance);
        let pairs: Vec<(String, String)> = crate::feature_context::scenario_criterion_pairs(feature)
            .into_iter()
            .filter(|(_, criterion)| required.contains(&criterion.as_str()))
            .collect();
        let matched: Vec<(&str, &Value)> = verdict["criteria"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let text = item["criterion"].as_str()?;
                pairs.iter().find(|(_, criterion)| criterion == text).map(|(id, _)| (id.as_str(), item))
            })
            .collect();
        if matched.is_empty() {
            return;
        }
        let slug = feature["slug"].as_str().unwrap_or("");
        let scenarios = crate::feature_state::validate_slug(slug)
            .and_then(|()| crate::feature_content::read_scenarios(&crate::feature_state::feature_dir(self, slug)));
        let entries: Vec<Value> = matched
            .into_iter()
            .map(|(id, item)| {
                let hash = scenarios.as_deref().ok().and_then(|text| crate::feature_content::scenario_hash(text, id).ok().flatten());
                json!({
                    "scenario_id": id,
                    "status": if item["status"] == "passed" { "passed" } else { "failed" },
                    "evidence": item["evidence"],
                    "role": verdict["role"],
                    "plan_id": verdict["plan_id"],
                    "milestone": feature["milestone"],
                    "unix": verdict["unix"],
                    "scenario_hash": hash,
                })
            })
            .collect();
        let count = entries.len();
        match crate::feature_state::append_scenario_results(self, slug, entries) {
            Ok(()) => self.log_event("features", &format!("feature {slug} recorded {count} scenario results")),
            Err(error) => self.log_event("error", &format!("could not record scenario results of feature {slug}: {error}")),
        }
    }

    /// The acceptance a stage review evidences. For the final stage of a
    /// milestone plan whose roles all review per stage, no plan review runs, so
    /// the stage carries the scenario criteria (S31) instead. Review prompts,
    /// mock reviews, normalization and commit revalidation all use this string.
    pub(super) fn stage_review_acceptance(&self, plan: &Value, idx: usize) -> Result<String, String> {
        let acceptance = plan["stages"][idx]["acceptance"].as_str().unwrap_or("");
        let last = plan["stages"].as_array().is_some_and(|stages| stages.len() == idx + 1);
        if last && plan["feature"].is_object() && self.deferred_plan_roles(plan)?.is_empty() {
            return Ok(crate::feature_context::with_scenario_criteria(acceptance, &plan["feature"]));
        }
        Ok(acceptance.to_string())
    }

    /// Registered business test files that the diff from `base` to the working
    /// tree (staged and unstaged) modifies, deletes or renames away (S33). The
    /// engine discloses them to reviewers and never blocks the diff (D14).
    pub(super) fn changed_business_tests(&self, base: &str, registered: &[String]) -> Result<Vec<String>, String> {
        let output = self.git(&["diff", "--name-status", "-z", "-M", base, "--", ".", ":(exclude).forge"])?;
        let affected = crate::feature_context::affected_paths(&output);
        if affected.is_empty() { return Ok(Vec::new()); }
        // A deleted or renamed file is no longer registered by discovery, so
        // also count every file declared at the base or in the working tree.
        let mut known: Vec<String> = registered.to_vec();
        let feature_file = |path: &str| path.strip_prefix("docs/features/")
            .and_then(|rest| rest.strip_suffix("/milestones.md"))
            .is_some_and(|slug| !slug.is_empty() && !slug.contains('/') && !slug.starts_with('_'));
        let listed = self.git(&["ls-tree", "-r", "--name-only", base, "--", "docs/features"])?;
        for path in listed.lines().filter(|path| feature_file(path)) {
            if let Ok(text) = self.git(&["show", &format!("{base}:{path}")]) {
                known.extend(crate::features::declared_business_tests(&text));
            }
        }
        let root = Path::new(self.project()).join("docs/features");
        for entry in fs::read_dir(root).into_iter().flatten().flatten() {
            if let Ok(text) = fs::read_to_string(entry.path().join("milestones.md"))
                && !entry.file_name().to_string_lossy().starts_with('_') {
                known.extend(crate::features::declared_business_tests(&text));
            }
        }
        let mut changed: Vec<String> = affected.into_iter().filter(|path| known.contains(path)).collect();
        changed.sort();
        changed.dedup();
        Ok(changed)
    }

    fn mock_review(
        &self,
        identity: &Value,
        prompt: &str,
        stage: &Value,
    ) -> Result<AgentResult, String> {
        let role = identity["role"].as_str().unwrap();
        let mut settings = self.app.settings.lock().unwrap();
        let key = format!("mock_{role}_prompts");
        settings
            .as_object_mut()
            .unwrap()
            .entry(key)
            .or_insert(json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!(prompt));
        let key = if role == "architect" {
            "mock_architect_verdicts"
        } else {
            "mock_verdicts"
        };
        #[cfg(test)]
        if let Some(action) = settings[format!("mock_{role}_actions")]
            .as_array_mut()
            .filter(|a| !a.is_empty())
            .map(|a| a.remove(0))
        {
            if let Some(writes) = action["write"].as_object() {
                for (path, content) in writes {
                    fs::write(
                        PathBuf::from(self.project()).join(path),
                        content.as_str().unwrap(),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            if let Some(args) = action["git"].as_array() {
                self.git(&args.iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>())?;
            }
            if action["stop"] == true {
                self.session.stop_requested.store(true, Ordering::SeqCst);
            }
            if let Some(error) = action["error"].as_str() {
                return Err(error.into());
            }
        }
        let supplied = settings[key]
            .as_array_mut()
            .filter(|a| !a.is_empty())
            .map(|a| a.remove(0));
        let mut v = supplied.unwrap_or(json!({"approved":true,"summary":"Inspected mock implementation","issues":[],"checks":["Verified fixture"]}));
        if let Some(raw) = v.as_str() {
            let raw = raw.to_string();
            drop(settings);
            let session = self.architecture_store().checkpoint(&self.load_plan().unwrap())?["session"]["reference"]
                .as_str().map(str::to_owned);
            return Ok(AgentResult { output:raw, session, completed:true, ..AgentResult::default() });
        }
        if v["identity"].is_null() {
            v["identity"] = identity.clone();
        }
        if v["requires_dual"].is_null() {
            v["requires_dual"] = json!(false);
        }
        // Only an approval needs per-criterion evidence; a rejection that names
        // no criteria must not gain invented "passed" results (S38).
        if v["criteria"].is_null() && v["approved"] != false {
            v["criteria"] = json!(acceptance_criteria_items(stage["acceptance"].as_str().unwrap_or("")).iter().map(|s| json!({"criterion":s,"status":"passed","evidence":"Mock individual criterion verified"})).collect::<Vec<_>>());
        }
        if v["notes"].is_null() {
            v["notes"] = json!([]);
        }
        if v["summary"].is_null() {
            v["summary"] = json!("Mock review");
        }
        if v["checks"].is_null() {
            v["checks"] = json!(["Verified mock fixture"]);
        }
        if v["acceptance_evidence"].is_null() {
            v["acceptance_evidence"] = json!({"acceptance":stage["acceptance"].as_str().unwrap_or(""),"verified":true,"evidence":"Mock acceptance verified"});
        }
        if v["project_checks"].is_null() {
            v["project_checks"] = json!([{"command":"mock checks","status":"passed","evidence":"Mock checks passed"}]);
        }
        let usage = settings["mock_usage"].as_object().map(|u| AgentUsage {
            input_tokens: u.get("input").and_then(Value::as_i64).unwrap_or(0),
            output_tokens: u.get("output").and_then(Value::as_i64).unwrap_or(0),
            total_tokens: u.get("total").and_then(Value::as_i64).unwrap_or(0),
            model: u.get("model").and_then(Value::as_str).unwrap_or("").into(),
        });
        drop(settings);
        if usage.is_some() {
            self.log_agent_finished(role, "mock", "", usage.as_ref());
        }
        let session = self
            .architecture_store()
            .checkpoint(&self.load_plan().unwrap())?["session"]["reference"]
            .as_str()
            .map(str::to_owned);
        Ok(AgentResult {
            output: v.to_string(),
            session,
            usage,
            completed: true,
            ..AgentResult::default()
        })
    }

    pub(in crate::app) fn run_review_stage(
        &self,
        plan: &mut Value,
        idx: usize,
    ) -> Result<&'static str, String> {
        let budget = plan["stages"][idx]["review_budget"].as_u64().unwrap_or(0);
        let sid = plan["stages"][idx]["id"].as_i64().unwrap();
        if plan["stages"][idx]["scope_clarification"]["source_inputs"] == crate::plan::stage_inputs(plan, idx)
            && let Some(reason) = plan["stages"][idx]["scope_clarification"]["blocked_reason"].as_str() {
            plan["stages"][idx]["review_gate"] = json!({"status":"scope_blocked","reason":reason,
                "roles":{"architect":"no_current_verdict","reviewer":"no_current_verdict"}});
            self.save_plan(plan)?;
            return Ok("scope_blocked");
        }
        // The anchor exists so a review judges the diff against the base the
        // attempt started from. A HEAD that moved while nothing is in flight —
        // clean worktree, nothing committed by this attempt — carries no such
        // diff, so re-anchor rather than wedging the stage until the plan is
        // revised: an unrelated commit between attempts is ordinary.
        let head = self.git(&["rev-parse", "HEAD"])?;
        let idle = plan["stages"][idx]["sha"].is_null()
            && self.git(&["status", "--porcelain"])?.trim().is_empty();
        if plan["stages"][idx]["attempt_head"].is_null()
            || (plan["stages"][idx]["attempt_head"] != head && idle)
        {
            plan["stages"][idx]["attempt_head"] = json!(head);
            self.save_plan(plan)?;
        }
        if plan["stages"][idx]["attempt_head"] != head {
            return Err("HEAD changed during stage attempt".into());
        }
        // Invariant: a `blocked` design export belongs to the round the agent
        // already finished. Resuming it at the same attempt, round and HEAD
        // re-enters that round at the export step: no new turn, no new round.
        let rounds = plan["stages"][idx]["rounds"].as_u64().unwrap_or(0);
        let export = &plan["stages"][idx]["design_export"];
        let resume_export = rounds > 0
            && export["status"] == "blocked"
            && export["attempt_id"] == plan["stages"][idx]["attempt_id"]
            && export["round"] == rounds
            && export["attempt_head"] == head;
        let start = if resume_export { rounds - 1 } else { rounds };
        if start > budget {
            let architect =
                if plan["stages"][idx]["review_policy"]["scope"] == "ordinary_documentation" {
                    "not_required"
                } else {
                    "no_current_verdict"
                };
            plan["stages"][idx]["review_gate"] = json!({"status":"exhausted","roles":{"architect":architect,"reviewer":"no_current_verdict"}});
            for role in ["architect", "reviewer"] {
                if crate::plan::review_cadence(&plan["stages"][idx], role) == "per_plan"
                    && (role == "reviewer" || architect != "not_required") {
                    plan["stages"][idx]["review_gate"]["roles"][role] = json!("deferred");
                }
            }
            self.save_plan(plan)?;
            return Ok("exhausted");
        }
        for round in start..=budget {
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }
            let resuming = resume_export && round == start;
            let mut assignment = self.assignment_boundary(plan, idx).map_err(|e| format!("model routing blocked: {e}"))?;
            let reviewer_runs = crate::plan::review_cadence(&plan["stages"][idx], "reviewer") == "per_stage";
            let mut reviewer_config = if reviewer_runs {
                Some(self.reviewer_config(assignment["effective"]["provider"].as_str().ok_or("missing agreed provider")?)
                    .map_err(|e| format!("model routing blocked: {e}"))?)
            } else { None };
            // A resumed design block retries only the export: the turn that produced
            // these edits already ran and its round is already reserved.
            if !resuming {
                let clarifying = plan["stages"][idx]["scope_clarification"]["pending"] == true
                    && plan["stages"][idx]["scope_clarification"]["source_inputs"] == crate::plan::stage_inputs(plan, idx);
                // Reserve clarification delivery with the round, before invocation.
                // Neither a restart nor repeated escalation refunds this follow-up.
                if clarifying {
                    plan["stages"][idx]["scope_clarification"]["pending"] = json!(false);
                    plan["stages"][idx]["scope_clarification"]["delivered_round"] = json!(round + 1);
                }
                // Reserve the round before any invocation; errors/restarts cannot replenish it.
                plan["stages"][idx]["rounds"] = json!(round + 1);
                plan["stages"][idx]["review_gate"] =
                    json!({"status":"pending","roles":{"architect":"pending","reviewer":"pending"}});
                for role in ["architect", "reviewer"] {
                    if crate::plan::review_cadence(&plan["stages"][idx], role) == "per_plan" {
                        plan["stages"][idx]["review_gate"]["roles"][role] = json!("deferred");
                    }
                }
                plan["stages"][idx]["last_verdict_valid"] = json!(false);
                self.save_plan(plan)?;
                let template = if round == 0 || clarifying {
                    IMPLEMENT_PROMPT
                } else {
                    FIX_PROMPT
                };
                self.set_step(
                    Some(sid),
                    if round == 0 || clarifying { "implementing" } else { "fixing" },
                );
                let mut stage = plan["stages"][idx].clone();
                if round > 0 {
                    stage["last_verdict"] = stage["previous_requests"].clone();
                    stage["last_verdict_valid"] = json!(true);
                }
                let role = if round == 0 || clarifying { "implementer" } else { "fixer" };
                let (output, turn) = loop {
                    if self.session.stop_requested.load(Ordering::SeqCst) { return Ok("stopped"); }
                    let turn = crate::architecture::identity();
                    stage = plan["stages"][idx].clone();
                    if round > 0 { stage["last_verdict"] = stage["previous_requests"].clone(); stage["last_verdict_valid"] = json!(true); }
                    let prompt = self.stage_prompt(template, plan, &stage)? + &self.outcome_prompt(plan, idx, &turn);
                    let effective = &assignment["effective"];
                    let implementer = effective["provider"].as_str().ok_or("missing agreed provider")?;
                    let model = effective["model"].as_str().ok_or("missing agreed model")?;
                    let effort = effective["native_effort"].as_str().ok_or("missing agreed effort")?;
                    plan["stages"][idx]["implementer_provider"] = json!(implementer);
                    let invocation = json!({"turn_id":turn,"agreement_id":assignment["agreement_id"],"selection_id":assignment["id"],"role":role,"proposed":assignment["validated_proposal"],"requested":effective,"unix":crate::util::unix_timestamp(),"status":"launching"});
                    if !plan["stages"][idx]["model_invocations"].is_array() { plan["stages"][idx]["model_invocations"] = json!([]); }
                    plan["stages"][idx]["model_invocations"].as_array_mut().unwrap().push(invocation);
                    self.save_plan(plan)?;
                    let result = self.run_agent(&crate::agent::AgentRequest { role, provider:implementer, model, effort, session:None, prompt:&prompt });
                    let record = plan["stages"][idx]["model_invocations"].as_array_mut().unwrap().last_mut().unwrap();
                    match &result {
                        Ok(output) => {
                            record["effective"] = json!({"provider":implementer,"model":output.effective_model,"native_effort":effort});
                            record["model_reported"] = json!(output.model_reported);
                            record["unexpected_substitution"] = json!((!output.model_reported || !crate::agent::same_model(implementer, model, &output.effective_model)));
                            record["status"] = json!("completed");
                            record["usage"] = output.usage.as_ref().map(|u| json!({"input":u.input_tokens,"output":u.output_tokens,"total":u.total_tokens})).unwrap_or(Value::Null);
                            record["verification_state"] = json!(if output.model_reported && crate::agent::same_model(implementer, model, &output.effective_model) { "execution_verified" } else { "unexpected_substitution" });
                        }
                        Err(error) => { record["status"] = json!("failed"); record["error"] = json!(error); record["failure_kind"] = json!(super::super::reassessment::failure_kind(error)); }
                    }
                    self.save_plan(plan)?;
                    if self.session.stop_requested.load(Ordering::SeqCst) { return Ok("stopped"); }
                    match result {
                        Ok(output) => {
                            self.record_stage_usage(plan, idx, role, implementer, output.usage.clone())?;
                            if !output.model_reported || !crate::agent::same_model(implementer, model, &output.effective_model) {
                                let error = "model routing blocked: unexpected provider model substitution; saved work retained. Correct the stage/global model constraint and reconcile before retrying";
                                plan["stages"][idx]["model_block"] = json!(error);
                                self.save_plan(plan)?;
                                return Err(error.into());
                            }
                            plan["stages"][idx]["reassessment"]["status"] = json!("reusing");
                            self.save_plan(plan)?;
                            break (output, turn);
                        }
                        Err(error) => {
                            if self.operational_retry(plan, idx, &error,role)? { continue; }
                            self.reassess(plan, idx, "provider_operational_failure", json!({"failure_kind":super::super::reassessment::failure_kind(&error),"error":crate::util::last_chars(&error,2000),"provider":implementer}))?;
                            assignment = self.assignment_boundary(plan, idx)?;
                            if reviewer_runs {
                                reviewer_config = Some(self.reviewer_config(assignment["effective"]["provider"].as_str().unwrap())?);
                            }
                        }
                    }
                };
                // The engine never decides what a history change means; the architect does.
                if let Some(base) = plan["stages"][idx]["attempt_head"].as_str().map(str::to_owned)
                    && let Some(evidence) = self.history_change(&base)?
                {
                    let decision = self.decide_history_change(plan, ReviewScope::Stage(idx), role, &evidence)?;
                    match decision["action"].as_str() {
                        Some("uncommit") => self.undo_agent_commits(&base, role)?,
                        Some("continue") => {
                            plan["stages"][idx]["attempt_head"] = evidence["head"].clone();
                            self.save_plan(plan)?;
                        }
                        _ => {
                            plan["stages"][idx]["review_gate"] = json!({"status":"history_blocked","reason":decision["reason"],
                                "roles":{"architect":"no_current_verdict","reviewer":"no_current_verdict"}});
                            self.save_plan(plan)?;
                            return Ok("history_blocked");
                        }
                    }
                }
                let effective = &assignment["effective"];
                let provider = effective["provider"].as_str().unwrap();
                let model = effective["model"].as_str().unwrap();
                let effort = effective["native_effort"].as_str().unwrap();
                let mut original_snapshot = None;
                let outcome_context = self.outcome_prompt(plan, idx, &turn);
                let structured = super::super::reassessment::structured_outcome(&output.output);
                let mut correction_usage = Vec::new();
                let (output, ()) = self.repair_response("implementer outcome", output,
                    |response| {
                        if structured && !super::super::reassessment::structured_outcome(&response.output) {
                            return Err("a corrected structured outcome must remain JSON".into());
                        }
                        self.validate_implementer_response(plan, idx, &turn, &response.output)
                    },
                    |response, error| {
                        if original_snapshot.is_none() { original_snapshot = Some(review_snapshot(self.project())?); }
                        let correction = crate::response::correction_prompt(&outcome_context, &response.output, error);
                        let result = self.run_agent(&AgentRequest { role:"response_correction", provider, model, effort,
                            session:None, prompt:&correction })?;
                        if Some(review_snapshot(self.project())?) != original_snapshot {
                            return Err("implementation or HEAD changed during outcome correction".into());
                        }
                        if !result.completed || !result.model_reported || !crate::agent::same_model(provider, model, &result.effective_model) {
                            return Err("outcome correction model changed or did not complete".into());
                        }
                        if let Some(usage) = &result.usage { correction_usage.push(usage.clone()); }
                        Ok(result)
                    })?;
                for usage in correction_usage { self.record_stage_usage(plan, idx, role, provider, Some(usage))?; }
                let trigger = self.implementer_outcome(plan, idx, &turn, &output)?;
                if let Some((kind,evidence)) = trigger {
                    // A scope escalation says this stage cannot be built as written.
                    // Re-running it against the same words only spends the remaining
                    // fix rounds, so the planner that owns the text decides instead.
                    // A constraint conflict (the stage's own constraints contradict
                    // each other) also goes to the planner. Until the stage loop has its
                    // own conflict hand-back it takes the scope path, recorded as an
                    // escalation so it is never dropped.
                    if kind == "material_scope_change" || kind == crate::constraint_conflict::KIND {
                        let record = if kind == crate::constraint_conflict::KIND {
                            let reason = evidence["request"]["reason"].as_str().unwrap_or("").to_owned();
                            Some(self.begin_constraint_escalation(plan, idx, role, &kind, &reason, std::slice::from_ref(&reason))?)
                        } else { None };
                        let resolution = self.renegotiate_scope(plan, idx, &evidence);
                        if let Some(at) = record {
                            use super::super::planning::ScopeResolution as R;
                            let (outcome, detail) = match &resolution {
                                Ok(R::Revised(changed)) => ("applied", format!("scope renegotiation revised the stage: {changed}")),
                                Ok(R::Clarified(message)) => ("refused", format!("scope renegotiation kept the stage: {message}")),
                                Ok(R::Blocked(message)) => ("blocked", message.clone()),
                                Err(error) => ("failed", error.clone()),
                            };
                            self.settle_constraint_escalation(plan, idx, at, outcome, Some(&detail))?;
                        }
                        let message = match resolution? {
                            super::super::planning::ScopeResolution::Revised(_) => return Ok("renegotiated"),
                            super::super::planning::ScopeResolution::Clarified(_) => {
                                if round < budget { continue; }
                                return Ok("exhausted");
                            }
                            super::super::planning::ScopeResolution::Blocked(message) => message,
                        };
                        plan["stages"][idx]["review_gate"] = json!({"status":"scope_blocked","reason":message,
                            "roles":{"architect":"no_current_verdict","reviewer":"no_current_verdict"}});
                        self.save_plan(plan)?;
                        return Ok("scope_blocked");
                    }
                    if round < budget { self.reassess(plan, idx, &kind, evidence)?; continue; }
                    return Ok("exhausted");
                }
                if output.usage.as_ref().is_some_and(|u| {
                    let limit = assignment["policy_inputs"]["limits"]["context_window"].as_u64().unwrap_or(0);
                    let percent = plan["stages"][idx]["reassessment"]["limits"]["context_percent"].as_u64().unwrap_or(85);
                    limit > 0 && u.input_tokens.max(0) as u64 >= limit.saturating_mul(percent) / 100
                }) && round < budget {
                    self.reassess(plan, idx, "context_pressure", json!({"measured_input_tokens":output.usage.as_ref().unwrap().input_tokens,"context_window":assignment["policy_inputs"]["limits"]["context_window"]}))?;
                }
            }
            match self.export_stage_designs(plan, idx, round + 1)? {
                DesignExport::Ready => {}
                DesignExport::Blocked => return Ok("design_blocked"),
                DesignExport::Failed => {
                    if round < budget { continue; }
                    return Ok("exhausted");
                }
            }
            let snap = review_snapshot(self.project())?;
            if snap["head"] != plan["stages"][idx]["attempt_head"] {
                return Err("implementer changed HEAD".into());
            }
            let mut policy = classify_review_scope(
                self.project(),
                &plan["stages"][idx],
                plan["stages"][idx]["dual_promoted"] == true,
            )?;
            if assignment["validated_proposal"]["task"] == "documentation" && policy["scope"] != "ordinary_documentation"
                && plan["stages"][idx]["routing_scope_floor"].is_null() {
                plan["stages"][idx]["routing_scope_floor"] = json!(2);
                self.save_plan(plan)?;
                self.reassess(plan,idx,"material_scope_change",json!({"engine_scope":policy,"planned_task":"documentation"}))?;
            }
            policy = partition_review_policy_by_cadence(policy, &plan["stages"][idx]);
            let mut base = json!({"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":sid,"attempt_id":plan["stages"][idx]["attempt_id"],"round":round+1,"policy":policy,"snapshot":snap});
            let mut records = vec![];
            if !plan["stages"][idx]["reviews"].is_array() {
                plan["stages"][idx]["reviews"] = json!([]);
            }
            loop {
                plan["stages"][idx]["review_policy"] = policy.clone();
                if policy["scope"] != "ordinary_documentation" {
                    plan["stages"][idx]["dual_promoted"] = json!(true);
                }
                plan["stages"][idx]["review_gate"] = aggregate_review_gate(&base, &records);
                self.save_plan(plan)?;
                // Independent goes first: a scope promotion can bind both required
                // verdicts to the promoted policy before any fixer is allowed.
                let promote = if let Some((reviewer, reviewer_model)) = &reviewer_config {
                    let v = self.review_with_retry(plan, idx, &base, "reviewer", reviewer, reviewer_model)?;
                    let promote = v["requires_dual"] == true
                        || v["architecture_context_gap"].as_str().is_some_and(|s| !s.trim().is_empty());
                    records.push(v);
                    plan["stages"][idx]["review_gate"] = aggregate_review_gate(&base, &records);
                    self.save_plan(plan)?;
                    promote
                } else { false };
                if policy["scope"] == "ordinary_documentation" && promote {
                    let findings = records.last().map(Ctx::review_requests).unwrap_or_default();
                    plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Scope promotion; verify prior requests under dual policy", "issues":findings,"notes":[],"checks":[]});
                    policy = partition_review_policy_by_cadence(dual_review_policy("Independent reviewer identified architectural impact"), &plan["stages"][idx]);
                    base["policy"] = policy.clone();
                    plan["stages"][idx]["dual_promoted"] = json!(true);
                    continue;
                }
                if stage_required_roles(&policy).unwrap().contains(&json!("architect")) {
                    records.push(self.architect_review_with_selection(plan, idx, &base)?);
                }
                break;
            }
            let gate = aggregate_review_gate(&base, &records);
            let requests: Vec<_> = gate["requests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {}",
                        r["role"].as_str().unwrap(),
                        r["text"].as_str().unwrap()
                    )
                })
                .collect();
            plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Combined unresolved role requests; both retain authority", "issues":requests,"notes":[],"checks":[]});
            plan["stages"][idx]["review_gate"] = gate.clone();
            self.save_plan(plan)?;
            self.log_event(
                "review",
                &format!(
                    "stage {sid} gate {}: architect {}, reviewer {}",
                    gate["status"], gate["roles"]["architect"], gate["roles"]["reviewer"]
                ),
            );
            if gate["status"] == "approved" {
                return Ok("approved");
            }
            if gate["status"] == "deferred" {
                return Ok("deferred");
            }
            // Record clarification without erasing either role's requests.
            let gaps: Vec<_> = records
                .iter()
                .filter_map(|r| r["architecture_context_gap"].as_str())
                .filter(|s| !s.trim().is_empty())
                .collect();
            if !gaps.is_empty() && round < budget {
                let current = self.load_plan().ok_or("missing plan")?;
                let mut cp = self.architecture_store().checkpoint(&current)?;
                cp["guidance"][sid.to_string()]["valid"] = json!(false);
                cp["guidance"][sid.to_string()]["invalidation_trigger"] =
                    json!("reviewer_context_gap");
                cp["context_gap"] = json!({"stage_id":sid,"text":gaps.join("\n"),"unresolved_requests":gate["requests"]});
                let current = self.architecture_store().publish(current, cp, json!({"kind":"review_clarification_requested","requests":gate["requests"],"gaps":gaps}))?;
                *plan = current.clone();
                if self.repeated_findings(plan, idx, &gate["requests"])? { continue; }
                let current = self.load_plan().ok_or("missing clarification plan")?;
                *plan = self.architect_publish(
                    current.clone(),
                    Some(&current),
                    "review architectural clarification; retain both roles' unresolved authority",
                )?;
            } else if round < budget {
                self.repeated_findings(plan, idx, &gate["requests"])?;
            }
        }
        Ok("exhausted")
    }

    /// Export PNGs for `.pen` files changed in this attempt before anything
    /// reviews or commits the snapshot, and persist the outcome on the stage.
    fn export_stage_designs(&self, plan: &mut Value, idx: usize, round: u64) -> Result<DesignExport, String> {
        let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
        let base = plan["stages"][idx]["attempt_head"].as_str().ok_or("missing attempt head for design export")?.to_string();
        let root = Path::new(self.project());
        let files = crate::pen::changed_pen_files(root, &base)?;
        if files.is_empty() {
            if let Some(stage) = plan["stages"][idx].as_object_mut()
                && stage.remove("design_export").is_some() {
                self.save_plan(plan)?;
            }
            return Ok(DesignExport::Ready);
        }
        self.set_step(Some(sid), "exporting designs");
        let mut record = json!({"round":round,"attempt_id":plan["stages"][idx]["attempt_id"],
            "attempt_head":base,"files":files,"unix":unix_timestamp()});
        let outcome = match crate::pen::export_changed(root, &base, &self.pen_search_path()) {
            Ok(pngs) => {
                record["status"] = json!("exported");
                record["pngs"] = json!(pngs);
                self.log_event("design", &format!("stage {sid}: exported {} PNG(s) for {} changed design(s)", pngs.len(), files.len()));
                DesignExport::Ready
            }
            Err(crate::pen::ExportError::Unavailable(reason)) => {
                record["status"] = json!("blocked");
                record["reason"] = json!(reason);
                plan["stages"][idx]["review_gate"] = json!({"status":"design_blocked","reason":reason,
                    "roles":{"architect":"no_current_verdict","reviewer":"no_current_verdict"}});
                self.log_event("design", &format!("stage {sid}: {reason}"));
                DesignExport::Blocked
            }
            Err(crate::pen::ExportError::Failed(reason)) => {
                let request = format!("[engine] pen.dev export failed: {reason}; repair the design so it opens and exports headlessly");
                // Keep every unresolved request; a newer export failure supersedes older ones.
                let previous = &plan["stages"][idx]["previous_requests"];
                let mut issues: Vec<String> = Self::review_requests(previous).into_iter()
                    .filter(|r| !r.starts_with("[engine] pen.dev export failed:")).collect();
                issues.push(request.clone());
                let summary = previous["summary"].as_str()
                    .unwrap_or("Engine design export failed; prior unresolved findings retain authority").to_string();
                plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":summary,"issues":issues,"notes":[],"checks":previous["checks"].as_array().cloned().unwrap_or_default()});
                plan["stages"][idx]["review_gate"] = json!({"status":"blocked","reason":reason,
                    "roles":{"architect":"no_current_verdict","reviewer":"no_current_verdict"},
                    "requests":[{"role":"engine","text":request}]});
                record["status"] = json!("failed");
                record["reason"] = json!(reason);
                self.log_event("design", &format!("stage {sid}: {reason}; returned to the stage's fixer"));
                DesignExport::Failed
            }
        };
        plan["stages"][idx]["design_export"] = record;
        plan["stages"][idx]["last_verdict_valid"] = json!(false);
        self.save_plan(plan)?;
        Ok(outcome)
    }

    /// Changed designs may only commit with this attempt and round's successful export.
    pub(crate) fn validate_design_export(&self, stage: &Value) -> Result<(), String> {
        let Some(base) = stage["attempt_head"].as_str() else { return Ok(()); };
        let files = crate::pen::changed_pen_files(Path::new(self.project()), base)?;
        let export = &stage["design_export"];
        if !files.is_empty()
            && (export["status"] != "exported"
                || export["attempt_id"] != stage["attempt_id"]
                || export["round"] != stage["rounds"]
                || export["attempt_head"] != base
                || export["files"] != json!(files))
        {
            return Err("changed .pen designs have no current successful pen.dev export".into());
        }
        Ok(())
    }

    fn validate_commit_approval(&self, plan: &Value, idx: usize) -> Result<(), String> {
        let stage = &plan["stages"][idx];
        let gate = &stage["review_gate"];
        if !matches!(gate["status"].as_str(), Some("approved" | "deferred"))
            || gate["identity"]["plan_id"] != plan["plan_id"]
            || gate["identity"]["revision"] != plan["revision"]
            || gate["identity"]["stage_id"] != stage["id"]
            || gate["identity"]["attempt_id"] != stage["attempt_id"]
            || (!stage["attempt_revision"].is_null() && stage["attempt_revision"] != plan["revision"])
            || gate["identity"]["round"] != stage["rounds"]
        {
            return Err("no current aggregate review approval".into());
        }
        let persisted = self.load_plan().ok_or("missing persisted review gate")?;
        if persisted["plan_id"] != plan["plan_id"]
            || persisted["revision"] != plan["revision"]
            || persisted["stages"][idx]["review_gate"] != *gate
            || ["attempt_id", "attempt_revision", "rounds", "review_cadence", "review_policy", "reviews", "design_export"]
                .iter().any(|field| persisted["stages"][idx][*field] != stage[*field])
            || crate::plan::stage_inputs(&persisted, idx) != crate::plan::stage_inputs(plan, idx)
        {
            return Err("plan changed after review".into());
        }
        self.validate_design_export(stage)?;
        let policy = &gate["policy"];
        let required = match policy["scope"].as_str() {
            Some("ordinary_documentation") => json!(["reviewer"]),
            Some("code_or_contract") => json!(["architect", "reviewer"]),
            _ => return Err("invalid gate scope".into()),
        };
        if policy["version"] != 1 || policy["required_roles"] != required
            || *policy != gate["identity"]["policy"] || *policy != stage["review_policy"] {
            return Err("inconsistent gate policy".into());
        }
        let expected = partition_review_policy_by_cadence(policy.clone(), stage);
        // Legacy gates have no scheduling metadata and retain every classified role.
        let legacy = policy.get("stage_required_roles").is_none() && policy.get("deferred_roles").is_none();
        if (legacy && expected["deferred_roles"] != json!([]))
            || (!legacy && *policy != expected) {
            return Err("gate policy does not match attempt review cadence".into());
        }
        let required = stage_required_roles(policy).ok_or("invalid gate roles")?;
        if gate["status"] == "deferred"
            && (!required.is_empty() || policy["deferred_roles"].as_array().is_none_or(Vec::is_empty)) {
            return Err("deferred gate requires outstanding deferred roles and no stage-required roles".into());
        }
        let current = aggregate_review_gate(
            &gate["identity"],
            stage["reviews"].as_array().ok_or("missing reviews")?,
        );
        if current != *gate {
            return Err("missing current role approvals".into());
        }
        for role in required {
            let mut identity = gate["identity"].clone();
            identity["role"] = role.clone();
            let record = stage["reviews"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["identity"] == identity)
                .ok_or("missing current immutable verdict")?;
            if normalize_review_verdict(
                &record.to_string(),
                &identity,
                &self.stage_review_acceptance(plan, idx)?,
            )?["approved"]
                != true
            {
                return Err("current verdict is not clean and evidenced".into());
            }
        }
        Ok(())
    }

    /// Recover the narrow crash window after Git's CAS commit and before the
    /// completed-stage checkpoint publication. Never synthesize an approval.
    pub(crate) fn recover_committed_stages(&self) -> Result<Vec<i64>, String> {
        let Some(mut plan) = self.load_plan() else { return Ok(Vec::new()); };
        let Some(idx) = plan["stages"].as_array().ok_or("invalid stages")?.iter()
            .position(|stage| stage["status"] != "committed") else { return Ok(Vec::new()); };
        let stage = &plan["stages"][idx];
        if !matches!(stage["review_gate"]["status"].as_str(), Some("approved" | "deferred")) { return Ok(Vec::new()); }
        let expected = &stage["review_gate"]["identity"]["snapshot"];
        let actual = review_snapshot(self.project())?;
        if actual["head"] == expected["head"] { return Ok(Vec::new()); }
        self.validate_commit_approval(&plan, idx)?;
        self.git(&["diff", "--quiet"])
            .and_then(|_| self.git(&["diff", "--cached", "--quiet"]))
            .map_err(|_| "uncommitted index/worktree changes prevent commit recovery")?;
        let parents = self.git(&["rev-list", "--parents", "-n", "1", "HEAD"])?;
        let parents: Vec<_> = parents.split_whitespace().collect();
        if parents.len() != 2 || Some(parents[1]) != expected["head"].as_str()
            || actual["tree"] != expected["tree"] || actual["content"] != expected["content"]
            || self.git(&["rev-parse", "HEAD^{tree}"])? != expected["tree"]
            || self.git(&["show", "-s", "--format=%B", "HEAD"])? != stage["commit"].as_str().unwrap_or("forge: stage") {
            return Err("HEAD/worktree does not match the saved independently reviewed commit; recovery refused".into());
        }
        let sid = stage["id"].as_i64().ok_or("missing stage ID")?;
        plan["stages"][idx]["sha"] = json!(self.git(&["rev-parse", "--short", "HEAD"])?);
        self.finish_stage(&mut plan, idx, "committed")?;
        self.log_event("stage", &format!("stage {sid} recovered: exact reviewed commit already exists; completion checkpoint restored"));
        Ok(vec![sid])
    }

    /// Carry out the architect's "uncommit" decision: commits stacked on the
    /// expected base become staged changes again, so review still sees the whole
    /// diff and the engine makes the only commit. Any other HEAD movement is left
    /// for the callers' HEAD checks to reject.
    pub(in crate::app) fn undo_agent_commits(&self, base: &str, role: &str) -> Result<(), String> {
        let head = self.git(&["rev-parse", "HEAD"])?;
        if head == base || self.git(&["merge-base", "--is-ancestor", base, &head]).is_err() {
            return Ok(());
        }
        let count = self.git(&["rev-list", "--count", &format!("{base}..{head}")])?;
        // CAS on the observed HEAD; the index and worktree keep the committed content.
        self.git(&["update-ref", "-m", &format!("forge: undo {role} commits"), "HEAD", base, &head])?;
        self.log_event("git", &format!("[{role}] made {count} commit(s); moved them back to uncommitted changes"));
        Ok(())
    }

    pub(in crate::app) fn commit_reviewed(
        &self,
        plan: &Value,
        idx: usize,
        message: &str,
    ) -> Result<Option<String>, String> {
        self.validate_commit_approval(plan, idx)?;
        let stage = &plan["stages"][idx];
        let gate = &stage["review_gate"];
        let expected = &gate["identity"]["snapshot"];
        if review_snapshot(self.project())? != *expected {
            return Err("implementation/index/HEAD changed after review".into());
        }
        if classify_review_scope(self.project(), stage, stage["dual_promoted"] == true)?["scope"]
            != gate["policy"]["scope"]
        {
            return Err("review scope changed before commit".into());
        }
        // Match snapshot's staging: git add rejects an explicitly excluded ignored path.
        self.git(&["add", "-A"])?;
        self.git(&["reset", "-q", "HEAD", "--", ".forge"])?;
        let staged = self.git(&["write-tree"])?;
        let actual = review_snapshot(self.project())?;
        if actual["head"] != expected["head"]
            || actual["content"] != expected["content"]
            || actual["tree"] != expected["tree"]
            || staged != expected["tree"]
        {
            return Err("staged tree differs from reviewed content".into());
        }
        if staged == self.git(&["rev-parse", "HEAD^{tree}"])? {
            return Ok(None);
        }
        // commit-tree fixes the exact reviewed tree; CAS update-ref rejects concurrent HEAD
        // changes, and avoids hooks mutating the tree between verification and commit.
        let head = expected["head"].as_str().unwrap();
        let sha = self.git(&["commit-tree", &staged, "-p", head, "-m", message])?;
        let last = review_snapshot(self.project())?;
        if last != actual || self.session.stop_requested.load(Ordering::SeqCst) {
            return Err("external edit or stop before commit".into());
        }
        self.git(&["update-ref", "-m", message, "HEAD", &sha, head])?;
        let short = self.git(&["rev-parse", "--short", "HEAD"])?;
        self.log_event("git", &format!("committed {short}: {message}"));
        Ok(Some(short))
    }
}
