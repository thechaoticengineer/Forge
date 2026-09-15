use crate::app::Ctx;
use crate::http::ApiResponse;
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;

pub(super) fn api_discussion_message(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let message = body["message"].as_str().unwrap_or("").trim().to_string();
    if message.is_empty() {
        return (400, json!({"error": "message required"}));
    }
    if message.chars().count() > 20000 {
        return (400, json!({"error": "message too long"}));
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let request_id = {
        let mut s = ctx.session.state.lock().unwrap();
        s.discussion_serial += 1;
        let request_id = s.discussion_serial;
        s.discussion_activity = json!({"status":"running","request_id":request_id,"message":message,"unix":unix_timestamp()});
        request_id
    };
    let short: String = message.chars().take(300).collect();
    ctx.log_event("discussion", &short);
    let ctx2 = ctx.clone();
    std::thread::spawn(move || ctx2.discuss_worker(&message, request_id));
    (200, json!({"ok": true, "request_id": request_id}))
}

pub(super) fn api_discussion_reset(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    match fs::remove_file(ctx.forge_path("discussion.jsonl")) {
        Ok(()) => {},
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return (500, json!({"error": format!("could not remove discussion: {e}")})),
    }
    ctx.session.state.lock().unwrap().discussion_activity = Value::Null;
    ctx.log_event("discussion", "discussion cleared");
    (200, json!({"ok": true}))
}
