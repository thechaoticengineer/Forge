//! Frozen plan subjects and the deferred review publication gate.
use super::*;
use std::collections::BTreeSet;

const FIX_MESSAGE: &str = "fix(review): apply deferred plan review findings";

const INDEPENDENCE: &str = "independent plan review requires a provider no stage implementer used; set the reviewer review cadence to per stage for a revised plan or pin the implementer provider; existing commits remain local";

fn subject(plan: &Value) -> Result<Value, String> {
    let stages = plan["stages"].as_array().ok_or("invalid plan stages")?;
    if stages.iter().any(|s| s["status"] != "committed") {
        return Err("plan review requires every stage committed; resume the pending stages".into());
    }
    Ok(json!({"goal":plan["goal"],"revision":plan["revision"],"stages":stages.iter().map(|s|
        json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"],
            "commit":s["commit"],"sha":s["sha"],"attempt_head":s["attempt_head"],"attempt_id":s["attempt_id"],
            "review_policy":s["review_policy"],"implementer_provider":s["implementer_provider"],
            "model_invocations":s["model_invocations"]})).collect::<Vec<_>>()}))
}
fn acceptance(subject: &Value) -> String {
    subject["stages"].as_array().unwrap().iter().flat_map(|stage| {
        criteria(stage["acceptance"].as_str().unwrap_or("")).into_iter().map(|line|
            format!("stage {} ({}): {line}", stage["id"], stage["title"].as_str().unwrap_or("")))
    }).collect::<Vec<_>>().join("\n")
}
fn providers(plan: &Value) -> Result<BTreeSet<String>, String> {
    let mut result = BTreeSet::new();
    for stage in plan["plan_review"]["subject"]["stages"].as_array().ok_or("missing plan review subject")? {
        let provider = stage["implementer_provider"].as_str().filter(|p| matches!(*p,"codex"|"claude"|"mock"))
            .ok_or("unknown plan implementation provenance; restore the recorded implementer provider before retrying")?;
        result.insert(provider.to_string());
    }
    let stage_calls = plan["plan_review"]["subject"]["stages"].as_array().into_iter().flatten()
        .flat_map(|s| s["model_invocations"].as_array().into_iter().flatten());
    let fix_calls = plan["plan_review"]["model_invocations"].as_array().into_iter().flatten();
    for call in stage_calls.chain(fix_calls) {
        if matches!(call["role"].as_str(), Some("implementer" | "fixer")) {
            let requested = call["requested"]["provider"].as_str().filter(|p| matches!(*p,"codex"|"claude"|"mock"))
                .ok_or("unknown implementation invocation provider; restore provenance before retrying")?;
            result.insert(requested.into());
            if let Some(effective) = call["effective"]["provider"].as_str() { result.insert(effective.into()); }
            if call["status"] == "completed" && (call["model_reported"] != true || call["verification_state"] != "execution_verified") {
                return Err("unverified plan implementation provenance; restore verified invocation evidence before retrying".into());
            }
        }
    }
    Ok(result)
}

impl Ctx {
    pub(crate) fn deferred_plan_roles(&self, plan: &Value) -> Result<Vec<Value>, String> {
        let mut roles = BTreeSet::new();
        for stage in plan["stages"].as_array().ok_or("invalid plan stages")? {
            for role in stage["review_policy"]["deferred_roles"].as_array().into_iter().flatten() {
                let role = role.as_str().filter(|r| matches!(*r,"architect"|"reviewer"))
                    .ok_or("invalid recorded deferred role; restore the stage review policy")?;
                roles.insert(role);
            }
        }
        Ok(roles.into_iter().map(|r| json!(r)).collect())
    }

