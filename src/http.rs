use crate::app::{App, Ctx};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

#[path = "http_admin.rs"]
mod admin;
#[path = "http_discussion.rs"]
mod discussion;
#[path = "http_features.rs"]
mod features;
#[path = "http_plan.rs"]
mod plan;
#[path = "http_queue.rs"]
mod queue;
#[path = "http_read.rs"]
mod read;

use admin::*;
use discussion::*;
use features::*;
use plan::*;
use queue::*;
use read::*;

pub(crate) const PORT: u16 = 8734;

pub(super) type ApiResponse = (u32, Value);

struct ApiRequest<'a> {
    method: tiny_http::Method,
    path: &'a str,
    query: &'a str,
    body: Value,
}

impl<'a> ApiRequest<'a> {
    fn decode(url: &'a str, method: tiny_http::Method, body: Value) -> Self {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        Self {
            method,
            path,
            query,
            body,
        }
    }

    fn project_endpoint(&self) -> bool {
        matches!(
            self.path,
            "/api/state"
                | "/api/features"
                | "/api/features/state"
                | "/api/features/create"
                | "/api/features/chat"
                | "/api/features/review"
                | "/api/architecture/history"
                | "/api/architecture/reviews"
                | "/api/agent_log"
                | "/api/agent_records"
                | "/api/diff"
                | "/api/plan"
                | "/api/plan/edit"
                | "/api/plan/revise"
                | "/api/plan/chat"
                | "/api/goal/enhance"
                | "/api/discussion/message"
                | "/api/discussion/reset"
                | "/api/models/suggest"
                | "/api/models/suggestion"
                | "/api/approve"
                | "/api/run"
                | "/api/stop"
                | "/api/reset_plan"
        ) || self.path.starts_with("/api/queue/")
    }

    fn project_target(&self) -> Result<Option<String>, &'static str> {
        if !self.project_endpoint() {
            Ok(None)
        } else if self.method == tiny_http::Method::Get {
            query_value(self.query, "project")
        } else {
            match self.body.get("project") {
                None => Ok(None),
                Some(Value::String(project)) => Ok(Some(project.clone())),
                Some(_) => Err("project must be a string"),
            }
        }
    }
}

// ---------------------------------------------------------------- http

fn respond(req: tiny_http::Request, code: u32, body: Value) {
    let data = body.to_string();
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let resp = tiny_http::Response::from_string(data)
        .with_status_code(code)
        .with_header(header);
    let _ = req.respond(resp);
}

pub(super) fn query_value(query: &str, key: &str) -> Result<Option<String>, &'static str> {
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
                let high = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or("invalid project query encoding")?;
                let low = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or("invalid project query encoding")?;
                (high * 16 + low) as u8
            }
            byte => byte,
        });
    }
    String::from_utf8(decoded)
        .map(Some)
        .map_err(|_| "invalid project query encoding")
}

pub(crate) fn handle(app: &Arc<App>, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let method = req.method().clone();
    let mut body_text = String::new();
    let _ = req.as_reader().read_to_string(&mut body_text);
    let body = serde_json::from_str(&body_text).unwrap_or(json!({}));
    let request = ApiRequest::decode(&url, method, body);
    let project_endpoint = request.project_endpoint();
    let target = match request.project_target() {
        Ok(target) => target,
        Err(error) => {
            respond(req, 400, json!({"error": error}));
            return;
        }
    };
    if let Some(project) = &target
        && !PathBuf::from(project).join(".git").exists()
    {
        respond(
            req,
            400,
            json!({"error": format!("{project} is not a git repository")}),
        );
        return;
    }
    let active_project = app.active_project.lock().unwrap().clone();
    let ctx = app.context(if project_endpoint {
        target.as_deref().unwrap_or(&active_project)
    } else {
        &active_project
    });
    let (code, response) = dispatch(app, &ctx, &active_project, &request);
    respond(req, code, response);
}

