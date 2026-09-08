#[path = "review.rs"]
mod review;
#[path = "reassessment.rs"]
mod reassessment;
use crate::agent::{AgentUsage, AgentRequest, AgentResult, stream_agent_result};
use crate::prompts::{CHAT_PROMPT, FIX_PROMPT, IMPLEMENT_PROMPT, PLANNER_PROMPT, REFACTOR_PROMPT, REVIEW_PROMPT, REVISE_PROMPT};
use crate::usage::{accumulate_invocation_usage, merge_planner_revision_totals};
use crate::util::{canonical_project, clock_hms, fill_template, fmt_duration, unix_timestamp};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const FORGE_DIR: &str = ".forge";

pub(crate) enum PlanMode {
    Standard,
    Refactor { focus: String },
}

pub(crate) struct App {
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
                log_project_event(&project, "update",
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

/// Append a history event for a project without a live session; used for
/// events that must outlive an engine restart, like self-update completion.
fn log_project_event(project: &str, kind: &str, text: &str) {
    let dir = PathBuf::from(project).join(FORGE_DIR);
    let _ = fs::create_dir_all(&dir);
    let entry = json!({"t": clock_hms(), "unix": unix_timestamp(), "kind": kind, "text": text});
    append_history_event(project, &entry, kind, text);
}

fn append_history_event(project: &str, entry: &Value, kind: &str, text: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PathBuf::from(project).join(FORGE_DIR).join("history.jsonl"))
    {
        let _ = writeln!(f, "{entry}");
    }
    println!("[{kind}] {text}");
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

    pub(crate) fn log_event(&self, kind: &str, text: &str) {
        self.ensure_forge_dir();
        let t = clock_hms();
        let mut entry = json!({"t": t, "unix": unix_timestamp(), "kind": kind, "text": text});
        {
            // Callers release the session state lock before logging.
            let s = self.session.state.lock().unwrap();
            if !s.goal.is_empty() {
                entry["goal"] = json!(s.goal.chars().take(120).collect::<String>());
            }
            if let Some(stage) = s.current_stage {
                entry["stage"] = json!(stage);
            }
        }
        append_history_event(self.project(), &entry, kind, text);
    }

    fn read_jsonl_tail(&self, name: &str, keep: usize) -> Value {
        let text = (|| -> std::io::Result<String> {
            let mut file = fs::File::open(self.forge_path(name))?;
            let size = file.metadata()?.len();
            let start = size.saturating_sub(2 * 1024 * 1024);
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::new(); file.take(2 * 1024 * 1024).read_to_end(&mut bytes)?;
            if start > 0 {
                let boundary = bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |p| p + 1);
                bytes.drain(..boundary);
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })().unwrap_or_default();
        let items: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let skip = items.len().saturating_sub(keep);
        Value::Array(items.into_iter().skip(skip).collect())
    }

    pub(crate) fn read_history(&self) -> Value {
        self.read_jsonl_tail("history.jsonl", 400)
    }

    pub(crate) fn read_chat(&self) -> Value {
        self.read_jsonl_tail("chat.jsonl", 100)
    }

    pub(crate) fn read_reports(&self) -> Value {
        self.read_jsonl_tail("reports.jsonl", 100)
    }

    fn save_json(&self, name: &str, value: &Value) {
        self.ensure_forge_dir();
        let _ = fs::write(
            self.forge_path(name),
            serde_json::to_string_pretty(value).unwrap(),
        );
    }

    pub(crate) fn architecture_store(&self) -> crate::architecture::Store {
        crate::architecture::Store::new(self.forge_path(""))
    }

    pub(crate) fn load_plan(&self) -> Option<Value> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        match self.architecture_store().load() {
            Ok(plan) => plan,
            Err(error) => {
                *self.session.persistence_error.lock().unwrap() = Some(error);
                None
            }
        }
    }

    pub(crate) fn save_plan(&self, plan: &Value) -> Result<(), String> {
        self.publish_plan(plan, false).map(|_| ())
    }