    fn plan_review_base(&self, plan: &Value) -> Result<String, String> {
        let first = plan["stages"].as_array().ok_or("invalid stages")?.iter().find(|s|
            s["review_policy"]["deferred_roles"].as_array().is_some_and(|r| !r.is_empty()))
            .ok_or("no deferred stage")?;
        let base = if let Some(base) = first["attempt_head"].as_str().filter(|s| !s.is_empty()) { base.to_string() }
            else {
                let sha = first["sha"].as_str().filter(|s| !s.is_empty())
                    .ok_or("cannot derive plan review base: earliest deferred stage has neither attempt_head nor sha; restore its recorded base and retry")?;
                self.git(&["rev-parse", "--verify", &format!("{sha}^{{commit}}^")])
                    .map_err(|e| format!("cannot derive plan review base from stage commit parent; restore the missing commit history and retry: {e}"))?
            };
        self.git(&["merge-base", "--is-ancestor", &base, "HEAD"])
            .map_err(|e| format!("plan review base {base} is not an ancestor of HEAD; restore the plan branch/history and retry: {e}"))?;
        Ok(base)
    }

    pub(super) fn validate_plan_subject(&self, plan: &Value) -> Result<(), String> {
        self.validate_plan_subject_inputs(plan)?;
        if plan["plan_review"]["head"] != self.git(&["rev-parse", "HEAD"])? {
            return Err("plan review subject changed: HEAD moved; restore the captured branch or edit and approve a new plan revision before retrying".into());
        }
        Ok(())
    }

    fn validate_plan_subject_inputs(&self, plan: &Value) -> Result<(), String> {
        let review = &plan["plan_review"];
        if review["version"] != 1 || review["subject"] != subject(plan)?
            || review["required_roles"] != json!(self.deferred_plan_roles(plan)?)
            || review["acceptance"] != acceptance(&review["subject"])
            || review["base"] != self.plan_review_base(plan)? {
            return Err("plan review subject changed; restore the captured plan inputs and branch or edit and approve a new plan revision before retrying".into());
        }
        for stage in review["subject"]["stages"].as_array().ok_or("missing reviewed stages")? {
            if let Some(sha) = stage["sha"].as_str() {
                self.git(&["merge-base", "--is-ancestor", sha, review["head"].as_str().unwrap()])
                    .map_err(|e| format!("reviewed stage {} commit {sha} is not an ancestor of the captured HEAD; restore the plan history and retry: {e}",stage["id"]))?;
            }
        }
        Ok(())
    }

