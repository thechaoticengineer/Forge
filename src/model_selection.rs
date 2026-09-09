//! Shared model resolution, availability and fallback policy for every agent role.
//! Callers supply role constraints; stage agreements and session recovery remain
//! owned by their lifecycle, never silently replaced inside the process launcher.
use crate::{
    app::Ctx,
    catalogue::{Policy, Provider, Tier},
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(crate) type ModelChoice = (String, String, String);

#[derive(Clone)]
pub(crate) struct ModelRequirements {
    role: String,
    provider: String,
    model: String,
    minimum_tier: Option<Tier>,
    fallback: bool,
}

fn tier_rank(tier: &Tier) -> u8 {
    match tier {
        Tier::Basic => 1,
        Tier::Standard => 2,
        Tier::Strong => 3,
    }
}

impl ModelRequirements {
    pub(crate) fn pinned(mut self) -> Self {
        self.fallback = false;
        self
    }

    pub(crate) fn for_class(role: &str, provider: &str, minimum_tier: Tier) -> Self {
        Self {
            role: role.into(),
            provider: provider.into(),
            model: String::new(),
            minimum_tier: Some(minimum_tier),
            fallback: true,
        }
    }
}

pub(crate) fn failure_kind(error: &str) -> &'static str {
    let error = error.to_lowercase();
    if crate::catalogue::auth_error(&error) {
        return "auth";
    }
    if [
        "rate limit",
        "rate_limit",
        "429",
        "overloaded",
        "503",
        "timeout",
        "temporarily unavailable",
        "model is at capacity",
    ]
    .iter()
    .any(|s| error.contains(s))
    {
        "transient"
    } else {
        "tool_process"
    }
}

pub(crate) fn claude_model_limit(error: &str, model: &str) -> Option<String> {
    if !error.starts_with("claude exited with exit status:") {
        return None;
    }
    let error = error.to_lowercase();
    let family = ["fable", "opus", "sonnet", "haiku"]
        .into_iter()
        .find(|family| {
            model
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|part| part.eq_ignore_ascii_case(family))
                && error.contains(&format!("you've reached your {family} limit."))
        })?;
    Some(format!("Claude CLI reports {family} quota exhausted"))
}

impl Ctx {
    pub(crate) fn model_requirements(
        &self,
        role: &str,
        implementer: Option<&str>,
    ) -> Result<ModelRequirements, String> {
        let settings = self.app.settings.lock().unwrap();
        let configured_role = if role == "chat" { "planner" } else { role };
        let automatic = settings["automatic_routing"] != false;
        let model = settings[format!("{configured_role}_model")]
            .as_str()
            .unwrap_or("")
            .to_string();
        let configured = settings[configured_role].as_str().unwrap_or("codex");
        let provider = if role == "reviewer" {
            let other = match implementer {
                Some("codex") => "claude",
                Some("claude") => "codex",
                Some("mock") if configured == "mock" => "mock",
                _ => return Err("cannot resolve independent reviewer provider".into()),
            };
            if configured != other && (!automatic || !model.is_empty()) {
                return Err(format!(
                    "independent reviewer must be configured as {other}, the other provider relative to {}",
                    implementer.unwrap()
                ));
            }
            other
        } else if role == "architect" && settings["planner"] == "mock" && model.is_empty() {
            "mock"
        } else {
            configured
        };
        let mut requirements = ModelRequirements::for_class(role, provider, Tier::Strong);
        if role == "reviewer" && !model.is_empty() {
            requirements.minimum_tier = None;
        }
        requirements.fallback = automatic && model.is_empty();
        requirements.model = model;
        Ok(requirements)
    }

    /// All low-level catalogue resolution uses this adapter, including fixed
    /// implementation agreements and post-execution identity verification.
    pub(crate) fn model_facts(
        &self,
        policy: &Policy,
        provider: Provider,
        model: &str,
        effort: Option<&str>,
    ) -> Value {
        match effort {
            Some(effort) => self
                .app
                .catalogue
                .execution_with_effort(policy, provider, model, effort),
            None => self.app.catalogue.execution_input(policy, provider, model),
        }
    }

    pub(crate) fn model_quota_reason(
        &self,
        policy: &Policy,
        provider: Provider,
        model: &str,
        refresh: bool,
    ) -> Option<String> {
        if provider != Provider::Claude {
            return None;
        }
        if refresh {
            self.app.quota.check(&policy.claude_bridge, model).err()
        } else {
            self.app.quota.blocked_reason(&policy.claude_bridge, model)
        }
    }

    pub(crate) fn execution_selection(
        &self,
        policy: &Policy,
        provider: Provider,
        model: &str,
        effort: &str,
    ) -> Value {
        let mut facts = self.model_facts(policy, provider, model, Some(effort));
        if facts["eligible"] == true {
            let resolved = facts["resolved_id"].as_str().unwrap_or(model);
            if let Some(reason) = self.model_quota_reason(policy, provider, resolved, true) {
                facts["eligible"] = Value::Bool(false);
                facts["error"] = Value::String(reason);
            }
        }
        facts
    }

