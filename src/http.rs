use crate::app::{App, Ctx, PlanMode, WorkerGuard};
use crate::plan::{edit_plan, mutate_queue};
use crate::util::{last_chars, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) const PORT: u16 = 8734;

// ---------------------------------------------------------------- http

fn respond(req: tiny_http::Request, code: u32, body: Value) {
    let data = body.to_string();
    let header = tiny_http::Header::from_bytes(
        &b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let resp = tiny_http::Response::from_string(data)
        .with_status_code(code)
        .with_header(header);
    let _ = req.respond(resp);
}

fn query_value(query: &str, key: &str) -> Result<Option<String>, &'static str> {
    let Some(value) = query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    }) else {
        return Ok(None);
    };
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes.next().and_then(|b| (b as char).to_digit(16))
                    .ok_or("invalid project query encoding")?;
                let low = bytes.next().and_then(|b| (b as char).to_digit(16))
                    .ok_or("invalid project query encoding")?;
                (high * 16 + low) as u8
            }
            byte => byte,
        });
    }
    String::from_utf8(decoded).map(Some).map_err(|_| "invalid project query encoding")
}

pub(crate) fn handle(app: &Arc<App>, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let method = req.method().clone();
    let mut body_text = String::new();
    let _ = req.as_reader().read_to_string(&mut body_text);
    let body: Value = serde_json::from_str(&body_text).unwrap_or(json!({}));
    let project_endpoint = matches!(path, "/api/state" | "/api/architecture/history" | "/api/architecture/reviews" | "/api/agent_log" | "/api/diff"
        | "/api/plan" | "/api/plan/edit" | "/api/plan/revise" | "/api/plan/chat" | "/api/approve" | "/api/run" | "/api/stop" | "/api/reset_plan")
        || path.starts_with("/api/queue/");
    let target = if !project_endpoint {
        Ok(None)
    } else if method == tiny_http::Method::Get {
        query_value(query, "project")
    } else {
        match body.get("project") {
            None => Ok(None),
            Some(Value::String(project)) => Ok(Some(project.clone())),
            Some(_) => Err("project must be a string"),
        }
    };
    let target = match target {
        Ok(target) => target,
        Err(error) => {
            respond(req, 400, json!({"error": error}));
            return;
        }
    };
    if let Some(project) = &target
        && !PathBuf::from(project).join(".git").exists()
    {
        respond(req, 400, json!({"error": format!("{project} is not a git repository")}));
        return;
    }
    let active_project = app.active_project.lock().unwrap().clone();
    let ctx = app.context(if project_endpoint { target.as_deref().unwrap_or(&active_project) }
        else { &active_project });

    let (code, response) = match (method, path) {
        (tiny_http::Method::Get, "/api/architecture/reviews") => api_architecture_reviews(&ctx, query),
        (tiny_http::Method::Get, "/api/architecture/history") => api_architecture_history(&ctx, query),
        (tiny_http::Method::Get, "/api/models") => api_models(app, query),
        (tiny_http::Method::Post, "/api/models/refresh") =>
            (202, json!({"ok":true,"started":app.refresh_catalogue()})),
        (tiny_http::Method::Post, "/api/models/cancel") => {
            app.catalogue.cancel(); (202, json!({"ok":true}))
        },
        (tiny_http::Method::Get, "/api/state") => api_state(app, &ctx, &active_project),
        (tiny_http::Method::Get, "/api/agent_log") => api_agent_log(&ctx, query),
        (tiny_http::Method::Get, "/api/diff") => api_diff(&ctx),
        (tiny_http::Method::Get, "/api/projects") => api_projects(app),
        (tiny_http::Method::Post, "/api/settings") => api_settings(app, &body),
        (tiny_http::Method::Post, "/api/project") => api_project(app, &body),
        (tiny_http::Method::Post, "/api/project/select") => api_project_select(app, &body),
        (tiny_http::Method::Post, "/api/queue/add" | "/api/queue/remove"
            | "/api/queue/move" | "/api/queue/clear") => api_queue_mutate(&ctx, path, &body),
        (tiny_http::Method::Post, "/api/queue/start") => api_queue_start(&ctx),
        (tiny_http::Method::Post, "/api/plan") => api_plan(&ctx, &body),
        (tiny_http::Method::Post, "/api/plan/revise") => api_plan_revise(&ctx, &body),
        (tiny_http::Method::Post, "/api/plan/chat") => api_plan_chat(&ctx, &body),
        (tiny_http::Method::Post, "/api/plan/edit") => api_plan_edit(&ctx, &body),
        (tiny_http::Method::Post, "/api/approve") => api_approve(&ctx),
        (tiny_http::Method::Post, "/api/run") => api_run(&ctx),
        (tiny_http::Method::Post, "/api/stop") => api_stop(&ctx),
        (tiny_http::Method::Post, "/api/reset_plan") => api_reset_plan(&ctx),
        (tiny_http::Method::Post, "/api/self_update") => api_self_update(app, &ctx),
        _ => (404, json!({"error": "not found"})),
    };
    respond(req, code, response);
}

