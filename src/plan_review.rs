//! Frozen plan subjects and the deferred review publication gate.
use super::*;
use std::collections::BTreeSet;

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
        for call in stage["model_invocations"].as_array().into_iter().flatten() {
            if matches!(call["role"].as_str(), Some("implementer" | "fixer")) {
                let requested = call["requested"]["provider"].as_str().ok_or("unknown implementation invocation provider; restore provenance before retrying")?;
                result.insert(requested.into());
                if let Some(effective) = call["effective"]["provider"].as_str() { result.insert(effective.into()); }
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
        let review = &plan["plan_review"];
        if review["version"] != 1 || review["subject"] != subject(plan)?
            || review["required_roles"] != json!(self.deferred_plan_roles(plan)?)
            || review["acceptance"] != acceptance(&review["subject"])
            || review["base"] != self.plan_review_base(plan)?
            || review["head"] != self.git(&["rev-parse", "HEAD"])? {
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
        if used.contains(provider) && !(provider == "mock" && used.len() == 1) { return Err(INDEPENDENCE.into()); }
        Ok(())
    }

    fn plan_reviewer(&self, plan: &mut Value, identity: &Value) -> Result<Value, String> {
        let used = providers(plan)?;
        if used.len() != 1 { return Err(INDEPENDENCE.into()); }
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

    pub(crate) fn validate_plan_approval(&self, plan: &Value) -> Result<(), String> {
        if self.deferred_plan_roles(plan)?.is_empty() { return Ok(()); }
        self.validate_plan_subject(plan)?;
        let review = &plan["plan_review"];
        let identity = &review["gate"]["identity"];
        if review["status"] != "approved" || review["gate"]["status"] != "approved"
            || identity["scope"] != "plan" || !identity["stage_id"].is_null()
            || identity["plan_id"] != plan["plan_id"] || identity["revision"] != plan["revision"]
            || identity["attempt_id"] != review["attempt_id"] || identity["round"] != review["rounds"]
            || identity["policy"] != plan_policy(&review["required_roles"])
            || identity["snapshot"] != snapshot(self.project())? {
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

    pub(crate) fn run_plan_review(&self, plan: &mut Value) -> Result<bool, String> {
        let result = self.run_plan_review_inner(plan);
        match result {
            Ok(true) => Ok(true),
            outcome => {
                let stopped = self.session.stop_requested.load(Ordering::SeqCst);
                let failed = outcome.is_err();
                let error = outcome.err().unwrap_or_else(|| "plan review rejected; address the recorded requests before retrying; commits remain local".into());
                if plan["plan_review"].is_object() {
                    plan["plan_review"]["status"] = json!(if stopped {"interrupted"} else {"blocked"});
                    if stopped || failed || plan["plan_review"]["gate"]["status"] != "blocked" {
                        plan["plan_review"]["gate"] = json!({"status":if stopped {"interrupted"} else {"error"},"error":error});
                    }
                    self.save_plan(plan)?;
                }
                self.set_phase(if stopped {"plan_ready"} else {"blocked"});
                self.log_event("plan_review",&error);
                Ok(false)
            }
        }
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
                "acceptance":acceptance(&captured),"subject":captured});
            self.validate_plan_subject(&next)?;
            self.save_plan(&next)?;
            *plan = next;
        }
        self.validate_plan_subject(plan)?;
        if plan["plan_review"]["status"] == "approved" {
            self.validate_plan_approval(plan)?;
            return Ok(true);
        }
        let rounds = plan["plan_review"]["rounds"].as_u64().ok_or("invalid plan round counter")?;
        let budget = plan["plan_review"]["budget"].as_u64().ok_or("invalid plan budget")?;
        if rounds > budget { return Err("plan review budget exhausted; revise the plan or resolve its review configuration; commits remain local".into()); }
        let identity = json!({"plan_id":plan["plan_id"],"revision":plan["revision"],"stage_id":null,"scope":"plan",
            "attempt_id":plan["plan_review"]["attempt_id"],"round":rounds+1,"policy":plan_policy(&json!(roles)),"snapshot":snapshot(self.project())?});
        plan["plan_review"]["rounds"] = json!(rounds+1);
        plan["plan_review"]["status"] = json!("reviewing");
        plan["plan_review"]["gate"] = json!({"status":"pending","identity":identity});
        self.save_plan(plan)?;
        self.set_step(None,"plan review");
        if self.session.stop_requested.load(Ordering::SeqCst) { return Err("plan review stopped; resume to continue".into()); }
        if roles.contains(&json!("reviewer")) { self.plan_reviewer(plan,&identity)?; }
        if roles.contains(&json!("architect")) { self.architect_review_for_scope(plan,ReviewScope::Plan,&identity)?; }
        self.validate_plan_subject(plan)?;
        if self.session.stop_requested.load(Ordering::SeqCst) { return Err("plan review stopped; resume to continue".into()); }
        let gate = aggregate(&identity,plan["plan_review"]["reviews"].as_array().unwrap());
        let approved = gate["status"] == "approved";
        plan["plan_review"]["status"] = gate["status"].clone();
        plan["plan_review"]["gate"] = gate;
        self.save_plan(plan)?;
        if approved { self.validate_plan_approval(plan)?; }
        Ok(approved)
    }
}
fn plan_policy(roles: &Value) -> Value {
    json!({"version":1,"required_roles":roles,"scope":"plan_aggregate","rationale":"Union of recorded deferred stage review obligations; review all completed stages together"})
}
