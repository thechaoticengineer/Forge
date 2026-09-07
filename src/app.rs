use crate::agent::{AgentUsage, stream_agent_output};
use crate::prompts::{CHAT_PROMPT, FIX_PROMPT, IMPLEMENT_PROMPT, PLANNER_PROMPT, REFACTOR_PROMPT, REVIEW_PROMPT, REVISE_PROMPT};
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
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub(crate) active_project: Mutex<String>,
    pub(crate) settings: Mutex<Value>,
    gh_cache: Mutex<Option<(Instant, Value, Option<String>)>>,
}

pub(crate) struct Session {
    pub(crate) state: Mutex<State>,
    pub(crate) queue_lock: Mutex<()>,
    pub(crate) persistence_lock: Mutex<()>,
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
    pub(crate) agent_started_unix: i64,
    pub(crate) agent_lines: i64,
    pub(crate) agent_last_line: String,
}

impl App {
    pub(crate) fn new(project: &str, settings: Value) -> Self {
        let app = Self {
            sessions: Mutex::new(HashMap::new()),
            active_project: Mutex::new(canonical_project(project)),
            settings: Mutex::new(settings),
            gh_cache: Mutex::new(None),
        };
        app.active_session();
        app
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
                    // Never resume the mutable tail of a session after a revision.
                    if !cp["session"].is_null() { cp["context_status"] = json!("needs_recovery"); }
                    for key in ["guidance", "agreements"] {
                        if let Some(records) = cp[key].as_object_mut() {
                            for (id, record) in records {
                                let valid = next["stages"].as_array().unwrap().iter().any(|s|
                                    s["id"].to_string() == *id && !affected.contains(&s["id"]));
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
                        record["id"] = json!(crate::architecture::identity());
                        record["policy"] = cp["review_policy"].clone();
                        record["stage_id"] = stage["id"].clone();
                        if record["role"].is_null() { record["role"] = json!("reviewer"); }
                        reviews.push(record);
                    }
                }
            }
            if old.as_ref() == Some(&next) { return Ok(next); }
            store.publish(next, cp, json!({"kind": kind, "reviews": reviews, "removed_stage_ids": removed, "invalidations": invalidations}))
        })();
        *self.session.persistence_error.lock().unwrap() = result.as_ref().err().cloned();
        result
    }

    fn add_usage(parent: &mut Value, key: &str, tool: &str, usage: &AgentUsage) {
        if usage.is_empty() {
            return;
        }
        if !parent.is_object() {
            *parent = json!({});
        }
        let target = &mut parent[key];
        if !target.is_object() {
            *target = json!({});
        }
        let totals = &mut target[tool];
        if !totals.is_object() {
            *totals = json!({});
        }
        for (field, amount) in [
            ("input_tokens", usage.input_tokens),
            ("output_tokens", usage.output_tokens),
            ("total_tokens", usage.total_tokens),
            ("calls", 1),
        ] {
            totals[field] = json!(totals[field].as_i64().unwrap_or(0) + amount);
        }
        if !totals["models"].is_object() {
            totals["models"] = json!({});
        }
        if !usage.model.is_empty() {
            let model_total = &mut totals["models"][&usage.model];
            *model_total = json!(model_total.as_i64().unwrap_or(0) + usage.total_tokens);
        }
    }

    fn record_stage_usage(&self, plan: &mut Value, idx: usize, tool: &str, usage: Option<AgentUsage>) -> Result<(), String> {
        if let Some(usage) = usage.filter(|usage| !usage.is_empty()) {
            Self::add_usage(&mut plan["stages"][idx], "usage", tool, &usage);
            Self::add_usage(plan, "usage", tool, &usage);
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

    fn commit_stage(&self, message: &str) -> Result<Option<String>, String> {
        // A pathspec naming FORGE_DIR makes `git add` fail when the project also
        // gitignores it, so stage everything and unstage FORGE_DIR instead.
        self.git(&["add", "-A"])?;
        self.git(&["reset", "-q", "--", FORGE_DIR])?;
        let exclude = format!(":(exclude){FORGE_DIR}");
        let dirty = self.git(&["status", "--porcelain", "--", ".", &exclude])?;
        if dirty.is_empty() {
            self.log_event("git", "nothing to commit for this stage");
            return Ok(None);
        }
        self.git(&["commit", "-m", message])?;
        let sha = self.git(&["rev-parse", "--short", "HEAD"])?;
        self.log_event("git", &format!("committed {sha}: {message}"));
        Ok(Some(sha))
    }

    // ---------------------------------------------------------- agents

    fn agent_command(tool: &str, prompt: &str, model: &str) -> Result<Command, String> {
        let cmd = match tool {
            "claude" => {
                let mut c = Command::new("claude");
                c.args([
                    "-p", prompt, "--dangerously-skip-permissions",
                    "--verbose", "--output-format", "stream-json",
                ]);
                if !model.is_empty() {
                    c.args(["--model", model]);
                }
                c
            }
            "codex" => {
                let mut c = Command::new("codex");
                c.args([
                    "exec",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--skip-git-repo-check",
                ]);
                if !model.is_empty() {
                    c.args(["-m", model]);
                }
                c.arg(prompt);
                c
            }
            other => return Err(format!("unknown tool {other}")),
        };
        Ok(cmd)
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

    fn run_agent(&self, role: &str, tool: &str, prompt: &str, model: &str)
        -> Result<Option<AgentUsage>, String>
    {
        if tool == "mock" {
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
            return Ok(usage);
        }
        let mut cmd = Self::agent_command(tool, prompt, model)?;
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

        let child = cmd
            .current_dir(self.project())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                self.clear_agent_activity();
                return Err(self.agent_error(role, format!("failed to launch {tool}: {e}")));
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_log = Arc::clone(&log);
        let (stdout_result, stderr_result, status_result) = std::thread::scope(|scope| {
            let stderr_reader = scope.spawn(|| {
                stream_agent_output(stderr, &stderr_log, &self.session.state, 1500, false)
                    .map(|(tail, _)| tail)
            });
            let stdout_result = stream_agent_output(stdout, &log, &self.session.state, 600, tool == "claude");
            let status_result = child.wait();
            let stderr_result = stderr_reader
                .join()
                .unwrap_or_else(|_| Err("agent stderr reader panicked".to_string()));
            (stdout_result, stderr_result, status_result)
        });
        let model = self.session.state.lock().unwrap().agent_model.clone();
        self.clear_agent_activity();

        let (tail, mut usage) = match stdout_result {
            Ok(result) => result,
            Err(e) => {
                self.agent_error(role, format!("{tool} output failed: {e}"));
                return Err(e);
            }
        };
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
            self.log_event("error", &format!("[{role}] {tool} failed: {etail}"));
            return Err(format!("{tool} exited with {status}: {etail}"));
        }
        if let Some(usage) = &mut usage
            && usage.model.is_empty()
        {
            usage.model = model;
        }
        self.log_agent_finished(role, tool, &tail, usage.as_ref());
        Ok(usage)
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
                self.set_phase("failed");
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
            self.set_phase("plan_ready");
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
            self.run_agent("chat", &self.setting("planner"), &prompt, &self.setting("planner_model"))?;
            let text = fs::read_to_string(self.forge_path("answer.json"))
                .map_err(|e| format!("could not read answer file: {e}"))?;
            let output: Value = serde_json::from_str(&text)
                .map_err(|e| format!("invalid answer JSON: {e}"))?;
            let answer = output["answer"].as_str().filter(|answer| !answer.trim().is_empty())
                .ok_or_else(|| "agent did not produce a non-empty answer string".to_string())?;
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
        let tool = self.setting("planner");
        self.ensure_forge_dir();
        let _ = fs::remove_file(self.forge_path("chat.jsonl"));
        let path = self.forge_path("plan-candidate.json");
        match fs::remove_file(&path) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(format!("could not remove previous candidate: {e}")),
        }
        let usage = self.run_agent("planner", &tool, prompt, &self.setting("planner_model"))?;
        let mut plan: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid candidate: {e}"))?;
        if !plan["stages"].is_array() { return Err("planner did not produce valid stages".into()); }
        // Agent-authored usage may be an echo of the previous revision. Only
        // the invocation result contributes new planner usage.
        plan.as_object_mut().unwrap().remove("planner_usage");
        if let Some(usage) = usage { Self::add_usage(&mut plan, "planner_usage", &tool, &usage); }
        Ok(plan)
    }

    fn finalize_plan(&self, mut plan: Value, goal: &str, previous: Option<&Value>, action: &str)
        -> Result<(), String>
    {
        let stages = plan["stages"].as_array_mut().unwrap();
        for stage in stages.iter_mut() {
            if !stage.is_object() {
                return Err("planner produced a stage that is not an object".into());
            }
            if stage.get("status").and_then(Value::as_str).is_none() {
                stage["status"] = json!("pending");
            }
            if stage.get("rounds").is_none() {
                stage["rounds"] = json!(0);
            }
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
            if let Some(usage) = plan.get("planner_usage") {
                // Preserve previous planning totals; the new invocation is additive.
                for (tool, value) in usage.as_object().into_iter().flatten() {
                    let target = &mut edited["planner_usage"];
                    if !target.is_object() { *target = json!({}); }
                    if !target[tool].is_object() { target[tool] = json!({}); }
                    for (key, count) in value.as_object().into_iter().flatten() {
                        if let Some(n) = count.as_i64() { target[tool][key] = json!(target[tool][key].as_i64().unwrap_or(0) + n); }
                        else if key == "models" {
                            if !target[tool][key].is_object() { target[tool][key] = json!({}); }
                            for (model, n) in count.as_object().into_iter().flatten() { target[tool][key][model] = json!(target[tool][key][model].as_i64().unwrap_or(0) + n.as_i64().unwrap_or(0)); }
                        }
                    }
                }
            }
            self.publish_plan(&edited, false)?;
        } else {
            let base = json!({"stages": [], "goal": goal});
            let normalized = crate::plan::edit_plan(&base, &json!({"plan": plan})).map_err(str::to_owned)?;
            plan["stages"] = normalized["stages"].clone();
            plan["stage_id_high_water"] = normalized["stage_id_high_water"].clone();
            self.publish_plan(&plan, true)?;
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
                    self.set_queue_status(&mut queue, id, "failed");
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

    fn stage_prompt(&self, template: &str, plan: &Value, stage: &Value) -> String {
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
        fill_template(template, &[
            ("{goal}", plan["goal"].as_str().unwrap_or("")),
            ("{plan_overview}", &overview),
            ("{sid}", &stage["id"].to_string()),
            ("{title}", stage["title"].as_str().unwrap_or("")),
            ("{instructions}", stage["instructions"].as_str().unwrap_or("")),
            ("{acceptance}", stage["acceptance"].as_str().unwrap_or("")),
            ("{forge_dir}", FORGE_DIR),
            ("{verdict_path}", &format!("{FORGE_DIR}/verdict.json")),
            ("{review_context}", &Self::stage_review_context(stage)),
        ])
    }

    /// Implement + independent review + bounded fix loop for one stage.
    fn run_one_stage(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let max_rounds = self.app.settings.lock().unwrap()["max_fix_rounds"]
            .as_i64()
            .unwrap_or(3);
        let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);

        for round in 0..=max_rounds {
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }
            plan["stages"][idx]["rounds"] = json!(round + 1);
            self.save_plan(plan)?;

            let stage = plan["stages"][idx].clone();
            let implementer = self.setting("implementer");
            let usage = match round {
                0 => {
                    self.set_step(Some(sid), "implementing");
                    let p = self.stage_prompt(IMPLEMENT_PROMPT, plan, &stage);
                    self.run_agent("implementer", &implementer, &p,
                                   &self.setting("implementer_model"))?
                }
                _ => {
                    self.set_step(Some(sid), &format!("fixing (round {round})"));
                    let p = self.stage_prompt(FIX_PROMPT, plan, &stage);
                    self.run_agent("fixer", &implementer, &p,
                                   &self.setting("implementer_model"))?
                }
            };
            self.record_stage_usage(plan, idx, &implementer, usage)?;
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }

            self.set_step(Some(sid), "reviewing");
            let verdict_path = self.forge_path("verdict.json");
            let _ = fs::remove_file(&verdict_path);
            let p = self.stage_prompt(REVIEW_PROMPT, plan, &stage);
            let reviewer = self.setting("reviewer");
            let usage = self.run_agent("reviewer", &reviewer, &p,
                                       &self.setting("reviewer_model"))?;
            self.record_stage_usage(plan, idx, &reviewer, usage)?;
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }

            let verdict: Option<Value> = fs::read_to_string(&verdict_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            let summary = verdict.as_ref()
                .and_then(|v| v["summary"].as_str()).unwrap_or("");
            let string_list = |field: &str| -> Vec<String> {
                verdict.as_ref().and_then(|v| v[field].as_array())
                    .map(|a| a.iter().filter_map(|i| i.as_str().map(String::from)).collect())
                    .unwrap_or_default()
            };
            let notes = string_list("notes");
            let checks = string_list("checks");
            let (approved, list) = match &verdict {
                Some(v) => {
                    let mut list = string_list("issues");
                    // Legacy notes are requested edits too, regardless of the incoming decision.
                    let approved = v["approved"].as_bool() == Some(true)
                        && list.is_empty() && notes.is_empty();
                    if !approved && list.is_empty() && notes.is_empty() {
                        list.push("reviewer rejected without details".into());
                    }
                    (approved, list)
                }
                None => (false, vec![
                    "The previous review session failed to produce a verdict. \
                     Re-verify the implementation end to end."
                        .into(),
                ]),
            };
            let plan_revision = plan["revision"].clone();
            let stage = &mut plan["stages"][idx];
            stage["last_verdict"] = json!({
                "approved": approved, "summary": summary, "issues": list,
                "notes": notes, "checks": checks,
            });
            stage["last_verdict_valid"] = json!(true);
            stage["context_valid"] = json!(true);
            let attempt = stage["attempt_id"].clone();
            let requests = Self::review_requests(&stage["last_verdict"]);
            stage.as_object_mut().unwrap().entry("reviews")
                .or_insert_with(|| json!([])).as_array_mut().unwrap().push(json!({
                    "role": "reviewer", "attempt_id": attempt, "revision": plan_revision,
                    "round": round + 1, "approved": approved, "summary": summary,
                    "issues": list, "notes": notes, "checks": checks, "unix": unix_timestamp(),
                }));
            self.save_plan(plan)?;

            if approved {
                let message = if summary.is_empty() {
                    format!("stage {sid} approved by reviewer")
                } else {
                    let summary: String = summary.chars().take(300).collect();
                    format!("stage {sid} approved: {summary}")
                };
                self.log_event("review", &message);
                return Ok("approved");
            }
            if verdict.is_none() {
                self.log_event("error", "reviewer produced no readable verdict; retrying stage");
            } else {
                let summary: String = requests.join("; ").chars().take(1500).collect();
                self.log_event("review", &format!("stage {sid} changes requested: {summary}"));
            }
        }
        Ok("exhausted")
    }

    pub(crate) fn run_worker(&self) {
        let _worker = WorkerGuard(&self.session);
        self.run_worker_core();
    }

    /// Run and record failures without releasing the worker's busy claim.
    fn run_worker_core(&self) {
        let result = self.run_worker_inner();
        if let Err(e) = result {
            self.set_phase("failed");
            self.log_event("error", &format!("run failed: {e}"));
        }
        self.set_step(None, "");
        self.session.state.lock().unwrap().run_started_unix = 0;
    }

    fn run_worker_inner(&self) -> Result<(), String> {
        let loaded = self.load_plan().ok_or("no plan")?;
        let mut plan = self.publish_plan(&loaded, false)?;
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
            let stage = plan["stages"][idx].as_object_mut().unwrap();
            stage.insert("attempt_id".into(), json!(crate::architecture::identity()));
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
                "exhausted" => {
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "blocked")?);
                    self.set_phase("blocked");
                    self.log_event("stage", &format!(
                        "stage {sid} blocked after {duration}: reviewer still rejecting after max fix rounds — needs a human"));
                    return Ok(());
                }
                _approved => {
                    let msg = plan["stages"][idx]["commit"].as_str().unwrap_or("forge: stage").to_string();
                    let sha = self.commit_stage(&msg)?;
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
        let duration_secs = if started > 0 { (now - started).max(0) } else { 0 };
        let commits: Vec<Value> = plan["stages"].as_array().unwrap().iter()
            .filter_map(|stage| stage.get("sha").map(|sha| json!({
                "sha": sha, "message": stage["commit"].as_str().unwrap_or("forge: stage"),
                "title": stage["title"],
            })))
            .collect();
        let mut report = json!({
            "unix": now, "goal": plan["goal"], "duration_secs": duration_secs,
            "stages": count, "commits": commits,
        });
        for key in ["usage", "planner_usage"] {
            if let Some(usage) = plan.get(key) {
                report[key] = usage.clone();
            }
        }
        self.ensure_forge_dir();
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true).append(true).open(self.forge_path("reports.jsonl"))
        {
            let _ = writeln!(f, "{report}");
        }
        let duration = fmt_duration(duration_secs);
        let mut text = format!("all stages committed — run complete in {duration}");
        if let Some(usage) = plan["usage"].as_object().filter(|usage| !usage.is_empty()) {
            let tokens = usage.iter().map(|(tool, usage)| {
                format!("{tool}: {} tokens", usage["total_tokens"].as_i64().unwrap_or(0))
            }).collect::<Vec<_>>().join(", ");
            let noun = if commits.len() == 1 { "commit" } else { "commits" };
            text.push_str(&format!(" — {} {noun} — {tokens}", commits.len()));
        }
        self.log_event("run", &text);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_accumulates_separate_tools_and_models_and_skips_empty_values() {
        let mut target = Value::Null;
        Ctx::add_usage(&mut target, "usage", "codex", &AgentUsage::default());
        assert!(target.is_null());
        target = json!({"goal": "Test goal"});
        Ctx::add_usage(&mut target, "usage", "codex", &AgentUsage::default());
        assert_eq!(target, json!({"goal": "Test goal"}));
        let mut usage = AgentUsage {
            input_tokens: 80, output_tokens: 20, total_tokens: 100, model: "model-a".into(),
        };
        Ctx::add_usage(&mut target, "usage", "codex", &usage);
        Ctx::add_usage(&mut target, "usage", "codex", &usage);
        usage.model = "model-b".into();
        Ctx::add_usage(&mut target, "usage", "codex", &usage);
        usage.model.clear();
        Ctx::add_usage(&mut target, "usage", "claude", &usage);
        Ctx::add_usage(&mut target, "usage", "unused", &AgentUsage::default());
        Ctx::add_usage(&mut target, "planner_usage", "codex", &AgentUsage::default());
        assert_eq!(target, json!({
            "goal": "Test goal",
            "usage": {
                "codex": {"input_tokens": 240, "output_tokens": 60, "total_tokens": 300,
                    "calls": 3, "models": {"model-a": 200, "model-b": 100}},
                "claude": {"input_tokens": 80, "output_tokens": 20, "total_tokens": 100,
                    "calls": 1, "models": {}},
            },
        }));
    }
}