fn api_state(app: &Arc<App>, ctx: &Ctx, active_project: &str) -> (u32, Value) {
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
    if let Ok(policy) = crate::catalogue::Policy::from_settings(&snap["settings"]) {
        snap["model_catalogue"] = app.catalogue.summary(&policy);
        snap["model_catalogue"]["policy_error"] = json!(*app.model_policy_error.lock().unwrap());
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
            format!("{}:{}:{}:{}:{}:{}", m.ino(), m.len(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec())
        });
        let mut cache = ctx.session.legacy_state_cache.lock().unwrap();
        let cached = cache.as_ref().filter(|(key, _)| Some(key) == stamp.as_ref()).map(|(_, p)| p.clone());
        let loaded = match cached { Some(p) => Ok(Some(p)), None => store.load_raw() };
        match loaded {
            Ok(plan) => {
                snap["architecture"] = store.summary(plan.as_ref()).unwrap_or(Value::Null);
                let bounded = plan.map(|p| store.state_plan(p));
                if let Some(p) = bounded.as_ref().filter(|p| p.get("architecture").is_none()) {
                    if let Some(stamp) = stamp { *cache = Some((stamp, p.clone())); }
                } else { *cache = None; }
                snap["plan"] = bounded.unwrap_or(Value::Null);
            }
            Err(error) => {
                snap["plan"] = Value::Null;
                snap["architecture"] = json!({"context_status": "error", "error": error});
            }
        }
        snap["persistence_error"] = json!(*ctx.session.persistence_error.lock().unwrap());
    }
    snap["queue"] = ctx.load_queue()["items"].clone();
    snap["queue_active"] = json!(ctx.session.queue_active.load(Ordering::SeqCst));
    drop(_queue_guard);
    snap["active_project"] = json!(active_project);
    snap["sessions"] = app.session_summaries(active_project);
    snap["history"] = ctx.read_history();
    snap["chat"] = ctx.read_chat();
    snap["reports"] = ctx.read_reports();
    snap["git_log"] = json!(ctx.git(&["log", "--oneline", "-12"]).unwrap_or_default());
    (200, snap)
}

fn api_agent_log(ctx: &Ctx, query: &str) -> (u32, Value) {
    let data = fs::read(ctx.forge_path("agent.log")).unwrap_or_default();
    let size = data.len();
    let offset = query.split('&').find_map(|part| {
        part.strip_prefix("offset=")?.parse::<usize>().ok()
    });
    let log = match offset {
        Some(offset) if offset <= size => {
            String::from_utf8_lossy(&data[offset..]).into_owned()
        }
        Some(_) | None => last_chars(&String::from_utf8_lossy(&data), 30_000),
    };
    (200, json!({"log": log, "size": size}))
}

fn api_diff(ctx: &Ctx) -> (u32, Value) {
    let diff = ctx.git(&["diff", "HEAD"]).unwrap_or_default();
    let tail: String = diff.chars().rev().take(40000).collect::<Vec<_>>()
        .into_iter().rev().collect();
    (200, json!({"diff": tail}))
}

fn api_projects(app: &App) -> (u32, Value) {
    let projects_root = app.setting("projects_root");
    let (entries, local_error) = match fs::read_dir(&projects_root) {
        Ok(entries) => (Some(entries), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let mut local: Vec<Value> = entries
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let git = path.join(".git");
            if !path.is_dir() || (!git.is_dir() && !git.is_file()) {
                return None;
            }
            Some(json!({
                "name": entry.file_name().to_string_lossy(),
                "path": path.display().to_string(),
            }))
        })
        .collect();
    local.sort_by(|a, b| {
        let a = a["name"].as_str().unwrap_or("");
        let b = b["name"].as_str().unwrap_or("");
        a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b))
    });
    let local_names: Vec<String> = local
        .iter()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect();
    let (remote, remote_error) = app.remote_repos(&local_names);
    let mut resp = json!({
        "projects_root": projects_root,
        "local": local,
        "remote": remote,
    });
    if let Some(e) = remote_error {
        resp["remote_error"] = json!(e);
    }
    if let Some(e) = local_error {
        resp["error"] = json!(e);
    }
    (200, resp)
}