    pub(crate) fn select_model(
        &self,
        requirements: &ModelRequirements,
        excluded: &[(String, String)],
    ) -> Result<ModelChoice, String> {
        if requirements.provider == "mock" {
            return Ok((
                "mock".into(),
                requirements.model.clone(),
                "provider_default".into(),
            ));
        }
        let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
        let provider = Provider::parse(&requirements.provider).ok_or("invalid model provider")?;
        let mut candidates: Vec<_> = if requirements.model.is_empty() {
            policy
                .entries
                .iter()
                .filter(|e| {
                    e.provider == provider
                        && requirements
                            .minimum_tier
                            .as_ref()
                            .is_none_or(|minimum| tier_rank(&e.tier) >= tier_rank(minimum))
                })
                .map(|e| e.model.clone())
                .collect()
        } else {
            vec![requirements.model.clone()]
        };
        if requirements.model.is_empty() {
            candidates.sort_by_key(|model| {
                policy
                    .entries
                    .iter()
                    .find(|e| e.provider == provider && &e.model == model)
                    .map(|e| tier_rank(&e.tier))
                    .unwrap_or(0)
            });
        }
        let mut blockers = Vec::new();
        for model in candidates {
            if excluded
                .iter()
                .any(|(p, m)| p == provider.name() && m == &model)
            {
                continue;
            }
            if requirements.minimum_tier.as_ref().is_some_and(|minimum| {
                !policy.entries.iter().any(|e| {
                    e.provider == provider
                        && e.model == model
                        && tier_rank(&e.tier) >= tier_rank(minimum)
                })
            }) {
                continue;
            }
            let facts = self.model_facts(&policy, provider, &model, None);
            if facts["eligible"] != true {
                continue;
            }
            if let Some(reason) = self.model_quota_reason(
                &policy,
                provider,
                facts["resolved_id"].as_str().unwrap_or(&model),
                false,
            ) {
                if !requirements.fallback {
                    return Err(reason);
                }
                blockers.push(reason);
                continue;
            }
            return Ok((
                provider.name().into(),
                model,
                facts["effort"]
                    .as_str()
                    .unwrap_or("provider_default")
                    .into(),
            ));
        }
        let label = if requirements.role == "reviewer" {
            "independent reviewer"
        } else {
            &requirements.role
        };
        let class = requirements
            .minimum_tier
            .as_ref()
            .map(|tier| match tier {
                Tier::Basic => "basic ",
                Tier::Standard => "standard ",
                Tier::Strong => "strong ",
            })
            .unwrap_or("");
        Err(format!(
            "no eligible {class}{label} model{}",
            if blockers.is_empty() {
                String::new()
            } else {
                format!(": {}", blockers.join("; "))
            }
        ))
    }

    /// One replacement decision for fresh planner/chat/review turns and for
    /// architect recovery. Callers must reconstruct a persistent session when
    /// accepting a different choice; fixed stage assignments never use this path.
    pub(crate) fn model_fallback(
        &self,
        requirements: &ModelRequirements,
        current: &ModelChoice,
        error: &str,
        visited: &[(String, String)],
    ) -> Result<Option<ModelChoice>, String> {
        if !requirements.fallback
            || current.0 != "claude"
            || self.session.stop_requested.load(Ordering::SeqCst)
        {
            return Ok(None);
        }
        let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
        let facts = self.model_facts(&policy, Provider::Claude, &current.1, Some(&current.2));
        let resolved = facts["resolved_id"].as_str().unwrap_or(&current.1);
        let reason = self
            .model_quota_reason(&policy, Provider::Claude, resolved, false)
            .or_else(|| claude_model_limit(error, resolved));
        let Some(reason) = reason else {
            return Ok(None);
        };
        let next = self
            .select_model(requirements, visited)
            .map_err(|error| format!("{error}; {reason}"))?;
        if next == *current {
            return Ok(None);
        }
        self.log_event(
            "model",
            &format!(
                "[{}] quota fallback {}/{} -> {}/{}: {reason}",
                requirements.role, current.0, current.1, next.0, next.1
            ),
        );
        Ok(Some(next))
    }

    pub(crate) fn with_selected_model<T>(
        &self,
        requirements: &ModelRequirements,
        initial: Option<ModelChoice>,
        mut invoke: impl FnMut(&ModelChoice) -> Result<T, String>,
    ) -> Result<(T, ModelChoice), String> {
        let mut choice = match initial {
            Some(choice) => choice,
            None => self.select_model(requirements, &[])?,
        };
        let mut visited = vec![(choice.0.clone(), choice.1.clone())];
        loop {
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Err("model selection stopped; saved work retained".into());
            }
            match invoke(&choice) {
                Ok(output) => return Ok((output, choice)),
                Err(error) => match self.model_fallback(requirements, &choice, &error, &visited)? {
                    Some(next) => {
                        visited.push((next.0.clone(), next.1.clone()));
                        choice = next;
                    }
                    None => return Err(error),
                },
            }
        }
    }

    pub(crate) fn bootstrap(&self, role: &str) -> Result<ModelChoice, String> {
        self.select_model(&self.model_requirements(role, None)?, &[])
    }
    pub(crate) fn reviewer_config(&self, implementer: &str) -> Result<(String, String), String> {
        let choice = self.select_model(
            &self.model_requirements("reviewer", Some(implementer))?,
            &[],
        )?;
        Ok((choice.0, choice.1))
    }
}

impl Ctx {
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
}

impl Ctx {
    /// Quota affects new candidates, not the durable validity of a saved agreement.
    pub(crate) fn routing_candidates(&self) -> Result<Vec<Value>, String> {
        let mut options = self.routing_options()?;
        let policy = Policy::from_settings(&self.app.settings.lock().unwrap())?;
        for option in &mut options {
            if option["eligible"] == true
                && let Some(provider) = option["provider"].as_str().and_then(Provider::parse)
                && let Some(reason) = self.model_quota_reason(
                    &policy,
                    provider,
                    option["resolved_id"]
                        .as_str()
                        .or_else(|| option["model"].as_str())
                        .unwrap_or(""),
                    false,
                )
            {
                option["eligible"] = json!(false);
                option["error"] = json!(reason);
            }
        }
        Ok(options)
    }
}

#[cfg(test)]
#[path = "model_selection_tests.rs"]
mod tests;
