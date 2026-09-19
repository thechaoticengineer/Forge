//! Feature-spec endpoints of the spec phase (M2): per-feature runtime state
//! and creation from the template, plus planning an approved milestone (M3).
//! Discovery itself stays in `crate::features` and is read by `/api/features`.
use crate::app::{Ctx, PlanMode};
use crate::feature_state;
use crate::http::{ApiResponse, query_value};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::Ordering;

pub(super) fn api_feature_state(ctx: &Ctx, query: &str) -> ApiResponse {
    let slug = match query_value(query, "slug") {
        Ok(Some(slug)) => slug,
        Ok(None) => return (400, json!({"error": "slug required"})),
        Err(error) => return (400, json!({"error": error})),
    };
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let Some(feature) = crate::features::discover(Path::new(ctx.project()))
        .into_iter()
        .find(|feature| feature.slug == slug)
    else {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    };
    match feature_state::snapshot(ctx, &slug) {
        Ok(snapshot) => (
            200,
            json!({
                "project": ctx.project(),
                "slug": slug,
                "valid": feature.reasons.is_empty(),
                "reasons": feature.reasons,
                "spec_status": snapshot.spec_status,
                "content_hash": snapshot.content_hash,
                "latest_review": snapshot.latest_review,
                "review_current": snapshot.review_current,
                "state": snapshot.state,
                "activity": feature_state::activity(ctx),
            }),
        ),
        Err(error) => (500, json!({"error": error})),
    }
}

/// `GET /api/features/content` (M5 S37): transport only; the read-only
/// content policy lives in `crate::feature_content`.
pub(super) fn api_feature_content(ctx: &Ctx, query: &str) -> ApiResponse {
    let slug = match query_value(query, "slug") {
        Ok(Some(slug)) => slug,
        Ok(None) => return (400, json!({"error": "slug required"})),
        Err(error) => return (400, json!({"error": error})),
    };
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let Some(feature) = crate::features::discover(Path::new(ctx.project()))
        .into_iter()
        .find(|feature| feature.slug == slug)
    else {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    };
    match crate::feature_content::read(&feature) {
        Ok(content) => (200, content),
        Err(error) => (403, json!({"error": error})),
    }
}

pub(super) fn api_feature_create(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let slug = body["slug"].as_str().unwrap_or("").to_string();
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let title = match feature_state::validate_title(body["title"].as_str().unwrap_or("")) {
        Ok(title) => title,
        Err(error) => return (400, json!({"error": error})),
    };
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.session.busy.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    match feature_state::create(ctx, &slug, &title) {
        Ok(()) => {
            ctx.log_event("features", &format!("created feature {slug}"));
            (200, json!({"ok": true, "slug": slug}))
        },
        Err(refusal) => (refusal.status, json!({"error": refusal.message})),
    }
}