fn api_models(app: &App, query: &str) -> (u32, Value) {
    let policy = match crate::catalogue::Policy::from_settings(&app.settings.lock().unwrap()) {
        Ok(p) => p, Err(e) => return (400, json!({"error":e})),
    };
    let result = (|| -> Result<Value, String> {
        let model = query_value(query, "model")?;
        if let Some(model) = model {
            let p = query_value(query, "provider")?.ok_or("provider required")?;
            let provider = crate::catalogue::Provider::parse(&p).ok_or("invalid provider")?;
            let effort = query_value(query, "effort")?.unwrap_or_else(|| "provider_default".into());
            Ok(app.catalogue.select(&policy, provider, &model, &effort))
        } else {
            let mut details = app.catalogue.details(&policy);
            details["policy"] = json!(policy);
            Ok(details)
        }
    })();
    match result { Ok(v) => (200,v), Err(e) => (400,json!({"error":e})) }
}

fn api_settings(app: &App, body: &Value) -> (u32, Value) {
    let Some(obj) = body.as_object() else { return (400,json!({"error":"settings must be an object"})); };
    let mut settings = app.settings.lock().unwrap();
    let mut candidate = settings.clone();
    for (k,v) in obj { if candidate.get(k).is_some() { candidate[k] = v.clone(); } }
    let policy = match crate::catalogue::Policy::from_settings(&candidate) {
        Ok(p) => p, Err(e) => return (400,json!({"error":e})),
    };
    if candidate["model_catalogue"] != settings["model_catalogue"] && app.catalogue.running() {
        return (409,json!({"error":"cancel or finish the catalogue refresh before changing its policy"}));
    }
    if candidate["model_catalogue"] != settings["model_catalogue"]
        && candidate["model_catalogue"]["policy_revision"] == settings["model_catalogue"]["policy_revision"] {
        return (400,json!({"error":"increment policy_revision when changing the model catalogue policy"}));
    }
    // Validate effort overrides against known provider evidence before accepting policy.
    if candidate["model_catalogue"] != settings["model_catalogue"] {
        for e in &policy.entries {
            if e.effort != "provider_default" {
                let option = app.catalogue.select(&policy, e.provider, &e.model, &e.effort);
                if option["eligible"] != true { return (400,json!({"error":option["error"]})); }
            }
        }
    }
    let refresh = candidate["model_catalogue"]["codex_scope"] != settings["model_catalogue"]["codex_scope"]
        || candidate["model_catalogue"]["claude_scope"] != settings["model_catalogue"]["claude_scope"]
        || candidate["model_catalogue"]["claude_bridge"] != settings["model_catalogue"]["claude_bridge"];
    if candidate["model_catalogue"] != settings["model_catalogue"] {
        if let Some(path) = &app.model_policy_path {
            if let Err(error) = crate::catalogue::save_policy(path, &policy) {
                *app.model_policy_error.lock().unwrap() = Some(error.clone());
                return (500,json!({"error":error}));
            }
        }
        *app.model_policy_error.lock().unwrap() = None;
    }
    *settings = candidate;
    drop(settings);
    if refresh { app.catalogue.refresh(policy); }
    (200, json!({"ok": true}))
}

fn api_set_project(app: &Arc<App>, path: &str) -> (u32, Value) {
    match app.set_project(path) {
        Ok(()) => (200, json!({"ok": true})),
        Err(e) => (400, json!({"error": e})),
    }
}

fn api_project(app: &Arc<App>, body: &Value) -> (u32, Value) {
    let path = body["path"].as_str().unwrap_or("").trim().to_string();
    api_set_project(app, &path)
}

