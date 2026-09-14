use crate::app::{Ctx, PlanMode};
use crate::http::ApiResponse;
use crate::plan::edit_plan;
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;

pub(super) fn api_plan(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let focus = body["goal"].as_str().unwrap_or("").trim().to_string();
    let mode = match body.get("mode") {
        None => PlanMode::Standard,
        Some(Value::String(mode)) if mode.is_empty() || mode == "standard" => PlanMode::Standard,
        Some(Value::String(mode)) if mode == "refactor" => PlanMode::Refactor {
            focus: focus.clone(),
        },
        _ => return (400, json!({"error": "unknown mode"})),
    };
    let goal = match &mode {
        PlanMode::Standard => focus,
        PlanMode::Refactor { focus } if focus.is_empty() => "Refactor the codebase".into(),
        PlanMode::Refactor { focus } => format!("Refactor the codebase — focus: {focus}"),
    };
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        (409, json!({"error": "busy"}))
    } else {
        ctx.session.stop_requested.store(false, Ordering::SeqCst);
        {
            let mut s = ctx.session.state.lock().unwrap();
            s.goal = goal.clone();
            s.phase = "planning".into();
            s.architect_activity = Value::Null;
            s.role_usage = Value::Null;
        }
        let short: String = goal.chars().take(300).collect();
        ctx.log_event("plan", &format!("planning started for goal: {short}"));
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.plan_worker(&goal, &mode));
        (200, json!({"ok": true}))
    }
}

pub(super) fn api_plan_revise(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let feedback = body["feedback"].as_str().unwrap_or("").trim().to_string();
    if feedback.is_empty() {
        return (400, json!({"error": "feedback required"}));
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    let Some(plan) = ctx.load_plan() else {
        ctx.session.busy.store(false, Ordering::SeqCst);
        return (400, json!({"error": "no plan"}));
    };
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    {
        let mut s = ctx.session.state.lock().unwrap();
        s.goal = plan["goal"].as_str().unwrap_or("").to_string();
        s.phase = "planning".into();
    }
    let short: String = feedback.chars().take(300).collect();
    ctx.log_event("plan", &format!("revision started: {short}"));
    let ctx2 = ctx.clone();
    std::thread::spawn(move || ctx2.revise_worker(&plan, &feedback));
    (200, json!({"ok": true}))
}

pub(super) fn api_plan_chat(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let question = body["question"].as_str().unwrap_or("").trim().to_string();
    if question.is_empty() {
        return (400, json!({"error": "question required"}));
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    let Some(plan) = ctx.load_plan() else {
        ctx.session.busy.store(false, Ordering::SeqCst);
        return (400, json!({"error": "no plan"}));
    };
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let short: String = question.chars().take(300).collect();
    ctx.log_event("chat", &short);
    let ctx2 = ctx.clone();
    std::thread::spawn(move || ctx2.chat_worker(&plan, &question));
    (200, json!({"ok": true}))
}

pub(super) fn api_goal_enhance(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let goal = body["goal"].as_str().unwrap_or("").trim().to_string();
    if goal.is_empty() {
        return (400, json!({"error": "goal required"}));
    }
    if goal.chars().count() > 20000 {
        return (400, json!({"error": "goal too long"}));
    }
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error": "busy"}));
    }
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let request_id = {
        let mut s = ctx.session.state.lock().unwrap();
        s.goal_enhancement_serial += 1;
        let request_id = s.goal_enhancement_serial;
        s.goal_enhancement = json!({"status":"running","request_id":request_id,"original":goal,"unix":unix_timestamp()});
        request_id
    };
    let short: String = goal.chars().take(300).collect();
    ctx.log_event("plan", &format!("enhancing goal description: {short}"));
    let ctx2 = ctx.clone();
    std::thread::spawn(move || ctx2.enhance_goal_worker(&goal, request_id));
    (200, json!({"ok": true, "request_id": request_id}))
}

pub(super) fn api_plan_edit(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let Some(plan) = ctx.load_plan() else {
        return (400, json!({"error": "no plan"}));
    };
    if ctx.acquire_busy().is_err() {
        return (409, json!({"error":"busy"}));
    }
    let _worker = crate::app::WorkerGuard(&ctx.session);
    drop(_queue_guard);
    match edit_plan(&plan, body) {
        Ok(edited) => {
            if let Err(error) = ctx.architect_publish(edited.clone(), Some(&plan), "manual edit") {
                return (400, json!({"error": error}));
            }
            {
                let mut s = ctx.session.state.lock().unwrap();
                s.phase = "plan_ready".into();
                s.goal = edited["goal"].as_str().unwrap_or("").to_string();
            }
            ctx.log_event("plan", "plan edited by user");
            (200, json!({"ok": true}))
        }
        Err(e) => (400, json!({"error": e})),
    }
}

pub(super) fn api_approve(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let Some(mut plan) = ctx.load_plan() else {
        return (400, json!({"error": "no plan"}));
    };
    if let Err(error) = crate::plan::check_queue_plan_order(&ctx.load_queue(), &plan) {
        return (409, json!({"error": error}));
    }
    if ctx.acquire_busy().is_err() {
        return (409, json!({"error":"busy"}));
    }
    let _approval_guard = crate::app::WorkerGuard(&ctx.session);
    drop(_queue_guard);
    plan = match ctx.architect_publish(plan.clone(), Some(&plan), "approval") {
        Ok(plan) => plan,
        Err(error) => return (400, json!({"error":error})),
    };
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let mut queue = ctx.load_queue();
    if let Err(error) = crate::plan::check_queue_plan_order(&queue, &plan) {
        return (409, json!({"error": error}));
    }
    let awaiting = crate::plan::queue_head(&queue)
        .filter(|item| item["status"] == "awaiting_approval")
        .and_then(|item| item["id"].as_u64());
    if let Some(id) = awaiting {
        ctx.session.stop_requested.store(false, Ordering::SeqCst);
        ctx.session.queue_active.store(true, Ordering::SeqCst);
        if let Err(error) = ctx.start_queue_run(&mut queue, id, &mut plan) {
            ctx.session.busy.store(false, Ordering::SeqCst);
            ctx.session.queue_active.store(false, Ordering::SeqCst);
            return (500, json!({"error": error}));
        }
        ctx.log_event("queue", &format!("goal {id}: approved by user"));
        std::mem::forget(_approval_guard);
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.queue_worker(Some(id)));
    } else {
        plan["status"] = json!("approved");
        if let Err(error) = ctx.save_plan(&plan) {
            return (500, json!({"error": error}));
        }
    }
    ctx.log_event("plan", "plan approved by user");
    (200, json!({"ok": true}))
}