    pub(super) fn plan_review_prompt(&self, plan: &Value) -> String {
        let r = &plan["plan_review"];
        let stages = r["subject"]["stages"].as_array().unwrap().iter().map(|s|
            json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],
                "acceptance":s["acceptance"],"commit":s["commit"],"sha":s["sha"]})).collect::<Vec<_>>();
        let prior = r["reviews"].as_array().into_iter().flatten().filter(|v|
            v["attempt_id"] == r["attempt_id"] && v["round"].as_u64() < r["rounds"].as_u64())
            .map(|v| json!({"role":v["role"],"round":v["round"],"requests":Self::review_requests(v)})).collect::<Vec<_>>();
        let context = format!("Review round: {}. Review the complete captured plan subject independently. BEGIN PREVIOUS REVIEW CONTEXT\n{}\nEND PREVIOUS REVIEW CONTEXT",r["rounds"],json!(prior));
        crate::util::fill_template(PLAN_REVIEW_PROMPT, &[
            ("{goal}",r["subject"]["goal"].as_str().unwrap_or("")),
            ("{stages}",&json!(stages).to_string()),
            ("{base}",r["base"].as_str().unwrap_or("")),
            ("{head}",r["head"].as_str().unwrap_or("")),
            ("{acceptance}",r["acceptance"].as_str().unwrap_or("")),
            ("{review_context}",&context),
        ])
    }

    pub(super) fn validate_plan_reviewer(&self, plan: &Value, provider: &str) -> Result<(), String> {
        let used = providers(plan)?;
        if !self.configured_reviewer() && used.contains(provider) && !(provider == "mock" && used.len() == 1) { return Err(INDEPENDENCE.into()); }
        Ok(())
    }

    fn plan_reviewer(&self, plan: &mut Value, identity: &Value) -> Result<Value, String> {
        let used = providers(plan)?;
        if !self.configured_reviewer() && used.len() != 1 { return Err(INDEPENDENCE.into()); }
        let implementer = used.iter().next().ok_or(INDEPENDENCE)?;
        let (provider,model) = self.reviewer_config(implementer).map_err(|e| format!("{INDEPENDENCE}: {e}"))?;
        let requirements = self.model_requirements("reviewer",Some(implementer))?;
        let (verdict,_) = self.with_selected_model(&requirements,Some((provider,model,"provider_default".into())), |choice| {
            // Every retry rechecks all contributors and uses a fresh session.
            self.validate_plan_reviewer(plan,&choice.0)?;
            self.invoke_review(plan,ReviewScope::Plan,identity,"reviewer",&choice.0,&choice.1)
        })?;
        Ok(verdict)
    }

    fn validate_plan_evidence(&self, plan: &Value) -> Result<(), String> {
        self.validate_plan_subject_inputs(plan)?;
        let persisted = self.load_plan().ok_or("missing persisted plan review")?;
        if persisted["plan_id"] != plan["plan_id"] || persisted["revision"] != plan["revision"]
            || persisted["plan_review"] != plan["plan_review"] || subject(&persisted)? != subject(plan)? {
            return Err("plan changed after review".into());
        }
        let review = &plan["plan_review"];
        let identity = &review["gate"]["identity"];
        if review["status"] != "approved" || review["gate"]["status"] != "approved"
            || identity["scope"] != "plan" || !identity["stage_id"].is_null()
            || identity["plan_id"] != plan["plan_id"] || identity["revision"] != plan["revision"]
            || identity["attempt_id"] != review["attempt_id"] || identity["round"] != review["rounds"]
            || identity["policy"] != plan_policy(&review["required_roles"])
            || identity["snapshot"]["head"] != review["head"] {
            return Err("plan review approval is missing or stale; resume plan review before completion".into());
        }
        let records = review["reviews"].as_array().ok_or("missing plan reviews")?;
        if aggregate(identity,records) != review["gate"] { return Err("plan gate does not match immutable verdicts".into()); }
        for role in review["required_roles"].as_array().ok_or("invalid plan roles")? {
            let mut expected = identity.clone(); expected["role"] = role.clone();
            let verdict = records.iter().find(|r| r["identity"] == expected).ok_or("missing plan role evidence")?;
            if normalize(&verdict.to_string(),&expected,review["acceptance"].as_str().unwrap_or(""))?["approved"] != true {
                return Err("plan verdict is not clean and evidenced".into());
            }
            if role == "reviewer" { self.validate_plan_reviewer(plan,verdict["provider"].as_str().ok_or("missing reviewer provider")?)?; }
        }
        Ok(())
    }

    pub(crate) fn plan_fixer_boundary(&self, plan: &Value) -> Result<(), String> {
        if self.session.stop_requested.load(Ordering::SeqCst) { return Err("plan fixer stopped; saved work retained".into()); }
        self.validate_plan_subject(plan)?;
        let current = self.load_plan().ok_or("missing plan during fixer")?;
        self.validate_plan_subject(&current)?;
        if current["plan_id"] != plan["plan_id"] || current["revision"] != plan["revision"]
            || current["plan_review"] != plan["plan_review"] {
            return Err("plan changed during fixer; saved work retained".into());
        }
        Ok(())
    }

    fn run_plan_fixer(&self, plan: &mut Value) -> Result<(), String> {
        if !plan["plan_review"]["retries"].is_object() {
            plan["plan_review"]["retries"] = json!({"operational_retries":0,"history":[],
                "limits":self.app.settings.lock().unwrap()["reassessment_limits"]});
            self.save_plan(plan)?;
        }
        let minimum = plan["plan_review"]["minimum_tier"].as_u64().unwrap_or(3);
        let minimum = match minimum {
            0 | 1 => crate::catalogue::Tier::Basic,
            2 => crate::catalogue::Tier::Standard,
            3 => crate::catalogue::Tier::Strong,
            _ => return Err("unknown agreed plan capability requirement; restore stage assignments".into()),
        };
        let requirements = self.model_requirements("implementer",None)?.requiring_at_least(minimum);
        self.set_step(None,"fixing plan review findings");
        self.with_selected_model(&requirements,None, |choice| {
            loop {
                self.plan_fixer_boundary(plan)?;
                *plan = self.load_plan().ok_or("missing plan before fixer")?;
                let used = providers(plan)?;
                if plan["plan_review"]["required_roles"].as_array().unwrap().contains(&json!("reviewer")) {
                    // Do not introduce a contributor that would leave no independent reviewer.
                    if !self.configured_reviewer() && (used.len() != 1 || !used.contains(&choice.0)) { return Err(INDEPENDENCE.into()); }
                    self.reviewer_config(&choice.0).map_err(|e| format!("{INDEPENDENCE}: {e}"))?;
                }
                let cp = self.architecture_store().checkpoint(plan)?;
                let r = &plan["plan_review"];
                let prompt = crate::util::fill_template(crate::prompts::PLAN_FIX_PROMPT, &[
                    ("{goal}",r["subject"]["goal"].as_str().unwrap_or("")),
                    ("{stages}",&r["subject"]["stages"].to_string()),
                    ("{base}",r["base"].as_str().unwrap_or("")),
                    ("{head}",r["head"].as_str().unwrap_or("")),
                    ("{requests}",&r["outstanding_requests"].to_string()),
                    ("{guidance}",&cp["guidance"].to_string()),
                    ("{constraints}",&cp["constraints"].to_string()),
                    ("{interfaces}",&cp["completed_interfaces"].to_string()),
                    ("{plan_id}",plan["plan_id"].as_str().unwrap_or("")),
                ]);
                let turn = crate::architecture::identity();
                if !plan["plan_review"]["model_invocations"].is_array() { plan["plan_review"]["model_invocations"] = json!([]); }
                let round = plan["plan_review"]["rounds"].clone();
                plan["plan_review"]["model_invocations"].as_array_mut().unwrap().push(json!({
                    "turn_id":turn,"role":"fixer","round":round,
                    "requested":{"provider":choice.0,"model":choice.1,"native_effort":choice.2},
                    "unix":crate::util::unix_timestamp(),"status":"launching"}));
                self.save_plan(plan)?;
                let result = self.run_agent(&AgentRequest {role:"fixer",provider:&choice.0,model:&choice.1,effort:&choice.2,session:None,prompt:&prompt});
                // Check concurrent plan changes before publishing invocation accounting.
                let current = self.load_plan().ok_or("missing plan after fixer")?;
                if current["plan_review"] != plan["plan_review"] || subject(&current)? != subject(plan)? {
                    *plan = current;
                    return Err("plan changed during fixer; restore the reviewed subject before retrying".into());
                }
                let record = plan["plan_review"]["model_invocations"].as_array_mut().unwrap().last_mut().unwrap();
                match &result {
                    Ok(output) => {
                        let expected_model = if choice.0 == "mock" { choice.1.clone() } else {
                            let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
                            let facts = self.model_facts(&policy,crate::catalogue::Provider::parse(&choice.0).ok_or("invalid fixer provider")?,&choice.1,Some(&choice.2));
                            facts["resolved_id"].as_str().unwrap_or(&choice.1).to_string()
                        };
                        let verified = output.model_reported && (crate::agent::same_model(&choice.0,&expected_model,&output.effective_model)
                            || (choice.0 == "mock" && expected_model.is_empty() && output.effective_model.is_empty()));
                        record["effective"] = json!({"provider":choice.0,"model":output.effective_model,"native_effort":choice.2});
                        record["model_reported"] = json!(output.model_reported);
                        record["unexpected_substitution"] = json!(!verified);
                        record["status"] = json!("completed");
                        record["verification_state"] = json!(if verified {"execution_verified"} else {"unexpected_substitution"});
                        record["usage"] = output.usage.as_ref().map(|u| json!({"input":u.input_tokens,"output":u.output_tokens,"total":u.total_tokens})).unwrap_or(Value::Null);
                    }
                    Err(error) => {
                        record["status"] = json!("failed");
                        record["error"] = json!(error);
                        record["failure_kind"] = json!(crate::model_selection::failure_kind(error));
                    }
                }
                self.save_plan(plan)?;
                if let Ok(output) = &result { self.record_plan_usage(plan,"fixer",&choice.0,output.usage.clone())?; }
                self.plan_fixer_boundary(plan)?;
                match result {
                    Ok(_) => {
                        if plan["plan_review"]["model_invocations"].as_array().unwrap().last().unwrap()["verification_state"] != "execution_verified" {
                            return Err("model routing blocked: unexpected provider model substitution; saved plan fixes retained".into());
                        }
                        return Ok(());
                    }
                    Err(error) => {
                        if self.operational_retry_for_scope(plan,None,&error,"fixer")? { continue; }
                        return Err(error);
                    }
                }
            }
        })?;
        Ok(())
    }

    fn finalized_plan_snapshot(&self, plan: &Value) -> Result<Value, String> {
        self.validate_plan_evidence(plan)?;
        let expected = &plan["plan_review"]["gate"]["identity"]["snapshot"];
        let parent = expected["head"].as_str().ok_or("missing reviewed HEAD")?;
        let actual = snapshot_against(self.project(), parent)?;
        if actual["tree"] != expected["tree"] || actual["content"] != expected["content"]
            || self.git(&["rev-parse","HEAD^{tree}"])? != expected["tree"] {
            return Err("HEAD/worktree does not match approved plan fixes; finalization refused".into());
        }
        if actual["head"] != expected["head"] {
            if self.git(&["rev-parse", &format!("{parent}^{{tree}}")])? == expected["tree"] {
                return Err("clean plan review cannot authorize an extra commit".into());
            }
            let parents = self.git(&["rev-list","--parents","-n","1","HEAD"])?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            if parents.len() != 2 || parents[1] != parent
                || self.git(&["show","-s","--format=%B","HEAD"])? != FIX_MESSAGE {
                return Err("HEAD is not the exact approved plan fix commit; recovery refused".into());
            }
        }
        // Also reject an index which would hide work after the reviewed commit.
        self.git(&["diff","--cached","--quiet","HEAD","--",".",":(exclude).forge"])
            .map_err(|_| "index differs from finalized plan commit")?;
        Ok(actual)
    }

    pub(crate) fn validate_plan_approval(&self, plan: &Value) -> Result<(), String> {
        if self.deferred_plan_roles(plan)?.is_empty() { return Ok(()); }
        let actual = self.finalized_plan_snapshot(plan)?;
        let r = &plan["plan_review"];
        if r["next_action"] != "complete" || r["finalization"] != json!({"head":actual["head"],"tree":actual["tree"],"content":actual["content"]}) {
            return Err("plan fixes have not been durably finalized; resume plan review before completion".into());
        }
        let fixed = actual["head"] != r["head"];
        if (fixed && r["fix_sha"] != self.git(&["rev-parse","--short","HEAD"])? ) || (!fixed && !r["fix_sha"].is_null()) {
            return Err("plan fix commit reference does not match finalized HEAD".into());
        }
        Ok(())
    }

    pub(super) fn commit_plan_reviewed(&self, plan: &mut Value) -> Result<(), String> {
        self.validate_plan_evidence(plan)?;
        if plan["plan_review"]["next_action"] == "complete" { return self.validate_plan_approval(plan); }
        let expected = plan["plan_review"]["gate"]["identity"]["snapshot"].clone();
        let current = snapshot(self.project())?;
        if current["head"] == expected["head"] {
            if current != expected && !(plan["plan_review"]["commit_state"] == "staging"
                && current["content"] == expected["content"] && current["tree"] == expected["tree"]
                && self.git(&["write-tree"])? == expected["tree"]) {
                return Err("implementation/index/HEAD changed after plan review".into());
            }
            if self.session.stop_requested.load(Ordering::SeqCst) { return Err("stop before plan fix commit".into()); }
            plan["plan_review"]["commit_state"] = json!("staging");
            self.save_plan(plan)?;
            self.git(&["add","-A"])?;
            self.git(&["reset","-q","HEAD","--",".forge"])?;
            #[cfg(test)]
            if self.app.settings.lock().unwrap()["test_plan_staging_crash"] == true {
                return Err("simulated crash after staging plan fixes".into());
            }
            let staged = self.git(&["write-tree"])?;
            let actual = snapshot(self.project())?;
            if actual["head"] != expected["head"] || actual["content"] != expected["content"]
                || actual["tree"] != expected["tree"] || staged != expected["tree"] {
                return Err("staged tree differs from reviewed plan content".into());
            }
            if staged != self.git(&["rev-parse","HEAD^{tree}"])? {
                let head = expected["head"].as_str().ok_or("missing reviewed HEAD")?;
                let sha = self.git(&["commit-tree",&staged,"-p",head,"-m",FIX_MESSAGE])?;
                self.validate_plan_evidence(plan)?;
                if snapshot(self.project())? != actual || self.session.stop_requested.load(Ordering::SeqCst) {
                    return Err("external edit or stop before plan fix commit".into());
                }
                self.git(&["update-ref","-m",FIX_MESSAGE,"HEAD",&sha,head])?;
                #[cfg(test)]
                if self.app.settings.lock().unwrap()["test_plan_commit_crash"] == true {
                    return Err("simulated crash after plan update-ref".into());
                }
            }
        }
        // Recovery after update-ref uses exactly the same immutable gate, parent,
        // tree, content and deterministic message. It never creates a second commit.
        let actual = self.finalized_plan_snapshot(plan)?;
        let fix_sha = if actual["head"] != expected["head"] { json!(self.git(&["rev-parse","--short","HEAD"])? ) } else { Value::Null };
        let mut finalized = plan.clone();
        finalized["plan_review"]["fix_sha"] = fix_sha.clone();
        finalized["plan_review"]["finalization"] = json!({"head":actual["head"],"tree":actual["tree"],"content":actual["content"]});
        finalized["plan_review"]["next_action"] = json!("complete");
        self.save_plan(&finalized)?;
        *plan = finalized;
        if let Some(sha) = fix_sha.as_str() { self.log_event("git",&format!("committed {sha}: {FIX_MESSAGE}")); }
        self.validate_plan_approval(plan)
    }

    pub(crate) fn run_plan_review(&self, plan: &mut Value) -> Result<bool, String> {
        let result = self.run_plan_review_inner(plan);
        match result {
            Ok(true) => Ok(true),
            outcome => {
                let stopped = self.session.stop_requested.load(Ordering::SeqCst);
                let failed = outcome.is_err();
                let error = outcome.err().unwrap_or_else(|| "plan review rejected; address the recorded requests before retrying; commits remain local".into());
                if plan["plan_review"].is_object() {
                    let recoverable_finalization = plan["plan_review"]["next_action"] == "finalize"
                        && self.validate_plan_evidence(plan).is_ok()
                        && (snapshot(self.project()).is_ok_and(|s| s == plan["plan_review"]["gate"]["identity"]["snapshot"])
                            || self.finalized_plan_snapshot(plan).is_ok()
                            || (plan["plan_review"]["commit_state"] == "staging" && snapshot(self.project()).is_ok_and(|s| {
                                let expected = &plan["plan_review"]["gate"]["identity"]["snapshot"];
                                s["head"] == expected["head"] && s["tree"] == expected["tree"] && s["content"] == expected["content"]
                            })));
                    if !recoverable_finalization {
                        plan["plan_review"]["status"] = json!(if stopped {"interrupted"} else {"blocked"});
                        if stopped || failed {
                            plan["plan_review"]["gate"] = json!({"status":if stopped {"interrupted"} else {"error"},"error":error});
                        }
                    }
                    self.save_plan(plan)?;
                }
                self.set_phase(if stopped {"plan_ready"} else {"blocked"});
                self.log_event("plan_review",&error);
                Ok(false)
            }
        }
    }

    fn plan_action_checkpoint(&self, plan: &Value) -> Result<(), String> {
        self.save_plan(plan)?;
        #[cfg(test)]
        if plan["plan_review"]["rounds"] == 2
            && self.app.settings.lock().unwrap()["test_plan_crash_action"] == plan["plan_review"]["next_action"] {
            return Err("simulated crash at durable plan action boundary".into());
        }
        Ok(())
    }

    fn run_plan_review_inner(&self, plan: &mut Value) -> Result<bool, String> {
        let roles = self.deferred_plan_roles(plan)?;
        if roles.is_empty() { return Ok(true); }
        // Ordinary persistence can also increment revision. Require the edit
        // path's marker and subsequent approval before replenishing an attempt;
        // unexpected input/HEAD drift must still fail frozen-subject checks.
        let approved_revision = plan["status"] == "approved"
            && plan["plan_review"]["pending_revision"] == plan["revision"]
            && plan["revision"].as_u64().zip(plan["plan_review"]["subject"]["revision"].as_u64())
                .is_some_and(|(current, captured)| current > captured);
        if !plan["plan_review"].is_object() || approved_revision {
            let captured = subject(plan)?;
            let reviews = if plan["plan_review"].is_object() {
                plan["plan_review"]["reviews"].as_array().ok_or("invalid plan review history; restore verdict evidence before retrying")?.clone()
            } else { vec![] };
            let mut next = plan.clone();
            next["plan_review"] = json!({"version":1,"attempt_id":crate::architecture::identity(),
                "base":self.plan_review_base(plan)?,"head":self.git(&["rev-parse","HEAD"])?,
                "required_roles":roles,"budget":self.app.settings.lock().unwrap()["max_fix_rounds"].as_i64().unwrap_or(3).max(0),
                "rounds":0,"status":"pending","gate":{"status":"pending"},"reviews":reviews,
                "next_action":"reserve_review","outstanding_requests":[],
                "model_invocations":plan["plan_review"]["model_invocations"].as_array().cloned().unwrap_or_default(),
                "minimum_tier":plan["stages"].as_array().unwrap().iter().map(|s|
                    s["model_agreement"]["policy_inputs"]["minimum_tier"].as_u64().unwrap_or(3)).max().unwrap_or(3),
                "retries":{"operational_retries":0,"history":[],"limits":self.app.settings.lock().unwrap()["reassessment_limits"]},
                "acceptance":acceptance(&captured),"subject":captured});
            for key in ["usage", "role_usage"] {
                if let Some(value) = plan["plan_review"].get(key) {
                    next["plan_review"][key] = value.clone();
                }
            }
            self.validate_plan_subject(&next)?;
            self.save_plan(&next)?;
            *plan = next;
        }
        if matches!(plan["plan_review"]["next_action"].as_str(), Some("finalize" | "complete"))
            || plan["plan_review"]["status"] == "approved" {
            self.commit_plan_reviewed(plan)?;
            return Ok(true);
        }
        self.validate_plan_subject(plan)?;
        if plan["plan_review"]["next_action"].is_null() {
            // Stage-3 checkpoints only ran reviews. Preserve their rejection and
            // consumed round when upgrading an unfinished attempt to the fix loop.
            let rejected = plan["plan_review"]["gate"]["status"] == "blocked";
            plan["plan_review"]["next_action"] = json!(if rejected {"reserve_fix"} else {"reserve_review"});
            if rejected {
                plan["plan_review"]["outstanding_requests"] = json!(plan["plan_review"]["gate"]["requests"].as_array().into_iter().flatten()
                    .map(|r| format!("[{}] {}",r["role"].as_str().unwrap_or("unknown"),r["text"].as_str().unwrap_or(""))).collect::<Vec<_>>());
            }
            self.save_plan(plan)?;
        }
        // An in-flight invocation may have run before a crash. Never replay it or
        // reuse a partial approval under the same reserved round identity.
        if matches!(plan["plan_review"]["next_action"].as_str(), Some("fixing" | "reviewing")) {
            plan["plan_review"]["next_action"] = json!(if plan["plan_review"]["next_action"] == "fixing" {"reserve_fix"} else {"reserve_review"});
        }
        loop {
            self.validate_plan_subject(plan)?;
            if self.session.stop_requested.load(Ordering::SeqCst) { return Err("plan review stopped; resume to continue".into()); }
            let action = plan["plan_review"]["next_action"].as_str().unwrap_or("reserve_review").to_string();
            if matches!(action.as_str(), "reserve_review" | "reserve_fix") {
                let rounds = plan["plan_review"]["rounds"].as_u64().ok_or("invalid plan round counter")?;
                let budget = plan["plan_review"]["budget"].as_u64().ok_or("invalid plan budget")?;
                // Same contract as stages: initial review + B corrective cycles.
                if rounds > budget {
                    plan["plan_review"]["gate"]["status"] = json!("exhausted");
                    plan["plan_review"]["status"] = json!("blocked");
                    plan["plan_review"]["next_action"] = json!("exhausted");
                    self.plan_action_checkpoint(plan)?;
                    self.log_event("plan_review", "plan review budget exhausted; inspect outstanding requests and edit and approve a revised plan; commits remain local");
                    return Ok(false);
                }
                plan["plan_review"]["rounds"] = json!(rounds + 1);
                plan["plan_review"]["gate"] = json!({"status":"pending"});
                plan["plan_review"]["next_action"] = json!(if action == "reserve_fix" {"fix_pending"} else {"review_pending"});
                self.plan_action_checkpoint(plan)?;
            }
            match plan["plan_review"]["next_action"].as_str().ok_or("missing plan next action")? {
                "exhausted" => return Ok(false),
                "fix_pending" => {
                    plan["plan_review"]["next_action"] = json!("fixing");
                    plan["plan_review"]["status"] = json!("fixing");
                    self.plan_action_checkpoint(plan)?;
                    self.run_plan_fixer(plan)?;
                    plan["plan_review"]["next_action"] = json!("review_pending");
                    self.plan_action_checkpoint(plan)?;
                }
                "review_pending" => {
                    let identity = json!({"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":null,"scope":"plan",
                        "attempt_id":plan["plan_review"]["attempt_id"],"round":plan["plan_review"]["rounds"],
                        "policy":plan_policy(&json!(roles)),"snapshot":snapshot(self.project())?});
                    plan["plan_review"]["status"] = json!("reviewing");
                    plan["plan_review"]["next_action"] = json!("reviewing");
                    plan["plan_review"]["gate"] = json!({"status":"pending","identity":identity});
                    self.plan_action_checkpoint(plan)?;
                    self.set_step(None,"plan review");
                    if roles.contains(&json!("reviewer")) { self.plan_reviewer(plan,&identity)?; }
                    if roles.contains(&json!("architect")) { self.architect_review_for_scope(plan,ReviewScope::Plan,&identity)?; }
                    self.validate_plan_subject(plan)?;
                    if self.session.stop_requested.load(Ordering::SeqCst) { return Err("plan review stopped; resume to continue".into()); }
                    let gate = aggregate(&identity,plan["plan_review"]["reviews"].as_array().ok_or("missing plan reviews")?);
                    let approved = gate["status"] == "approved";
                    plan["plan_review"]["outstanding_requests"] = json!(gate["requests"].as_array().unwrap().iter()
                        .map(|r| format!("[{}] {}",r["role"].as_str().unwrap(),r["text"].as_str().unwrap())).collect::<Vec<_>>());
                    plan["plan_review"]["status"] = gate["status"].clone();
                    plan["plan_review"]["gate"] = gate;
                    plan["plan_review"]["next_action"] = json!(if approved {"finalize"} else {"reserve_fix"});
                    self.plan_action_checkpoint(plan)?;
                    if approved {
                        self.commit_plan_reviewed(plan)?;
                        return Ok(true);
                    }
                }
                _ => return Err("invalid plan review next action; restore the checkpoint before retrying".into()),
            }
        }
    }
}
fn plan_policy(roles: &Value) -> Value {
    json!({"version":1,"required_roles":roles,"scope":"plan_aggregate","rationale":"Union of recorded deferred stage review obligations; review all completed stages together"})
}
