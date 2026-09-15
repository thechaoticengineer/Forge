use crate::app::{App, Ctx};
use crate::http::{ApiResponse, query_value};
use crate::util::last_chars;
use serde_json::{Value, json};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(super) fn api_state(app: &Arc<App>, ctx: &Ctx, active_project: &str) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let snap = {
        let s = ctx.session.state.lock().unwrap();
        json!({
            "project": ctx.project,
            "phase": s.phase,
            "goal": s.goal,
            "current_stage": s.current_stage,
            "current_step": s.current_step,
            "run_started_unix": s.run_started_unix,
            "model_selection": s.model_selection,
            "goal_enhancement": s.goal_enhancement,
            "discussion_activity": s.discussion_activity,
            "model_policy_suggestion": {
                "status":s.model_policy_suggestion["status"],
                "request_id":s.model_policy_suggestion["request_id"],
                "error":s.model_policy_suggestion["error"],
            },
            "architect_activity":s.architect_activity,
            "role_usage":s.role_usage,
            "agent": {
                "role": s.agent_role,
                "tool": s.agent_tool,
                "model": s.agent_model,
                "started_unix": s.agent_started_unix,
                "lines": s.agent_lines,
                "last_line": s.agent_last_line,
            },
        })
    };
    let mut snap = snap;
    snap["settings"] = app.settings.lock().unwrap().clone();
    snap["claude_quota"] = app.quota.snapshot();
    if let Ok(policy) = crate::catalogue::Policy::from_settings(&snap["settings"]) {
        snap["model_catalogue"] = app.catalogue.summary(&policy);
        snap["model_catalogue"]["policy_error"] = json!(*app.model_policy_error.lock().unwrap());
        snap["model_catalogue"]["metadata"] = app.metadata.summary();
        // Full registry options belong to the read-only details endpoint.
        snap["settings"]["model_catalogue"]["entries"] = json!([]);
    }
    snap["busy"] = json!(ctx.session.busy.load(Ordering::SeqCst));
    {
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        let store = ctx.architecture_store();
        // Legacy files remain untouched on reads. Cache only their bounded view;
        // metadata changes force revalidation. Indexed publications need no cache.
        let stamp = fs::metadata(ctx.forge_path("plan.json")).ok().map(|m| {
            use std::os::unix::fs::MetadataExt;
            format!(
                "{}:{}:{}:{}:{}:{}",
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec()
            )
        });
        let mut cache = ctx.session.legacy_state_cache.lock().unwrap();
        let cached = cache
            .as_ref()
            .filter(|(key, _)| Some(key) == stamp.as_ref())
            .map(|(_, p)| p.clone());
        let was_cached = cached.is_some();
        let loaded = match cached {
            Some(p) => Ok(Some(p)),
            None => store.load_state(),
        };
        match loaded {
            Ok(plan) => {
                snap["architecture"] = store.state_summary(plan.as_ref()).unwrap_or(Value::Null);
                // Cached data is already a projection; hashing its shortened
                // reviews again would manufacture a different snapshot identity.
                let bounded = plan.map(|p| if was_cached { p } else { store.state_plan(p) });
                if let Some(p) = bounded.as_ref().filter(|p| p.get("architecture").is_none()) {
                    if let Some(stamp) = stamp {
                        *cache = Some((stamp, p.clone()));
                    }
                } else {
                    *cache = None;
                }
                snap["plan"] = bounded.unwrap_or(Value::Null);
                // Session counters are transient; plan-owned role totals survive restart.
                snap["session_role_usage"] = snap["role_usage"].clone();
                if snap["plan"]["role_usage"].is_object() {
                    snap["role_usage"] = snap["plan"]["role_usage"].clone();
                }
            }
            Err(error) => {
                snap["plan"] = Value::Null;
                snap["architecture"] = json!({"context_status": "error", "error": error});
            }
        }
        snap["persistence_error"] = json!(*ctx.session.persistence_error.lock().unwrap());
    }
    // Older engines left failed stages in_progress. Correct their idle display
    // without rewriting the persisted plan, review history or attempt budget.
    if snap["busy"] == false {
        for stage in snap
            .get_mut("plan")
            .and_then(|p| p.get_mut("stages"))
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if stage["status"] == "in_progress"
                && matches!(
                    stage["review_gate"]["status"].as_str(),
                    Some("error" | "blocked" | "exhausted" | "invalidated")
                )
            {
                stage["status"] = json!("blocked");
            }
        }
    }
    snap["queue"] = ctx.load_queue()["items"].clone();
    snap["queue_active"] = json!(ctx.session.queue_active.load(Ordering::SeqCst));
    drop(_queue_guard);
    snap["active_project"] = json!(active_project);
    snap["sessions"] = app.session_summaries(active_project);
    snap["history"] = ctx.read_history();
    snap["chat"] = ctx.read_chat();
    snap["discussion"] = ctx.read_discussion();
    snap["reports"] = ctx.read_reports();
    snap["git_log"] = json!(ctx.git(&["log", "--oneline", "-12"]).unwrap_or_default());
    (200, snap)
}

