//! Agent subprocess execution, activity logging, and production mock support.
use super::Ctx;
use crate::agent::{AgentUsage, AgentRequest, AgentResult, stream_agent_result_on};
use crate::usage::accumulate_invocation_usage;
use crate::util::{clock_hms, unix_timestamp};
use serde_json::json;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct PreparedAgentCommand {
    command: Command,
    prompt_stdin: bool,
    codex_home: Option<PathBuf>,
}

/// Results stay unevaluated until process cleanup, thread joins and activity clearing finish.
struct SupervisionResults {
    stdout: Result<AgentResult, String>,
    stderr: Result<String, String>,
    status: std::io::Result<ExitStatus>,
    prompt_input: Result<(), String>,
}

impl Ctx {
    fn open_agent_log(&self, role: &str, tool: &str, model: &str) -> Result<Arc<Mutex<crate::agent_log::InvocationLog>>, String> {
        crate::agent_log::InvocationLog::start(&self.forge_path(""),
            &format!("=== [{role}] {tool} ({model}) started {} ===\n", clock_hms()))
            .map(|log| Arc::new(Mutex::new(log)))
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
            return self.mock_agent_result(request);
        }
        let provider = crate::catalogue::Provider::parse(tool).ok_or_else(|| format!("unknown tool {tool}"))?;
        let policy = crate::catalogue::Policy::from_settings(&self.app.settings.lock().unwrap())?;
        let mut selection = self.execution_selection(&policy, provider, model, effort);
        self.session.state.lock().unwrap().model_selection = selection.clone();
        if selection["eligible"] != true {
            return Err(self.agent_error(role, selection["error"].as_str().unwrap_or("model unavailable").into()));
        }
        if self.session.stop_requested.load(Ordering::SeqCst) {
            return Err("provider launch stopped; saved work retained".into());
        }
        let requested_model = model.to_string();
        let PreparedAgentCommand { command: mut cmd, prompt_stdin, codex_home } =
            self.prepare_agent_command(request)?;
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
        let child = match child {
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
        let results = self.supervise_agent(child, request, &log);
        self.clear_agent_activity();
        results.prompt_input.map_err(|e| self.agent_error(role, e))?;
        let mut result = results.stdout?;
        let tail = result.output.clone();
        let mut usage = result.usage.take();
        let etail = match results.stderr {
            Ok(tail) => tail,
            Err(e) => {
                self.agent_error(role, format!("{tool} output failed: {e}"));
                return Err(e);
            }
        };
        let status = match results.status {
            Ok(status) => status,
            Err(e) => {
                return Err(self.agent_error(role, format!("failed to wait for {tool}: {e}")));
            }
        };

        if !status.success() {
            let provider_error = result.error.as_deref().unwrap_or("");
            let diagnostic = format!("{etail}\n{tail}\n{provider_error}").to_lowercase();
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
            self.log_event("error", &format!("[{role}] {tool} failed: {etail} {provider_error}"));
            return Err(format!("{tool} exited with {status}: {etail} {tail} {provider_error}"));
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

    fn mock_agent_result(&self, request: &AgentRequest<'_>) -> Result<AgentResult, String> {
        let AgentRequest { role, provider: tool, model, .. } = *request;
        #[cfg(test)]
        let prompt = request.prompt;
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
        Ok(AgentResult { output, usage, effective_model, model_reported:true, completed: true, ..AgentResult::default() })
    }

    fn prepare_agent_command(&self, request: &AgentRequest<'_>) -> Result<PreparedAgentCommand, String> {
        let AgentRequest { role, provider: tool, .. } = *request;
        let executable = tool.to_string();
        #[cfg(test)]
        let executable = self.app.settings.lock().unwrap()[format!("test_cli_{tool}")].as_str().unwrap_or(&executable).to_string();
        crate::agent::verify_capabilities(request, self.project(), &executable)?;
        let built = crate::agent::command(request)?;
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
        Ok(PreparedAgentCommand { command: cmd, prompt_stdin, codex_home })
    }

    fn supervise_agent(
        &self,
        mut child: Child,
        request: &AgentRequest<'_>,
        log: &Arc<Mutex<crate::agent_log::InvocationLog>>,
    ) -> SupervisionResults {
        let tool = request.provider;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = child.stdin.take();
        let stderr_log = Arc::clone(log);
        let pid = child.id() as i32;
        let failed_reader = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let input_writer = scope.spawn(|| {
                let result = stdin.map_or(Ok(()), |mut input| input.write_all(request.prompt.as_bytes()));
                if result.is_err() { failed_reader.store(true, Ordering::SeqCst); }
                result.map_err(|e| format!("agent prompt input failed: {e}"))
            });
            let stdout_reader = scope.spawn(|| {
                let result = stream_agent_result_on(stdout, log, &self.session.state, 1500, tool == "claude", "stdout");
                if result.is_err() { failed_reader.store(true, Ordering::SeqCst); }
                result
            });
            let stderr_reader = scope.spawn(|| {
                let result = stream_agent_result_on(stderr, &stderr_log, &self.session.state, 1500, false, "stderr").map(|r| r.output);
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
            SupervisionResults {
                stdout: stdout_reader.join().unwrap_or_else(|_| Err("agent stdout reader panicked".into())),
                stderr: stderr_reader.join().unwrap_or_else(|_| Err("agent stderr reader panicked".into())),
                status,
                prompt_input: input_writer.join().unwrap_or_else(|_| Err("agent prompt writer panicked".into())),
            }
        })
    }

    pub(super) fn log_agent_finished(&self, role: &str, tool: &str, tail: &str, usage: Option<&AgentUsage>) {
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
                {
                    let mut settings = self.app.settings.lock().unwrap();
                    if let Some(output) = settings.get_mut("mock_plan_output") {
                        // An array scripts successive candidates, so a rejected
                        // one followed by a repair round can be exercised.
                        let output = match output.as_array_mut() {
                            Some(queue) if queue.len() > 1 => queue.remove(0),
                            Some(queue) => queue.first().cloned().unwrap_or(serde_json::Value::Null),
                            None => output.clone(),
                        };
                        drop(settings);
                        // Null simulates an agent exiting successfully without writing a plan.
                        if !output.is_null() {
                            fs::write(self.forge_path("plan-candidate.json"),
                                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()))
                                .map_err(|e| e.to_string())?;
                        }
                        return Ok(());
                    }
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
}
