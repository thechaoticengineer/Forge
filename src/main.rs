//! Forge — a minimal wrapper around AI coding agents.
//!
//! Loop: plan (codex/claude) -> human approves -> per stage:
//! implement (tool A) -> independent check (tool B, fresh session) ->
//! bounded fix loop -> commit proposed message -> next stage -> push.
//!
//! Serves a JSON API on 127.0.0.1:8734 for the Quickshell panel.

use serde_json::{Value, json};
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
    state: Mutex<State>,
    stop_requested: AtomicBool,
    busy: AtomicBool,
    gh_cache: Mutex<Option<(Instant, Value, Option<String>)>>,
}

struct State {
    project: String,
    settings: Value,
    phase: String, // idle|planning|plan_ready|running|blocked|done|failed
    goal: String,
    current_stage: Option<i64>,
    current_step: String,
    agent_role: String,
    agent_tool: String,
    agent_model: String,
    agent_started_unix: i64,
    agent_lines: i64,
    agent_last_line: String,
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
        "checker": "claude",
        "planner_model": "",
        "implementer_model": "",
        "checker_model": "",
        "max_fix_rounds": 3,
        "auto_push": true,
    })
}

impl App {
    fn project(&self) -> String {
        self.state.lock().unwrap().project.clone()
    }

    fn forge_path(&self, name: &str) -> PathBuf {
        let dir = PathBuf::from(self.project()).join(FORGE_DIR);
        let _ = fs::create_dir_all(&dir);
        dir.join(name)
    }

    fn log_event(&self, kind: &str, text: &str) {
        let t = clock_hms();
        let entry = json!({"t": t, "kind": kind, "text": text});
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
        let _ = fs::write(
            self.forge_path("plan.json"),
            serde_json::to_string_pretty(plan).unwrap(),
        );
    }

    fn set_phase(&self, phase: &str) {
        self.state.lock().unwrap().phase = phase.to_string();
    }

    fn set_step(&self, stage: Option<i64>, step: &str) {
        let mut s = self.state.lock().unwrap();
        s.current_stage = stage;
        s.current_step = step.to_string();
    }

    fn setting(&self, key: &str) -> String {
        self.state.lock().unwrap().settings[key]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    /// Atomically claim the busy flag; Err means other work is in flight.
    /// Prevents two requests that both saw busy=false from racing to spawn.
    fn acquire_busy(&self) -> Result<(), ()> {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| ())
    }