    pub(crate) fn publish_plan(&self, plan: &Value, replacement: bool) -> Result<Value, String> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        let result = (|| {
            let store = self.architecture_store();
            let mut old = store.load()?;
            if let Some(legacy) = old.as_ref().filter(|p| p.get("architecture").is_none()) {
                old = Some(store.publish(legacy.clone(), crate::architecture::checkpoint_default(),
                    json!({"kind": "legacy_import"}))?);
            }
            let mut next = plan.clone();
            // Import legacy metadata on first mutation, never on a read.
            let same = !replacement && old.as_ref().is_some_and(|p|
                next["plan_id"].is_null() || next["plan_id"] == p["plan_id"]);
            if !replacement && old.is_some() && !same { return Err("plan identity does not match the active project plan".into()); }
            let mut cp = crate::architecture::checkpoint_default();
            let mut invalidations = Vec::new();
            if same {
                let old = old.as_ref().unwrap();
                if old.get("architecture").is_some() {
                    cp = store.checkpoint(old)?;
                    for key in ["plan_id", "contract_version", "architecture"] { next[key] = old[key].clone(); }
                    if next["revision"].is_null() { next["revision"] = old["revision"].clone(); }
                }
                let affected = crate::plan::affected_stages(old, &next).map_err(str::to_owned)?;
                if !affected.is_empty() && next["revision"] == old["revision"] {
                    next["revision"] = json!(old["revision"].as_u64().unwrap().checked_add(1).ok_or("revision limit reached")?);
                }
                if next["revision"] != old["revision"] {
                    // Legacy references cannot prove their mutable tails were committed.
                    if cp["session"]["resume_policy"] == "fork_from_checkpoint" { cp["context_status"] = json!("needs_recovery"); }
                    // Exact committed sessions remain usable after an engine-only edit.
                    for key in ["guidance", "agreements"] {
                        if let Some(records) = cp[key].as_object_mut() {
                            for (id, record) in records {
                                let valid = next["stages"].as_array().unwrap().iter().any(|s|
                                    s["id"].to_string() == *id && (s["status"] == "committed" || !affected.contains(&s["id"])));
                                if !valid && record.is_object() {
                                    let trigger = if old["goal"] != next["goal"] { "goal_changed" }
                                        else if !next["stages"].as_array().unwrap().iter().any(|s| s["id"].to_string() == *id) { "stage_removed" }
                                        else { "stage_or_dependency_changed" };
                                    invalidations.push(json!({"kind": key, "stage_id": id, "record_id": record["id"],
                                        "trigger": trigger, "previous_revision": old["revision"], "revision": next["revision"]}));
                                    record["valid"] = json!(false);
                                    record["invalidation_trigger"] = json!(trigger);
                                }
                            }
                        }
                    }
                }
            } else {
                for key in ["plan_id", "revision", "architecture", "contract_version"] { next.as_object_mut().ok_or("invalid plan")?.remove(key); }
            }
            let kind = if replacement { "plan_created" } else if old.as_ref().is_some_and(|p| p.get("architecture").is_none()) { "legacy_import" } else { "plan_saved" };
            let mut reviews = Vec::new();
            let mut removed = Vec::new();
            if let Some(old) = old.as_ref().filter(|_| same) {
                for stage in old["stages"].as_array().unwrap() {
                    if !next["stages"].as_array().unwrap().iter().any(|s| s["id"] == stage["id"]) {
                        removed.push(stage["id"].clone());
                    }
                }
            }
            for stage in next["stages"].as_array().ok_or("invalid stages")? {
                let count = old.as_ref().filter(|_| same).and_then(|p| p["stages"].as_array())
                    .and_then(|stages| stages.iter().find(|s| s["id"] == stage["id"]))
                    .and_then(|s| s["reviews"].as_array()).map_or(0, Vec::len);
                for review in stage["reviews"].as_array().into_iter().flatten().skip(count) {
                    let mut record = review.clone();
                    if record.is_object() {
                        record["version"] = json!(crate::architecture::VERSION);
                        if record["id"].is_null() { record["id"] = json!(crate::architecture::identity()); }
                        if record["policy"].is_null() { record["policy"] = cp["review_policy"].clone(); }
                        record["stage_id"] = stage["id"].clone();
                        if record["role"].is_null() { record["role"] = json!("reviewer"); }
                        reviews.push(record);
                    }
                }
            }
            let mut outcomes = Vec::new();
            if let Some(old) = old.as_ref().filter(|_| same) {
                for stage in next["stages"].as_array().unwrap() {
                    let before = old["stages"].as_array().unwrap().iter().find(|s| s["id"] == stage["id"]);
                    if stage["status"] == "committed" && before.is_none_or(|s| s["status"] != "committed") {
                        let outcome = json!({"stage_id":stage["id"],"revision":next["revision"],"status":"committed","sha":stage["sha"],"review_gate":stage["review_gate"],"review_policy":stage["review_policy"],"acceptance":stage["acceptance"].as_str().unwrap_or("").chars().take(500).collect::<String>(),"unix":unix_timestamp()});
                        outcomes.push(outcome.clone());
                        if !cp["execution_outcomes"].is_array() { cp["execution_outcomes"] = json!([]); }
                        let recent = cp["execution_outcomes"].as_array_mut().unwrap(); recent.push(outcome);
                        if recent.len() > 8 { recent.remove(0); }
                    }
                }
            }
            if old.as_ref() == Some(&next) { return Ok(next); }
            store.publish(next, cp, json!({"kind": kind, "reviews": reviews, "removed_stage_ids": removed, "invalidations": invalidations, "execution_outcomes":outcomes}))
        })();
        *self.session.persistence_error.lock().unwrap() = result.as_ref().err().cloned();
        result
    }

    fn record_stage_usage(&self, plan: &mut Value, idx: usize, role: &str, tool: &str, usage: Option<AgentUsage>) -> Result<(), String> {
        if let Some(usage) = usage.filter(|usage| !usage.is_empty()) {
            accumulate_invocation_usage(&mut plan["stages"][idx], "usage", tool, &usage);
            accumulate_invocation_usage(plan, "usage", tool, &usage);
            accumulate_invocation_usage(&mut plan["role_usage"], role, tool, &usage);
            self.save_plan(plan)?;
        }
        Ok(())
    }

    pub(crate) fn load_queue(&self) -> Value {
        fs::read_to_string(self.forge_path("queue.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|queue| queue.get("items").is_some_and(Value::is_array))
            .unwrap_or_else(|| json!({"items": []}))
    }

    pub(crate) fn save_queue(&self, queue: &Value) {
        self.save_json("queue.json", queue);
    }

    fn finish_stage(&self, plan: &mut Value, idx: usize, status: &str) -> Result<i64, String> {
        let stage = &mut plan["stages"][idx];
        let finished = unix_timestamp();
        let started = stage["started_unix"].as_i64().unwrap_or(finished);
        stage["status"] = json!(status);
        stage["finished_unix"] = json!(finished);
        stage["duration_secs"] = json!(finished - started);
        self.save_plan(plan)?;
        Ok(finished - started)
    }

    pub(crate) fn set_phase(&self, phase: &str) {
        self.session.state.lock().unwrap().phase = phase.to_string();
    }

    pub(crate) fn set_step(&self, stage: Option<i64>, step: &str) {
        let mut s = self.session.state.lock().unwrap();
        s.current_stage = stage;
        s.current_step = step.to_string();
    }

    fn setting(&self, key: &str) -> String {
        self.app.setting(key)
    }

    /// Atomically claim the busy flag; Err means other work is in flight.
    /// Prevents two requests that both saw busy=false from racing to spawn.
    pub(crate) fn acquire_busy(&self) -> Result<(), ()> {
        self.session.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| ())
    }

    // ------------------------------------------------------------ git

    pub(crate) fn git(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(self.project())
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    // ---------------------------------------------------------- agents

    fn agent_command(request: &AgentRequest<'_>) -> Result<Command, String> {
        crate::agent::command(request)
    }

    fn open_agent_log(&self, role: &str, tool: &str, model: &str) -> Result<Arc<Mutex<fs::File>>, String> {
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.forge_path("agent.log"))
            .and_then(|mut file| {
                writeln!(
                    file,
                    "=== [{role}] {tool} ({model}) started {} ===",
                    clock_hms()
                )?;
                file.flush()?;
                Ok(Arc::new(Mutex::new(file)))
            })
            .map_err(|e| self.agent_error(role, format!("failed to initialize agent log: {e}")))
    }

    fn agent_error(&self, role: &str, message: String) -> String {
        self.log_event("error", &format!("[{role}] {message}"));
        message
    }

    pub(crate) fn run_agent(&self, request: &AgentRequest<'_>) -> Result<AgentResult, String> {
        let AgentRequest { role, provider: tool, prompt, model, effort, session } = *request;
        if self.session.stop_requested.load(Ordering::SeqCst) { return Err("provider launch stopped; saved work retained".into()); }
        let _ = prompt;
        #[cfg(test)]
        {
            let mut settings = self.app.settings.lock().unwrap();
            if !settings["mock_agent_requests"].is_array() { settings["mock_agent_requests"] = json!([]); }
            settings["mock_agent_requests"].as_array_mut().unwrap().push(json!({"role":role,"provider":tool,"model":model,"effort":effort,"prompt":prompt,"session":session}));
        }
        let mock_execution = tool == "mock";
        #[cfg(test)]
        let mock_execution = mock_execution || (matches!(role,"implementer"|"fixer") && {
            let settings = self.app.settings.lock().unwrap();
            settings["test_fake_providers"] == true && settings["test_real_implementation_cli"] != true
        });
        if mock_execution {
            #[cfg(test)]
            if role == "planner" {
                let mut settings = self.app.settings.lock().unwrap();
                settings["mock_planner_prompt"] = json!(prompt);
                settings["mock_planner_phase"] = json!(self.session.state.lock().unwrap().phase);
                settings["mock_planner_had_plan"] = json!(self.forge_path("plan-candidate.json").exists());
            }
            #[cfg(test)]
            if role == "chat" {
                let mut settings = self.app.settings.lock().unwrap();
                let state = self.session.state.lock().unwrap();
                settings["mock_chat_prompt"] = json!(prompt);
                settings["mock_chat_model"] = json!(model);
                settings["mock_chat_phase"] = json!(state.phase);
                settings["mock_chat_step"] = json!(state.current_step);
                settings["mock_chat_busy"] = json!(self.session.busy.load(Ordering::SeqCst));
            }
            #[cfg(test)]
            if role == "fixer" || role == "reviewer" {
                let mut settings = self.app.settings.lock().unwrap();
                // Reviewer capture is opt-in to keep unrelated API test settings small.
                let key = format!("mock_{role}_prompts");
                if role == "fixer" || settings.get(&key).is_some() {
                    settings.as_object_mut().unwrap().entry(key)
                        .or_insert_with(|| json!([])).as_array_mut().unwrap().push(json!(prompt));
                }
            }
            self.mock_agent(role)?;
            #[cfg(test)]
            if matches!(role, "implementer" | "fixer") {
                let mut settings = self.app.settings.lock().unwrap();
                if let Some(error) = settings["mock_implementation_errors"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0)).and_then(|v| v.as_str().map(str::to_owned)) { return Err(error); }
            }
            let usage = self.app.settings.lock().unwrap().get("mock_usage")
                .filter(|usage| usage.is_object()).map(|usage| AgentUsage {
                    input_tokens: usage["input"].as_i64().unwrap_or(0),
                    output_tokens: usage["output"].as_i64().unwrap_or(0),
                    total_tokens: usage["total"].as_i64().unwrap_or(0),
                    model: usage["model"].as_str().filter(|model| !model.is_empty())
                        .unwrap_or(model).to_string(),
                });
            if usage.is_some() {
                self.log_agent_finished(role, tool, "", usage.as_ref());
            }
            let effective_model = model.to_string();
            #[cfg(test)]
            let effective_model = if matches!(role, "implementer" | "fixer") {
                self.app.settings.lock().unwrap()["mock_effective_models"].as_array_mut().filter(|a| !a.is_empty())
                    .map(|a| a.remove(0).as_str().unwrap().to_string()).unwrap_or(effective_model)
            } else { effective_model };
            #[allow(unused_mut)]
            let mut output = String::new();
            #[cfg(test)]
            if matches!(role, "implementer" | "fixer") {
                output = self.app.settings.lock().unwrap()["mock_implementation_outputs"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0).to_string()).unwrap_or_default();
            }
            return Ok(AgentResult { output, usage, effective_model, model_reported:true, completed: true, ..AgentResult::default() });
        }
        let provider = crate::catalogue::Provider::parse(tool).ok_or_else(|| format!("unknown tool {tool}"))?;
        let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
        let mut selection = self.app.catalogue.execution_with_effort(&policy, provider, model, effort);
        self.session.state.lock().unwrap().model_selection = selection.clone();
        if selection["eligible"] != true {
            return Err(self.agent_error(role, selection["error"].as_str().unwrap_or("model unavailable").into()));
        }
        let requested_model = model.to_string();
        let executable = tool.to_string();
        #[cfg(test)]
        let executable = self.app.settings.lock().unwrap()[format!("test_cli_{tool}")].as_str().unwrap_or(&executable).to_string();
        crate::agent::verify_capabilities(request, self.project(), &executable)?;
        let built = Self::agent_command(request)?;
        let mut cmd = Command::new(&executable);
        // Linux limits each argv entry independently of ARG_MAX. Large saved
        // architecture contexts travel through stdin, never a shell or argv.
        let prompt_stdin = request.prompt.len() > 32 * 1024;
        let args: Vec<_> = built.get_args().collect();
        if prompt_stdin {
            cmd.args(&args[..args.len() - 1]);
            if tool == "codex" { cmd.arg("-"); }
        } else {
            cmd.args(args);
        }
        let codex_home = crate::agent::codex_session::home();
        #[cfg(test)]
        let codex_home = self.app.settings.lock().unwrap()["test_codex_home"].as_str()
            .map(std::path::PathBuf::from).or(codex_home);
        let codex_home = codex_home.map(|home| if home.is_absolute() { home } else {
            std::path::Path::new(self.project()).join(home)
        });
        if matches!(role, "reviewer" | "architect_review") { cmd = crate::agent::review_sandbox(&cmd, self.project(), tool)?; }
        #[cfg(test)]
        if let Some(home) = &codex_home { cmd.env("CODEX_HOME", home); }
        self.log_event("model", &format!("[{role}] {tool}/{} · {} · policy {} · effort {}",
            if model.is_empty() { "provider default" } else { model }, selection["availability"].as_str().unwrap_or("unverified"),
            policy.policy_revision, selection["effort"].as_str().unwrap_or("provider_default")));
        self.log_event("agent", &format!("[{role}] starting {tool} session"));
        let log = self.open_agent_log(role, tool, model)?;

        {
            let mut s = self.session.state.lock().unwrap();
            s.agent_role = role.to_string();
            s.agent_tool = tool.to_string();
            s.agent_model = model.to_string();
            s.agent_started_unix = unix_timestamp();
            s.agent_lines = 0;
            s.agent_last_line.clear();
        }

        use std::os::unix::process::CommandExt;
        let codex_snapshot = (tool == "codex").then(|| codex_home.as_deref()
            .ok_or_else(|| "Codex home unavailable".to_string())
            .and_then(crate::agent::codex_session::Snapshot::capture));
        let child = cmd
            .process_group(0)
            .stdin(if prompt_stdin { Stdio::piped() } else { Stdio::null() })
            .current_dir(self.project())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    self.app.catalogue.observe(&policy, provider, &requested_model,
                        Err(crate::catalogue::Failure::new(crate::catalogue::FailureKind::MissingExecutable)));
                    selection["availability"] = json!("unavailable");
                    selection["availability_unverified"] = json!(false);
                    selection["eligible"] = json!(false);
                }
                selection["execution_status"] = json!("failed_to_launch");
                self.session.state.lock().unwrap().model_selection = selection;
                self.clear_agent_activity();
                return Err(self.agent_error(role, format!("failed to launch {tool}: {e}")));
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();
        let stderr_log = Arc::clone(&log);
        let pid = child.id() as i32;
        let failed_reader = AtomicBool::new(false);
        let (stdout_result, stderr_result, status_result, input_result) = std::thread::scope(|scope| {
            let input_writer = scope.spawn(|| {
                let result = stdin.map_or(Ok(()), |mut input| input.write_all(request.prompt.as_bytes()));
                if result.is_err() { failed_reader.store(true, Ordering::SeqCst); }
                result.map_err(|e| format!("agent prompt input failed: {e}"))
            });
            let stdout_reader = scope.spawn(|| {
                let result = stream_agent_result(stdout, &log, &self.session.state, 1500, tool == "claude");
                if result.is_err() { failed_reader.store(true, Ordering::SeqCst); }
                result
            });
            let stderr_reader = scope.spawn(|| {
                let result = stream_agent_result(stderr, &stderr_log, &self.session.state, 1500, false).map(|r| r.output);
                if result.is_err() { failed_reader.store(true, Ordering::SeqCst); }
                result
            });
            let status = loop {
                if self.session.stop_requested.load(Ordering::SeqCst) || failed_reader.load(Ordering::SeqCst) {
                    unsafe { libc::kill(-pid, libc::SIGKILL); }
                }
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Err(e) => break Err(e),
                    _ => std::thread::sleep(Duration::from_millis(25)),
                }
            };
            // Descendants must not keep pipes open or survive a completed/failed invocation.
            unsafe { libc::kill(-pid, libc::SIGKILL); }
            let _ = child.wait();
            (stdout_reader.join().unwrap_or_else(|_| Err("agent stdout reader panicked".into())),
             stderr_reader.join().unwrap_or_else(|_| Err("agent stderr reader panicked".into())), status,
             input_writer.join().unwrap_or_else(|_| Err("agent prompt writer panicked".into())))
        });
        self.clear_agent_activity();
        input_result.map_err(|e| self.agent_error(role, e))?;
        let mut result = stdout_result?;
        let tail = result.output.clone();
        let mut usage = result.usage.take();
        let etail = match stderr_result {
            Ok(tail) => tail,
            Err(e) => {
                self.agent_error(role, format!("{tool} output failed: {e}"));
                return Err(e);
            }
        };
        let status = match status_result {
            Ok(status) => status,
            Err(e) => {
                return Err(self.agent_error(role, format!("failed to wait for {tool}: {e}")));
            }
        };

        if !status.success() {
            let diagnostic = format!("{etail}\n{tail}").to_lowercase();
            if selection["effort"] != "provider_default" && diagnostic.contains("effort")
                && ["unsupported", "invalid value", "unknown option", "not supported"].iter().any(|s| diagnostic.contains(s)) {
                self.app.catalogue.reject_effort(&policy, provider, &requested_model, selection["effort"].as_str().unwrap());
                selection["eligible"] = json!(false);
                selection["error"] = json!("native effort rejected during execution");
            }
            let observed = if crate::catalogue::auth_error(&diagnostic) { Some(crate::catalogue::FailureKind::Auth) }
                else if diagnostic.contains("model_not_found") || diagnostic.contains("model is not available") { Some(crate::catalogue::FailureKind::Rejected) }
                else { None };
            if let Some(kind) = observed {
                self.app.catalogue.observe(&policy, provider, &requested_model, Err(crate::catalogue::Failure::new(kind)));
                selection["availability"] = json!("unavailable"); selection["availability_unverified"] = json!(false); selection["eligible"] = json!(false);
            }
            selection["execution_status"] = json!("failed");
            self.session.state.lock().unwrap().model_selection = selection;
            self.log_event("error", &format!("[{role}] {tool} failed: {etail}"));
            return Err(format!("{tool} exited with {status}: {etail} {tail}"));
        }
        if let Some(error) = result.error { return Err(self.agent_error(role, error)); }
        if !result.completed {
            return Err("provider ended without a successful structured result".into());
        }
        if session.is_some() && result.session.as_deref() != session {
            return Err("resumed session identity mismatch".into());
        }
        if !result.model_reported && let Some(snapshot) = codex_snapshot {
            let reported = snapshot.and_then(|snapshot| result.session.as_deref()
                .ok_or_else(|| "Codex stream omitted exact session identity".to_string())
                .and_then(|id| snapshot.model(id, self.project())));
            match reported {
                Ok(model) => {
                    result.effective_model = model;
                    result.model_reported = true;
                }
                Err(error) => self.log_event("model", &format!("[{role}] effective model unavailable: {error}")),
            }
        }
        self.app.catalogue.observe(&policy, provider, &requested_model, Ok(()));
        selection["availability"] = json!("execution_verified"); selection["availability_unverified"] = json!(false);
        selection["execution_status"] = json!("succeeded");
        self.session.state.lock().unwrap().model_selection = selection;
        self.log_event("model", &format!("[{role}] {tool}/{} availability verified by successful execution", requested_model));
        if let Some(usage) = &mut usage
            && usage.model.is_empty()
        {
            usage.model = if result.effective_model.is_empty() { model.into() } else { result.effective_model.clone() };
        }
        self.log_agent_finished(role, tool, &tail, usage.as_ref());
        if result.effective_model.is_empty() { result.effective_model = model.into(); }
        if let Some(u) = &usage {
            let mut state = self.session.state.lock().unwrap();
            accumulate_invocation_usage(&mut state.role_usage, role, tool, u);
        }
        result.usage = usage;
        Ok(result)
    }

    fn log_agent_finished(&self, role: &str, tool: &str, tail: &str, usage: Option<&AgentUsage>) {
        let tokens = usage.map(|usage| format!(" ({} tokens)", usage.total_tokens))
            .unwrap_or_default();
        let tail = if tail.is_empty() { String::new() } else { format!(": {tail}") };
        self.log_event("agent", &format!("[{role}] {tool} finished{tokens}{tail}"));
    }

    fn clear_agent_activity(&self) {
        let mut s = self.session.state.lock().unwrap();
        s.agent_role.clear();
        s.agent_tool.clear();
        s.agent_model.clear();
        s.agent_started_unix = 0;
        s.agent_lines = 0;
        s.agent_last_line.clear();
    }

    /// Fake agent used by the self-test and local API smoke tests.
    pub(crate) fn mock_agent(&self, role: &str) -> Result<(), String> {
        match role {
            "planner" => {
                #[cfg(test)]
                if let Some(output) = self.app.settings.lock().unwrap().get("mock_plan_output") {
                    // Null simulates an agent exiting successfully without writing a plan.
                    if !output.is_null() {
                        fs::write(self.forge_path("plan-candidate.json"),
                            output.as_str().map(String::from).unwrap_or_else(|| output.to_string()))
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                let goal = self.session.state.lock().unwrap().goal.clone();
                crate::architecture::atomic_json(&self.forge_path("plan-candidate.json"), &json!({
                    "goal": goal, "status": "draft",
                    "stages": [
                        {"id": 1, "title": "first", "instructions": "append line one",
                         "acceptance": "file has line one", "commit": "feat: line one",
                         "status": "pending", "rounds": 0},
                        {"id": 2, "title": "second", "instructions": "append line two",
                         "acceptance": "file has line two", "commit": "feat: line two",
                         "status": "pending", "rounds": 0},
                    ],
                }))?;
            }
            "chat" => {
                #[cfg(test)]
                if let Some(output) = self.app.settings.lock().unwrap().get("mock_chat_output") {
                    // Null simulates an agent exiting without writing an answer.
                    if !output.is_null() {
                        fs::write(self.forge_path("answer.json"),
                            output.as_str().map(String::from).unwrap_or_else(|| output.to_string()))
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                fs::write(self.forge_path("answer.json"), json!({"answer": "mock answer"}).to_string())
                    .map_err(|e| e.to_string())?;
            }
            "implementer" | "fixer" => {
                #[cfg(test)]
                if let Some(edits) = self.app.settings.lock().unwrap()["mock_edits"].as_array_mut().filter(|a| !a.is_empty()).map(|a| a.remove(0)) {
                    for (path, content) in edits.as_object().ok_or("invalid mock edits")? {
                        fs::write(PathBuf::from(self.project()).join(path), content.as_str().unwrap()).map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(PathBuf::from(self.project()).join("mock.txt"))
                    .map_err(|e| e.to_string())?;
                let _ = writeln!(f, "work by {role}");
            }
            "reviewer" => {
                #[cfg(test)]
                if let Some(verdicts) = self.app.settings.lock().unwrap()["mock_verdicts"].as_array_mut()
                    && !verdicts.is_empty()
                {
                    let verdict = verdicts.remove(0);
                    return fs::write(
                        self.forge_path("verdict.json"),
                        verdict.as_str().map(String::from).unwrap_or_else(|| verdict.to_string()),
                    ).map_err(|e| e.to_string());
                }
                let _ = fs::write(
                    self.forge_path("verdict.json"),
                    json!({
                        "approved": true,
                        "summary": "Inspected the mock implementation; no issues found.",
                        "issues": [],
                    }).to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------ orchestrator

    pub(crate) fn plan_worker(&self, goal: &str, mode: &PlanMode) {
        let _worker = WorkerGuard(&self.session);
        self.plan_worker_inner(goal, mode);
    }

    /// Plan without releasing the worker's busy claim.
    fn plan_worker_inner(&self, goal: &str, mode: &PlanMode) -> bool {
        let prompt = match mode {
            PlanMode::Standard => PLANNER_PROMPT
                .replace("{goal}", goal)
                .replace("{plan_path}", &format!("{FORGE_DIR}/plan-candidate.json")),
            PlanMode::Refactor { focus } => fill_template(REFACTOR_PROMPT, &[
                ("{focus}", focus.as_str()), ("{plan_path}", ".forge/plan-candidate.json"),
            ]),
        };
        match self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, None, "ready"))
        {
            Ok(()) => true,
            Err(e) => {
                let blocked = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
                self.set_phase(if blocked { "blocked" } else { "failed" });
                self.log_event("error", &format!("planning failed: {e}"));
                false
            }
        }
    }

    pub(crate) fn revise_worker(&self, current_plan: &Value, feedback: &str) {
        let _worker = WorkerGuard(&self.session);
        let authoritative = self.load_plan().unwrap_or_else(|| current_plan.clone());
        let current_plan = &authoritative;
        let snapshot = serde_json::to_string_pretty(current_plan).unwrap();
        let goal = current_plan["goal"].as_str().unwrap_or("");
        let prompt = fill_template(REVISE_PROMPT, &[
            ("{current_plan}", snapshot.as_str()), ("{feedback}", feedback),
            ("{goal}", goal), ("{plan_path}", ".forge/plan-candidate.json"),
        ]);
        let result = self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, Some(current_plan), "revised"));
        if let Err(error) = result {
            let architecture_failed = self.session.state.lock().unwrap().architect_activity["status"] == "failed";
            self.set_phase(if architecture_failed { "blocked" } else { "plan_ready" });
            self.log_event("error", &format!("revision failed: {error}"));
        }
    }

    pub(crate) fn chat_worker(&self, current_plan: &Value, question: &str) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "answering plan question");
        let snapshot = serde_json::to_string_pretty(current_plan).unwrap();
        let history = serde_json::to_string_pretty(&self.read_chat()).unwrap();
        let prompt = fill_template(CHAT_PROMPT, &[
            ("{current_plan}", snapshot.as_str()), ("{history}", history.as_str()),
            ("{question}", question), ("{answer_path}", ".forge/answer.json"),
        ]);
        let result = (|| -> Result<(), String> {
            self.ensure_forge_dir();
            match fs::remove_file(self.forge_path("answer.json")) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("could not remove previous answer: {e}")),
            }
            let tool = self.setting("planner");
            let model = self.setting("planner_model");
            let context = if current_plan.get("architecture").is_some() { self.architecture_store().checkpoint(current_plan)? } else { crate::architecture::checkpoint_default() };
            let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: read-only Q&A in a fresh conversation. Do not write files or alter plan/decisions. Return ONLY {{\"answer\":\"your answer\"}}. Saved architecture context: {context}");
            let result = self.run_agent(&AgentRequest { role:"chat", provider:&tool, model:&model, effort:"provider_default", session:None, prompt:if tool == "mock" {&prompt} else {&readonly_prompt} })?;
            let text = if tool == "mock" { fs::read_to_string(self.forge_path("answer.json")).map_err(|e| format!("could not read answer file: {e}"))? } else { result.output };
            let output: Value = serde_json::from_str(&text)
                .map_err(|e| format!("invalid answer JSON: {e}"))?;
            let answer = output["answer"].as_str().filter(|answer| !answer.trim().is_empty())
                .ok_or_else(|| "agent did not produce a non-empty answer string".to_string())?;
            crate::architecture::atomic_json(&self.forge_path("answer.json"), &output)?;
            let user = json!({"role": "user", "text": question, "unix": unix_timestamp()});
            let assistant = json!({"role": "assistant", "text": answer, "unix": unix_timestamp()});
            let mut file = fs::OpenOptions::new().create(true).append(true)
                .open(self.forge_path("chat.jsonl")).map_err(|e| e.to_string())?;
            writeln!(file, "{user}\n{assistant}").map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            self.log_event("error", &format!("chat failed: {error}"));
        }
    }

    fn generate_plan(&self, prompt: &str) -> Result<Value, String> {
        let (tool, model, effort) = self.bootstrap("planner")?;
        self.ensure_forge_dir();
        let _ = fs::remove_file(self.forge_path("chat.jsonl"));
        let path = self.forge_path("plan-candidate.json");
        match fs::remove_file(&path) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not remove previous candidate: {e}")),
        }
        let prompt = format!("{prompt}\n{}", self.routing_prompt()?);
        let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: inspect only; do not write any files, implement, commit or push. Return the complete candidate plan as a JSON object in your final response, without markdown fences. Forge validates and writes the candidate file itself.");
        let result = self.run_agent(&AgentRequest { role:"planner", provider:&tool, model:&model, effort:&effort, session:None, prompt: if tool == "mock" {&prompt} else {&readonly_prompt} })?;
        let mut plan: Value = if tool == "mock" {
            serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| format!("invalid candidate: {e}"))?
        } else { serde_json::from_str(&result.output).map_err(|e| format!("invalid candidate: {e}"))? };
        plan["planner_selection_actor"] = json!({"provider":tool,"model":result.effective_model,"native_effort":effort});
        let usage = result.usage;
        if !plan["stages"].is_array() { return Err("planner did not produce valid stages".into()); }
        // Agent-authored usage may be an echo of the previous revision. Only
        // the invocation result contributes new planner usage.
        plan.as_object_mut().unwrap().remove("planner_usage");
        plan.as_object_mut().unwrap().remove("role_usage");
        if let Some(usage) = usage { accumulate_invocation_usage(&mut plan, "planner_usage", &tool, &usage); }
        if !plan["planner_usage"].is_null() { plan["role_usage"] = json!({"planner":plan["planner_usage"]}); }
        crate::architecture::atomic_json(&path, &plan)?;
        Ok(plan)
    }

    fn bind_planner_proposals(&self, plan: &mut Value) {
        for idx in 0..plan["stages"].as_array().unwrap().len() {
            if plan["stages"][idx]["status"] != "committed" && plan["stages"][idx]["model_proposal"].is_object() {
                plan["stages"][idx]["model_proposal_inputs"] = self.proposal_inputs(plan, idx);
                plan["stages"][idx]["model_proposer"] = plan["planner_selection_actor"].clone();
            }
        }
    }

    fn finalize_plan(&self, mut plan: Value, goal: &str, previous: Option<&Value>, action: &str)
        -> Result<(), String>
    {
        let stages = plan["stages"].as_array_mut().unwrap();
        for stage in stages.iter_mut() {
            if !stage.is_object() {
                return Err("planner produced a stage that is not an object".into());
            }
            // Planner output cannot forge execution records or relax user constraints.
            let saved_constraint = previous.and_then(|p| p["stages"].as_array()).and_then(|stages| stages.iter().find(|s| s["id"] == stage["id"]))
                .map(|s| s["model_constraint"].clone()).unwrap_or(Value::Null);
            stage.as_object_mut().unwrap().retain(|key, _| ["id", "title", "instructions", "acceptance", "commit", "depends_on", "model_proposal"].contains(&key.as_str()));
            if !saved_constraint.is_null() { stage["model_constraint"] = saved_constraint; }
            stage["status"] = json!("pending");
            stage["rounds"] = json!(0);
        }
        let committed: Vec<Value> = previous.into_iter().flat_map(|p| p["stages"].as_array().unwrap())
            .filter(|s| s["status"] == "committed").cloned().collect();
        // Restore originals even if the agent edited, reordered, duplicated or dropped them.
        stages.retain(|stage| !committed.iter().any(|original| original["id"] == stage["id"]));
        stages.splice(0..0, committed.iter().cloned());
        let n = stages.len();
        plan["goal"] = json!(goal);
        plan["status"] = json!("draft");
        if let Some(previous) = previous {
            let mut edited = crate::plan::edit_plan(previous, &json!({"plan": plan})).map_err(str::to_owned)?;
            for (idx, stage) in edited["stages"].as_array_mut().unwrap().iter_mut().enumerate() {
                if stage["status"] != "committed" { stage["model_proposal"] = plan["stages"][idx]["model_proposal"].clone(); }
            }
            if let Some(usage) = plan.get("planner_usage") {
                merge_planner_revision_totals(&mut edited, usage);
            }
            if !edited["planner_usage"].is_null() {
                if !edited["role_usage"].is_object() { edited["role_usage"] = json!({}); }
                edited["role_usage"]["planner"] = edited["planner_usage"].clone();
            }
            edited["planner_selection_actor"] = plan["planner_selection_actor"].clone();
            self.bind_planner_proposals(&mut edited);
            self.architect_publish(edited, Some(previous), "draft revision")?;
        } else {
            let base = json!({"stages": [], "goal": goal});
            let normalized = crate::plan::edit_plan(&base, &json!({"plan": plan})).map_err(str::to_owned)?;
            let proposals: Vec<Value> = plan["stages"].as_array().unwrap().iter().map(|s| s["model_proposal"].clone()).collect();
            plan["stages"] = normalized["stages"].clone();
            for (idx, proposal) in proposals.into_iter().enumerate() { plan["stages"][idx]["model_proposal"] = proposal; }
            plan["stage_id_high_water"] = normalized["stage_id_high_water"].clone();
            for key in ["plan_id", "revision", "architecture", "contract_version"] { plan.as_object_mut().unwrap().remove(key); }
            self.bind_planner_proposals(&mut plan);
            self.architect_publish(plan, None, "initial plan guidance")?;
        }
        self.set_phase("plan_ready");
        self.log_event("plan", &format!("plan {action} with {n} stages"));
        Ok(())
    }

    /// Caller holds queue_lock so API edits cannot overwrite worker transitions.
    pub(crate) fn set_queue_status(&self, queue: &mut Value, id: u64, status: &str) {
        if let Some(item) = queue["items"].as_array_mut().unwrap().iter_mut()
            .find(|item| item["id"].as_u64() == Some(id))
        {
            item["status"] = json!(status);
            self.save_queue(queue);
            self.log_event("queue", &format!("goal {id}: {status}"));
        }
    }

    /// Caller holds queue_lock and owns the busy claim.
    pub(crate) fn start_queue_run(&self, queue: &mut Value, id: u64, plan: &mut Value) -> Result<(), String> {
        *plan = self.architect_publish(plan.clone(), Some(plan), "queue approval")?;
        plan["status"] = json!("approved");
        self.save_plan(plan)?;
        self.set_queue_status(queue, id, "running");
        let mut s = self.session.state.lock().unwrap();
        s.goal = plan["goal"].as_str().unwrap_or("").to_string();
        s.phase = "running".into();
        s.run_started_unix = unix_timestamp();
        Ok(())
    }

    fn run_queue_item(&self, id: u64) -> bool {
        self.run_worker_core();
        let _queue_guard = self.session.queue_lock.lock().unwrap();
        let phase = self.session.state.lock().unwrap().phase.clone();
        // A user-stopped run retains its plan for human intervention.
        let status = match phase.as_str() {
            "done" => "done",
            "failed" => "failed",
            _ => "blocked",
        };
        let mut queue = self.load_queue();
        if status == "done" {
            queue["items"].as_array_mut().unwrap()
                .retain(|item| item["id"].as_u64() != Some(id));
            self.save_queue(&queue);
            self.log_event("queue", &format!("goal {id}: done — removed from queue"));
        } else {
            self.set_queue_status(&mut queue, id, status);
            self.session.queue_active.store(false, Ordering::SeqCst);
        }
        status == "done" && self.session.queue_active.load(Ordering::SeqCst)
    }

    /// Start fresh, or finish an explicitly approved item before taking the next.
    pub(crate) fn queue_worker(&self, approved: Option<u64>) {
        let _worker = WorkerGuard(&self.session);
        if let Some(id) = approved
            && !self.run_queue_item(id)
        {
            return;
        }
        loop {
            let (id, goal) = {
                let _queue_guard = self.session.queue_lock.lock().unwrap();
                if !self.session.queue_active.load(Ordering::SeqCst) {
                    return;
                }
                let mut queue = self.load_queue();
                let Some(item) = queue["items"].as_array().unwrap().iter()
                    .find(|item| item["status"] == "queued")
                else {
                    self.session.queue_active.store(false, Ordering::SeqCst);
                    self.log_event("queue", "queue complete");
                    return;
                };
                let id = item["id"].as_u64().unwrap();
                let goal = item["goal"].as_str().unwrap_or("").to_string();
                {
                    let mut s = self.session.state.lock().unwrap();
                    s.goal = goal.clone();
                    s.phase = "planning".into();
                }
                self.set_queue_status(&mut queue, id, "planning");
                (id, goal)
            };
            let planned = self.plan_worker_inner(&goal, &PlanMode::Standard);
            {
                let _queue_guard = self.session.queue_lock.lock().unwrap();
                let mut queue = self.load_queue();
                if !planned {
                    let blocked = self.session.state.lock().unwrap().phase == "blocked";
                    self.set_queue_status(&mut queue, id, if blocked { "blocked" } else { "failed" });
                    self.session.queue_active.store(false, Ordering::SeqCst);
                    return;
                }
                if !self.session.queue_active.load(Ordering::SeqCst) {
                    self.set_queue_status(&mut queue, id, "blocked");
                    return;
                }
                let auto_approve = self.app.settings.lock().unwrap()["queue_auto_approve"]
                    .as_bool().unwrap_or(false);
                if !auto_approve {
                    self.set_queue_status(&mut queue, id, "awaiting_approval");
                    return;
                }
                let mut plan = self.load_plan().unwrap();
                self.log_event("queue", &format!("goal {id}: automatically approved"));
                if let Err(error) = self.start_queue_run(&mut queue, id, &mut plan) {
                    self.set_phase("failed");
                    self.log_event("error", &error);
                    self.set_queue_status(&mut queue, id, "failed");
                    break;
                }
            }
            if !self.run_queue_item(id) {
                return;
            }
        }
    }

    /// The saved verdict keeps legacy fields; both agents get the same effective requests.
    fn review_requests(verdict: &Value) -> Vec<String> {
        let mut requests = Vec::new();
        for field in ["issues", "notes"] {
            for request in verdict[field].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if !requests.iter().any(|existing| existing == request) {
                    requests.push(request.to_string());
                }
            }
        }
        requests
    }

    fn stage_review_context(stage: &Value) -> String {
        let round = stage["rounds"].as_i64().unwrap_or(1);
        let kind = if round == 1 { "Initial review" } else { "Re-review" };
        let mut context = format!("REVIEW ROUND: {round} (current attempt) — {kind}.\n");
        let Some(verdict) = stage.get("last_verdict").filter(|v| v.is_object()
            && stage["context_valid"] != false && stage["last_verdict_valid"] != false) else {
            context.push_str("No previous review findings for this stage.\n");
            return context;
        };
        if round == 1 {
            context.push_str("The feedback below is from this stage's previous attempt; current-attempt numbering restarts at 1.\n");
        } else {
            context.push_str("The feedback below is from the immediately preceding review in this attempt.\n");
        }
        let requests = Self::review_requests(verdict);
        let decision = if verdict["approved"] == true && requests.is_empty() {
            "approved"
        } else {
            "changes requested"
        };
        context.push_str(&format!(
            "BEGIN PREVIOUS REVIEW CONTEXT\nEffective decision: {decision}\nSummary:\n{}\nOutstanding change requests (including legacy notes):\n",
            verdict["summary"].as_str().unwrap_or(""),
        ));
        for request in requests {
            context.push_str(&format!("- {request}\n"));
        }
        context.push_str("Previous checks (must be verified again):\n");
        for check in verdict["checks"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            context.push_str(&format!("- {check}\n"));
        }
        context.push_str("END PREVIOUS REVIEW CONTEXT\n");
        context
    }

    fn stage_prompt(&self, template: &str, plan: &Value, stage: &Value) -> Result<String, String> {
        let overview: String = plan["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                format!(
                    "  {}. {} -> {}\n",
                    s["id"], s["title"].as_str().unwrap_or(""), s["commit"].as_str().unwrap_or("")
                )
            })
            .collect();
        let mut prompt = fill_template(template, &[
            ("{goal}", plan["goal"].as_str().unwrap_or("")),
            ("{plan_overview}", &overview),
            ("{sid}", &stage["id"].to_string()),
            ("{title}", stage["title"].as_str().unwrap_or("")),
            ("{instructions}", stage["instructions"].as_str().unwrap_or("")),
            ("{acceptance}", stage["acceptance"].as_str().unwrap_or("")),
            ("{forge_dir}", FORGE_DIR),
            ("{verdict_path}", &format!("{FORGE_DIR}/verdict.json")),
            ("{review_context}", &Self::stage_review_context(stage)),
        ]);
        if template != REVIEW_PROMPT {
            let current = self.load_plan().ok_or("missing architectural plan")?;
            let cp = self.architecture_store().checkpoint(&current)?;
            let guidance = &cp["guidance"][stage["id"].to_string()];
            let idx = current["stages"].as_array().unwrap().iter().position(|s| s["id"] == stage["id"]).ok_or("missing guidance stage")?;
            if cp["context_status"] != "ready" || guidance["valid"] != true || guidance["relevant_inputs"] != crate::plan::stage_inputs(&current, idx) {
                return Err("required architectural guidance is missing or stale".into());
            }
            prompt.push_str(&format!("\nARCHITECT GUIDANCE:\n{}\nSaved constraints: {}\nCompleted interfaces: {}\nOutstanding risks: {}\nDecision history: .forge/architecture/{}/events.jsonl\n", guidance["text"].as_str().unwrap_or(""), cp["constraints"], cp["completed_interfaces"], cp["unresolved_risks"], current["plan_id"].as_str().unwrap_or("")));
            let completed: Vec<Value> = current["stages"].as_array().unwrap().iter().filter(|s| s["status"] == "committed")
                .map(|s| json!({"id":s["id"],"title":s["title"],"instructions":s["instructions"],"acceptance":s["acceptance"],"sha":s["sha"]})).collect();
            prompt.push_str(&format!("\nCompleted stage interfaces and verified outcomes: {}\nRecent execution outcomes: {}\nRead the decision history for relevant decisions omitted from the recent preview; inspect completed interfaces in code before changing them.\n", json!(completed), cp["execution_outcomes"]));
            prompt.push_str(&format!("\n[implementer] Last validated outcome/escalation request: {}\n[engine] Latest routing handoff: {}\n", stage["implementer_outcome"], stage["reassessment"]["history"].as_array().and_then(|h| h.last()).map(|h| json!({"kind":h["kind"],"evidence":h["evidence"]})).unwrap_or(Value::Null)));
            let worktree = self.git(&["status", "--short"])?;
            let diff = self.git(&["diff", "HEAD", "--", ".", ":(exclude).forge"])?;
            prompt.push_str(&format!("\nSAVED ARCHITECTURAL SUMMARY: {}\nRelevant decisions: {}\nWORKTREE: {}\nDIFF (bounded preview; inspect full staged/unstaged diff and all untracked contents yourself):\n{}\nOUTSTANDING FINDINGS:\n{}\nYou may be inheriting partial work from another agent. Inspect and preserve all existing changes, completed interfaces and accepted decisions before editing. Saved findings remain authoritative until resolved with evidence.\n", cp["summary"], cp["recent_decisions"], worktree, diff.chars().take(16000).collect::<String>(), Self::stage_review_context(stage)));

        }
        Ok(prompt)
    }

    /// Implement + independent review + bounded fix loop for one stage.
    fn run_one_stage(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let result = self.run_review_stage(plan, idx);
        if let Err(error) = &result {
            plan["stages"][idx]["review_gate"]["status"] = json!("error");
            plan["stages"][idx]["review_gate"]["error"] = json!(error);
            if error.contains("model routing blocked:") { plan["stages"][idx]["reassessment"]["status"] = json!("blocked"); plan["stages"][idx]["reassessment"]["error"] = json!(error); }
            plan["stages"][idx]["last_verdict_valid"] = json!(false);
            let stage = &plan["stages"][idx];
            let mut requests = Self::review_requests(&stage["previous_requests"]);
            for record in stage["reviews"].as_array().into_iter().flatten().filter(|r|
                r["attempt_id"] == stage["attempt_id"] && r["round"] == stage["rounds"]) {
                for request in Self::review_requests(record) {
                    requests.push(format!("[{}] {request}", record["role"].as_str().unwrap_or("reviewer")));
                }
            }
            requests.push(format!("[engine] {error}; re-verify the entire stage and all required checks"));
            plan["stages"][idx]["previous_requests"] = json!({"approved":false,"summary":"Review interrupted or invalid; prior unresolved findings retain authority","issues":requests,"notes":[],"checks":[]});
            if self.session.stop_requested.load(Ordering::SeqCst) {
                plan["stages"][idx]["review_gate"]["status"] = json!("interrupted");
                self.save_plan(plan)?;
                return Ok("stopped");
            }
            self.finish_stage(plan, idx, "blocked")?;
        }
        if matches!(result, Ok("stopped")) {
            plan["stages"][idx]["review_gate"]["status"] = json!("interrupted");
            plan["stages"][idx]["last_verdict_valid"] = json!(false);
            self.save_plan(plan)?;
        }
        result
    }

    pub(crate) fn run_worker(&self) {
        let _worker = WorkerGuard(&self.session);
        self.run_worker_core();
    }

    /// Run and record failures without releasing the worker's busy claim.
    fn run_worker_core(&self) {
        let result = self.run_worker_inner();
        if let Err(e) = result {
            self.set_phase(if e.contains("model routing blocked:") { "blocked" } else { "failed" });
            self.log_event("error", &format!("run failed: {e}"));
        }
        self.set_step(None, "");
        self.session.state.lock().unwrap().run_started_unix = 0;
    }

    fn run_worker_inner(&self) -> Result<(), String> {
        let loaded = self.load_plan().ok_or("no plan")?;
        let cp = if loaded["architecture"].is_object() { self.architecture_store().checkpoint(&loaded)? } else { Value::Null };
        let pending_turn = loaded["plan_id"].as_str().and_then(|id| fs::read(self.forge_path("architecture").join(id).join("architect-pending.json")).ok())
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap_or(json!({"turn":"invalid"})));
        let reusable = cp["context_status"] == "ready" && cp["session"]["resume_policy"] != "fork_from_checkpoint"
            && pending_turn.as_ref().is_none_or(|p| p["turn"] == cp["last_turn"])
            && loaded["stages"].as_array().into_iter().flatten().enumerate().all(|(idx,s)| s["status"] == "committed"
                || (cp["guidance"][s["id"].to_string()]["valid"] == true && cp["guidance"][s["id"].to_string()]["relevant_inputs"] == crate::plan::stage_inputs(&loaded,idx)));
        let mut plan = if reusable { loaded } else {
            self.architect_publish(loaded.clone(), Some(&loaded), "stage run").map_err(|e| format!("model routing blocked: {e}"))?
        };
        let count = plan["stages"].as_array().unwrap().len();
        for idx in 0..count {
            if plan["stages"][idx]["status"] == json!("committed") {
                continue;
            }
            if self.session.stop_requested.load(Ordering::SeqCst) {
                self.set_phase("plan_ready");
                self.log_event("run", "stopped by user; progress is saved, run again to continue");
                return Ok(());
            }
            let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
            let title = plan["stages"][idx]["title"].as_str().unwrap_or("").to_string();
            plan["stages"][idx]["status"] = json!("in_progress");
            plan["stages"][idx]["started_unix"] = json!(unix_timestamp());
            // A resumed stage starts a new attempt; completion timing is no longer current.
            let plan_revision = plan["revision"].clone();
            let stage = plan["stages"][idx].as_object_mut().unwrap();
            if stage.get("attempt_id").is_none() || stage.get("attempt_revision") != Some(&plan_revision) {
                stage.insert("attempt_id".into(), json!(crate::architecture::identity()));
                stage.insert("attempt_revision".into(), plan_revision);
                stage.insert("rounds".into(), json!(0));
                stage.insert("review_budget".into(), json!(self.app.settings.lock().unwrap()["max_fix_rounds"].as_i64().unwrap_or(3).max(0)));
                stage.insert("dual_promoted".into(), json!(false));
                stage.remove("attempt_head");
                stage.remove("reassessment");
                stage.remove("previous_requests");
            }
            stage.insert("review_gate".into(), json!({"status":"pending"}));
            stage.insert("last_verdict_valid".into(), json!(false));
            stage.remove("finished_unix");
            stage.remove("duration_secs");
            self.save_plan(&plan)?;
            self.set_step(Some(sid), "implementing");
            self.log_event("stage", &format!("stage {sid} started: {title}"));

            match self.run_one_stage(&mut plan, idx)? {
                "stopped" => {
                    self.set_phase("plan_ready");
                    self.log_event("run", "stopped by user; progress is saved, run again to continue");
                    return Ok(());
                }
                "configuration_blocked" => {
                    self.finish_stage(&mut plan, idx, "blocked")?;
                    self.set_phase("blocked");
                    self.log_event("stage", &format!("stage {sid} blocked: independent reviewer configuration requires correction"));
                    return Ok(());
                }
                "exhausted" => {
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "blocked")?);
                    self.set_phase("blocked");
                    self.log_event("stage", &format!(
                        "stage {sid} blocked after {duration}: required review gate is not clean after max fix rounds — needs a human"));
                    return Ok(());
                }
                _approved => {
                    let msg = plan["stages"][idx]["commit"].as_str().unwrap_or("forge: stage").to_string();
                    let sha = match self.commit_reviewed(&plan, idx, &msg) {
                        Ok(sha) => sha,
                        Err(error) => {
                            plan["stages"][idx]["review_gate"]["status"] = json!("invalidated");
                            plan["stages"][idx]["review_gate"]["error"] = json!(error);
                            plan["stages"][idx]["last_verdict_valid"] = json!(false);
                            self.finish_stage(&mut plan, idx, "blocked")?;
                            return Err(error);
                        }
                    };
                    if let Some(sha) = sha {
                        plan["stages"][idx]["sha"] = json!(sha);
                    }
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "committed")?);
                    self.log_event("stage", &format!("stage {sid} committed in {duration}"));
                }
            }
        }

        plan["status"] = json!("done");
        self.save_plan(&plan)?;
        if self.app.settings.lock().unwrap()["auto_push"].as_bool() == Some(true) {
            self.set_step(None, "pushing");
            match self.git(&["push", "-u", "origin", "HEAD"]) {
                Ok(out) => self.log_event("git", &format!("pushed to origin: {}",
                    if out.is_empty() { "ok" } else { &out })),
                Err(e) => self.log_event("error",
                    &format!("push failed (commits are safe locally): {e}")),
            }
        }
        self.set_phase("done");
        let started = self.session.state.lock().unwrap().run_started_unix;
        let now = unix_timestamp();
        let project = self.project();
        let published = self.architecture_store().load_raw()?.ok_or("missing completed plan")?;
        let architecture = self.architecture_store().summary(Some(&published))?;
        let report = crate::reports::completed_run_report(&plan, project, count, started, now, architecture);
        self.ensure_forge_dir();
        let mut f = fs::OpenOptions::new().create(true).append(true)
            .open(self.forge_path("reports.jsonl")).map_err(|e| format!("run report: {e}"))?;
        writeln!(f, "{report}").and_then(|_| f.sync_all())
            .and_then(|_| fs::File::open(self.forge_path("")).and_then(|dir| dir.sync_all()))
            .map_err(|e| format!("run report: {e}"))?;
        let text = crate::reports::completion_message(&report);
        self.log_event("run", &text);
        Ok(())
    }
}