fn api_project_select(app: &Arc<App>, body: &Value) -> (u32, Value) {
    let path = body["path"].as_str().unwrap_or("").trim().to_string();
    let repo = body["repo"].as_str().unwrap_or("").trim().to_string();
    if !path.is_empty() {
        api_set_project(app, &path)
    } else if !repo.is_empty() {
        let parts: Vec<&str> = repo.split('/').collect();
        let valid = parts.len() == 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && *p != "."
                    && *p != ".."
                    && p.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
                    })
            });
        if !valid {
            return (400, json!({"error": format!("invalid repo name: {repo}")}));
        }
        let name = parts[1].to_string();
        let target = PathBuf::from(app.setting("projects_root")).join(&name);
        let ctx = app.context(&target.display().to_string());
        if target.join(".git").exists() {
            api_set_project(app, &target.display().to_string())
        } else if ctx.acquire_busy().is_err() {
            (409, json!({"error": "busy"}))
        } else {
            ctx.set_step(None, &format!("cloning {repo}"));
            println!("cloning {repo} into {}", target.display());
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _worker = WorkerGuard(&ctx2.session);
                let out = Command::new("gh")
                    .args(["repo", "clone", &repo])
                    .arg(&target)
                    .output();
                match out {
                    Ok(o) if o.status.success() => {
                        match ctx2.app.set_project(&target.display().to_string()) {
                            Ok(()) => ctx2.log_event("git",
                                &format!("clone finished: {repo} -> {}", target.display())),
                            Err(e) => ctx2.log_event("error",
                                &format!("clone finished but selection failed: {e}")),
                        }
                    }
                    Ok(o) => eprintln!("clone of {repo} failed: {}",
                        String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("failed to launch gh clone: {e}"),
                }
            });
            (200, json!({"ok": true, "cloning": true}))
        }
    } else {
        (400, json!({"error": "path or repo required"}))
    }
}

fn api_queue_mutate(ctx: &Ctx, path: &str, body: &Value) -> (u32, Value) {
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

fn api_queue_start(ctx: &Ctx) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let queue = ctx.load_queue();
    if !queue["items"].as_array().unwrap().iter()
        .any(|item| item["status"] == "queued")
    {
        (400, json!({"error": "no queued goals"}))
    } else if ctx.acquire_busy().is_err() {
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

fn api_plan(ctx: &Ctx, body: &Value) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let focus = body["goal"].as_str().unwrap_or("").trim().to_string();
    let mode = match body.get("mode") {
        None => PlanMode::Standard,
        Some(Value::String(mode)) if mode.is_empty() || mode == "standard" => PlanMode::Standard,
        Some(Value::String(mode)) if mode == "refactor" => PlanMode::Refactor { focus: focus.clone() },
        _ => {
            return (400, json!({"error": "unknown mode"}));
        }
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
        }
        let short: String = goal.chars().take(300).collect();
        ctx.log_event("plan", &format!("planning started for goal: {short}"));
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.plan_worker(&goal, &mode));
        (200, json!({"ok": true}))
    }
}