fn dispatch(
    app: &Arc<App>,
    ctx: &Ctx,
    active_project: &str,
    request: &ApiRequest<'_>,
) -> ApiResponse {
    match (&request.method, request.path) {
        (tiny_http::Method::Get, "/api/architecture/reviews") => {
            api_architecture_reviews(ctx, request.query)
        }
        (tiny_http::Method::Get, "/api/architecture/history") => {
            api_architecture_history(ctx, request.query)
        }
        (tiny_http::Method::Get, "/api/models") => api_models(app, request.query),
        (tiny_http::Method::Post, "/api/models/suggest") => api_model_suggest(ctx, &request.body),
        (tiny_http::Method::Get, "/api/models/suggestion") => api_model_suggestion(ctx),
        (tiny_http::Method::Post, "/api/quota/refresh") => api_quota_refresh(app),
        (tiny_http::Method::Post, "/api/models/refresh") => api_models_refresh(app),
        (tiny_http::Method::Post, "/api/models/cancel") => api_models_cancel(app),
        (tiny_http::Method::Post, "/api/models/metadata/refresh") => api_metadata_refresh(app),
        (tiny_http::Method::Get, "/api/state") => api_state(app, ctx, active_project),
        (tiny_http::Method::Get, "/api/features") => api_features(ctx),
        (tiny_http::Method::Get, "/api/features/state") => api_feature_state(ctx, request.query),
        (tiny_http::Method::Post, "/api/features/create") => {
            api_feature_create(ctx, &request.body)
        }
        (tiny_http::Method::Post, "/api/features/chat") => api_feature_chat(ctx, &request.body),
        (tiny_http::Method::Post, "/api/features/review") => api_feature_review(ctx, &request.body),
        (tiny_http::Method::Get, "/api/agent_records") => api_agent_records(ctx, request.query),
        (tiny_http::Method::Get, "/api/agent_log") => api_agent_log(ctx, request.query),
        (tiny_http::Method::Get, "/api/diff") => api_diff(ctx),
        (tiny_http::Method::Get, "/api/projects") => api_projects(app),
        (tiny_http::Method::Post, "/api/settings") => api_settings(app, &request.body),
        (tiny_http::Method::Post, "/api/project") => api_project(app, &request.body),
        (tiny_http::Method::Post, "/api/project/select") => api_project_select(app, &request.body),
        (
            tiny_http::Method::Post,
            "/api/queue/add" | "/api/queue/remove" | "/api/queue/move" | "/api/queue/clear",
        ) => api_queue_mutate(ctx, request.path, &request.body),
        (tiny_http::Method::Post, "/api/queue/start") => api_queue_start(ctx),
        (tiny_http::Method::Post, "/api/plan") => api_plan(ctx, &request.body),
        (tiny_http::Method::Post, "/api/plan/revise") => api_plan_revise(ctx, &request.body),
        (tiny_http::Method::Post, "/api/plan/chat") => api_plan_chat(ctx, &request.body),
        (tiny_http::Method::Post, "/api/goal/enhance") => api_goal_enhance(ctx, &request.body),
        (tiny_http::Method::Post, "/api/discussion/message") => {
            api_discussion_message(ctx, &request.body)
        }
        (tiny_http::Method::Post, "/api/discussion/reset") => api_discussion_reset(ctx),
        (tiny_http::Method::Post, "/api/plan/edit") => api_plan_edit(ctx, &request.body),
        (tiny_http::Method::Post, "/api/approve") => api_approve(ctx),
        (tiny_http::Method::Post, "/api/run") => api_run(ctx),
        (tiny_http::Method::Post, "/api/stop") => api_stop(ctx),
        (tiny_http::Method::Post, "/api/reset_plan") => api_reset_plan(ctx),
        (tiny_http::Method::Post, "/api/self_update") => api_self_update(app, ctx),
        _ => (404, json!({"error": "not found"})),
    }
}
