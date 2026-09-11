//! Shared application/session state, metadata scheduling, and repository discovery.
mod agent_execution;
mod execution;
mod git;
mod history;
mod persistence;
mod planning;
mod queue;
#[path = "review.rs"]
mod review;
#[path = "reassessment.rs"]
mod reassessment;
use crate::util::{canonical_project, unix_timestamp};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const FORGE_DIR: &str = ".forge";

pub(crate) enum PlanMode {
    Standard,
    Refactor { focus: String },
}

pub(crate) struct App {
    pub(crate) quota: crate::quota::Service,
    pub(crate) catalogue: crate::catalogue::Catalogue,
    pub(crate) metadata: crate::metadata::Service,
    pub(crate) scheduler: crate::metadata::Scheduler,
    pub(crate) model_policy_path: Option<PathBuf>,
    pub(crate) model_policy_error: Mutex<Option<String>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub(crate) active_project: Mutex<String>,
    pub(crate) settings: Mutex<Value>,
    gh_cache: Mutex<Option<(Instant, Value, Option<String>)>>,
}

pub(crate) struct Session {
    pub(crate) state: Mutex<State>,
    pub(crate) queue_lock: Mutex<()>,
    pub(crate) persistence_lock: Mutex<()>,
    pub(crate) architect_lock: Mutex<()>,
    pub(crate) persistence_error: Mutex<Option<String>>,
    pub(crate) legacy_state_cache: Mutex<Option<(String, Value)>>,
    pub(crate) stop_requested: AtomicBool,
    pub(crate) busy: AtomicBool,
    pub(crate) queue_active: AtomicBool,
}

/// A worker keeps this project and session even when the UI switches projects.
#[derive(Clone)]
pub(crate) struct Ctx {
    pub(crate) app: Arc<App>,
    pub(crate) project: String,
    pub(crate) session: Arc<Session>,
}

/// Each worker owns one session's busy claim, including while advancing its queue.
pub(crate) struct WorkerGuard<'a>(pub(crate) &'a Session);

impl Drop for WorkerGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.current_stage = None;
        state.current_step.clear();
        state.run_started_unix = 0;
        self.0.stop_requested.store(false, Ordering::SeqCst);
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) phase: String, // idle|planning|plan_ready|running|blocked|done|failed
    pub(crate) goal: String,
    pub(crate) goal_enhancement: Value,
    pub(crate) goal_enhancement_serial: i64,
    pub(crate) current_stage: Option<i64>,
    pub(crate) current_step: String,
    pub(crate) run_started_unix: i64,
    pub(crate) agent_role: String,
    pub(crate) agent_tool: String,
    pub(crate) agent_model: String,
    pub(crate) model_selection: Value,
    pub(crate) architect_activity: Value,
    pub(crate) role_usage: Value,
    pub(crate) agent_started_unix: i64,
    pub(crate) agent_lines: i64,
    pub(crate) agent_last_line: String,
}

impl App {
    pub(crate) fn new(project: &str, mut settings: Value) -> Self {
        if settings.get("automatic_routing").is_none() { settings["automatic_routing"] = json!(true); }
        if settings.get("routing_billing_basis").is_none() { settings["routing_billing_basis"] = Value::Null; }
        let app = Self {
            quota: crate::quota::Service::default(),
            catalogue: crate::catalogue::Catalogue::default(),
            metadata: crate::metadata::Service::default(),
            scheduler: crate::metadata::Scheduler::new(unix_timestamp()),
            model_policy_path: None,
            model_policy_error: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            active_project: Mutex::new(canonical_project(project)),
            settings: Mutex::new(settings),
            gh_cache: Mutex::new(None),
        };
        app.active_session();
        app
    }

    pub(crate) fn refresh_catalogue(&self) -> bool {
        let settings = self.settings.lock().unwrap();
        let policy = crate::catalogue::Policy::from_settings(&settings);
        policy.is_ok_and(|policy| self.catalogue.refresh(policy))
    }

    pub(crate) fn refresh_quota(&self, manual: bool) -> bool {
        let bridge = self.settings.lock().unwrap()["model_catalogue"]["claude_bridge"]
            .as_str().unwrap_or("").to_string();
        self.quota.refresh(bridge, manual)
    }