fn api_plan_revise(ctx: &Ctx, body: &Value) -> (u32, Value) {
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

fn api_plan_chat(ctx: &Ctx, body: &Value) -> (u32, Value) {
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

fn api_plan_edit(ctx: &Ctx, body: &Value) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let Some(plan) = ctx.load_plan() else {
        return (400, json!({"error": "no plan"}));
    };
    match edit_plan(&plan, body) {
        Ok(edited) => {
            if let Err(error) = ctx.save_plan(&edited) { return (500, json!({"error": error})); }
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

fn api_approve(ctx: &Ctx) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let Some(mut plan) = ctx.load_plan() else {
        return (400, json!({"error": "no plan"}));
    };
    let mut queue = ctx.load_queue();
    let awaiting = queue["items"].as_array().unwrap().iter()
        .find(|item| !matches!(item["status"].as_str(), Some("done" | "failed" | "blocked")))
        .filter(|item| item["status"] == "awaiting_approval")
        .and_then(|item| item["id"].as_u64());
    if let Some(id) = awaiting {
        if ctx.acquire_busy().is_err() {
            return (409, json!({"error": "busy"}));
        }
        ctx.session.stop_requested.store(false, Ordering::SeqCst);
        ctx.session.queue_active.store(true, Ordering::SeqCst);
        if let Err(error) = ctx.start_queue_run(&mut queue, id, &mut plan) {
            ctx.session.busy.store(false, Ordering::SeqCst);
            ctx.session.queue_active.store(false, Ordering::SeqCst);
            return (500, json!({"error": error}));
        }
        ctx.log_event("queue", &format!("goal {id}: approved by user"));
        let ctx2 = ctx.clone();
        std::thread::spawn(move || ctx2.queue_worker(Some(id)));
    } else {
        plan["status"] = json!("approved");
        if let Err(error) = ctx.save_plan(&plan) { return (500, json!({"error": error})); }
    }
    ctx.log_event("plan", "plan approved by user");
    (200, json!({"ok": true}))
}

fn api_run(ctx: &Ctx) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.queue_active.load(Ordering::SeqCst) {
        return (409, json!({"error": "busy"}));
    }
    let plan = ctx.load_plan();
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

fn api_stop(ctx: &Ctx) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    ctx.session.stop_requested.store(true, Ordering::SeqCst);
    ctx.session.queue_active.store(false, Ordering::SeqCst);
    // No worker remains to transition a plan paused for approval.
    let mut queue = ctx.load_queue();
    let awaiting: Vec<u64> = queue["items"].as_array().unwrap().iter()
        .filter(|item| item["status"] == "awaiting_approval")
        .filter_map(|item| item["id"].as_u64())
        .collect();
    for id in awaiting {
        ctx.set_queue_status(&mut queue, id, "blocked");
    }
    ctx.log_event("queue", "queue stopped by user");
    ctx.log_event("run", "stop requested; finishing current agent session");
    (200, json!({"ok": true}))
}

fn api_reset_plan(ctx: &Ctx) -> (u32, Value) {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
        (409, json!({"error": "busy"}))
    } else {
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        if let Err(error) = ctx.architecture_store().reset() { return (500, json!({"error": error})); }
        let _ = fs::remove_file(ctx.forge_path("chat.jsonl"));
        ctx.set_phase("idle");
        ctx.log_event("plan", "plan discarded");
        (200, json!({"ok": true}))
    }
}

fn api_self_update(app: &App, ctx: &Ctx) -> (u32, Value) {
    if app.any_busy() {
        return (409, json!({"error": "busy"}));
    }
    let repo = std::env::var("FORGE_REPO").unwrap_or_else(|_| {
        format!("{}/Projects/Forge", std::env::var("HOME").unwrap_or_default())
    });
    let script = PathBuf::from(&repo).join("install.sh");
    if !script.is_file() {
        return (400, json!({"error": format!("no install.sh in {repo}")}));
    }
    // install.sh restarts this service, so a plain child process would be
    // killed with us mid-update; a transient unit detaches it. The fixed
    // unit name also rejects a second update while one is running.
    let out = Command::new("systemd-run")
        .args(["--user", "--collect", "--unit", "forge-update", "bash"])
        .arg(&script)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            // The confirmation event has to outlive the engine restart
            // install.sh performs, so leave a marker that the next boot
            // turns into a "self-update finished" feed event.
            ctx.ensure_forge_dir();
            let _ = fs::write(ctx.forge_path("update-pending"),
                unix_timestamp().to_string());
            ctx.log_event("update",
                "self-update started; the engine restarts and the shell reloads the plugin \
                 if it changed (log: journalctl --user -u forge-update)");
            (200, json!({"ok": true}))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            (500, json!({"error": format!("systemd-run failed: {err}")}))
        }
        Err(e) => {
            (500, json!({"error": format!("systemd-run failed: {e}")}))
        }
    }
}

fn api_architecture_history(ctx: &Ctx, query: &str) -> (u32, Value) {
    let result = (|| {
        let id = query_value(query, "plan_id")?;
        let cursor = query_value(query, "cursor")?.unwrap_or_else(|| "0".into())
            .parse::<u64>().map_err(|_| "invalid cursor")?;
        let limit = query_value(query, "limit")?.unwrap_or_else(|| "20".into())
            .parse::<usize>().map_err(|_| "invalid limit")?;
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        ctx.architecture_store().history(id.as_deref(), cursor, limit)
    })();
    match result { Ok(page) => (200, page), Err(error) => (400, json!({"error": error})) }
}

fn api_architecture_reviews(ctx: &Ctx, query: &str) -> (u32, Value) {
    let result = (|| {
        let id = query_value(query, "plan_id")?;
        let checkpoint = query_value(query, "checkpoint")?;
        let stage = query_value(query, "stage_id")?.ok_or("stage_id required")?
            .parse::<i64>().map_err(|_| "invalid stage_id")?;
        let cursor = query_value(query, "cursor")?.unwrap_or_else(|| "0".into())
            .parse::<u64>().map_err(|_| "invalid cursor")?;
        let limit = query_value(query, "limit")?.unwrap_or_else(|| "20".into())
            .parse::<usize>().map_err(|_| "invalid limit")?;
        let _guard = ctx.session.persistence_lock.lock().unwrap();
        ctx.architecture_store().reviews(id.as_deref(), stage, checkpoint.as_deref(), cursor, limit)
    })();
    match result { Ok(page) => (200, page), Err(error) => (400, json!({"error": error})) }
}
