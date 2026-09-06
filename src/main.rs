//! Forge — a minimal wrapper around AI coding agents.
//!
//! Loop: plan (codex/claude) -> human approves -> per stage:
//! implement (tool A) -> independent review (tool B, fresh session) ->
//! bounded fix loop -> commit proposed message -> next stage -> push.
//!
//! Persistent goal queue: process goals sequentially with optional automatic approval.
//!
//! Serves a JSON API on 127.0.0.1:8734 for the Quickshell panel.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PORT: u16 = 8734;
const FORGE_DIR: &str = ".forge";

struct App {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    active_project: Mutex<String>,
    settings: Mutex<Value>,
    gh_cache: Mutex<Option<(Instant, Value, Option<String>)>>,
}

struct Session {
    state: Mutex<State>,
    queue_lock: Mutex<()>,
    stop_requested: AtomicBool,
    busy: AtomicBool,
    queue_active: AtomicBool,
}

/// A worker keeps this project and session even when the UI switches projects.
#[derive(Clone)]
struct Ctx {
    app: Arc<App>,
    project: String,
    session: Arc<Session>,
}

/// Each worker owns one session's busy claim, including while advancing its queue.
struct WorkerGuard<'a>(&'a Session);

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
struct State {
    phase: String, // idle|planning|plan_ready|running|blocked|done|failed
    goal: String,
    current_stage: Option<i64>,
    current_step: String,
    run_started_unix: i64,
    agent_role: String,
    agent_tool: String,
    agent_model: String,
    agent_started_unix: i64,
    agent_lines: i64,
    agent_last_line: String,
}

/// Resolve existing ancestors too, so a clone destination has a stable key
/// before its directory exists (including under a symlinked projects root).
fn canonical_project(path: &str) -> String {
    fn resolve(path: &std::path::Path) -> PathBuf {
        if let Ok(path) = fs::canonicalize(path) {
            return path;
        }
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => resolve(parent).join(name),
            _ => path.to_path_buf(),
        }
    }
    let path = PathBuf::from(path);
    let path = if path.is_absolute() { path } else {
        std::env::current_dir().unwrap().join(path)
    };
    resolve(&path).display().to_string()
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn clock_hms() -> String {
    Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn last_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(limit)).collect()
}

#[derive(Default)]
struct ClaudeActivity {
    lines: Vec<String>,
    result: Option<String>,
}

fn claude_activity(line: &str) -> ClaudeActivity {
    let event: Value = match serde_json::from_str(line) {
        Ok(event) => event,
        Err(_) => {
            return ClaudeActivity {
                lines: vec![line.to_string()],
                result: None,
            };
        }
    };
    let mut activity = ClaudeActivity::default();
    match event["type"].as_str() {
        Some("assistant") => {
            if let Some(contents) = event["message"]["content"].as_array() {
                for content in contents {
                    match content["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = content["text"].as_str() {
                                activity.lines.push(text.chars().take(400).collect());
                            }
                        }
                        Some("tool_use") => {
                            let name = content["name"].as_str().unwrap_or("tool");
                            let input = &content["input"];
                            let summary = ["file_path", "command", "pattern", "description", "url"]
                                .iter()
                                .find_map(|key| input.get(*key))
                                .unwrap_or(input);
                            let summary = summary
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| summary.to_string());
                            let summary: String = summary
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                .chars()
                                .take(160)
                                .collect();
                            activity.lines.push(format!("» {name}: {summary}"));
                        }
                        _ => {}
                    }
                }
            }
        }
        Some("result") => {
            activity.result = event["result"].as_str().map(str::to_string);
            let result = if event["is_error"].as_bool() == Some(true) {
                event["subtype"].as_str()
            } else {
                activity
                    .result
                    .as_deref()
                    .or_else(|| event["subtype"].as_str())
            };
            if let Some(result) = result {
                let result: String = result.chars().take(400).collect();
                activity.lines.push(format!("✔ result: {result}"));
            }
        }
        Some("system") if event["subtype"] == "init" => {
            let model = event["model"].as_str().unwrap_or("unknown");
            activity
                .lines
                .push(format!("session started (model {model})"));
        }
        _ => {}
    }
    activity
}

fn stream_agent_output<R: std::io::Read, W: std::io::Write>(
    input: R,
    log: &Arc<Mutex<W>>,
    state: &Mutex<State>,
    tail_limit: usize,
    claude: bool,
) -> Result<String, String> {
    let mut reader = BufReader::new(input);
    let mut bytes = Vec::new();
    let mut tail = String::new();
    let mut result_tail = None;

    loop {
        bytes.clear();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| format!("failed to read agent output: {e}"))?;
        if read == 0 {
            break;
        }

        let lossy = String::from_utf8_lossy(&bytes);
        let line = lossy.trim_end_matches(&['\r', '\n'][..]);
        // Retain raw output as a fallback when Claude never sends a result.
        tail.push_str(line);
        tail.push('\n');
        tail = last_chars(&tail, tail_limit);

        let lines = if claude {
            let activity = claude_activity(line);
            if let Some(result) = activity.result {
                result_tail = Some(last_chars(&result, tail_limit));
            }
            activity.lines
        } else {
            vec![line.to_string()]
        };
        for line in lines {
            {
                let mut file = log
                    .lock()
                    .map_err(|_| "agent log lock was poisoned".to_string())?;
                writeln!(file, "{line}").map_err(|e| format!("failed to write agent log: {e}"))?;
                file.flush()
                    .map_err(|e| format!("failed to flush agent log: {e}"))?;
            }

            let mut s = state.lock().unwrap();
            s.agent_lines += 1;
            let latest = line.trim();
            if !latest.is_empty() {
                s.agent_last_line = latest.chars().take(200).collect();
            }
        }
    }

    Ok(result_tail.unwrap_or(tail).trim().to_string())
}

fn default_settings() -> Value {
    json!({
        "projects_root": "",
        "planner": "claude",
        "implementer": "codex",
        "reviewer": "claude",
        "planner_model": "",
        "implementer_model": "",
        "reviewer_model": "",
        "max_fix_rounds": 3,
        "auto_push": true,
        "queue_auto_approve": false,
    })
}

/// Build a replacement only after validation, leaving the saved plan untouched on errors.
fn edit_plan(plan: &Value, body: &Value) -> Result<Value, &'static str> {
    let old_stages = plan["stages"].as_array().ok_or("no plan")?;
    let incoming = body.get("plan").filter(|value| value.is_object())
        .ok_or("plan must be an object")?;
    let stages = incoming["stages"].as_array().filter(|stages| !stages.is_empty())
        .ok_or("stages must be a non-empty array")?;
    let mut submitted_ids = Vec::new();
    for stage in stages {
        for field in ["title", "instructions", "acceptance", "commit"] {
            let text = stage[field].as_str().ok_or("stage content fields must be strings")?;
            if matches!(field, "title" | "instructions") && text.trim().is_empty() {
                return Err("stage title and instructions must not be blank");
            }
        }
        if let Some(id) = stage.get("id") {
            if submitted_ids.contains(&id) {
                return Err("duplicate stage ids");
            }
            submitted_ids.push(id);
        }
    }

    let committed: Vec<&Value> = old_stages.iter()
        .filter(|stage| stage["status"] == "committed").collect();
    for stage in &committed {
        if !submitted_ids.contains(&&stage["id"]) {
            return Err("cannot remove a committed stage");
        }
    }
    // Completed work remains a prefix in its original order; editable stages may move freely.
    for (index, stage) in committed.iter().enumerate() {
        if stages[index].get("id") != stage.get("id") {
            return Err("cannot reorder committed stages");
        }
    }

    let valid_id = |stage: &Value| stage["id"].as_i64().filter(|id| *id > 0);
    let mut next_id = old_stages.iter().filter_map(valid_id).max().unwrap_or(0);
    let mut used_ids: Vec<i64> = stages.iter().filter_map(valid_id).collect();
    let mut edited_stages = Vec::with_capacity(stages.len());
    for (index, stage) in stages.iter().enumerate() {
        if index < committed.len() {
            edited_stages.push(committed[index].clone());
            continue;
        }
        let id = match valid_id(stage) {
            Some(id) => id,
            None => loop {
                next_id = next_id.checked_add(1).ok_or("stage id limit reached")?;
                if !used_ids.contains(&next_id) {
                    used_ids.push(next_id);
                    break next_id;
                }
            },
        };
        // Only editable content is accepted; old reviews and execution metadata are discarded.
        edited_stages.push(json!({
            "id": id,
            "title": stage["title"],
            "instructions": stage["instructions"],
            "acceptance": stage["acceptance"],
            "commit": stage["commit"],
            "status": "pending",
            "rounds": 0,
        }));
    }
    let mut edited = plan.clone();
    if let Some(goal) = incoming["goal"].as_str().filter(|goal| !goal.trim().is_empty()) {
        edited["goal"] = json!(goal);
    }
    edited["stages"] = json!(edited_stages);
    edited["status"] = json!("draft");
    Ok(edited)
}

fn mutate_queue(queue: &mut Value, action: &str, body: &Value) -> Result<String, &'static str> {
    let items = queue["items"].as_array_mut().ok_or("invalid queue")?;
    match action {
        "add" => {
            let goal = body["goal"].as_str().unwrap_or("").trim();
            if goal.is_empty() {
                return Err("goal required");
            }
            let id = items.iter().filter_map(|item| item["id"].as_u64())
                .max().unwrap_or(0).checked_add(1).ok_or("queue id limit reached")?;
            items.push(json!({
                "id": id,
                "goal": goal,
                "status": "queued",
                "added_unix": unix_timestamp(),
            }));
            Ok(format!("added goal {id}"))
        }
        "remove" | "move" => {
            let id = body["id"].as_u64().ok_or("id required")?;
            let idx = items.iter().position(|item| item["id"].as_u64() == Some(id))
                .ok_or("queue item not found")?;
            if items[idx]["status"] != "queued" {
                return Err("item is not queued");
            }
            if action == "remove" {
                items.remove(idx);
                return Ok(format!("removed goal {id}"));
            }
            let dir = body["dir"].as_str().unwrap_or("");
            let neighbor = match dir {
                "up" => (0..idx).rev().find(|&i| items[i]["status"] == "queued"),
                "down" => (idx + 1..items.len()).find(|&i| items[i]["status"] == "queued"),
                _ => return Err("dir must be up or down"),
            };
            if let Some(neighbor) = neighbor {
                items.swap(idx, neighbor);
            }
            Ok(format!("moved goal {id} {dir}"))
        }
        "clear" => {
            let before = items.len();
            items.retain(|item| item["status"] != "queued");
            Ok(format!("cleared {} queued goals", before - items.len()))
        }
        _ => Err("unknown queue action"),
    }
}

impl App {
    fn new(project: &str, settings: Value) -> Self {
        let app = Self {
            sessions: Mutex::new(HashMap::new()),
            active_project: Mutex::new(canonical_project(project)),
            settings: Mutex::new(settings),
            gh_cache: Mutex::new(None),
        };
        app.active_session();
        app
    }

