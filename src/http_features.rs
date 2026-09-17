//! Feature-spec endpoints of the spec phase (M2): per-feature runtime state
//! and creation from the template. Discovery itself stays in
//! `crate::features` and is read by `/api/features`.
use crate::app::Ctx;
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