pub(super) fn api_agent_records(ctx: &Ctx, query: &str) -> ApiResponse {
    let parameters = (|| {
        let session = query_value(query, "session")?;
        let archive = query_value(query, "archive")?;
        let cursor = query_value(query, "cursor")?
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|_| "invalid agent log cursor")?
            .unwrap_or(0);
        let limit = query_value(query, "limit")?
            .map(|s| s.parse::<usize>())
            .transpose()
            .map_err(|_| "invalid agent log limit")?
            .unwrap_or(50);
        let history = query_value(query, "history")?.is_some_and(|s| s == "true");
        Ok::<_, &str>((session, archive, cursor, limit, history))
    })();
    let (session, archive, cursor, limit, history) = match parameters {
        Ok(values) => values,
        Err(error) => return (400, json!({"error":error})),
    };
    match crate::agent_log::page(
        &ctx.forge_path(""),
        ctx.project(),
        session.as_deref(),
        archive.as_deref(),
        cursor,
        limit,
        history,
    ) {
        Ok(page) => (200, page),
        Err(error) => (
            if error.starts_with("invalid agent log cursor")
                || error.starts_with("agent log cursor")
                || error == "invalid agent log archive"
            {
                400
            } else {
                500
            },
            json!({"error":error}),
        ),
    }
}

pub(super) fn api_agent_log(ctx: &Ctx, query: &str) -> ApiResponse {
    let data = fs::read(ctx.forge_path("agent.log")).unwrap_or_default();
    let size = data.len();
    let offset = query
        .split('&')
        .find_map(|part| part.strip_prefix("offset=")?.parse::<usize>().ok());
    let log = match offset {
        Some(offset) if offset <= size => String::from_utf8_lossy(&data[offset..]).into_owned(),
        Some(_) | None => last_chars(&String::from_utf8_lossy(&data), 30_000),
    };
    (200, json!({"log": log, "size": size}))
}

pub(super) fn api_diff(ctx: &Ctx) -> ApiResponse {
    let diff = ctx.git(&["diff", "HEAD"]).unwrap_or_default();
    let tail: String = diff
        .chars()
        .rev()
        .take(40000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (200, json!({"diff": tail}))
}

pub(super) fn api_architecture_history(ctx: &Ctx, query: &str) -> ApiResponse {
    let result = (|| {
        let id = query_value(query, "plan_id")?;
        let cursor = query_value(query, "cursor")?
            .unwrap_or_else(|| "0".into())
            .parse::<u64>()
            .map_err(|_| "invalid cursor")?;
        let limit = query_value(query, "limit")?
            .unwrap_or_else(|| "20".into())
            .parse::<usize>()
            .map_err(|_| "invalid limit")?;
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        ctx.architecture_store()
            .history(id.as_deref(), cursor, limit)
    })();
    match result {
        Ok(page) => (200, page),
        Err(error) => (400, json!({"error": error})),
    }
}

pub(super) fn api_architecture_reviews(ctx: &Ctx, query: &str) -> ApiResponse {
    let result = (|| {
        let id = query_value(query, "plan_id")?;
        let checkpoint = query_value(query, "checkpoint")?;
        let stage = query_value(query, "stage_id")?
            .ok_or("stage_id required")?
            .parse::<i64>()
            .map_err(|_| "invalid stage_id")?;
        let cursor = query_value(query, "cursor")?
            .unwrap_or_else(|| "0".into())
            .parse::<u64>()
            .map_err(|_| "invalid cursor")?;
        let limit = query_value(query, "limit")?
            .unwrap_or_else(|| "20".into())
            .parse::<usize>()
            .map_err(|_| "invalid limit")?;
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        let mut page = ctx.architecture_store().reviews(
            id.as_deref(),
            stage,
            checkpoint.as_deref(),
            cursor,
            limit,
        )?;
        page["project"] = json!(ctx.project());
        Ok::<Value, String>(page)
    })();
    match result {
        Ok(page) => (200, page),
        Err(error) => (400, json!({"error": error})),
    }
}