    pub(crate) fn refresh_metadata(&self) -> bool {
        let policy = {
            let settings = self.settings.lock().unwrap();
            crate::catalogue::Policy::from_settings(&settings)
        };
        let Ok(policy) = policy else { return false };
        if !policy.metadata_research {
            return false;
        }
        let snapshots = self.catalogue.metadata_snapshot(&policy);
        self.metadata.refresh(&policy, snapshots)
    }

    /// Application-owned periodic refresh; the panel never polls for this.
    /// Metadata waits for discovery so it works from current snapshots.
    pub(crate) fn scheduler_tick(&self, now: i64) {
        self.refresh_quota(false);
        let policy = {
            let settings = self.settings.lock().unwrap();
            crate::catalogue::Policy::from_settings(&settings)
        };
        let Ok(policy) = policy else { return };
        let (discovery, metadata) = self.scheduler.due(now, &policy);
        if discovery {
            self.scheduler.mark_discovery(now);
            self.catalogue.refresh(policy.clone());
        }
        if metadata && !self.catalogue.running() && !self.metadata.running() {
            self.scheduler.mark_metadata(now);
            if policy.metadata_research {
                let snapshots = self.catalogue.metadata_snapshot(&policy);
                self.metadata.refresh(&policy, snapshots);
            }
        }
    }

    pub(crate) fn session(&self, project: &str) -> Arc<Session> {
        let project = canonical_project(project);
        if let Some(session) = self.sessions.lock().unwrap().get(&project).cloned() {
            return session;
        }
        // Read persisted state before taking the map lock. No session state
        // or queue lock may be held while the sessions map is locked.
        let loaded = crate::architecture::Store::new(PathBuf::from(&project).join(FORGE_DIR)).load_raw();
        let persistence_error = loaded.as_ref().err().cloned();
        let plan = loaded.ok().flatten();
        // A pending self-update marker means install.sh restarted the engine
        // since this project last had a session; surface the completion in
        // its feed. A stale marker (update never restarted us) is discarded.
        let marker = PathBuf::from(&project).join(FORGE_DIR).join("update-pending");
        if let Ok(text) = fs::read_to_string(&marker) {
            let _ = fs::remove_file(&marker);
            let requested = text.trim().parse::<i64>().unwrap_or(0);
            if unix_timestamp() - requested <= 900 {
                history::log_project_event(&project, "update",
                    "self-update finished; engine restarted on the new build");
            }
        }
        let session = Arc::new(Session {
            state: Mutex::new(State {
                phase: if persistence_error.is_some() { "failed" } else if plan.is_some() { "plan_ready" } else { "idle" }.into(),
                goal: plan.as_ref().and_then(|plan| plan["goal"].as_str())
                    .unwrap_or("").to_string(),
                ..State::default()
            }),
            queue_lock: Mutex::new(()),
            persistence_lock: Mutex::new(()),
            architect_lock: Mutex::new(()),
            persistence_error: Mutex::new(persistence_error),
            legacy_state_cache: Mutex::new(None),
            stop_requested: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            queue_active: AtomicBool::new(false),
        });
        self.sessions.lock().unwrap().entry(project).or_insert(session).clone()
    }

    pub(crate) fn active_session(&self) -> Arc<Session> {
        let project = self.active_project.lock().unwrap().clone();
        self.session(&project)
    }

    pub(crate) fn context(self: &Arc<Self>, project: &str) -> Ctx {
        let project = canonical_project(project);
        let session = self.session(&project);
        Ctx { app: Arc::clone(self), project, session }
    }

    pub(crate) fn setting(&self, key: &str) -> String {
        self.settings.lock().unwrap()[key].as_str().unwrap_or("").to_string()
    }

    /// Selection never changes or waits for an existing session's work.
    pub(crate) fn set_project(&self, path: &str) -> Result<(), String> {
        if !PathBuf::from(path).join(".git").exists() {
            return Err(format!("{path} is not a git repository"));
        }
        let project = canonical_project(path);
        self.session(&project);
        *self.active_project.lock().unwrap() = project;
        Ok(())
    }

    pub(crate) fn open_sessions(&self) -> Vec<(String, Arc<Session>)> {
        self.sessions.lock().unwrap().iter()
            .map(|(project, session)| (project.clone(), Arc::clone(session))).collect()
    }