pub(super) fn api_run(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let plan = ctx.load_plan();
    if let Some(plan) = &plan {
        if let Err(error) = crate::plan::check_queue_plan_order(&ctx.load_queue(), plan) {
            return (409, json!({"error": error}));
        }
    }
    let status = plan
        .as_ref()
        .and_then(|p| p["status"].as_str())
        .unwrap_or("");
    if status != "approved" && status != "done" {
        (400, json!({"error": "plan is not approved"}))
    } else if ctx.acquire_busy().is_err() {
        (409, json!({"error": "busy"}))
    } else {
        {
            let mut s = ctx.session.state.lock().unwrap();
            s.phase = "running".into();
            s.run_started_unix = unix_timestamp();
            if let Some(g) = plan.as_ref().and_then(|p| p["goal"].as_str()) {
                s.goal = g.to_string();
            }
        }
        ctx.session.stop_requested.store(false, Ordering::SeqCst);
        ctx.log_event("run", "run started");
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.run_worker());
        (200, json!({"ok": true}))
    }
}

pub(super) fn api_stop(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    ctx.session.stop_requested.store(true, Ordering::SeqCst);
    ctx.session.queue_active.store(false, Ordering::SeqCst);
    let mut queue = ctx.load_queue();
    let awaiting: Vec<u64> = queue["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["status"] == "awaiting_approval")
        .filter_map(|item| item["id"].as_u64())
        .collect();
    for id in awaiting {
        ctx.set_queue_status(&mut queue, id, "blocked");
    }
    ctx.log_event("queue", "queue stopped by user");
    ctx.log_event(
        "run",
        "stop requested; stopping current agent and retaining saved progress",
    );
    (200, json!({"ok": true}))
}

pub(super) fn api_reset_plan(ctx: &Ctx) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        (409, json!({"error": "busy"}))
    } else {
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        if let Err(error) = ctx.architecture_store().reset() {
            return (500, json!({"error": error}));
        }
        let _ = fs::remove_file(ctx.forge_path("chat.jsonl"));
        ctx.set_phase("idle");
        {
            let mut state = ctx.session.state.lock().unwrap();
            state.architect_activity = Value::Null;
            state.role_usage = Value::Null;
        }
        ctx.log_event("plan", "plan discarded");
        (200, json!({"ok": true}))
    }
}