    fn session(&self, project: &str) -> Arc<Session> {
        let project = canonical_project(project);
        if let Some(session) = self.sessions.lock().unwrap().get(&project).cloned() {
            return session;
        }
        // Read persisted state before taking the map lock. No session state
        // or queue lock may be held while the sessions map is locked.
        let plan = fs::read_to_string(PathBuf::from(&project).join(FORGE_DIR).join("plan.json"))
            .ok().and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|plan| plan.get("stages").is_some_and(Value::is_array));
        let session = Arc::new(Session {
            state: Mutex::new(State {
                phase: if plan.is_some() { "plan_ready" } else { "idle" }.into(),
                goal: plan.as_ref().and_then(|plan| plan["goal"].as_str())
                    .unwrap_or("").to_string(),
                ..State::default()
            }),
            queue_lock: Mutex::new(()),
            stop_requested: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            queue_active: AtomicBool::new(false),
        });
        self.sessions.lock().unwrap().entry(project).or_insert(session).clone()
    }

    fn active_session(&self) -> Arc<Session> {
        let project = self.active_project.lock().unwrap().clone();
        self.session(&project)
    }

    fn context(self: &Arc<Self>, project: &str) -> Ctx {
        let project = canonical_project(project);
        let session = self.session(&project);
        Ctx { app: Arc::clone(self), project, session }
    }

    fn setting(&self, key: &str) -> String {
        self.settings.lock().unwrap()[key].as_str().unwrap_or("").to_string()
    }

    /// Selection never changes or waits for an existing session's work.
    fn set_project(&self, path: &str) -> Result<(), String> {
        if !PathBuf::from(path).join(".git").exists() {
            return Err(format!("{path} is not a git repository"));
        }
        let project = canonical_project(path);
        self.session(&project);
        *self.active_project.lock().unwrap() = project;
        Ok(())
    }

    fn open_sessions(&self) -> Vec<(String, Arc<Session>)> {
        self.sessions.lock().unwrap().iter()
            .map(|(project, session)| (project.clone(), Arc::clone(session))).collect()
    }

    fn any_busy(&self) -> bool {
        self.open_sessions().iter().any(|(_, session)| {
            session.busy.load(Ordering::SeqCst) || session.queue_active.load(Ordering::SeqCst)
        })
    }

    fn session_summaries(self: &Arc<Self>, active: &str) -> Value {
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
    fn remote_repos(&self, local_names: &[String]) -> (Value, Option<String>) {
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
    fn project(&self) -> &str {
        &self.project
    }

    fn forge_path(&self, name: &str) -> PathBuf {
        PathBuf::from(self.project()).join(FORGE_DIR).join(name)
    }

    fn ensure_forge_dir(&self) {
        let _ = fs::create_dir_all(self.forge_path(""));
    }

    fn log_event(&self, kind: &str, text: &str) {
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
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.forge_path("history.jsonl"))
        {
            let _ = writeln!(f, "{entry}");
        }
        println!("[{kind}] {text}");
    }

    fn read_history(&self) -> Value {
        let text = fs::read_to_string(self.forge_path("history.jsonl")).unwrap_or_default();
        let items: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let skip = items.len().saturating_sub(400);
        Value::Array(items.into_iter().skip(skip).collect())
    }

    fn load_plan(&self) -> Option<Value> {
        let text = fs::read_to_string(self.forge_path("plan.json")).ok()?;
        let plan: Value = serde_json::from_str(&text).ok()?;
        plan.get("stages")?.as_array()?;
        Some(plan)
    }

    fn save_plan(&self, plan: &Value) {
        self.ensure_forge_dir();
        let _ = fs::write(
            self.forge_path("plan.json"),
            serde_json::to_string_pretty(plan).unwrap(),
        );
    }

    fn load_queue(&self) -> Value {
        fs::read_to_string(self.forge_path("queue.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|queue| queue.get("items").is_some_and(Value::is_array))
            .unwrap_or_else(|| json!({"items": []}))
    }

    fn save_queue(&self, queue: &Value) {
        self.ensure_forge_dir();
        let _ = fs::write(
            self.forge_path("queue.json"),
            serde_json::to_string_pretty(queue).unwrap(),
        );
    }

    fn finish_stage(&self, plan: &mut Value, idx: usize, status: &str) -> i64 {
        let stage = &mut plan["stages"][idx];
        let finished = unix_timestamp();
        let started = stage["started_unix"].as_i64().unwrap_or(finished);
        stage["status"] = json!(status);
        stage["finished_unix"] = json!(finished);
        stage["duration_secs"] = json!(finished - started);
        self.save_plan(plan);
        finished - started
    }

    fn set_phase(&self, phase: &str) {
        self.session.state.lock().unwrap().phase = phase.to_string();
    }

    fn set_step(&self, stage: Option<i64>, step: &str) {
        let mut s = self.session.state.lock().unwrap();
        s.current_stage = stage;
        s.current_step = step.to_string();
    }

    fn setting(&self, key: &str) -> String {
        self.app.setting(key)
    }

    /// Atomically claim the busy flag; Err means other work is in flight.
    /// Prevents two requests that both saw busy=false from racing to spawn.
    fn acquire_busy(&self) -> Result<(), ()> {
        self.session.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| ())
    }

    // ------------------------------------------------------------ git

    fn git(&self, args: &[&str]) -> Result<String, String> {
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

    fn run_agent(&self, role: &str, tool: &str, prompt: &str, model: &str) -> Result<(), String> {
        if tool == "mock" {
            #[cfg(test)]
            if role == "planner" {
                let mut settings = self.app.settings.lock().unwrap();
                settings["mock_planner_prompt"] = json!(prompt);
                settings["mock_planner_phase"] = json!(self.session.state.lock().unwrap().phase);
                settings["mock_planner_had_plan"] = json!(self.forge_path("plan.json").exists());
            }
            return self.mock_agent(role);
        }
        let mut cmd = match tool {
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
        self.log_event("agent", &format!("[{role}] starting {tool} session"));
        let log = match fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.forge_path("agent.log"))
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(
                    file,
                    "=== [{role}] {tool} ({model}) started {} ===",
                    clock_hms()
                )
                .and_then(|_| file.flush())
                {
                    let message = format!("failed to initialize agent log: {e}");
                    self.log_event("error", &format!("[{role}] {message}"));
                    return Err(message);
                }
                Arc::new(Mutex::new(file))
            }
            Err(e) => {
                let message = format!("failed to initialize agent log: {e}");
                self.log_event("error", &format!("[{role}] {message}"));
                return Err(message);
            }
        };

        {
            let mut s = self.session.state.lock().unwrap();
            s.agent_role = role.to_string();
            s.agent_tool = tool.to_string();
            s.agent_model = model.to_string();
            s.agent_started_unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
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
                let message = format!("failed to launch {tool}: {e}");
                self.log_event("error", &format!("[{role}] {message}"));
                return Err(message);
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_log = Arc::clone(&log);
        let (stdout_result, stderr_result, status_result) = std::thread::scope(|scope| {
            let stderr_reader = scope.spawn(|| {
                stream_agent_output(stderr, &stderr_log, &self.session.state, 1500, false)
            });
            let stdout_result = stream_agent_output(stdout, &log, &self.session.state, 600, tool == "claude");
            let status_result = child.wait();
            let stderr_result = stderr_reader
                .join()
                .unwrap_or_else(|_| Err("agent stderr reader panicked".to_string()));
            (stdout_result, stderr_result, status_result)
        });
        self.clear_agent_activity();

        let tail = match stdout_result {
            Ok(tail) => tail,
            Err(e) => {
                self.log_event("error", &format!("[{role}] {tool} output failed: {e}"));
                return Err(e);
            }
        };
        let etail = match stderr_result {
            Ok(tail) => tail,
            Err(e) => {
                self.log_event("error", &format!("[{role}] {tool} output failed: {e}"));
                return Err(e);
            }
        };
        let status = match status_result {
            Ok(status) => status,
            Err(e) => {
                let message = format!("failed to wait for {tool}: {e}");
                self.log_event("error", &format!("[{role}] {message}"));
                return Err(message);
            }
        };

        if !status.success() {
            self.log_event("error", &format!("[{role}] {tool} failed: {etail}"));
            return Err(format!("{tool} exited with {status}: {etail}"));
        }
        self.log_event("agent", &format!("[{role}] {tool} finished: {tail}"));
        Ok(())
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
    fn mock_agent(&self, role: &str) -> Result<(), String> {
        match role {
            "planner" => {
                #[cfg(test)]
                if let Some(output) = self.app.settings.lock().unwrap().get("mock_plan_output") {
                    // Null simulates an agent exiting successfully without writing a plan.
                    if !output.is_null() {
                        fs::write(self.forge_path("plan.json"),
                            output.as_str().map(String::from).unwrap_or_else(|| output.to_string()))
                            .map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                let goal = self.session.state.lock().unwrap().goal.clone();
                self.save_plan(&json!({
                    "goal": goal, "status": "draft",
                    "stages": [
                        {"id": 1, "title": "first", "instructions": "append line one",
                         "acceptance": "file has line one", "commit": "feat: line one",
                         "status": "pending", "rounds": 0},
                        {"id": 2, "title": "second", "instructions": "append line two",
                         "acceptance": "file has line two", "commit": "feat: line two",
                         "status": "pending", "rounds": 0},
                    ],
                }));
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

    fn plan_worker(&self, goal: &str) {
        let _worker = WorkerGuard(&self.session);
        self.plan_worker_inner(goal);
    }

    /// Plan without releasing the worker's busy claim.
    fn plan_worker_inner(&self, goal: &str) -> bool {
        let prompt = PLANNER_PROMPT
            .replace("{goal}", goal)
            .replace("{plan_path}", &format!("{FORGE_DIR}/plan.json"));
        match self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, &[], "ready"))
        {
            Ok(()) => true,
            Err(e) => {
                self.set_phase("failed");
                self.log_event("error", &format!("planning failed: {e}"));
                false
            }
        }
    }

    fn revise_worker(&self, current_plan: &Value, feedback: &str) {
        let _worker = WorkerGuard(&self.session);
        let snapshot = serde_json::to_string_pretty(current_plan).unwrap();
        let goal = current_plan["goal"].as_str().unwrap_or("");
        let committed: Vec<Value> = current_plan["stages"].as_array().unwrap().iter()
            .filter(|stage| stage["status"] == "committed").cloned().collect();
        // Substitute template segments once so placeholders in user content stay literal.
        let prompt: String = REVISE_PROMPT.split_inclusive('}').map(|part| {
            for (key, value) in [
                ("{current_plan}", snapshot.as_str()), ("{feedback}", feedback),
                ("{goal}", goal), ("{plan_path}", ".forge/plan.json"),
            ] {
                if let Some(prefix) = part.strip_suffix(key) {
                    return format!("{prefix}{value}");
                }
            }
            part.to_string()
        }).collect();
        let result = self.generate_plan(&prompt)
            .and_then(|plan| self.finalize_plan(plan, goal, &committed, "revised"));
        if let Err(mut error) = result {
            if let Err(e) = fs::write(self.forge_path("plan.json"), snapshot) {
                error.push_str(&format!("; could not restore previous plan: {e}"));
            }
            self.set_phase("plan_ready");
            self.log_event("error", &format!("revision failed: {error}"));
        }
    }

    fn generate_plan(&self, prompt: &str) -> Result<Value, String> {
        // A previous goal's plan must never count as the new planner's output.
        let path = self.forge_path("plan.json");
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not remove previous plan: {e}")),
        }
        .and_then(|_| self.run_agent(
            "planner", &self.setting("planner"), prompt, &self.setting("planner_model"),
        ))
        .and_then(|_| {
            self.load_plan()
                .ok_or_else(|| "planner did not produce a valid plan file".to_string())
        })
    }

    fn finalize_plan(&self, mut plan: Value, goal: &str, committed: &[Value], action: &str)
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
        // Restore originals even if the agent edited, reordered, duplicated or dropped them.
        stages.retain(|stage| !committed.iter().any(|original| original["id"] == stage["id"]));
        stages.splice(0..0, committed.iter().cloned());
        let n = stages.len();
        plan["goal"] = json!(goal);
        plan["status"] = json!("draft");
        fs::write(self.forge_path("plan.json"), serde_json::to_string_pretty(&plan).unwrap())
            .map_err(|e| format!("could not save plan: {e}"))?;
        self.set_phase("plan_ready");
        self.log_event("plan", &format!("plan {action} with {n} stages"));
        Ok(())
    }

    /// Caller holds queue_lock so API edits cannot overwrite worker transitions.
    fn set_queue_status(&self, queue: &mut Value, id: u64, status: &str) {
        if let Some(item) = queue["items"].as_array_mut().unwrap().iter_mut()
            .find(|item| item["id"].as_u64() == Some(id))
        {
            item["status"] = json!(status);
            self.save_queue(queue);
            self.log_event("queue", &format!("goal {id}: {status}"));
        }
    }

    /// Caller holds queue_lock and owns the busy claim.
    fn start_queue_run(&self, queue: &mut Value, id: u64, plan: &mut Value) {
        plan["status"] = json!("approved");
        self.save_plan(plan);
        self.set_queue_status(queue, id, "running");
        let mut s = self.session.state.lock().unwrap();
        s.goal = plan["goal"].as_str().unwrap_or("").to_string();
        s.phase = "running".into();
        s.run_started_unix = unix_timestamp();
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
        self.set_queue_status(&mut self.load_queue(), id, status);
        if status != "done" {
            self.session.queue_active.store(false, Ordering::SeqCst);
        }
        status == "done" && self.session.queue_active.load(Ordering::SeqCst)
    }

    /// Start fresh, or finish an explicitly approved item before taking the next.
    fn queue_worker(&self, approved: Option<u64>) {
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
            let planned = self.plan_worker_inner(&goal);
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
                self.start_queue_run(&mut queue, id, &mut plan);
            }
            if !self.run_queue_item(id) {
                return;
            }
        }
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
        template
            .replace("{goal}", plan["goal"].as_str().unwrap_or(""))
            .replace("{plan_overview}", &overview)
            .replace("{sid}", &stage["id"].to_string())
            .replace("{title}", stage["title"].as_str().unwrap_or(""))
            .replace("{instructions}", stage["instructions"].as_str().unwrap_or(""))
            .replace("{acceptance}", stage["acceptance"].as_str().unwrap_or(""))
            .replace("{forge_dir}", FORGE_DIR)
            .replace("{verdict_path}", &format!("{FORGE_DIR}/verdict.json"))
    }

    /// Implement + independent review + bounded fix loop for one stage.
    fn run_one_stage(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let max_rounds = self.app.settings.lock().unwrap()["max_fix_rounds"]
            .as_i64()
            .unwrap_or(3);
        let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
        let mut issues: Option<Vec<String>> = None;

        for round in 0..=max_rounds {
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }
            plan["stages"][idx]["rounds"] = json!(round + 1);
            self.save_plan(plan);

            let stage = plan["stages"][idx].clone();
            match &issues {
                None => {
                    self.set_step(Some(sid), "implementing");
                    let p = self.stage_prompt(IMPLEMENT_PROMPT, plan, &stage);
                    self.run_agent("implementer", &self.setting("implementer"), &p,
                                   &self.setting("implementer_model"))?;
                }
                Some(list) => {
                    self.set_step(Some(sid), &format!("fixing (round {round})"));
                    let joined: String = list.iter().map(|i| format!("- {i}\n")).collect();
                    let p = self
                        .stage_prompt(FIX_PROMPT, plan, &stage)
                        .replace("{issues}", &joined);
                    self.run_agent("fixer", &self.setting("implementer"), &p,
                                   &self.setting("implementer_model"))?;
                }
            }
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }

            self.set_step(Some(sid), "reviewing");
            let verdict_path = self.forge_path("verdict.json");
            let _ = fs::remove_file(&verdict_path);
            let p = self.stage_prompt(REVIEW_PROMPT, plan, &stage);
            self.run_agent("reviewer", &self.setting("reviewer"), &p,
                           &self.setting("reviewer_model"))?;
            if self.session.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }

            let verdict: Option<Value> = fs::read_to_string(&verdict_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            let summary = verdict.as_ref()
                .and_then(|v| v["summary"].as_str()).unwrap_or("");
            let (approved, list) = match &verdict {
                Some(v) => {
                    let approved = v["approved"].as_bool() == Some(true);
                    let list: Vec<String> = v["issues"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|i| i.as_str().map(String::from))
                                .collect()
                        })
                        .filter(|l: &Vec<String>| approved || !l.is_empty())
                        .or_else(|| approved.then(Vec::new))
                        .unwrap_or_else(|| vec!["reviewer rejected without details".into()]);
                    (approved, list)
                }
                None => (false, vec![
                    "The previous review session failed to produce a verdict. \
                     Re-verify the implementation end to end."
                        .into(),
                ]),
            };
            let stage = &mut plan["stages"][idx];
            stage["last_verdict"] = json!({
                "approved": approved, "summary": summary, "issues": list,
            });
            stage.as_object_mut().unwrap().entry("reviews")
                .or_insert_with(|| json!([])).as_array_mut().unwrap().push(json!({
                    "round": round + 1, "approved": approved, "summary": summary,
                    "issues": list, "unix": unix_timestamp(),
                }));
            self.save_plan(plan);

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
                let summary: String = list.join("; ").chars().take(1500).collect();
                self.log_event("review", &format!("stage {sid} rejected: {summary}"));
            }
            issues = Some(list);
        }
        Ok("exhausted")
    }

    fn run_worker(&self) {
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
        let mut plan = self.load_plan().ok_or("no plan")?;
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
            stage.remove("finished_unix");
            stage.remove("duration_secs");
            self.save_plan(&plan);
            self.set_step(Some(sid), "implementing");
            self.log_event("stage", &format!("stage {sid} started: {title}"));

            match self.run_one_stage(&mut plan, idx)? {
                "stopped" => {
                    self.set_phase("plan_ready");
                    self.log_event("run", "stopped by user; progress is saved, run again to continue");
                    return Ok(());
                }
                "exhausted" => {
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "blocked"));
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
                    let duration = fmt_duration(self.finish_stage(&mut plan, idx, "committed"));
                    self.log_event("stage", &format!("stage {sid} committed in {duration}"));
                }
            }
        }

        plan["status"] = json!("done");
        self.save_plan(&plan);
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
        let duration = fmt_duration(if started > 0 { unix_timestamp() - started } else { 0 });
        self.log_event("run", &format!("all stages committed — run complete in {duration}"));
        Ok(())
    }
}

