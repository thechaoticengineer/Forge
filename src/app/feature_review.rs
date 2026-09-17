//! Architect spec review of one feature folder (M2, S13/S14, decision D9).
//!
//! The review is a turn of the *persistent* architect: read-only, with no
//! stage, no snapshot and no project checks, returning a structured verdict
//! about consistency, feasibility and conflicts with the existing
//! architecture. It reuses the existing review invariants rather than a
//! weaker parallel path:
//!
//! * the whole turn holds `session.architect_lock`, so it never interleaves
//!   with a plan's architect turn;
//! * when the plan's architect session is ready and compatible it is resumed,
//!   the `architect-pending.json` marker is published before the invocation,
//!   and the validated turn is published through the architecture store under
//!   `persistence_lock`, failing when the plan changed meanwhile;
//! * otherwise the feature owns its own architect session, recorded in
//!   `.forge/features/<slug>.json` so a later review resumes it;
//! * every rejection travels the shared correction budget in
//!   `crate::response`, on the same session and the same effective model.
//!
//! Lock order for the whole spec phase: `queue_lock` (admission only, released
//! before the worker starts) -> `busy` -> `architect_lock` -> `persistence_lock`
//! -> `feature_lock`. `feature_lock` is innermost and is never held across a
//! provider invocation.
use super::{Ctx, WorkerGuard};
use crate::agent::{AgentRequest, AgentResult};
use crate::feature_state;
use crate::prompts::FEATURE_REVIEW_PROMPT;
use crate::util::{fill_template, json_payload, unix_timestamp};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

/// Bounds on one verdict, so a runaway response cannot be stored.
pub(crate) const MAX_VERDICT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_VERDICT_ENTRIES: usize = 64;
pub(crate) const MAX_ENTRY_BYTES: usize = 4000;

/// The outcome of one validated architect turn.
struct Turn {
    verdict: Value,
    session: String,
    provider: String,
    model: String,
    /// True when the session belongs to this feature rather than to the plan.
    /// Only such a session is recorded as the feature's own; a plan session is
    /// owned by the plan's checkpoint and is resumed from there next time.
    owned: bool,
}

// ---------------------------------------------------------------- validation

/// Pure validation of one spec-review verdict, called inside the shared
/// correction budget: every rejection here is returned to the architect.
///
/// `identity` is `{"slug","content_hash"}` and must be echoed exactly, so a
/// verdict can never be attributed to a different feature or to content the
/// architect did not see. There is deliberately no stage, snapshot or
/// project-check requirement (D9).
pub(crate) fn validate_verdict(identity: &Value, text: &str) -> Result<Value, String> {
    if text.len() > MAX_VERDICT_BYTES {
        return Err(format!(
            "verdict is too large: {} bytes (at most {MAX_VERDICT_BYTES})",
            text.len()
        ));
    }
    let output: Value = crate::response::parse_json(json_payload(text))
        .map_err(|e| format!("invalid spec review JSON: {e}"))?;
    crate::response::object_fields(
        &output,
        &["slug", "content_hash", "approved", "summary", "issues", "questions"],
    )?;
    for key in ["slug", "content_hash"] {
        if output[key] != identity[key] {
            return Err(format!(
                "verdict {key} {} does not echo the reviewed feature's {key} {}",
                output[key], identity[key]
            ));
        }
    }
    let approved = output["approved"]
        .as_bool()
        .ok_or("`approved` must be true or false")?;
    let summary = output["summary"]
        .as_str()
        .filter(|summary| !summary.trim().is_empty())
        .ok_or("`summary` must be a non-empty string")?;
    if summary.len() > MAX_VERDICT_BYTES {
        return Err("`summary` is too long".into());
    }
    let mut lists = Vec::new();
    for key in ["issues", "questions"] {
        let entries = output[key]
            .as_array()
            .ok_or_else(|| format!("`{key}` must be an array of strings"))?;
        if entries.len() > MAX_VERDICT_ENTRIES {
            return Err(format!(
                "too many {key}: {} (at most {MAX_VERDICT_ENTRIES})",
                entries.len()
            ));
        }
        for entry in entries {
            let entry = entry
                .as_str()
                .filter(|entry| !entry.trim().is_empty())
                .ok_or_else(|| format!("every entry of `{key}` must be a non-empty string"))?;
            if entry.len() > MAX_ENTRY_BYTES {
                return Err(format!(
                    "an entry of `{key}` is too long: {} bytes (at most {MAX_ENTRY_BYTES})",
                    entry.len()
                ));
            }
        }
        lists.push(Value::Array(entries.clone()));
    }
    let (issues, questions) = (lists.remove(0), lists.remove(0));
    let empty = issues.as_array().unwrap().is_empty() && questions.as_array().unwrap().is_empty();
    if approved && !empty {
        return Err("an approving verdict must leave issues and questions empty".into());
    }
    if !approved && empty {
        return Err("a rejecting verdict needs at least one issue or question".into());
    }
    Ok(json!({"approved": approved, "summary": summary, "issues": issues, "questions": questions}))
}

