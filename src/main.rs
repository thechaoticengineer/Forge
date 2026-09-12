//! Forge — a minimal wrapper around AI coding agents.
//!
//! Loop: plan (codex/claude) -> human approves -> per stage:
//! implement (tool A) -> independent review (tool B, fresh session) ->
//! bounded fix loop -> commit proposed message -> next stage -> push.
//!
//! Persistent goal queue: process goals sequentially with optional automatic approval.
//!
//! Serves a JSON API on 127.0.0.1:8734 for the Quickshell panel.

mod catalogue;
mod catalogue_process;
mod metadata;
mod agent;
mod agent_log;
mod architecture;
mod architect;
mod review_history;
mod reports;
mod contracts;
mod app;
mod http;
mod plan;
mod candidate_draft;
mod response;
mod routing;
mod model_selection;
mod model_policy_ai;
mod model_policy_cost;
mod prompts;
mod util;
mod usage;
mod quota;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod architecture_integration_tests;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod report_tests;
#[cfg(test)]
mod provider_stream_tests;

use crate::app::App;
use crate::http::{PORT, handle};
use crate::plan::default_settings;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    let project = std::env::args()
        .nth(1)
        .unwrap_or_else(|| std::env::current_dir().unwrap().display().to_string());
    let project = fs::canonicalize(&project)
        .map(|p| p.display().to_string())
        .unwrap_or(project);
    let projects_root = PathBuf::from(&project)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(&project))
        .display()
        .to_string();
    let mut settings = default_settings();
    settings["projects_root"] = json!(projects_root);
    let policy_path = crate::catalogue::policy_path();
    let loaded = crate::catalogue::load_policy(&policy_path);
    if let Ok(Some(policy)) = &loaded { settings["model_catalogue"] = json!(policy); }
    let mut app = App::new(&project, settings);
    app.model_policy_path = Some(policy_path);
    *app.model_policy_error.lock().unwrap() = loaded.err();
    let app = Arc::new(app);
    if std::env::args().nth(2).as_deref() == Some("--recover-committed") {
        let ctx = app.context(&project);
        match ctx.recover_committed_stages() {
            Ok(stages) => println!("{}", json!({"recovered_stages":stages})),
            Err(error) => { eprintln!("{error}"); std::process::exit(1); }
        }
        return;
    }
    app.refresh_catalogue();
    app.refresh_quota(false);
    {
        // Engine-side periodic discovery/metadata refresh; no panel polling.
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                app.scheduler_tick(crate::util::unix_timestamp());
            }
        });
    }
    let port: u16 = std::env::var("FORGE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(PORT);
    let server = tiny_http::Server::http(("127.0.0.1", port)).expect("bind server");
    println!(
        "Forge engine on http://127.0.0.1:{port}  (project: {})",
        app.active_project.lock().unwrap()
    );
    for req in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || handle(&app, req));
    }
}