// ------------------------------------------------------------- prompts

const PLANNER_PROMPT: &str = r#"You are the planning agent of Forge, an AI build orchestrator.
Explore this repository, then produce an implementation plan for the goal below.

GOAL:
{goal}

Write the plan as JSON to the file {plan_path} (create the directory if needed) with exactly this schema:
{"goal": "...", "status": "draft", "stages": [
  {"id": 1, "title": "short title",
   "instructions": "complete, self-contained instructions for an implementing agent that has NOT seen this conversation",
   "acceptance": "concrete acceptance criteria",
   "commit": "proposed conventional commit message",
   "status": "pending", "rounds": 0}
]}

Rules: 2 to 8 stages, each independently committable, ordered by dependency.
Do NOT implement anything, do not modify any other file. Only write {plan_path}."#;

const REVISE_PROMPT: &str = r#"You are the planning agent of Forge, an AI build orchestrator.
You are revising an existing draft plan for this repository.

Here is the current plan JSON:
{current_plan}

Here is the user's feedback about what is wrong or should be improved:
{feedback}

Keep the same overall goal:
{goal}

Rewrite the plan and write it as JSON to the file {plan_path} (create the directory if needed) with exactly this schema:
{"goal": "...", "status": "draft", "stages": [
  {"id": 1, "title": "short title",
   "instructions": "complete, self-contained instructions for an implementing agent that has NOT seen this conversation",
   "acceptance": "concrete acceptance criteria",
   "commit": "proposed conventional commit message",
   "status": "pending", "rounds": 0}
]}

Stages whose status is "committed" are already done and MUST be kept exactly as-is at the start of the plan, in their original relative order (same id, title, instructions, acceptance, commit, status, rounds).
Apply the feedback to the remaining stages: you may rewrite, merge, split, add, remove, or reorder them.
Rules: 2 to 8 stages total, each independently committable, ordered by dependency.
Do NOT implement anything and do NOT modify any other file. Only write {plan_path}."#;

const IMPLEMENT_PROMPT: &str = r#"You are the implementing agent of Forge for exactly one stage of an approved plan.

OVERALL GOAL:
{goal}

FULL PLAN (context only — do NOT work on other stages):
{plan_overview}

YOUR STAGE {sid}: {title}
INSTRUCTIONS:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

Implement this stage completely. Verify your work runs (build/tests/quick manual check as appropriate).
Do NOT commit, do NOT push, do NOT touch the {forge_dir}/ directory.
CRITICAL: the Forge engine that orchestrates you is itself running from this repository on port 8734.
Never kill it (no `pkill forge` or similar) and never start another instance on its port.
To test the engine binary, run it on a different port: `FORGE_PORT=18734 ./target/debug/forge`."#;

const FIX_PROMPT: &str = r#"You are the implementing agent of Forge for exactly one stage of an approved plan.

OVERALL GOAL:
{goal}

FULL PLAN (context only — do NOT work on other stages):
{plan_overview}

YOUR STAGE {sid}: {title}
INSTRUCTIONS:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

You already implemented this stage; the uncommitted changes are yours.
An independent reviewer looked at them and requests fixes:
{issues}

Address every issue (or make the code obviously correct where the reviewer was wrong).
Do NOT commit, do NOT push, do NOT touch the {forge_dir}/ directory.
CRITICAL: the Forge engine that orchestrates you is itself running from this repository on port 8734.
Never kill it (no `pkill forge` or similar) and never start another instance on its port.
To test the engine binary, run it on a different port: `FORGE_PORT=18734 ./target/debug/forge`."#;

const REVIEW_PROMPT: &str = r#"You are an independent reviewer in a fresh session. Another agent implemented one stage of a plan in this repository. Judge only whether the current uncommitted changes correctly implement the stage.

STAGE: {title}
INSTRUCTIONS GIVEN TO THE IMPLEMENTER:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

Inspect with `git status` and `git diff` (all uncommitted changes belong to this stage), read files, and run tests/builds if useful.
Then write your verdict as JSON to the file {verdict_path}:
{"approved": true/false, "summary": "short feedback: what you inspected and what you found, even when approving", "issues": ["specific, actionable issue", ...]}

approved=true only if the acceptance criteria are met and you found no real defect.
Always fill summary with short feedback describing what you inspected and what you found, even when approving.
Do NOT fix anything yourself; do NOT modify any file except {verdict_path}.
CRITICAL: the Forge engine that orchestrates you is itself running from this repository on port 8734.
Never kill it (no `pkill forge` or similar) and never start another instance on its port.
To test the engine binary, run it on a different port: `FORGE_PORT=18734 ./target/debug/forge`."#;

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

