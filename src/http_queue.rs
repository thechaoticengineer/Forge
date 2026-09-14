use crate::app::Ctx;
use crate::http::ApiResponse;
use crate::plan::mutate_queue;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(super) fn api_queue_mutate(ctx: &Ctx, path: &str, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let mut queue = ctx.load_queue();
    let action = path.strip_prefix("/api/queue/").unwrap();
    match mutate_queue(&mut queue, action, body) {
        Ok(event) => {
            ctx.save_queue(&queue);
            ctx.log_event("queue", &event);
            (200, json!({"ok": true}))
        }
        Err(e) => (400, json!({"error": e})),
    }
}

pub(super) fn api_queue_start(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let queue = ctx.load_queue();
    let Some(head) = crate::plan::queue_head(&queue) else {
        return (400, json!({"error": "no queued goals"}));
    };
    if !matches!(
        head["status"].as_str(),
        Some("queued" | "blocked" | "failed" | "planning" | "awaiting_approval" | "running")
    ) {
        return (409, json!({"error": crate::plan::queue_order_error(head)}));
    }
    if ctx.acquire_busy().is_err() {
        (409, json!({"error": "busy"}))
    } else {
        ctx.session.stop_requested.store(false, Ordering::SeqCst);
        ctx.session.queue_active.store(true, Ordering::SeqCst);
        ctx.log_event("queue", "queue started");
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.queue_worker(None));
        (200, json!({"ok": true}))
    }
}