/// Admission of one co-authoring message (S11/S12). Mirrors the discussion
/// endpoint: the queue lock plus `acquire_busy` make sure only one worker is
/// ever in flight, and the worker itself owns every file write.
pub(super) fn api_feature_chat(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let slug = body["slug"].as_str().unwrap_or("").to_string();
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let message = body["message"].as_str().unwrap_or("").trim().to_string();
    if message.is_empty() {
        return (400, json!({"error": "message required"}));
    }
    if message.chars().count() > 20000 {
        return (400, json!({"error": "message too long"}));
    }
    if !crate::features::discover(Path::new(ctx.project()))
        .iter()
        .any(|feature| feature.slug == slug)
    {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let request_id = {
        let mut state = ctx.session.state.lock().unwrap();
        state.feature_serial += 1;
        let request_id = state.feature_serial;
        state.feature_activity = json!({"kind":"chat","slug":slug,"request_id":request_id,
            "status":"running","unix":crate::util::unix_timestamp()});
        request_id
    };
    let short: String = message.chars().take(300).collect();
    ctx.log_event("features", &format!("co-authoring {slug}: {short}"));
    let worker = ctx.clone();
    let worker_slug = slug.clone();
    std::thread::spawn(move || worker.feature_chat_worker(&worker_slug, &message, request_id));
    (200, json!({"ok": true, "request_id": request_id}))
}

/// Admission of one architect spec review (S13/S14).
///
/// An invalid feature is refused with its validation reasons *before* the
/// engine takes the busy claim or starts any agent, so a refusal never blocks
/// other work and never reaches a provider (S14).
pub(super) fn api_feature_review(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let slug = body["slug"].as_str().unwrap_or("").to_string();
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let Some(feature) = crate::features::discover(Path::new(ctx.project()))
        .into_iter()
        .find(|feature| feature.slug == slug)
    else {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    };
    if !feature.reasons.is_empty() {
        return (
            409,
            json!({
                "error": format!("feature is invalid: {}", feature.reasons.join("; ")),
                "reasons": feature.reasons,
            }),
        );
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let request_id = {
        let mut state = ctx.session.state.lock().unwrap();
        state.feature_serial += 1;
        let request_id = state.feature_serial;
        state.feature_activity = json!({"kind":"review","slug":slug,"request_id":request_id,
            "status":"running","unix":crate::util::unix_timestamp()});
        request_id
    };
    ctx.log_event("features", &format!("architect spec review of {slug}"));
    let worker = ctx.clone();
    let worker_slug = slug.clone();
    std::thread::spawn(move || worker.feature_review_worker(&worker_slug, request_id));
    (200, json!({"ok": true, "request_id": request_id}))
}

/// Which approval one request asks for.
enum Approval {
    Spec,
    Scenarios,
}

/// Approval of the spec (S15/S16) or of the scenarios (S17), both synchronous.
///
/// Admission refuses an invalid feature with its reasons before claiming the
/// busy flag, then holds the busy claim for the whole approval so the Git work
/// and the state append can never interleave with a chat, a review or another
/// approval (M2-ARCH-003). The claim is released by `BusyGuard` on every path.
fn api_feature_approve(ctx: &Ctx, body: &Value, approval: Approval) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let slug = body["slug"].as_str().unwrap_or("").to_string();
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let Some(feature) = crate::features::discover(Path::new(ctx.project()))
        .into_iter()
        .find(|feature| feature.slug == slug)
    else {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    };
    if !feature.reasons.is_empty() {
        return (
            409,
            json!({
                "error": format!("feature is invalid: {}", feature.reasons.join("; ")),
                "reasons": feature.reasons,
            }),
        );
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    let _busy = crate::app::feature_approval::BusyGuard(&ctx.session);
    let (what, result) = match approval {
        Approval::Spec => ("spec", ctx.approve_feature_spec(&slug)),
        Approval::Scenarios => ("scenarios", ctx.approve_feature_scenarios(&slug)),
    };
    match result {
        Ok(response) => {
            let commit = response["commit"].as_str().unwrap_or("");
            ctx.log_event("features", &format!("approved {slug} {what} at {commit}"));
            (200, response)
        },
        Err(refusal) => {
            ctx.log_event(
                if refusal.status == 409 { "features" } else { "error" },
                &format!("{what} approval of {slug} refused: {}", refusal.message),
            );
            (refusal.status, json!({"error": refusal.message}))
        },
    }
}

pub(super) fn api_feature_approve_spec(ctx: &Ctx, body: &Value) -> ApiResponse {
    api_feature_approve(ctx, body, Approval::Spec)
}

pub(super) fn api_feature_approve_scenarios(ctx: &Ctx, body: &Value) -> ApiResponse {
    api_feature_approve(ctx, body, Approval::Scenarios)
}

/// Admission of milestone planning (S27/S28).
///
/// Every refusal is decided from read-only checks before the busy claim, so a
/// refused request starts no agent and writes neither the plan nor the feature
/// state (M3-ARCH-02). Once the claim is taken, the plan link is recorded
/// first; if that write fails the claim is released and nothing starts.
pub(super) fn api_feature_plan(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let slug = body["slug"].as_str().unwrap_or("").to_string();
    if let Err(error) = feature_state::validate_slug(&slug) {
        return (400, json!({"error": error}));
    }
    let milestone_id = body["milestone"].as_str().unwrap_or("").trim().to_string();
    if milestone_id.is_empty() {
        return (400, json!({"error": "milestone required"}));
    }
    let Some(feature) = crate::features::discover(Path::new(ctx.project()))
        .into_iter()
        .find(|feature| feature.slug == slug)
    else {
        return (404, json!({"error": format!("unknown feature: {slug}")}));
    };
    if !feature.reasons.is_empty() {
        return (
            409,
            json!({
                "error": format!("feature is invalid: {}", feature.reasons.join("; ")),
                "reasons": feature.reasons,
            }),
        );
    }
    let snapshot = match feature_state::snapshot(ctx, &slug) {
        Ok(snapshot) => snapshot,
        Err(error) => return (500, json!({"error": error})),
    };
    if snapshot.spec_status != "scenarios approved" {
        return (
            409,
            json!({"error": format!(
                "feature {slug} is not scenarios approved for its current content (status: {})",
                snapshot.spec_status
            )}),
        );
    }
    let Some(milestone) = feature.milestones.iter().find(|m| m.id == milestone_id) else {
        return (404, json!({"error": format!("unknown milestone: {milestone_id}")}));
    };
    if milestone.implemented {
        return (409, json!({"error": format!("milestone {milestone_id} is already implemented")}));
    }
    if milestone.covers.is_empty() {
        return (
            409,
            json!({"error": format!("milestone {milestone_id} covers no scenarios yet (Covers: none yet)")}),
        );
    }
    let approved =
        feature_state::approved_scenario_ids(&snapshot.state, &snapshot.content_hash).unwrap_or_default();
    let unapproved: Vec<&str> = milestone
        .covers
        .iter()
        .filter(|id| !approved.contains(id))
        .map(String::as_str)
        .collect();
    if !unapproved.is_empty() {
        return (
            409,
            json!({"error": format!(
                "milestone {milestone_id} covers scenarios that are not approved: {}",
                unapproved.join(", ")
            )}),
        );
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }

    let folder = format!("docs/features/{slug}/");
    let goal = format!(
        "Implement milestone {milestone_id} ({}) of the feature {} specified in {folder}, covering scenarios {}.",
        milestone.title,
        feature.title,
        milestone.covers.join(", ")
    );
    let reference = json!({
        "slug": slug,
        "milestone": milestone_id,
        "title": milestone.title,
        "scenario_ids": milestone.covers,
        "folder": folder,
    });
    if let Err(error) = feature_state::append_plan_link(ctx, &reference, &goal) {
        ctx.session.busy.store(false, Ordering::SeqCst);
        ctx.log_event("error", &format!("planning {slug} {milestone_id} not started: {error}"));
        return (500, json!({"error": error}));
    }
    let short: String = goal.chars().take(300).collect();
    super::plan::start_planning(
        ctx,
        goal.clone(),
        &format!("planning started for goal: {short}"),
        PlanMode::Milestone { feature: reference },
    );
    (200, json!({"ok": true, "goal": goal}))
}