fn handle(app: &Arc<App>, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let method = req.method().clone();
    let mut body_text = String::new();
    let _ = req.as_reader().read_to_string(&mut body_text);
    let body: Value = serde_json::from_str(&body_text).unwrap_or(json!({}));
    let project_endpoint = matches!(path, "/api/state" | "/api/agent_log" | "/api/diff"
        | "/api/plan" | "/api/plan/edit" | "/api/plan/revise" | "/api/approve" | "/api/run" | "/api/stop" | "/api/reset_plan")
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

    match (method, path) {
        (tiny_http::Method::Get, "/api/state") => {
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
            snap["busy"] = json!(ctx.session.busy.load(Ordering::SeqCst));
            snap["plan"] = ctx.load_plan().unwrap_or(Value::Null);
            snap["queue"] = ctx.load_queue()["items"].clone();
            snap["queue_active"] = json!(ctx.session.queue_active.load(Ordering::SeqCst));
            drop(_queue_guard);
            snap["active_project"] = json!(active_project);
            snap["sessions"] = app.session_summaries(&active_project);
            snap["history"] = ctx.read_history();
            snap["git_log"] = json!(ctx.git(&["log", "--oneline", "-12"]).unwrap_or_default());
            respond(req, 200, snap);
        }
        (tiny_http::Method::Get, "/api/agent_log") => {
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
            respond(req, 200, json!({"log": log, "size": size}));
        }
        (tiny_http::Method::Get, "/api/diff") => {
            let diff = ctx.git(&["diff", "HEAD"]).unwrap_or_default();
            let tail: String = diff.chars().rev().take(40000).collect::<Vec<_>>()
                .into_iter().rev().collect();
            respond(req, 200, json!({"diff": tail}));
        }
        (tiny_http::Method::Get, "/api/projects") => {
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
            respond(req, 200, resp);
        }
        (tiny_http::Method::Post, "/api/settings") => {
            let mut settings = app.settings.lock().unwrap();
            if let Some(obj) = body.as_object() {
                for (k, v) in obj {
                    if settings.get(k).is_some() {
                        settings[k] = v.clone();
                    }
                }
            }
            drop(settings);
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/project") => {
            let path = body["path"].as_str().unwrap_or("").trim().to_string();
            match app.set_project(&path) {
                Ok(()) => respond(req, 200, json!({"ok": true})),
                Err(e) => respond(req, 400, json!({"error": e})),
            }
        }
        (tiny_http::Method::Post, "/api/project/select") => {
            let path = body["path"].as_str().unwrap_or("").trim().to_string();
            let repo = body["repo"].as_str().unwrap_or("").trim().to_string();
            if !path.is_empty() {
                match app.set_project(&path) {
                    Ok(()) => respond(req, 200, json!({"ok": true})),
                    Err(e) => respond(req, 400, json!({"error": e})),
                }
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
                    respond(req, 400, json!({"error": format!("invalid repo name: {repo}")}));
                    return;
                }
                let name = parts[1].to_string();
                let target = PathBuf::from(app.setting("projects_root")).join(&name);
                let ctx = app.context(&target.display().to_string());
                if target.join(".git").exists() {
                    match app.set_project(&target.display().to_string()) {
                        Ok(()) => respond(req, 200, json!({"ok": true})),
                        Err(e) => respond(req, 400, json!({"error": e})),
                    }
                } else if ctx.acquire_busy().is_err() {
                    respond(req, 409, json!({"error": "busy"}));
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
                    respond(req, 200, json!({"ok": true, "cloning": true}));
                }
            } else {
                respond(req, 400, json!({"error": "path or repo required"}));
            }
        }
        (tiny_http::Method::Post, "/api/queue/add" | "/api/queue/remove"
            | "/api/queue/move" | "/api/queue/clear") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            let mut queue = ctx.load_queue();
            let action = path.strip_prefix("/api/queue/").unwrap();
            match mutate_queue(&mut queue, action, &body) {
                Ok(event) => {
                    ctx.save_queue(&queue);
                    ctx.log_event("queue", &event);
                    respond(req, 200, json!({"ok": true}));
                }
                Err(e) => respond(req, 400, json!({"error": e})),
            }
        }
        (tiny_http::Method::Post, "/api/queue/start") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let queue = ctx.load_queue();
            if !queue["items"].as_array().unwrap().iter()
                .any(|item| item["status"] == "queued")
            {
                respond(req, 400, json!({"error": "no queued goals"}));
            } else if ctx.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                ctx.session.stop_requested.store(false, Ordering::SeqCst);
                ctx.session.queue_active.store(true, Ordering::SeqCst);
                ctx.log_event("queue", "queue started");
                let ctx2 = ctx.clone();
                std::thread::spawn(move || ctx2.queue_worker(None));
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/plan") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            let goal = body["goal"].as_str().unwrap_or("").trim().to_string();
            if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
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
                std::thread::spawn(move || ctx2.plan_worker(&goal));
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/plan/revise") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            let feedback = body["feedback"].as_str().unwrap_or("").trim().to_string();
            if feedback.is_empty() {
                respond(req, 400, json!({"error": "feedback required"}));
                return;
            }
            if ctx.session.queue_active.load(Ordering::SeqCst) || ctx.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let Some(plan) = ctx.load_plan() else {
                ctx.session.busy.store(false, Ordering::SeqCst);
                respond(req, 400, json!({"error": "no plan"}));
                return;
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
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/plan/edit") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let Some(plan) = ctx.load_plan() else {
                respond(req, 400, json!({"error": "no plan"}));
                return;
            };
            match edit_plan(&plan, &body) {
                Ok(edited) => {
                    ctx.save_plan(&edited);
                    {
                        let mut s = ctx.session.state.lock().unwrap();
                        s.phase = "plan_ready".into();
                        s.goal = edited["goal"].as_str().unwrap_or("").to_string();
                    }
                    ctx.log_event("plan", "plan edited by user");
                    respond(req, 200, json!({"ok": true}));
                }
                Err(e) => respond(req, 400, json!({"error": e})),
            }
        }
        (tiny_http::Method::Post, "/api/approve") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            if ctx.session.busy.load(Ordering::SeqCst) {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let Some(mut plan) = ctx.load_plan() else {
                respond(req, 400, json!({"error": "no plan"}));
                return;
            };
            let mut queue = ctx.load_queue();
            let awaiting = queue["items"].as_array().unwrap().iter()
                .find(|item| !matches!(item["status"].as_str(), Some("done" | "failed" | "blocked")))
                .filter(|item| item["status"] == "awaiting_approval")
                .and_then(|item| item["id"].as_u64());
            if let Some(id) = awaiting {
                if ctx.acquire_busy().is_err() {
                    respond(req, 409, json!({"error": "busy"}));
                    return;
                }
                ctx.session.stop_requested.store(false, Ordering::SeqCst);
                ctx.session.queue_active.store(true, Ordering::SeqCst);
                ctx.start_queue_run(&mut queue, id, &mut plan);
                ctx.log_event("queue", &format!("goal {id}: approved by user"));
                let ctx2 = ctx.clone();
                std::thread::spawn(move || ctx2.queue_worker(Some(id)));
            } else {
                plan["status"] = json!("approved");
                ctx.save_plan(&plan);
            }
            ctx.log_event("plan", "plan approved by user");
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/run") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            if ctx.session.queue_active.load(Ordering::SeqCst) {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let plan = ctx.load_plan();
            let status = plan
                .as_ref()
                .and_then(|p| p["status"].as_str())
                .unwrap_or("");
            if status != "approved" && status != "done" {
                respond(req, 400, json!({"error": "plan is not approved"}));
            } else if ctx.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
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
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/stop") => {
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
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/reset_plan") => {
            let _queue_guard = ctx.session.queue_lock.lock().unwrap();
            if ctx.session.busy.load(Ordering::SeqCst) || ctx.session.queue_active.load(Ordering::SeqCst) {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                let _ = fs::remove_file(ctx.forge_path("plan.json"));
                ctx.set_phase("idle");
                ctx.log_event("plan", "plan discarded");
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/self_update") => {
            if app.any_busy() {
                respond(req, 409, json!({"error": "busy"}));
                return;
            }
            let repo = std::env::var("FORGE_REPO").unwrap_or_else(|_| {
                format!("{}/Projects/Forge", std::env::var("HOME").unwrap_or_default())
            });
            let script = PathBuf::from(&repo).join("install.sh");
            if !script.is_file() {
                respond(req, 400, json!({"error": format!("no install.sh in {repo}")}));
                return;
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
                    ctx.log_event("update",
                        "self-update started; engine restarts, changed plugin files hot-reload in the shell \
                         (log: journalctl --user -u forge-update)");
                    respond(req, 200, json!({"ok": true}));
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    respond(req, 500, json!({"error": format!("systemd-run failed: {err}")}));
                }
                Err(e) => {
                    respond(req, 500, json!({"error": format!("systemd-run failed: {e}")}));
                }
            }
        }
        _ => respond(req, 404, json!({"error": "not found"})),
    }
}

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
    let app = Arc::new(App::new(&project, settings));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn editable_stage(id: i64) -> Value {
        json!({"id": id, "title": "Repaired title", "instructions": "Repaired instructions",
            "acceptance": "", "commit": "feat: repair stage"})
    }

    #[test]
    fn plan_revise_api_preserves_committed_work_and_passes_feedback_to_planner() {
        let test = QueueTest::new(false);
        let mut committed = editable_stage(1);
        committed["status"] = json!("committed");
        committed["rounds"] = json!(3);
        committed["sha"] = json!("abc123");
        committed["reviews"] = json!([{"approved": true}]);
        let original = json!({"goal": "Original {feedback} goal", "status": "draft",
            "stages": [committed, {"id": 2, "status": "pending"}]});
        test.app.save_plan(&original);
        let feedback = format!("Improve {{goal}} and {{plan_path}}: {}", "界🙂".repeat(200));
        assert_eq!(api_request(&test.app.app, "POST", "/api/plan/revise",
            json!({"feedback": format!(" \t{feedback}\n")})), (200, json!({"ok": true})));
        wait_for_worker(&test.app);
        let plan = test.app.load_plan().unwrap();
        assert_eq!(plan["stages"][0], original["stages"][0]);
        assert_eq!(plan["stages"].as_array().unwrap().len(), 2);
        assert_eq!(plan["goal"], original["goal"]);
        assert_eq!(plan["status"], "draft");
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        let settings = test.app.app.settings.lock().unwrap();
        assert_eq!(settings["mock_planner_phase"], "planning");
        assert_eq!(settings["mock_planner_had_plan"], false);
        let prompt = settings["mock_planner_prompt"].as_str().unwrap();
        assert!(prompt.contains(&serde_json::to_string_pretty(&original).unwrap()));
        assert!(prompt.contains(&feedback));
        assert!(prompt.contains("Keep the same overall goal:\nOriginal {feedback} goal"));
        assert!(prompt.contains("Only write .forge/plan.json."));
        let schema = PLANNER_PROMPT.split_once("with exactly this schema:\n").unwrap().1
            .split_once("\n\nRules:").unwrap().0;
        assert!(prompt.contains(schema));
        let history = test.app.read_history();
        assert_eq!(history[0]["kind"], "plan");
        assert_eq!(history[0]["text"], format!("revision started: {}",
            feedback.chars().take(300).collect::<String>()));
        assert_eq!(history[1]["text"], "plan revised with 2 stages");
    }

    #[test]
    fn plan_revise_restores_dropped_and_changed_committed_stages_in_original_order() {
        let test = QueueTest::new(false);
        let mut first = editable_stage(9);
        first["status"] = json!("committed");
        let mut second = editable_stage(1);
        second["status"] = json!("committed");
        second["rounds"] = json!(2);
        let original = json!({"goal": "Goal", "status": "approved",
            "stages": [first, second, editable_stage(2)]});
        test.app.save_plan(&original);
        test.app.acquire_busy().unwrap();
        test.app.revise_worker(&original, "Split the remaining work");
        let plan = test.app.load_plan().unwrap();
        assert_eq!(&plan["stages"].as_array().unwrap()[..2],
            &original["stages"].as_array().unwrap()[..2]);
        assert_eq!(plan["stages"].as_array().unwrap().len(), 3);
        assert_eq!(plan["stages"][2]["id"], 2);
        assert_eq!(plan["status"], "draft");
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
    }

    #[test]
    fn plan_revise_failure_restores_original_file_and_reports_error() {
        for (tool, output) in [
            ("unknown-planner", Value::Null),
            ("mock", Value::Null),
            ("mock", json!("not valid JSON")),
            ("mock", json!({"goal": "Unusable", "stages": {}})),
            ("mock", json!({"stages": [42]})),
        ] {
            let test = QueueTest::new(false);
            let original = json!({"goal": "Keep this goal", "status": "draft",
                "stages": [editable_stage(1), editable_stage(2)]});
            test.app.save_plan(&original);
            let before = fs::read(test.app.forge_path("plan.json")).unwrap();
            {
                let mut settings = test.app.app.settings.lock().unwrap();
                settings["planner"] = json!(tool);
                settings["mock_plan_output"] = output;
            }
            assert_eq!(api_request(&test.app.app, "POST", "/api/plan/revise",
                json!({"feedback": "Improve the plan"})), (200, json!({"ok": true})));
            wait_for_worker(&test.app);
            assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
            assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
            assert!(test.app.read_history().as_array().unwrap().iter().any(|event|
                event["kind"] == "error" && event["text"].as_str().unwrap().starts_with("revision failed: ")));
        }
    }

    #[test]
    fn plan_revise_api_validates_input_busy_flags_and_project_routing() {
        let first = QueueTest::new(false);
        let engine = &first.app.app;
        let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
        for feedback in [Value::Null, json!(42), json!(""), json!(" \t\n\u{2003}")] {
            assert_eq!(api_request(engine, "POST", "/api/plan/revise", json!({"feedback": feedback})),
                (400, json!({"error": "feedback required"})));
        }
        let body = json!({"feedback": "Improve the plan"});
        assert_eq!(api_request(engine, "POST", "/api/plan/revise", body.clone()),
            (400, json!({"error": "no plan"})));
        assert!(!first.app.session.busy.load(Ordering::SeqCst));
        let original = json!({"goal": "Goal", "status": "draft", "stages": [editable_stage(1)]});
        first.app.save_plan(&original);
        for flag in [&first.app.session.busy, &first.app.session.queue_active] {
            flag.store(true, Ordering::SeqCst);
            assert_eq!(api_request(engine, "POST", "/api/plan/revise", body.clone()),
                (409, json!({"error": "busy"})));
            assert_eq!(first.app.load_plan().unwrap(), original);
            flag.store(false, Ordering::SeqCst);
        }
        assert_eq!(first.app.read_history(), json!([]));
        first.app.acquire_busy().unwrap();
        let _worker = WorkerGuard(&first.app.session);
        second.app.save_plan(&original);
        let mut targeted = body;
        targeted["project"] = json!(second.app.project());
        assert_eq!(api_request(engine, "POST", "/api/plan/revise", targeted),
            (200, json!({"ok": true})));
        wait_for_worker(&second.app);
        assert_eq!(second.app.session.state.lock().unwrap().phase, "plan_ready");
        assert_eq!(first.app.load_plan().unwrap(), original);
    }

    #[test]
    fn planning_and_revision_share_normalization() {
        for revision in [false, true] {
            let test = QueueTest::new(false);
            let original = json!({"goal": "Original goal", "stages": [editable_stage(1)]});
            test.app.save_plan(&original);
            test.app.app.settings.lock().unwrap()["mock_plan_output"] = json!({
                "goal": "Wrong goal", "status": "approved", "stages": [editable_stage(2), editable_stage(3)]});
            test.app.acquire_busy().unwrap();
            if revision {
                test.app.revise_worker(&original, "Improve the plan");
            } else {
                test.app.plan_worker("Original goal");
            }
            let plan = test.app.load_plan().unwrap();
            assert_eq!(plan["goal"], "Original goal");
            assert_eq!(plan["status"], "draft");
            for stage in plan["stages"].as_array().unwrap() {
                assert_eq!(stage["status"], "pending");
                assert_eq!(stage["rounds"], 0);
            }
            assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        }
    }

    #[test]
    fn plan_edit_resets_execution_and_preserves_or_replaces_goal() {
        let original = json!({"goal": "Old goal", "status": "blocked", "metadata": "keep",
            "stages": [{"id": 3, "title": "Old title", "instructions": "Old instructions",
                "acceptance": "Old acceptance", "commit": "old", "status": "blocked",
                "rounds": 2, "started_unix": 10, "finished_unix": 20, "duration_secs": 10,
                "last_verdict": {"approved": false}, "reviews": [{"round": 2}], "sha": "old"}]});
        let mut stage = original["stages"][0].clone();
        for field in ["title", "instructions", "acceptance", "commit"] {
            stage[field] = editable_stage(3)[field].clone();
        }
        let edited = edit_plan(&original, &json!({"plan": {
            "goal": "New goal", "stages": [stage, editable_stage(9)]}})).unwrap();
        assert_eq!(edited["goal"], "New goal");
        assert_eq!(edited["status"], "draft");
        assert_eq!(edited["metadata"], "keep");
        for (stage, id) in edited["stages"].as_array().unwrap().iter().zip([3, 9]) {
            let mut expected = editable_stage(id);
            expected["status"] = json!("pending");
            expected["rounds"] = json!(0);
            assert_eq!(stage, &expected);
        }
        for goal in [Value::Null, json!(42), json!(" \t\n\u{2003}")] {
            let edited = edit_plan(&original, &json!({"plan": {
                "goal": goal, "stages": [editable_stage(3)]}})).unwrap();
            assert_eq!(edited["goal"], "Old goal");
        }
        let edited = edit_plan(&original, &json!({"plan": {"stages": [editable_stage(3)]}})).unwrap();
        assert_eq!(edited["goal"], "Old goal");
        assert_eq!(original["status"], "blocked");
    }

    #[test]
    fn plan_edit_preserves_committed_stages_verbatim_and_in_order() {
        let mut first = editable_stage(2);
        first["status"] = json!("committed");
        first["rounds"] = json!(3);
        first["sha"] = json!("abc123");
        first["reviews"] = json!([{"approved": true}]);
        first["started_unix"] = json!(10);
        first["finished_unix"] = json!(20);
        first["duration_secs"] = json!(10);
        first["last_verdict"] = json!({"approved": true});
        first["custom"] = json!({"preserve": true});
        let mut second = first.clone();
        second["id"] = json!(5);
        let original = json!({"goal": "Goal", "status": "approved",
            "stages": [first, second, editable_stage(8)]});
        let mut submitted = editable_stage(2);
        submitted["title"] = json!("Ignored edit to committed work");
        let edited = edit_plan(&original, &json!({"plan": {"stages": [
            submitted, editable_stage(5), editable_stage(8)]}})).unwrap();
        assert_eq!(edited["stages"][0], original["stages"][0]);
        assert_eq!(edited["stages"][1], original["stages"][1]);
        assert_eq!(edit_plan(&original, &json!({"plan": {"stages": [editable_stage(8)]}})),
            Err("cannot remove a committed stage"));
        for ids in [[5, 2, 8], [2, 8, 5], [8, 2, 5]] {
            assert_eq!(edit_plan(&original, &json!({"plan": {
                "stages": ids.map(editable_stage)}})), Err("cannot reorder committed stages"));
        }
    }

    #[test]
    fn plan_edit_assigns_unique_ids_and_allows_editable_reordering_and_removal() {
        let original = json!({"goal": "Goal", "stages": [
            editable_stage(1), editable_stage(4), editable_stage(7)]});
        let mut new = editable_stage(0);
        new.as_object_mut().unwrap().remove("id");
        let mut invalid_id = editable_stage(0);
        invalid_id["id"] = json!("invalid");
        let edited = edit_plan(&original, &json!({"plan": {"stages": [
            editable_stage(7), new, editable_stage(8), editable_stage(1), invalid_id]}})).unwrap();
        let ids: Vec<i64> = edited["stages"].as_array().unwrap().iter()
            .map(|stage| stage["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, [7, 9, 8, 1, 10]);
        let full = json!({"stages": [editable_stage(i64::MAX)]});
        assert_eq!(edit_plan(&full, &json!({"plan": {"stages": [editable_stage(0)]}})),
            Err("stage id limit reached"));
    }

    #[test]
    fn plan_edit_invalid_bodies_leave_saved_plan_unchanged() {
        let test = QueueTest::new(false);
        let mut committed = editable_stage(1);
        committed["status"] = json!("committed");
        let original = json!({"goal": "Goal", "status": "approved", "stages": [committed]});
        test.app.save_plan(&original);
        test.app.session.state.lock().unwrap().goal = "Goal".into();
        let before = fs::read(test.app.forge_path("plan.json")).unwrap();
        let mut bodies = vec![json!({}), json!(null), json!({"plan": null}),
            json!({"plan": []}), json!({"plan": {}}), json!({"plan": {"stages": null}}),
            json!({"plan": {"stages": {}}}), json!({"plan": {"stages": []}}),
            json!({"plan": {"stages": [null]}}),
            json!({"plan": {"stages": [editable_stage(1), editable_stage(1)]}}),
            json!({"plan": {"stages": [editable_stage(2)]}})];
        for field in ["title", "instructions", "acceptance", "commit"] {
            let mut stage = editable_stage(1);
            stage.as_object_mut().unwrap().remove(field);
            bodies.push(json!({"plan": {"stages": [stage]}}));
            for value in [Value::Null, json!(123), json!(false), json!([]), json!({})] {
                let mut stage = editable_stage(1);
                stage[field] = value;
                bodies.push(json!({"plan": {"stages": [stage]}}));
            }
        }
        for field in ["title", "instructions"] {
            let mut stage = editable_stage(1);
            stage[field] = json!(" \t\n\u{2003}");
            bodies.push(json!({"plan": {"stages": [stage]}}));
        }
        for body in bodies {
            assert!(edit_plan(&original, &body).is_err(), "accepted {body}");
            assert_eq!(api_request(&test.app.app, "POST", "/api/plan/edit", body.clone()).0,
                400, "accepted {body}");
            assert_eq!(fs::read(test.app.forge_path("plan.json")).unwrap(), before);
            assert_eq!(test.app.session.state.lock().unwrap().goal, "Goal");
        }
        assert_eq!(test.app.read_history(), json!([]));
    }

    #[test]
    fn plan_edit_api_routes_projects_updates_state_and_rejects_busy_sessions() {
        let first = QueueTest::new(false);
        let engine = &first.app.app;
        let second = QueueTest::with_engine(false, Some(Arc::clone(engine)));
        let body = json!({"plan": {"goal": "Edited goal", "stages": [editable_stage(1)]}});
        assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
            (400, json!({"error": "no plan"})));
        assert!(!first.app.forge_path("plan.json").exists());
        let original = json!({"goal": "Old goal", "status": "approved", "stages": [editable_stage(1)]});
        first.app.save_plan(&original);
        let before = fs::read(first.app.forge_path("plan.json")).unwrap();
        for flag in [&first.app.session.busy, &first.app.session.queue_active] {
            flag.store(true, Ordering::SeqCst);
            assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
                (409, json!({"error": "busy"})));
            assert_eq!(fs::read(first.app.forge_path("plan.json")).unwrap(), before);
            flag.store(false, Ordering::SeqCst);
        }
        assert_eq!(api_request(engine, "POST", "/api/plan/edit", body.clone()),
            (200, json!({"ok": true})));
        let first_plan = first.app.load_plan().unwrap();
        assert_eq!(first_plan["status"], "draft");
        first.app.session.busy.store(true, Ordering::SeqCst);
        second.app.save_plan(&original);
        second.app.set_phase("blocked");
        let mut targeted = body;
        targeted["project"] = json!(second.app.project());
        assert_eq!(api_request(engine, "POST", "/api/plan/edit", targeted),
            (200, json!({"ok": true})));
        let (status, state) = api_request(engine, "GET",
            &format!("/api/state?project={}", second.app.project()), json!({}));
        assert_eq!(status, 200);
        assert_eq!(state["phase"], "plan_ready");
        assert_eq!(state["goal"], "Edited goal");
        assert_eq!(state["plan"], edit_plan(&original, &json!({"plan": {
            "goal": "Edited goal", "stages": [editable_stage(1)]}})).unwrap());
        assert_eq!(first.app.load_plan().unwrap(), first_plan);
        let history = second.app.read_history();
        assert_eq!(history[0]["kind"], "plan");
        assert_eq!(history[0]["text"], "plan edited by user");
        assert_eq!(history[0]["goal"], "Edited goal");
    }

    #[test]
    fn duration_formats_seconds_and_minutes() {
        for (secs, expected) in [
            (-1, "0s"), (0, "0s"), (58, "58s"), (60, "1m 0s"),
            (61, "1m 1s"), (252, "4m 12s"), (3600, "60m 0s"),
        ] {
            assert_eq!(fmt_duration(secs), expected);
        }
    }

    #[test]
    fn queue_add_uses_max_id_and_records_goal_metadata() {
        let mut queue = json!({"items": []});
        let started = unix_timestamp();
        mutate_queue(&mut queue, "add", &json!({"goal": "  First goal\n"})).unwrap();
        mutate_queue(&mut queue, "add", &json!({"goal": "Second goal"})).unwrap();
        assert_eq!(queue["items"][0]["id"], 1);
        assert_eq!(queue["items"][1]["id"], 2);
        assert_eq!(queue["items"][0]["goal"], "First goal");
        assert_eq!(queue["items"][0]["status"], "queued");
        let added = queue["items"][0]["added_unix"].as_i64().unwrap();
        assert!((started..=unix_timestamp()).contains(&added));

        queue["items"][0]["id"] = json!(10);
        queue["items"][0]["status"] = json!("done");
        mutate_queue(&mut queue, "add", &json!({"goal": "Third goal"})).unwrap();
        assert_eq!(queue["items"][2]["id"], 11);
    }

    #[test]
    fn queue_invalid_mutations_leave_items_unchanged() {
        let original = json!({"items": [
            {"id": 1, "status": "queued"},
            {"id": 2, "status": "running"},
            {"id": 3, "status": "done"},
        ]});
        for (action, body) in [
            ("add", json!({})),
            ("add", json!({"goal": ""})),
            ("add", json!({"goal": " \t\n\u{2003}"})),
            ("add", json!({"goal": 123})),
            ("remove", json!({})),
            ("remove", json!({"id": "1"})),
            ("remove", json!({"id": -1})),
            ("remove", json!({"id": 1.5})),
            ("remove", json!({"id": 99})),
            ("remove", json!({"id": 2})),
            ("remove", json!({"id": 3})),
            ("move", json!({"id": 1})),
            ("move", json!({"id": 1, "dir": "left"})),
            ("move", json!({"id": 2, "dir": "up"})),
            ("move", json!({"id": 3, "dir": "down"})),
            ("move", json!({"id": 99, "dir": "down"})),
        ] {
            let mut queue = original.clone();
            assert!(mutate_queue(&mut queue, action, &body).is_err());
            assert_eq!(queue, original);
        }
    }

    #[test]
    fn queue_moves_only_swap_queued_neighbors_and_allow_edges() {
        let original = json!({"items": [
            {"id": 1, "status": "running"},
            {"id": 2, "status": "queued"},
            {"id": 3, "status": "done"},
            {"id": 4, "status": "queued"},
            {"id": 5, "status": "failed"},
        ]});
        let mut queue = original.clone();
        mutate_queue(&mut queue, "move", &json!({"id": 2, "dir": "up"})).unwrap();
        mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "down"})).unwrap();
        assert_eq!(queue, original);
        mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "up"})).unwrap();
        let mut swapped = original.clone();
        swapped["items"].as_array_mut().unwrap().swap(1, 3);
        assert_eq!(queue, swapped);
        mutate_queue(&mut queue, "move", &json!({"id": 4, "dir": "down"})).unwrap();
        assert_eq!(queue, original);
    }

    #[test]
    fn queue_remove_and_clear_preserve_nonqueued_items() {
        let mut queue = json!({"items": [
            {"id": 1, "status": "running"},
            {"id": 2, "status": "queued"},
            {"id": 3, "status": "done"},
            {"id": 4, "status": "queued"},
            {"id": 5, "status": "failed"},
        ]});
        mutate_queue(&mut queue, "remove", &json!({"id": 2})).unwrap();
        assert_eq!(queue["items"].as_array().unwrap().len(), 4);
        assert_eq!(queue["items"][1]["id"], 3);
        mutate_queue(&mut queue, "clear", &json!({})).unwrap();
        assert_eq!(queue, json!({"items": [
            {"id": 1, "status": "running"},
            {"id": 3, "status": "done"},
            {"id": 5, "status": "failed"},
        ]}));
        let preserved = queue.clone();
        mutate_queue(&mut queue, "clear", &json!({})).unwrap();
        assert_eq!(queue, preserved);
        let mut empty = json!({"items": []});
        mutate_queue(&mut empty, "clear", &json!({})).unwrap();
        assert_eq!(empty, json!({"items": []}));
    }

    #[test]
    fn queue_id_overflow_is_rejected_without_mutation() {
        let mut queue = json!({"items": [{"id": u64::MAX, "status": "done"}]});
        let original = queue.clone();
        assert!(mutate_queue(&mut queue, "add", &json!({"goal": "Next"})).is_err());
        assert_eq!(queue, original);
    }

    struct QueueTest {
        app: Ctx,
        path: PathBuf,
    }

    impl QueueTest {
        fn new(auto_approve: bool) -> Self {
            Self::with_engine(auto_approve, None)
        }

        fn with_engine(auto_approve: bool, engine: Option<Arc<App>>) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "forge-queue-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst),
            ));
            fs::create_dir(&path).unwrap();
            let mut settings = default_settings();
            settings["planner"] = json!("mock");
            settings["implementer"] = json!("mock");
            settings["reviewer"] = json!("mock");
            settings["auto_push"] = json!(false);
            settings["queue_auto_approve"] = json!(auto_approve);
            let engine = engine.unwrap_or_else(|| Arc::new(App::new(&path.display().to_string(), settings)));
            let app = engine.context(&path.display().to_string());
            app.git(&["init", "-q"]).unwrap();
            for (key, value) in [
                ("user.name", "Forge Test"), ("user.email", "test@example.invalid"),
                ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null"),
            ] {
                app.git(&["config", key, value]).unwrap();
            }
            app.git(&["commit", "--allow-empty", "-qm", "initial"]).unwrap();
            let mut queue = json!({"items": []});
            for goal in ["first goal", "second goal"] {
                mutate_queue(&mut queue, "add", &json!({"goal": goal})).unwrap();
            }
            app.save_queue(&queue);
            Self { app, path }
        }

        fn start(&self) {
            self.app.acquire_busy().unwrap();
            self.app.session.queue_active.store(true, Ordering::SeqCst);
            self.app.queue_worker(None);
        }

        fn statuses(&self) -> Vec<String> {
            self.app.load_queue()["items"].as_array().unwrap().iter()
                .map(|item| item["status"].as_str().unwrap().to_string()).collect()
        }
    }

    impl Drop for QueueTest {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn history_entries_include_timestamp_and_only_available_context() {
        let test = QueueTest::new(true);
        let before = unix_timestamp();
        test.app.log_event("run", "idle event");
        test.app.session.state.lock().unwrap().goal = "short goal".into();
        test.app.log_event("plan", "goal event");
        {
            let mut s = test.app.session.state.lock().unwrap();
            s.goal = "界🙂".repeat(61);
            s.current_stage = Some(3);
        }
        test.app.log_event("stage", "active event");
        test.app.session.state.lock().unwrap().goal.clear();
        test.app.log_event("stage", "stage event");

        let text = fs::read_to_string(test.path.join(FORGE_DIR).join("history.jsonl")).unwrap();
        let entries: Vec<Value> = text.lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(entries.len(), 4);
        for entry in &entries {
            assert!((before..=unix_timestamp()).contains(&entry["unix"].as_i64().unwrap()));
            assert_eq!(entry["t"].as_str().unwrap().split(':').count(), 3);
        }
        assert!(entries[0].get("goal").is_none());
        assert!(entries[0].get("stage").is_none());
        assert_eq!(entries[1]["goal"], "short goal");
        assert!(entries[1].get("stage").is_none());
        assert_eq!(entries[2]["goal"], "界🙂".repeat(60));
        assert_eq!(entries[2]["stage"], 3);
        assert_eq!(entries[2]["kind"], "stage");
        assert_eq!(entries[2]["text"], "active event");
        assert!(entries[3].get("goal").is_none());
        assert_eq!(entries[3]["stage"], 3);
    }

    #[test]
    fn history_keeps_last_400_parsed_entries_and_preserves_legacy_shape() {
        let test = QueueTest::new(true);
        let legacy: Vec<Value> = (0..402).map(|i| {
            json!({"t": "12:34:56", "kind": "stage", "text": format!("old event {i}")})
        }).collect();
        let mut text = legacy.iter().map(|entry| format!("{entry}\n")).collect::<String>();
        text.push_str("not json\n\n");
        let path = test.app.forge_path("history.jsonl");
        fs::write(&path, &text).unwrap();
        test.app.log_event("run", "new event");

        let history = test.app.read_history();
        let entries = history.as_array().unwrap();
        assert_eq!(entries.len(), 400);
        assert_eq!(&entries[..399], &legacy[3..]);
        assert!(entries[399]["unix"].is_i64());
        assert_eq!(entries[399]["text"], "new event");
        assert!(fs::read_to_string(path).unwrap().starts_with(&text));
    }

    #[test]
    fn run_completion_uses_run_start_time_before_cleanup() {
        let test = QueueTest::new(true);
        test.app.save_plan(&json!({"stages": [], "status": "approved"}));
        let started = unix_timestamp() - 252;
        {
            let mut s = test.app.session.state.lock().unwrap();
            s.goal = "timed goal".into();
            s.run_started_unix = started;
        }
        test.app.run_worker();
        let elapsed = unix_timestamp() - started;
        let history = test.app.read_history();
        let entry = history.as_array().unwrap().last().unwrap();
        assert_eq!(entry["kind"], "run");
        assert_eq!(entry["goal"], "timed goal");
        assert!((252..=elapsed).any(|secs| {
            entry["text"] == format!("all stages committed — run complete in {}", fmt_duration(secs))
        }));
        assert_eq!(test.app.session.state.lock().unwrap().run_started_unix, 0);
    }

    #[test]
    fn queue_auto_approval_commits_both_goals_and_releases_busy() {
        let test = QueueTest::new(true);
        test.start();
        assert_eq!(test.statuses(), ["done", "done"]);
        assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
        assert_eq!(test.app.session.state.lock().unwrap().goal, "second goal");
        assert_eq!(test.app.git(&["log", "--format=%s", "-4"]).unwrap(),
            "feat: line two\nfeat: line one\nfeat: line two\nfeat: line one");

        let history = test.app.read_history();
        let entries = history.as_array().unwrap();
        assert!(entries.iter().all(|entry| entry["unix"].is_i64()));
        for (id, goal) in [(1, "first goal"), (2, "second goal")] {
            let events: Vec<_> = entries.iter().filter(|entry| entry["goal"] == goal).collect();
            assert!(events.iter().any(|entry| entry["text"] == format!("goal {id}: planning")));
            for sid in [1, 2] {
                for prefix in [format!("stage {sid} started:"), format!("stage {sid} committed in ")] {
                    let event = events.iter().find(|entry| entry["text"].as_str().unwrap().starts_with(&prefix))
                        .expect("stage lifecycle event");
                    assert_eq!(event["stage"], sid);
                }
            }
            assert!(events.iter().any(|entry| entry["text"].as_str().unwrap()
                .starts_with("all stages committed — run complete in ")));
        }
        for stage in test.app.load_plan().unwrap()["stages"].as_array().unwrap() {
            let duration = fmt_duration(stage["duration_secs"].as_i64().unwrap());
            let text = format!("stage {} committed in {duration}", stage["id"]);
            assert!(entries.iter().any(|entry| entry["goal"] == "second goal" && entry["text"] == text));
            let reviews = stage["reviews"].as_array().unwrap();
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0]["round"], 1);
            assert_eq!(reviews[0]["approved"], true);
            assert_eq!(reviews[0]["issues"], json!([]));
            assert!(reviews[0]["unix"].as_i64().unwrap() > 0);
            let summary = reviews[0]["summary"].as_str().unwrap();
            assert!(!summary.is_empty());
            assert_eq!(stage["last_verdict"], json!({
                "approved": true, "summary": summary, "issues": [],
            }));
            assert!(entries.iter().any(|entry| entry["kind"] == "review"
                && entry["text"] == format!("stage {} approved: {summary}", stage["id"])));
        }
    }

    #[test]
    fn two_sessions_run_mock_queues_concurrently() {
        let first = QueueTest::new(true);
        let second = QueueTest::with_engine(true, Some(Arc::clone(&first.app.app)));
        let engine = &first.app.app;
        let barrier = std::sync::Barrier::new(2);
        // Distinct goals make crossed plan/history writes observable too.
        let mut queue = second.app.load_queue();
        for item in queue["items"].as_array_mut().unwrap() {
            item["goal"] = json!(format!("other project goal {}", item["id"]));
        }
        second.app.save_queue(&queue);
        for test in [&first, &second] {
            test.app.acquire_busy().unwrap();
            test.app.session.queue_active.store(true, Ordering::SeqCst);
        }
        engine.set_project(second.app.project()).unwrap();
        assert!(Arc::ptr_eq(&engine.active_session(), &second.app.session));
        let summaries = engine.session_summaries(second.app.project());
        assert_eq!(summaries[0]["project"], second.app.project());
        assert_eq!(summaries[1]["project"], first.app.project());
        for summary in summaries.as_array().unwrap() {
            assert_eq!(summary["busy"], true);
            assert_eq!(summary["queue_active"], true);
            assert_eq!(summary["queued"], 2);
        }
        std::thread::scope(|scope| {
            for test in [&first, &second] {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    test.app.queue_worker(None);
                });
            }
        });
        for test in [&first, &second] {
            assert_eq!(test.statuses(), ["done", "done"]);
            assert!(!test.app.session.busy.load(Ordering::SeqCst));
            assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
            assert_eq!(test.app.session.state.lock().unwrap().phase, "done");
            assert_eq!(test.app.git(&["rev-list", "--count", "HEAD"]).unwrap(), "5");
            assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap().lines().count(), 4);
            let goal = test.app.load_queue()["items"][1]["goal"].clone();
            assert_eq!(test.app.load_plan().unwrap()["goal"], goal);
            assert!(test.app.read_history().as_array().unwrap().iter().all(|entry| {
                test.app.load_queue()["items"].as_array().unwrap().iter()
                    .any(|item| item["goal"] == entry["goal"])
            }));
        }
        assert!(!engine.any_busy());
    }

    #[test]
    fn session_aliases_and_selection_preserve_existing_work() {
        let test = QueueTest::new(true);
        let engine = &test.app.app;
        let alias = test.path.join("alias");
        std::os::unix::fs::symlink(&test.path, &alias).unwrap();
        let session = engine.session(&alias.display().to_string());
        assert!(Arc::ptr_eq(&session, &test.app.session));
        test.app.acquire_busy().unwrap();
        session.state.lock().unwrap().goal = "in flight".into();
        test.app.set_phase("running");
        test.app.set_step(Some(2), "reviewing");
        engine.set_project(&alias.display().to_string()).unwrap();
        assert!(engine.set_project(&test.path.join(FORGE_DIR).display().to_string()).is_err());
        assert_eq!(*engine.active_project.lock().unwrap(), test.app.project());
        assert_eq!(engine.open_sessions().len(), 1);
        assert_eq!(session.state.lock().unwrap().phase, "running");
        let summary = engine.session_summaries(test.app.project());
        assert_eq!(summary[0]["goal"], "in flight");
        assert_eq!(summary[0]["current_step"], "reviewing");
        assert_eq!(summary[0]["name"], test.path.file_name().unwrap().to_string_lossy().as_ref());
        assert!(session.busy.load(Ordering::SeqCst));
    }

    fn api_request(engine: &Arc<App>, method: &str, path: &str, body: Value) -> (u16, Value) {
        use std::io::Read as _;
        let server = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
        let address = server.server_addr().to_ip().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| handle(engine, server.recv().unwrap()));
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let body = body.to_string();
            write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            let (headers, body) = response.split_once("\r\n\r\n").unwrap();
            let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, serde_json::from_str(body).unwrap())
        })
    }

    fn wait_for_worker(ctx: &Ctx) {
        let started = Instant::now();
        while ctx.session.busy.load(Ordering::SeqCst) {
            assert!(started.elapsed() < Duration::from_secs(5), "worker did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn api_routes_projects_while_another_session_is_busy() {
        let first = QueueTest::new(true);
        let engine = &first.app.app;
        let second = QueueTest::with_engine(true, Some(Arc::clone(engine)));
        first.app.acquire_busy().unwrap();
        let _worker = WorkerGuard(&first.app.session);
        first.app.set_phase("running");
        first.app.set_step(Some(1), "reviewing");
        first.app.session.state.lock().unwrap().goal = "first project's work".into();
        first.app.session.queue_active.store(true, Ordering::SeqCst);
        first.app.save_plan(&json!({"goal": "first project's work", "stages": []}));
        let post = |path: &str, body: Value| api_request(engine, "POST", path, body).0;
        for route in ["/api/project", "/api/project/select"] {
            assert_eq!(post(route, json!({"path": second.app.project()})), 200);
            assert_eq!(post(route, json!({"path": first.app.project()})), 200);
        }
        assert_eq!(post("/api/project/select", json!({"path": second.app.project()})), 200);
        assert_eq!(post("/api/plan", json!({"goal": "default target"})), 200);
        wait_for_worker(&second.app);
        assert_eq!(second.app.load_plan().unwrap()["goal"], "default target");
        assert_eq!(post("/api/plan", json!({"project": first.app.project(), "goal": "busy target"})), 409);

        // Explicit writes to B keep working when the active project is busy A.
        assert_eq!(post("/api/project", json!({"path": first.app.project()})), 200);
        let target = json!({"project": second.app.project()});
        assert_eq!(post("/api/plan", json!({"project": second.app.project(), "goal": "explicit target"})), 200);
        wait_for_worker(&second.app);
        assert_eq!(post("/api/approve", target.clone()), 200);
        assert_eq!(post("/api/run", target.clone()), 200);
        wait_for_worker(&second.app);
        assert_eq!(second.app.load_plan().unwrap()["goal"], "explicit target");
        assert_eq!(post("/api/queue/start", target.clone()), 200);
        wait_for_worker(&second.app);
        assert_eq!(second.statuses(), ["done", "done"]);
        assert_eq!(first.statuses(), ["queued", "queued"]);

        // Decode project paths independently of other query parameters.
        let alias = second.path.join("project + space");
        std::os::unix::fs::symlink(&second.path, &alias).unwrap();
        let encoded = alias.display().to_string().replace('/', "%2F").replace('+', "%2B").replace(' ', "%20");
        let get = |route: &str| api_request(engine, "GET", &format!("{route}?offset=2&project={encoded}"), json!({}));
        let (status, state) = get("/api/state");
        assert_eq!(status, 200);
        assert_eq!(state["project"], second.app.project());
        assert_eq!(state["active_project"], first.app.project());
        assert_eq!(state["phase"], "done");
        assert_eq!(state["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(state["sessions"][0]["project"], first.app.project());
        assert_eq!(state["sessions"][0]["busy"], true);
        assert_eq!(state["sessions"][0]["queued"], 2);
        assert_eq!(state["sessions"][0]["goal"], "first project's work");
        assert_eq!(state["sessions"][0]["current_step"], "reviewing");
        assert_eq!(state["sessions"][1]["busy"], false);
        assert_eq!(state["sessions"][1]["queued"], 0);
        fs::write(second.app.forge_path("agent.log"), "B log").unwrap();
        assert_eq!(get("/api/agent_log").1["log"], "log");
        fs::write(second.path.join("mock.txt"), "B diff\n").unwrap();
        assert!(get("/api/diff").1["diff"].as_str().unwrap().contains("+B diff"));
        assert_eq!(post("/api/stop", target.clone()), 200);
        assert!(!first.app.session.stop_requested.load(Ordering::SeqCst));
        assert!(first.app.session.queue_active.load(Ordering::SeqCst));
        assert_eq!(post("/api/reset_plan", target), 200);
        assert!(second.app.load_plan().is_none());
        assert_eq!(first.app.load_plan().unwrap()["goal"], "first project's work");
        assert_eq!(post("/api/project/select", json!({"path": second.app.forge_path("")})), 400);
        assert_eq!(post("/api/stop", json!({"project": 42})), 400);
        assert_eq!(api_request(engine, "GET", "/api/state?project=%ZZ", json!({})).0, 400);
    }

    #[test]
    fn queue_manual_approval_pauses_again_after_each_goal() {
        assert_eq!(default_settings()["queue_auto_approve"], false);
        let test = QueueTest::new(false);
        test.start();
        assert_eq!(test.statuses(), ["awaiting_approval", "queued"]);
        assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
        assert!(test.app.session.queue_active.load(Ordering::SeqCst));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        for id in [1, 2] {
            test.app.acquire_busy().unwrap();
            {
                let _queue_guard = test.app.session.queue_lock.lock().unwrap();
                test.app.start_queue_run(
                    &mut test.app.load_queue(), id, &mut test.app.load_plan().unwrap(),
                );
            }
            test.app.queue_worker(Some(id));
            assert!(!test.app.session.busy.load(Ordering::SeqCst));
            if id == 1 {
                assert_eq!(test.statuses(), ["done", "awaiting_approval"]);
                assert_eq!(test.app.session.state.lock().unwrap().phase, "plan_ready");
            }
        }
        assert_eq!(test.statuses(), ["done", "done"]);
        assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
    }

    #[test]
    fn queue_agent_failures_halt_and_leave_remaining_goals_queued() {
        for role in ["planner", "implementer", "reviewer"] {
            let test = QueueTest::new(true);
            test.app.app.settings.lock().unwrap()[role] = json!("invalid-agent");
            test.start();
            assert_eq!(test.statuses(), ["failed", "queued"], "{role}");
            assert_eq!(test.app.session.state.lock().unwrap().phase, "failed");
            assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
            assert!(!test.app.session.busy.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn queue_blocked_stage_halts_and_releases_busy() {
        let test = QueueTest::new(true);
        // An exhausted review budget exercises the blocked-stage exit.
        test.app.app.settings.lock().unwrap()["max_fix_rounds"] = json!(-1);
        test.start();
        assert_eq!(test.statuses(), ["blocked", "queued"]);
        assert_eq!(test.app.session.state.lock().unwrap().phase, "blocked");
        assert!(!test.app.session.queue_active.load(Ordering::SeqCst));
        assert!(!test.app.session.busy.load(Ordering::SeqCst));
        let plan = test.app.load_plan().unwrap();
        let duration = fmt_duration(plan["stages"][0]["duration_secs"].as_i64().unwrap());
        let history = test.app.read_history();
        let event = history.as_array().unwrap().iter().find(|entry| {
            entry["text"].as_str().unwrap().starts_with(&format!("stage 1 blocked after {duration}:"))
        }).expect("blocked stage duration");
        assert_eq!(event["goal"], "first goal");
        assert_eq!(event["stage"], 1);
    }

    #[test]
    fn review_history_preserves_rejections_after_fix_and_approval() {
        let test = QueueTest::new(true);
        let summary = "界🙂".repeat(200);
        test.app.app.settings.lock().unwrap()["mock_verdicts"] = json!([
            {"approved": false, "summary": "Inspected output; a line is missing.",
             "issues": ["Add the missing line.", 42]},
            {"approved": true, "summary": summary, "issues": []},
        ]);
        test.app.mock_agent("planner").unwrap();
        test.app.run_worker();
        let plan = test.app.load_plan().unwrap();
        let stage = &plan["stages"][0];
        assert_eq!(stage["status"], "committed");
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 2);
        for (i, review) in reviews.iter().enumerate() {
            assert_eq!(review["round"], i + 1);
            assert_eq!(review["approved"], i == 1);
            assert!(review["unix"].as_i64().unwrap() > 0);
        }
        assert_eq!(reviews[0]["summary"], "Inspected output; a line is missing.");
        assert_eq!(reviews[0]["issues"], json!(["Add the missing line."]));
        assert_eq!(reviews[1]["summary"], summary);
        assert_eq!(reviews[1]["issues"], json!([]));
        assert_eq!(stage["last_verdict"], json!({
            "approved": true, "summary": summary, "issues": [],
        }));
        let history = test.app.read_history();
        let events: Vec<_> = history.as_array().unwrap().iter()
            .filter(|entry| entry["kind"] == "review" && entry["stage"] == 1).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["text"], "stage 1 rejected: Add the missing line.");
        assert_eq!(events[1]["text"], format!("stage 1 approved: {}", "界🙂".repeat(150)));
        assert_eq!(fs::read_to_string(test.path.join("mock.txt")).unwrap(),
            "work by implementer\nwork by fixer\nwork by implementer\n");
    }

    #[test]
    fn review_history_accumulates_after_exhaustion_and_resume() {
        let test = QueueTest::new(true);
        {
            let mut settings = test.app.app.settings.lock().unwrap();
            settings["max_fix_rounds"] = json!(1);
            settings["mock_verdicts"] = json!([
                {"approved": false, "issues": []},
                "not readable JSON",
                {"approved": true, "issues": []},
            ]);
        }
        test.app.mock_agent("planner").unwrap();
        test.app.run_worker();
        let blocked = test.app.load_plan().unwrap();
        assert_eq!(blocked["stages"][0]["status"], "blocked");
        let previous = blocked["stages"][0]["reviews"].as_array().unwrap();
        assert_eq!(previous.len(), 2);
        for (i, review) in previous.iter().enumerate() {
            assert_eq!(review["round"], i + 1);
            assert_eq!(review["approved"], false);
            assert_eq!(review["summary"], "");
            assert!(review["unix"].as_i64().unwrap() > 0);
        }
        assert_eq!(previous[0]["issues"], json!(["reviewer rejected without details"]));
        let missing_verdict_issues = json!([
            "The previous review session failed to produce a verdict. Re-verify the implementation end to end."
        ]);
        assert_eq!(previous[1]["issues"], missing_verdict_issues);
        assert_eq!(blocked["stages"][0]["last_verdict"], json!({
            "approved": false, "summary": "", "issues": missing_verdict_issues,
        }));

        test.app.run_worker();
        let resumed = test.app.load_plan().unwrap();
        let stage = &resumed["stages"][0];
        assert_eq!(stage["status"], "committed");
        let reviews = stage["reviews"].as_array().unwrap();
        assert_eq!(reviews.len(), 3);
        assert_eq!(&reviews[..2], previous.as_slice());
        // Round numbers are local to the attempt; earlier attempts stay intact.
        assert_eq!(reviews[2]["round"], 1);
        assert_eq!(reviews[2]["approved"], true);
        assert_eq!(reviews[2]["summary"], "");
        assert_eq!(reviews[2]["issues"], json!([]));
        assert!(reviews[2]["unix"].as_i64().unwrap() > 0);
        assert_eq!(stage["last_verdict"], json!({
            "approved": true, "summary": "", "issues": [],
        }));
        assert!(test.app.read_history().as_array().unwrap().iter().any(|entry|
            entry["kind"] == "review" && entry["text"] == "stage 1 approved by reviewer"));
    }

    #[test]
    fn claude_assistant_text() {
        let event = json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "Inspecting the code."},
                {"type": "tool_result", "content": "ignored"},
                {"type": "text", "text": "界".repeat(401)}
            ]}
        });
        let activity = claude_activity(&event.to_string());
        assert_eq!(
            activity.lines,
            ["Inspecting the code.".to_string(), "界".repeat(400)]
        );
        assert!(activity.result.is_none());
    }

    #[test]
    fn claude_tool_use_summary_priority_and_single_line() {
        let event = json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "name": "Read", "input": {
                    "file_path": "src/main.rs", "command": "ignored"
                }},
                {"type": "tool_use", "name": "Bash", "input": {
                    "command": "cargo build\n  cargo test"
                }},
                {"type": "tool_use", "name": "Custom", "input": {"value": 42}}
            ]}
        });
        assert_eq!(
            claude_activity(&event.to_string()).lines,
            [
                "» Read: src/main.rs",
                "» Bash: cargo build cargo test",
                "» Custom: {\"value\":42}",
            ]
        );
    }

    #[test]
    fn claude_tool_summary_fields_and_unicode_limit() {
        for field in ["file_path", "command", "pattern", "description", "url"] {
            let event = json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use", "name": "Tool", "input": {field: "界".repeat(161)}
                }]}
            });
            assert_eq!(
                claude_activity(&event.to_string()).lines,
                [format!("» Tool: {}", "界".repeat(160))]
            );
        }
    }

    #[test]
    fn claude_result_keeps_full_text_for_history() {
        let result = "界".repeat(701);
        let event = json!({"type": "result", "subtype": "success", "result": result});
        let activity = claude_activity(&event.to_string());
        assert_eq!(activity.lines, [format!("✔ result: {}", "界".repeat(400))]);
        assert_eq!(activity.result.as_deref(), Some(result.as_str()));
    }

    #[test]
    fn claude_error_result_uses_subtype() {
        let event = r#"{"type":"result","is_error":true,"subtype":"error_during_execution","result":"failed"}"#;
        assert_eq!(
            claude_activity(event).lines,
            ["✔ result: error_during_execution"]
        );
        let event = r#"{"type":"result","subtype":"error_max_turns"}"#;
        assert_eq!(claude_activity(event).lines, ["✔ result: error_max_turns"]);
    }

    #[test]
    fn claude_init_and_ignored_events() {
        let event = r#"{"type":"system","subtype":"init","model":"claude-test"}"#;
        assert_eq!(
            claude_activity(event).lines,
            ["session started (model claude-test)"]
        );
        for event in [
            r#"{"type":"system","subtype":"other"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ignored"}]}}"#,
            r#"{"type":"assistant","message":{}}"#,
            r#"{"type":"unknown"}"#,
            "null",
        ] {
            assert!(claude_activity(event).lines.is_empty());
        }
    }

    #[test]
    fn claude_non_json_passes_through() {
        for line in ["plain output", "{invalid json", "", "  keep spacing  "] {
            assert_eq!(claude_activity(line).lines, [line]);
        }
    }

    fn capture_stream(input: &str, claude: bool) -> (String, String, State) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = Mutex::new(State {
            phase: String::new(),
            goal: String::new(),
            current_stage: None,
            current_step: String::new(),
            run_started_unix: 0,
            agent_role: String::new(),
            agent_tool: String::new(),
            agent_model: String::new(),
            agent_started_unix: 0,
            agent_lines: 0,
            agent_last_line: String::new(),
        });
        let tail = stream_agent_output(input.as_bytes(), &log, &state, 600, claude).unwrap();
        let output = String::from_utf8(log.lock().unwrap().clone()).unwrap();
        (tail, output, state.into_inner().unwrap())
    }

    #[test]
    fn claude_stream_updates_activity_and_prefers_result_for_history() {
        let input = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Working\"}]}}\n",
            "{\"type\":\"result\",\"result\":\"Done\"}\n",
            "{\"type\":\"unknown\"}\n",
        );
        let (tail, output, state) = capture_stream(input, true);
        assert_eq!(tail, "Done");
        assert_eq!(output, "Working\n✔ result: Done\n");
        assert_eq!(state.agent_lines, 2);
        assert_eq!(state.agent_last_line, "✔ result: Done");
    }

    #[test]
    fn claude_stream_history_falls_back_to_raw_output() {
        let input = "{\"type\":\"unknown\"}\nplain output";
        let (tail, output, state) = capture_stream(input, true);
        assert_eq!(tail, input);
        assert_eq!(output, "plain output\n");
        assert_eq!(state.agent_lines, 1);
        assert_eq!(state.agent_last_line, "plain output");
    }

    #[test]
    fn raw_stream_preserves_codex_and_stderr_behavior() {
        let input = "  Working  \r\n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone";
        let (tail, output, state) = capture_stream(input, false);
        let expected = "  Working  \n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone\n";
        assert_eq!(tail, expected.trim());
        assert_eq!(output, expected);
        assert_eq!(state.agent_lines, 4);
        assert_eq!(state.agent_last_line, "Done");
    }
}