// ---------------------------------------------------------------- worker

impl Ctx {
    /// Runs one spec review. Only a validated verdict of the *current* folder
    /// content is recorded; any failure records nothing and reports the reason
    /// as the feature activity's error.
    pub(crate) fn feature_review_worker(&self, slug: &str, request_id: i64) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "reviewing a feature spec");
        let result = self.run_feature_review(slug);
        let (activity, kind, text) = match &result {
            Ok(review) => (
                json!({"kind":"review","slug":slug,"request_id":request_id,"status":"ready",
                    "unix":unix_timestamp()}),
                "features",
                format!(
                    "spec review of {slug}: {}",
                    if review["approved"] == json!(true) { "approved" } else { "changes requested" }
                ),
            ),
            Err(error) => (
                json!({"kind":"review","slug":slug,"request_id":request_id,"status":"failed",
                    "error":error,"unix":unix_timestamp()}),
                "error",
                format!("spec review of {slug} failed: {error}"),
            ),
        };
        let current = {
            let mut state = self.session.state.lock().unwrap();
            let current = state.feature_serial == request_id;
            if current {
                state.feature_activity = activity;
            }
            current
        };
        if current {
            self.log_event(kind, &text);
        }
    }

    /// The appended review record on success.
    fn run_feature_review(&self, slug: &str) -> Result<Value, String> {
        // The architect turn owns this lock for its whole duration, exactly as
        // a plan review does, so a feature review and a plan turn can never
        // share the architect's session concurrently.
        let _turn_guard = self.session.architect_lock.lock().unwrap();
        self.ensure_forge_dir();
        let dir = feature_state::feature_dir(self, slug);
        let (saved_session, content_hash) = {
            let _guard = self.session.feature_lock.lock().unwrap();
            let state = feature_state::load(self, slug)?;
            (state["architect_session"].clone(), feature_state::content_hash(&dir)?)
        };
        let identity = json!({"slug": slug, "content_hash": content_hash});

        let requirements = self.model_requirements("architect", None)?;
        let (turn, _choice) = self.with_selected_model(&requirements, None, |choice| {
            self.feature_review_turn(slug, &identity, &saved_session, choice)
        })?;

        // The verdict describes exactly the content that was hashed into the
        // identity; a folder edited mid-review is never silently approved.
        let current = {
            let _guard = self.session.feature_lock.lock().unwrap();
            feature_state::content_hash(&dir)?
        };
        if current != content_hash {
            return Err("feature changed during review".into());
        }
        let Turn { verdict, session, provider, model, owned } = turn;
        let record = json!({
            "id": crate::architecture::identity(),
            "approved": verdict["approved"],
            "summary": verdict["summary"],
            "issues": verdict["issues"],
            "questions": verdict["questions"],
            "content_hash": content_hash,
            "provider": provider,
            "model": model,
            "session": session,
            "unix": unix_timestamp(),
        });
        let stored = record.clone();
        feature_state::update(self, slug, |state| {
            state["reviews"]
                .as_array_mut()
                .ok_or("feature state reviews is not an array")?
                .push(stored);
            if owned {
                state["architect_session"] = json!({"provider": provider, "reference": session});
            }
            Ok(())
        })?;
        Ok(record)
    }

    /// One complete architect turn for a chosen model: session selection,
    /// invocation, bounded correction and — for a resumed plan session — the
    /// publication of the turn into the plan's architecture history.
    fn feature_review_turn(
        &self,
        slug: &str,
        identity: &Value,
        saved_session: &Value,
        choice: &crate::model_selection::ModelChoice,
    ) -> Result<Turn, String> {
        let (provider, model, effort) = choice;
        // A ready, compatible plan session already holds this project's
        // architectural context, so the review continues it instead of
        // starting a parallel architect conversation.
        let plan = self.load_plan();
        let plan_session = plan.as_ref().and_then(|plan| {
            let cp = self.architecture_store().checkpoint(plan).ok()?;
            let reference = cp["session"]["reference"].as_str()?.to_string();
            (cp["context_status"] == "ready"
                && cp["session"]["provider"] == json!(provider)
                && crate::agent::session_id(&reference))
            .then_some((plan.clone(), cp, reference))
        });
        let session = match &plan_session {
            Some((_, _, reference)) => Some(reference.clone()),
            // The feature-owned session is only usable with the provider that
            // created it; a different provider starts a fresh conversation.
            None => saved_session["reference"]
                .as_str()
                .filter(|reference| {
                    saved_session["provider"] == json!(provider)
                        && crate::agent::session_id(reference)
                })
                .map(str::to_owned),
        };
        let owned = plan_session.is_none();
        let prompt = self.feature_review_prompt(slug, identity, plan_session.as_ref().map(|(_, cp, _)| cp))?;

        let turn = crate::architecture::identity();
        if let Some((plan, cp, _)) = &plan_session {
            // Exactly as a plan architect turn: the marker makes an
            // uncommitted or failed turn visible to the next one.
            let pending = self
                .forge_path("architecture")
                .join(plan["plan_id"].as_str().ok_or("missing plan identity")?)
                .join("architect-pending.json");
            crate::durable_json::publish_pretty(
                &pending,
                &json!({"turn":turn,"previous_session":cp["session"]}),
            )?;
        }
        let invoke = |prompt: &str, session: Option<&str>| -> Result<AgentResult, String> {
            let result = if provider == "mock" {
                self.mock_spec_review(slug, identity, prompt, session)?
            } else {
                self.run_agent(&AgentRequest {
                    role: "architect",
                    provider,
                    model,
                    effort,
                    session,
                    prompt,
                })?
            };
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Err("spec review stopped".into());
            }
            if provider != "mock" {
                let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
                let selected = self.model_facts(
                    &policy,
                    crate::catalogue::Provider::parse(provider).ok_or("invalid architect provider")?,
                    model,
                    None,
                );
                let expected = selected["resolved_id"].as_str().unwrap_or(model);
                if !result.model_reported
                    || selected["eligible"] != true
                    || !crate::agent::same_model(provider, expected, &result.effective_model)
                {
                    return Err(format!(
                        "model routing blocked: architect effective model or eligibility changed (expected {expected}, reported {}, model_reported {}, eligible {})",
                        result.effective_model, result.model_reported, selected["eligible"]
                    ));
                }
            }
            let reference = result
                .session
                .as_deref()
                .filter(|reference| crate::agent::session_id(reference))
                .ok_or("architect omitted exact session identity")?;
            if session.is_some_and(|resumed| resumed != reference) {
                return Err("architect spec review changed session identity".into());
            }
            Ok(result)
        };
        let initial = invoke(&prompt, session.as_deref())?;
        let reference = initial.session.clone().unwrap_or_default();
        let expected_model = initial.effective_model.clone();
        let (result, verdict) = self.repair_response(
            "architect spec review",
            initial,
            |response| validate_verdict(identity, &response.output),
            |response, error| {
                // Corrections continue the SAME session with the SAME effective
                // model, so the architect sees its own rejected verdict.
                let correction = crate::response::correction_prompt(&prompt, &response.output, error);
                let repaired = invoke(&correction, Some(&reference))?;
                if !repaired.completed
                    || (provider != "mock"
                        && !crate::agent::same_model(provider, &expected_model, &repaired.effective_model))
                {
                    return Err(
                        "architect spec review correction changed model or did not complete".into(),
                    );
                }
                Ok(repaired)
            },
        )?;
        let effective = if result.effective_model.is_empty() {
            model.clone()
        } else {
            result.effective_model.clone()
        };
        if let Some((plan, mut cp, _)) = plan_session {
            cp["last_turn"] = json!(turn);
            cp["session"]["checkpoint_reference"] = json!(turn);
            let _lock = self.session.persistence_lock.lock().unwrap();
            let store = self.architecture_store();
            if store.load()? != Some(plan.clone()) {
                return Err("plan changed during feature spec review".into());
            }
            store.publish(
                plan,
                cp,
                json!({"kind":"feature_spec_review","slug":slug,
                    "content_hash":identity["content_hash"],"turn":turn,"verdict":verdict}),
            )?;
        }
        Ok(Turn {
            verdict,
            session: reference,
            provider: provider.clone(),
            model: effective,
            owned,
        })
    }

    /// The read-only review prompt: what the architect is judging, the folder's
    /// current text, and the exact identity it must echo.
    fn feature_review_prompt(
        &self,
        slug: &str,
        identity: &Value,
        cp: Option<&Value>,
    ) -> Result<String, String> {
        let folder = format!("docs/features/{slug}");
        let folder_text = super::feature_author::folder_text(&feature_state::feature_dir(self, slug), &folder)?;
        let context = match cp {
            Some(cp) => format!(
                "You are continuing this project's saved architect session.\nSaved constraints: {}\nCompleted interfaces: {}\n",
                cp["constraints"], cp["completed_interfaces"]
            ),
            None => String::new(),
        };
        Ok(fill_template(FEATURE_REVIEW_PROMPT, &[
            ("{slug}", slug),
            ("{folder}", &folder),
            ("{folder_text}", &folder_text),
            ("{plan_context}", &context),
            ("{content_hash}", identity["content_hash"].as_str().unwrap_or("")),
            ("{max_entries}", &MAX_VERDICT_ENTRIES.to_string()),
        ]))
    }

    /// Deterministic stand-in for a provider spec review, used by the `mock`
    /// provider in the self-test and in tests. Like `mock_architect` it is not
    /// behind `cfg(test)`, so the mock provider works in a real engine too.
    ///
    /// `mock_spec_review_output` scripts the returned text: a string or object
    /// is used as is (an object without `slug`/`content_hash` gets the
    /// reviewed identity, so a script can state only the verdict), and an
    /// array scripts successive turns, popping the front while more than one
    /// entry remains. Null or absent produces an approving verdict.
    fn mock_spec_review(
        &self,
        slug: &str,
        identity: &Value,
        prompt: &str,
        session: Option<&str>,
    ) -> Result<AgentResult, String> {
        let mut settings = self.app.settings.lock().unwrap();
        // A resumed review keeps the caller's session; a fresh one mints a
        // stable UUID-shaped identity, exactly like the plan architect mock.
        let minted = crate::architect::mock_session_uuid(&crate::architecture::identity());
        let reference = session.unwrap_or(&minted).to_string();
        settings
            .as_object_mut()
            .ok_or("invalid settings")?
            .entry("mock_spec_review_requests")
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or("invalid mock_spec_review_requests")?
            .push(json!({"slug":slug,"session":reference,"prompt":prompt}));
        let scripted = match settings.get_mut("mock_spec_review_output").and_then(Value::as_array_mut) {
            Some(queue) if queue.len() > 1 => Some(queue.remove(0)),
            Some(queue) => queue.first().cloned(),
            None => settings.get("mock_spec_review_output").cloned(),
        };
        let output = match scripted.filter(|value| !value.is_null()) {
            Some(Value::String(text)) => text,
            Some(mut value) => {
                if let Some(object) = value.as_object_mut() {
                    for key in ["slug", "content_hash"] {
                        if !object.contains_key(key) {
                            object.insert(key.into(), identity[key].clone());
                        }
                    }
                }
                value.to_string()
            },
            None => json!({"slug": slug, "content_hash": identity["content_hash"], "approved": true,
                "summary": "The spec is consistent with the existing architecture.",
                "issues": [], "questions": []})
            .to_string(),
        };
        Ok(AgentResult {
            output,
            session: Some(reference),
            effective_model: "mock-architect".into(),
            completed: true,
            ..AgentResult::default()
        })
    }
}
