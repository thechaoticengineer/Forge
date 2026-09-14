use crate::app::{App, Ctx, WorkerGuard};
use crate::http::{ApiResponse, query_value};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(super) fn api_projects(app: &App) -> ApiResponse {
    let projects_root = app.setting("projects_root");
    let (entries, local_error) = match fs::read_dir(&projects_root) {
        Ok(entries) => (Some(entries), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let mut local: Vec<Value> = entries.into_iter().flatten().filter_map(Result::ok).filter_map(|entry| {
        let path = entry.path(); let git = path.join(".git");
        if !path.is_dir() || (!git.is_dir() && !git.is_file()) { return None; }
        Some(json!({"name": entry.file_name().to_string_lossy(), "path": path.display().to_string()}))
    }).collect();
    local.sort_by(|a, b| {
        let a = a["name"].as_str().unwrap_or("");
        let b = b["name"].as_str().unwrap_or("");
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    let local_names: Vec<String> = local
        .iter()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect();
    let (remote, remote_error) = app.remote_repos(&local_names);
    let mut resp = json!({"projects_root": projects_root, "local": local, "remote": remote});
    if let Some(e) = remote_error {
        resp["remote_error"] = json!(e);
    }
    if let Some(e) = local_error {
        resp["error"] = json!(e);
    }
    (200, resp)
}

pub(super) fn api_models(app: &App, query: &str) -> ApiResponse {
    let policy = match crate::catalogue::Policy::from_settings(&app.settings.lock().unwrap()) {
        Ok(p) => p,
        Err(e) => return (400, json!({"error":e})),
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
            details["metadata"] = app
                .metadata
                .details(&app.catalogue.metadata_snapshot(&policy));
            Ok(details)
        }
    })();
    match result {
        Ok(v) => (200, v),
        Err(e) => (400, json!({"error":e})),
    }
}

pub(super) fn api_model_suggest(ctx: &Ctx, body: &Value) -> ApiResponse {
    let _queue_guard = ctx.session.queue_lock.lock().unwrap();
    let base =
        match crate::catalogue::Policy::from_settings(&json!({"model_catalogue":body["policy"]})) {
            Ok(policy) => policy,
            Err(error) => return (400, json!({"error":error})),
        };
    let mut details = ctx.app.catalogue.details(&base);
    if details["refreshing"] == true {
        return (
            409,
            json!({"error":"Wait for model discovery to finish, then ask AI to assign tiers."}),
        );
    }
    details["metadata"] = ctx
        .app
        .metadata
        .details(&ctx.app.catalogue.metadata_snapshot(&base));
    let entries = match crate::model_policy_ai::candidates(&base, &details) {
        Ok(entries) => entries,
        Err(error) => return (400, json!({"error":error})),
    };
    if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
        return (409, json!({"error":"busy"}));
    }
    ctx.session.stop_requested.store(false, Ordering::SeqCst);
    let request_id = {
        let mut state = ctx.session.state.lock().unwrap();
        state.model_policy_suggestion_serial += 1;
        let id = state.model_policy_suggestion_serial;
        state.model_policy_suggestion = json!({"status":"running","request_id":id});
        id
    };
    let worker = ctx.clone();
    std::thread::spawn(move || worker.model_policy_worker(base, details, entries, request_id));
    (202, json!({"ok":true,"request_id":request_id}))
}

pub(super) fn api_model_suggestion(ctx: &Ctx) -> ApiResponse {
    (
        200,
        ctx.session
            .state
            .lock()
            .unwrap()
            .model_policy_suggestion
            .clone(),
    )
}
pub(super) fn api_quota_refresh(app: &App) -> ApiResponse {
    (202, json!({"ok":true,"started":app.refresh_quota(true)}))
}
pub(super) fn api_models_refresh(app: &App) -> ApiResponse {
    (202, json!({"ok":true,"started":app.refresh_catalogue()}))
}
pub(super) fn api_models_cancel(app: &App) -> ApiResponse {
    app.catalogue.cancel();
    (202, json!({"ok":true}))
}
pub(super) fn api_metadata_refresh(app: &App) -> ApiResponse {
    (202, json!({"ok":true,"started":app.refresh_metadata()}))
}

pub(super) fn api_settings(app: &App, body: &Value) -> ApiResponse {
    let Some(obj) = body.as_object() else {
        return (400, json!({"error":"settings must be an object"}));
    };
    let mut settings = app.settings.lock().unwrap();
    if let Some(expected) = body.get("expected_model_policy") {
        let current = crate::catalogue::Policy::from_settings(&settings).map(|p| json!(p));
        if current.as_ref().ok() != Some(expected) {
            return (
                409,
                json!({"error":"Model policy changed while this editor was open. Use Reload saved policy before saving."}),
            );
        }
    }
    let mut candidate = settings.clone();
    for (k, v) in obj {
        if candidate.get(k).is_some() {
            candidate[k] = v.clone();
        }
    }
    if obj.contains_key("reviewer") && !obj.contains_key("reviewer_provider_mode") {
        candidate["reviewer_provider_mode"] = json!("configured");
    }
    if let Err(error) = crate::engine_settings::validate(&candidate) {
        return (400, json!({"error":error}));
    }
    let policy = match crate::catalogue::Policy::from_settings(&candidate) {
        Ok(p) => p,
        Err(e) => return (400, json!({"error":e})),
    };
    if candidate["model_catalogue"] != settings["model_catalogue"] && app.catalogue.running() {
        return (
            409,
            json!({"error":"cancel or finish the catalogue refresh before changing its policy"}),
        );
    }
    if candidate["model_catalogue"] != settings["model_catalogue"]
        && candidate["model_catalogue"]["policy_revision"]
            == settings["model_catalogue"]["policy_revision"]
    {
        return (
            400,
            json!({"error":"increment policy_revision when changing the model catalogue policy"}),
        );
    }
    // Validate effort overrides against known provider evidence before accepting policy.
    if candidate["model_catalogue"] != settings["model_catalogue"] {
        for e in &policy.entries {
            if e.execution_effort() != "provider_default" {
                let option =
                    app.catalogue
                        .select(&policy, e.provider, &e.model, e.execution_effort());
                if option["eligible"] != true {
                    return (400, json!({"error":option["error"]}));
                }
            }
        }
    }
    let refresh = candidate["model_catalogue"]["codex_scope"]
        != settings["model_catalogue"]["codex_scope"]
        || candidate["model_catalogue"]["claude_scope"]
            != settings["model_catalogue"]["claude_scope"]
        || candidate["model_catalogue"]["claude_bridge"]
            != settings["model_catalogue"]["claude_bridge"];
    // A single authoritative file commits role settings and model policy together.
    // The legacy policy-only path remains available to embedded callers.
    if let Some(path) = &app.settings_path {
        if let Err(error) = crate::engine_settings::save(path, &candidate) {
            return (500, json!({"error":error}));
        }
    } else if candidate["model_catalogue"] != settings["model_catalogue"] {
        if let Some(path) = &app.model_policy_path {
            if let Err(error) = crate::catalogue::save_policy(path, &policy) {
                *app.model_policy_error.lock().unwrap() = Some(error.clone());
                return (500, json!({"error":error}));
            }
        }
    }
    *app.model_policy_error.lock().unwrap() = None;
    *settings = candidate;
    drop(settings);
    if refresh {
        app.catalogue.refresh(policy);
    }
    (200, json!({"ok": true}))
}

fn api_set_project(app: &Arc<App>, path: &str) -> ApiResponse {
    match app.set_project(path) {
        Ok(()) => (200, json!({"ok": true})),
        Err(e) => (400, json!({"error": e})),
    }
}
pub(super) fn api_project(app: &Arc<App>, body: &Value) -> ApiResponse {
    let path = body["path"].as_str().unwrap_or("").trim().to_string();
    api_set_project(app, &path)
}

pub(super) fn api_project_select(app: &Arc<App>, body: &Value) -> ApiResponse {
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
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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
                            Ok(()) => ctx2.log_event(
                                "git",
                                &format!("clone finished: {repo} -> {}", target.display()),
                            ),
                            Err(e) => ctx2.log_event(
                                "error",
                                &format!("clone finished but selection failed: {e}"),
                            ),
                        }
                    }
                    Ok(o) => eprintln!(
                        "clone of {repo} failed: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ),
                    Err(e) => eprintln!("failed to launch gh clone: {e}"),
                }
            });
            (200, json!({"ok": true, "cloning": true}))
        }
    } else {
        (400, json!({"error": "path or repo required"}))
    }
}

pub(super) fn api_self_update(app: &App, ctx: &Ctx) -> ApiResponse {
    if app.any_busy() {
        return (409, json!({"error": "busy"}));
    }
    let repo = std::env::var("FORGE_REPO").unwrap_or_else(|_| {
        format!(
            "{}/Projects/Forge",
            std::env::var("HOME").unwrap_or_default()
        )
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
            let _ = fs::write(
                ctx.forge_path("update-pending"),
                crate::util::unix_timestamp().to_string(),
            );
            ctx.log_event("update", "self-update started; the engine restarts and the shell reloads the plugin if it changed (log: journalctl --user -u forge-update)");
            (200, json!({"ok": true}))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            (500, json!({"error": format!("systemd-run failed: {err}")}))
        }
        Err(e) => (500, json!({"error": format!("systemd-run failed: {e}")})),
    }
}