    pub(crate) fn any_busy(&self) -> bool {
        self.open_sessions().iter().any(|(_, session)| {
            session.busy.load(Ordering::SeqCst) || session.queue_active.load(Ordering::SeqCst)
        })
    }

    pub(crate) fn session_summaries(self: &Arc<Self>, active: &str) -> Value {
        let mut sessions = self.open_sessions();
        sessions.sort_by(|(a, _), (b, _)| {
            (a != active).cmp(&(b != active)).then_with(|| a.cmp(b))
        });
        Value::Array(sessions.into_iter().map(|(project, session)| {
            let ctx = Ctx { app: Arc::clone(self), project, session };
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            let queued = ctx.load_queue()["items"].as_array().unwrap().iter()
                .filter(|item| item["status"] == "queued").count();
            let s = ctx.session.state.lock().unwrap();
            json!({
                "project": ctx.project,
                "name": PathBuf::from(&ctx.project).file_name().unwrap_or_default().to_string_lossy(),
                "phase": s.phase,
                "busy": ctx.session.busy.load(Ordering::SeqCst),
                "queue_active": ctx.session.queue_active.load(Ordering::SeqCst),
                "queued": queued,
                "current_step": s.current_step,
                "goal": s.goal,
            })
        }).collect())
    }

    /// Raw `gh repo list` result, cached for 60 seconds so panel polling
    /// does not spawn a gh process every request. Returns (repos, error).
    fn remote_repos_raw(&self) -> (Value, Option<String>) {
        let mut cache = self.gh_cache.lock().unwrap();
        if let Some((at, repos, err)) = cache.as_ref()
            && at.elapsed() < Duration::from_secs(60)
        {
            return (repos.clone(), err.clone());
        }
        let out = Command::new("gh")
            .args(["repo", "list", "--limit", "100",
                   "--json", "nameWithOwner,name,updatedAt,isPrivate"])
            .output();
        let (repos, err) = match out {
            Err(e) => (json!([]), Some(format!("failed to launch gh: {e}"))),
            Ok(o) if !o.status.success() => (
                json!([]),
                Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            ),
            Ok(o) => match serde_json::from_slice::<Value>(&o.stdout) {
                Ok(v) if v.is_array() => (v, None),
                _ => (json!([]), Some("unparseable gh output".to_string())),
            },
        };
        *cache = Some((Instant::now(), repos.clone(), err.clone()));
        (repos, err)
    }

    /// Remote entries for /api/projects, with `cloned` computed against
    /// the local project names, sorted by most recently updated.
    pub(crate) fn remote_repos(&self, local_names: &[String]) -> (Value, Option<String>) {
        let (raw, err) = self.remote_repos_raw();
        let mut remote: Vec<Value> = raw
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let name = r["name"].as_str().unwrap_or("");
                json!({
                    "name": name,
                    "full_name": r["nameWithOwner"].as_str().unwrap_or(""),
                    "private": r["isPrivate"].as_bool().unwrap_or(false),
                    "updated_at": r["updatedAt"].as_str().unwrap_or(""),
                    "cloned": local_names.iter().any(|l| l == name),
                })
            })
            .collect();
        remote.sort_by(|a, b| {
            b["updated_at"].as_str().unwrap_or("")
                .cmp(a["updated_at"].as_str().unwrap_or(""))
        });
        (Value::Array(remote), err)
    }

}

impl Ctx {
    pub(crate) fn project(&self) -> &str {
        &self.project
    }

    pub(crate) fn forge_path(&self, name: &str) -> PathBuf {
        PathBuf::from(self.project()).join(FORGE_DIR).join(name)
    }

    pub(crate) fn ensure_forge_dir(&self) {
        let _ = fs::create_dir_all(self.forge_path(""));
    }

    pub(crate) fn set_phase(&self, phase: &str) {
        self.session.state.lock().unwrap().phase = phase.to_string();
    }

    pub(crate) fn set_step(&self, stage: Option<i64>, step: &str) {
        let mut s = self.session.state.lock().unwrap();
        s.current_stage = stage;
        s.current_step = step.to_string();
    }

    /// Atomically claim the busy flag; Err means other work is in flight.
    /// Prevents two requests that both saw busy=false from racing to spawn.
    pub(crate) fn acquire_busy(&self) -> Result<(), ()> {
        self.session.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| ())
    }

}