    /// Point Forge at an existing local git repository and recompute phase.
    fn set_project(&self, path: &str) -> Result<(), String> {
        if !PathBuf::from(path).join(".git").exists() {
            return Err(format!("{path} is not a git repository"));
        }
        self.state.lock().unwrap().project = path.to_string();
        let phase = if self.load_plan().is_some() { "plan_ready" } else { "idle" };
        self.set_phase(phase);
        Ok(())
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
            let mut s = self.state.lock().unwrap();
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
                stream_agent_output(stderr, &stderr_log, &self.state, 1500, false)
            });
            let stdout_result = stream_agent_output(stdout, &log, &self.state, 600, tool == "claude");
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
        let mut s = self.state.lock().unwrap();
        s.agent_role.clear();
        s.agent_tool.clear();
        s.agent_model.clear();
        s.agent_started_unix = 0;
        s.agent_lines = 0;
        s.agent_last_line.clear();
    }

    /// Fake agent used by the self-test. Never reachable from normal settings.
    fn mock_agent(&self, role: &str) -> Result<(), String> {
        match role {
            "planner" => {
                let goal = self.state.lock().unwrap().goal.clone();
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
            "checker" => {
                let _ = fs::write(
                    self.forge_path("verdict.json"),
                    json!({"approved": true, "issues": []}).to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------ orchestrator

    fn plan_worker(&self, goal: &str) {
        let prompt = PLANNER_PROMPT
            .replace("{goal}", goal)
            .replace("{plan_path}", &format!("{FORGE_DIR}/plan.json"));
        let result = self
            .run_agent("planner", &self.setting("planner"), &prompt, &self.setting("planner_model"))
            .and_then(|_| {
                self.load_plan()
                    .ok_or_else(|| "planner did not produce a valid plan file".to_string())
            });
        match result {
            Ok(mut plan) => {
                for st in plan["stages"].as_array_mut().unwrap() {
                    if st.get("status").and_then(Value::as_str).is_none() {
                        st["status"] = json!("pending");
                    }
                    if st.get("rounds").is_none() {
                        st["rounds"] = json!(0);
                    }
                }
                plan["goal"] = json!(goal);
                plan["status"] = json!("draft");
                let n = plan["stages"].as_array().unwrap().len();
                self.save_plan(&plan);
                self.set_phase("plan_ready");
                self.log_event("plan", &format!("plan ready with {n} stages"));
            }
            Err(e) => {
                self.set_phase("failed");
                self.log_event("error", &format!("planning failed: {e}"));
            }
        }
        self.busy.store(false, Ordering::SeqCst);
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

    /// Implement + independent check + bounded fix loop for one stage.
    fn run_one_stage(&self, plan: &mut Value, idx: usize) -> Result<&'static str, String> {
        let max_rounds = self.state.lock().unwrap().settings["max_fix_rounds"]
            .as_i64()
            .unwrap_or(3);
        let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
        let mut issues: Option<Vec<String>> = None;

        for round in 0..=max_rounds {
            if self.stop_requested.load(Ordering::SeqCst) {
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
            if self.stop_requested.load(Ordering::SeqCst) {
                return Ok("stopped");
            }

            self.set_step(Some(sid), "checking");
            let verdict_path = self.forge_path("verdict.json");
            let _ = fs::remove_file(&verdict_path);
            let p = self.stage_prompt(CHECK_PROMPT, plan, &stage);
            self.run_agent("checker", &self.setting("checker"), &p,
                           &self.setting("checker_model"))?;

            let verdict: Option<Value> = fs::read_to_string(&verdict_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            match verdict {
                Some(v) if v["approved"].as_bool() == Some(true) => {
                    self.log_event("check", &format!("stage {sid} approved by checker"));
                    return Ok("approved");
                }
                Some(v) => {
                    let list: Vec<String> = v["issues"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|i| i.as_str().map(String::from))
                                .collect()
                        })
                        .filter(|l: &Vec<String>| !l.is_empty())
                        .unwrap_or_else(|| vec!["reviewer rejected without details".into()]);
                    let mut summary = list.join("; ");
                    summary.truncate(1500);
                    self.log_event("check", &format!("stage {sid} rejected: {summary}"));
                    issues = Some(list);
                }
                None => {
                    self.log_event("error", "checker produced no readable verdict; retrying stage");
                    issues = Some(vec![
                        "The previous review session failed to produce a verdict. \
                         Re-verify the implementation end to end."
                            .into(),
                    ]);
                }
            }
        }
        Ok("exhausted")
    }

    fn run_worker(&self) {
        let result = self.run_worker_inner();
        if let Err(e) = result {
            self.set_phase("failed");
            self.log_event("error", &format!("run failed: {e}"));
        }
        self.set_step(None, "");
        self.stop_requested.store(false, Ordering::SeqCst);
        self.busy.store(false, Ordering::SeqCst);
    }

    fn run_worker_inner(&self) -> Result<(), String> {
        let mut plan = self.load_plan().ok_or("no plan")?;
        let count = plan["stages"].as_array().unwrap().len();
        for idx in 0..count {
            if plan["stages"][idx]["status"] == json!("committed") {
                continue;
            }
            if self.stop_requested.load(Ordering::SeqCst) {
                self.set_phase("plan_ready");
                self.log_event("run", "stopped by user; progress is saved, run again to continue");
                return Ok(());
            }
            let sid = plan["stages"][idx]["id"].as_i64().unwrap_or(0);
            let title = plan["stages"][idx]["title"].as_str().unwrap_or("").to_string();
            plan["stages"][idx]["status"] = json!("in_progress");
            self.save_plan(&plan);
            self.log_event("stage", &format!("stage {sid} started: {title}"));

            match self.run_one_stage(&mut plan, idx)? {
                "stopped" => {
                    self.set_phase("plan_ready");
                    self.log_event("run", "stopped by user; progress is saved, run again to continue");
                    return Ok(());
                }
                "exhausted" => {
                    plan["stages"][idx]["status"] = json!("blocked");
                    self.save_plan(&plan);
                    self.set_phase("blocked");
                    self.log_event("stage", &format!(
                        "stage {sid} blocked: checker still rejecting after max fix rounds — needs a human"));
                    return Ok(());
                }
                _approved => {
                    let msg = plan["stages"][idx]["commit"].as_str().unwrap_or("forge: stage").to_string();
                    let sha = self.commit_stage(&msg)?;
                    plan["stages"][idx]["status"] = json!("committed");
                    if let Some(sha) = sha {
                        plan["stages"][idx]["sha"] = json!(sha);
                    }
                    self.save_plan(&plan);
                }
            }
        }

        plan["status"] = json!("done");
        self.save_plan(&plan);
        if self.state.lock().unwrap().settings["auto_push"].as_bool() == Some(true) {
            self.set_step(None, "pushing");
            match self.git(&["push", "-u", "origin", "HEAD"]) {
                Ok(out) => self.log_event("git", &format!("pushed to origin: {}",
                    if out.is_empty() { "ok" } else { &out })),
                Err(e) => self.log_event("error",
                    &format!("push failed (commits are safe locally): {e}")),
            }
        }
        self.set_phase("done");
        self.log_event("run", "all stages committed — run complete");
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

const CHECK_PROMPT: &str = r#"You are an independent reviewer in a fresh session. Another agent implemented one stage of a plan in this repository. Judge only whether the current uncommitted changes correctly implement the stage.

STAGE: {title}
INSTRUCTIONS GIVEN TO THE IMPLEMENTER:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

Inspect with `git status` and `git diff` (all uncommitted changes belong to this stage), read files, and run checks if useful.
Then write your verdict as JSON to the file {verdict_path}:
{"approved": true/false, "issues": ["specific, actionable issue", ...]}

approved=true only if the acceptance criteria are met and you found no real defect.
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

fn handle(app: &Arc<App>, mut req: tiny_http::Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let method = req.method().clone();
    let mut body_text = String::new();
    let _ = req.as_reader().read_to_string(&mut body_text);
    let body: Value = serde_json::from_str(&body_text).unwrap_or(json!({}));
    let busy = app.busy.load(Ordering::SeqCst);

    match (method, path) {
        (tiny_http::Method::Get, "/api/state") => {
            let snap = {
                let s = app.state.lock().unwrap();
                json!({
                    "project": s.project,
                    "settings": s.settings,
                    "phase": s.phase,
                    "goal": s.goal,
                    "current_stage": s.current_stage,
                    "current_step": s.current_step,
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
            snap["plan"] = app.load_plan().unwrap_or(Value::Null);
            snap["history"] = app.read_history();
            snap["git_log"] = json!(app.git(&["log", "--oneline", "-12"]).unwrap_or_default());
            respond(req, 200, snap);
        }
        (tiny_http::Method::Get, "/api/agent_log") => {
            let data = fs::read(app.forge_path("agent.log")).unwrap_or_default();
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
            let diff = app.git(&["diff", "HEAD"]).unwrap_or_default();
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
            let mut s = app.state.lock().unwrap();
            if let Some(obj) = body.as_object() {
                for (k, v) in obj {
                    if s.settings.get(k).is_some() {
                        s.settings[k] = v.clone();
                    }
                }
            }
            drop(s);
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/project") => {
            let path = body["path"].as_str().unwrap_or("").trim().to_string();
            if busy {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                match app.set_project(&path) {
                    Ok(()) => respond(req, 200, json!({"ok": true})),
                    Err(e) => respond(req, 400, json!({"error": e})),
                }
            }
        }
        (tiny_http::Method::Post, "/api/project/select") => {
            let path = body["path"].as_str().unwrap_or("").trim().to_string();
            let repo = body["repo"].as_str().unwrap_or("").trim().to_string();
            if busy {
                respond(req, 409, json!({"error": "busy"}));
            } else if !path.is_empty() {
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
                if target.join(".git").exists() {
                    match app.set_project(&target.display().to_string()) {
                        Ok(()) => respond(req, 200, json!({"ok": true})),
                        Err(e) => respond(req, 400, json!({"error": e})),
                    }
                } else if app.acquire_busy().is_err() {
                    respond(req, 409, json!({"error": "busy"}));
                } else {
                    app.set_step(None, &format!("cloning {repo}"));
                    app.log_event("git", &format!("cloning {repo} into {}", target.display()));
                    let app2 = Arc::clone(app);
                    std::thread::spawn(move || {
                        let out = Command::new("gh")
                            .args(["repo", "clone", &repo])
                            .arg(&target)
                            .output();
                        match out {
                            Ok(o) if o.status.success() => {
                                match app2.set_project(&target.display().to_string()) {
                                    Ok(()) => app2.log_event("git",
                                        &format!("clone finished: {repo} -> {}", target.display())),
                                    Err(e) => app2.log_event("error",
                                        &format!("clone finished but selection failed: {e}")),
                                }
                            }
                            Ok(o) => app2.log_event("error", &format!(
                                "clone of {repo} failed: {}",
                                String::from_utf8_lossy(&o.stderr).trim())),
                            Err(e) => app2.log_event("error",
                                &format!("failed to launch gh clone: {e}")),
                        }
                        app2.set_step(None, "");
                        app2.busy.store(false, Ordering::SeqCst);
                    });
                    respond(req, 200, json!({"ok": true, "cloning": true}));
                }
            } else {
                respond(req, 400, json!({"error": "path or repo required"}));
            }
        }
        (tiny_http::Method::Post, "/api/plan") => {
            let goal = body["goal"].as_str().unwrap_or("").trim().to_string();
            if app.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                {
                    let mut s = app.state.lock().unwrap();
                    s.goal = goal.clone();
                    s.phase = "planning".into();
                }
                let mut short = goal.clone();
                short.truncate(300);
                app.log_event("plan", &format!("planning started for goal: {short}"));
                let app2 = Arc::clone(app);
                std::thread::spawn(move || app2.plan_worker(&goal));
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/approve") => match app.load_plan() {
            None => respond(req, 400, json!({"error": "no plan"})),
            Some(mut plan) => {
                plan["status"] = json!("approved");
                app.save_plan(&plan);
                app.log_event("plan", "plan approved by user");
                respond(req, 200, json!({"ok": true}));
            }
        },
        (tiny_http::Method::Post, "/api/run") => {
            let plan = app.load_plan();
            let status = plan
                .as_ref()
                .and_then(|p| p["status"].as_str())
                .unwrap_or("");
            if status != "approved" && status != "done" {
                respond(req, 400, json!({"error": "plan is not approved"}));
            } else if app.acquire_busy().is_err() {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                {
                    let mut s = app.state.lock().unwrap();
                    s.phase = "running".into();
                    if let Some(g) = plan.as_ref().and_then(|p| p["goal"].as_str()) {
                        s.goal = g.to_string();
                    }
                }
                app.stop_requested.store(false, Ordering::SeqCst);
                app.log_event("run", "run started");
                let app2 = Arc::clone(app);
                std::thread::spawn(move || app2.run_worker());
                respond(req, 200, json!({"ok": true}));
            }
        }
        (tiny_http::Method::Post, "/api/stop") => {
            app.stop_requested.store(true, Ordering::SeqCst);
            app.log_event("run", "stop requested; finishing current agent session");
            respond(req, 200, json!({"ok": true}));
        }
        (tiny_http::Method::Post, "/api/reset_plan") => {
            if busy {
                respond(req, 409, json!({"error": "busy"}));
            } else {
                let _ = fs::remove_file(app.forge_path("plan.json"));
                app.set_phase("idle");
                app.log_event("plan", "plan discarded");
                respond(req, 200, json!({"ok": true}));
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
    let app = Arc::new(App {
        state: Mutex::new(State {
            project,
            settings,
            phase: "idle".into(),
            goal: String::new(),
            current_stage: None,
            current_step: String::new(),
            agent_role: String::new(),
            agent_tool: String::new(),
            agent_model: String::new(),
            agent_started_unix: 0,
            agent_lines: 0,
            agent_last_line: String::new(),
        }),
        stop_requested: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        gh_cache: Mutex::new(None),
    });
    if app.load_plan().is_some() {
        app.set_phase("plan_ready");
    }
    let port: u16 = std::env::var("FORGE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(PORT);
    let server = tiny_http::Server::http(("127.0.0.1", port)).expect("bind server");
    println!(
        "Forge engine on http://127.0.0.1:{port}  (project: {})",
        app.project()
    );
    for req in server.incoming_requests() {
        let app = Arc::clone(&app);
        std::thread::spawn(move || handle(&app, req));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            project: String::new(),
            settings: json!({}),
            phase: String::new(),
            goal: String::new(),
            current_stage: None,
            current_step: String::new(),
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
