use crate::app::State;
use crate::agent_log::{Message, Sink};
use crate::util::last_chars;
use serde_json::Value;
use std::io::{BufRead as _, BufReader};
use std::sync::{Arc, Mutex};

pub(crate) mod codex_session;

/// Provider-native request. Only the architect may attach an authoritative session.
#[derive(Clone, Debug)]
pub(crate) struct AgentRequest<'a> {
    pub role: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub session: Option<&'a str>,
    pub prompt: &'a str,
}
#[derive(Default, Debug)]
pub(crate) struct AgentResult {
    pub output: String,
    pub session: Option<String>,
    pub effective_model: String,
    pub model_reported: bool,
    pub usage: Option<AgentUsage>,
    pub completed: bool,
    pub error: Option<String>,
}

/// Claude's CLI/catalogue may append its long-context option to the model ID,
/// while assistant messages report the same wire model without it. Resolve
/// aliases through the catalogue first; never equate model families or versions.
pub(crate) fn same_model(provider: &str, expected: &str, reported: &str) -> bool {
    fn identity(model: &str) -> &str {
        if model.starts_with("claude-") { model.strip_suffix("[1m]").unwrap_or(model) }
        else { model }
    }
    !expected.is_empty() && !reported.is_empty()
        && if provider == "claude" { identity(expected) == identity(reported) }
            else { expected == reported }
}
pub(crate) fn session_id(id: &str) -> bool {
    // Both CLIs emit UUIDs. Refuse names, paths and option-like values.
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

pub(crate) fn command(request: &AgentRequest<'_>) -> Result<std::process::Command, String> {
    use std::process::Command;
    let AgentRequest {
        role,
        provider,
        model,
        effort,
        session,
        prompt,
    } = *request;
    if session.is_some_and(|s| !session_id(s)) || (session.is_some() && !matches!(role, "architect" | "architect_review")) {
        return Err("only architect may resume an exact UUID session".into());
    }
    if !model.is_empty() && !crate::catalogue::identifier(model) {
        return Err("invalid explicit model".into());
    }
    if !matches!(
        role,
        "architect" | "architect_review" | "chat" | "planner" | "implementer" | "fixer" | "reviewer"
    ) {
        return Err("unknown agent role".into());
    }
    let review = matches!(role, "architect_review" | "reviewer");
    let readonly = matches!(role, "architect" | "chat" | "planner") || review;
    let mut c = Command::new(provider);
    match provider {
        "codex" => {
            c.arg("exec");
            if readonly {
                c.args([
                    "--sandbox",
                    if review { "workspace-write" } else { "read-only" },
                    "--ignore-user-config",
                    "--ignore-rules",
                    "-c",
                    "approval_policy=\"never\"",
                    "-c",
                    "mcp_servers={}",
                    "-c",
                    "features.apps=false",
                    "-c",
                    "features.multi_agent=false",
                    "-c",
                    "features.hooks=false",
                    "-c",
                    "features.browser_use=false",
                    "-c",
                    "features.computer_use=false",
                    "-c",
                    "features.image_generation=false",
                ]);
                if review {
                    // The outer read-only Bubblewrap mount remains authoritative.
                    // Checks in scratch space need sockets for loopback fixtures.
                    c.args(["-c", "sandbox_workspace_write.network_access=true"]);
                }
            } else {
                c.arg("--dangerously-bypass-approvals-and-sandbox");
            }
            if let Some(id) = session {
                c.args(["resume", id]);
            }
            c.args(["--json", "--skip-git-repo-check"]);
            if !model.is_empty() {
                c.args(["-m", model]);
            }
            if effort != "provider_default" {
                if !crate::catalogue::identifier(effort) {
                    return Err("unsupported native Codex effort".into());
                }
                c.args([
                    "-c",
                    &format!("model_reasoning_effort={}", serde_json::json!(effort)),
                ]);
            }
        }
        "claude" => {
            c.args(["-p", "--verbose", "--output-format", "stream-json"]);
            if readonly {
                c.args([
                    "--safe-mode",
                    "--permission-mode",
                    "dontAsk",
                    "--tools",
                    if review { "Read,Glob,Grep,Bash" } else { "Read,Glob,Grep" },
                    "--strict-mcp-config",
                    "--mcp-config",
                    "{\"mcpServers\":{}}",
                    "--settings",
                    "{\"disableAllHooks\":true}",
                ]);
            } else {
                c.arg("--dangerously-skip-permissions");
            }
            if review { c.args(["--allowedTools", "Read,Glob,Grep,Bash"]); }
            if let Some(id) = session {
                c.args(["--resume", id]);
            }
            if !model.is_empty() {
                c.args(["--model", model]);
            }
            if effort != "provider_default" {
                if !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
                    return Err("unsupported native Claude effort".into());
                }
                c.args(["--effort", effort]);
            }
        }
        _ => return Err(format!("unknown tool {provider}")),
    }
    c.arg(prompt);
    Ok(c)
}

/// Probe before paid work; unsupported permission flags are a recoverable capability failure.
pub(crate) fn verify_capabilities(
    request: &AgentRequest<'_>,
    cwd: &str,
    executable: &str,
) -> Result<(), String> {
    use crate::catalogue_process::{Budget, CommandSpec, Launcher, SystemLauncher};
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    if !matches!(request.role, "architect" | "architect_review" | "reviewer" | "chat" | "planner") {
        return Ok(());
    }
    let args = if request.provider == "codex" {
        vec!["exec".into(), "--help".into()]
    } else {
        vec!["--help".into()]
    };
    let spec = CommandSpec {
        executable: executable.into(),
        args,
        cwd: cwd.into(),
    };
    let mut child = SystemLauncher
        .spawn(
            &spec,
            Budget {
                deadline: Instant::now() + Duration::from_secs(5),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .map_err(|_| "recoverable capability failure: cannot inspect provider help")?;
    let mut help = String::new();
    while let Some(line) = child
        .line()
        .map_err(|_| "recoverable capability failure: help failed")?
    {
        help.push_str(&line);
        help.push('\n');
    }
    child
        .finish()
        .map_err(|_| "recoverable capability failure: help failed")?;
    let flags: &[&str] = if request.provider == "codex" {
        &[
            "--sandbox",
            "read-only",
            "--ignore-user-config",
            "--ignore-rules",
            "--json",
            "resume",
        ]
    } else {
        &[
            "--safe-mode",
            "--tools",
            "--permission-mode",
            "dontAsk",
            "--strict-mcp-config",
            "--settings",
            "--resume",
        ]
    };
    if flags.iter().any(|flag| !help.contains(flag)) {
        return Err(
            "recoverable capability failure: provider cannot enforce read-only session permissions"
                .into(),
        );
    }
    Ok(())
}

/// Reviews can execute checks in private scratch space, but cannot write repository,
/// Git metadata, Forge state, or arbitrary host paths. Fail closed if bwrap is absent.
pub(crate) fn review_sandbox(command: &std::process::Command, project: &str, provider: &str) -> Result<std::process::Command, String> {
    use std::path::PathBuf;
    let mut c = std::process::Command::new("bwrap");
    c.args(["--die-with-parent", "--new-session", "--unshare-pid", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]);
    // Provider session persistence is the only writable host state. The provider
    // flags independently disable hooks, apps and external tool servers.
    let home = std::env::var_os("HOME").ok_or("missing provider home")?;
    let home = PathBuf::from(home);
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
    let project = std::fs::canonicalize(project).map_err(|e| e.to_string())?;
    for path in [if provider == "codex" { codex } else { claude }] {
        let path = std::fs::canonicalize(path).map_err(|e| format!("review provider state unavailable: {e}"))?;
        if project.starts_with(&path) || path.starts_with(&project) { return Err("review state overlaps implementation".into()); }
        c.arg("--bind").arg(&path).arg(&path);
    }
    // /tmp may contain the project in tests or user checkouts; restore it readonly.
    c.arg("--ro-bind").arg(&project).arg(&project);
    c.arg("--chdir").arg(&project).arg("--").arg(command.get_program()).args(command.get_args());
    Ok(c)
}

#[derive(Default, Clone, Debug)]
pub(crate) struct AgentUsage {
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) total_tokens: i64,
    pub(crate) model: String,
}

impl AgentUsage {
    pub(crate) fn is_empty(&self) -> bool {
        self.total_tokens == 0
    }
}

#[derive(Default)]
pub(crate) struct ClaudeActivity {
    #[cfg(test)]
    pub(crate) lines: Vec<String>,
    pub(crate) result: Option<String>,
    pub(crate) usage: Option<AgentUsage>,
}

pub(crate) fn claude_activity(line: &str) -> ClaudeActivity {
    let event: Value = match serde_json::from_str(line) {
        Ok(event) => event,
        Err(_) => {
            return ClaudeActivity {
                #[cfg(test)]
                lines: vec![line.to_string()],
                result: None,
                usage: None,
            };
        }
    };
    let mut activity = ClaudeActivity::default();
    match event["type"].as_str() {
        Some("result") => {
            if let Some(usage) = event["usage"].as_object() {
                let tokens = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0).max(0);
                let input_tokens = tokens("input_tokens")
                    .saturating_add(tokens("cache_creation_input_tokens"))
                    .saturating_add(tokens("cache_read_input_tokens"));
                let output_tokens = tokens("output_tokens");
                let model = event["modelUsage"]
                    .as_object()
                    .and_then(|models| {
                        models.iter().max_by_key(|(_, usage)| {
                            [
                                ("inputTokens", "input_tokens"),
                                ("outputTokens", "output_tokens"),
                                ("cacheCreationInputTokens", "cache_creation_input_tokens"),
                                ("cacheReadInputTokens", "cache_read_input_tokens"),
                            ]
                            .iter()
                            .map(|(camel, snake)| {
                                usage[*camel]
                                    .as_i64()
                                    .or_else(|| usage[*snake].as_i64())
                                    .unwrap_or(0) as i128
                            })
                            .sum::<i128>()
                        })
                    })
                    .map(|(model, _)| model.clone())
                    .unwrap_or_default();
                activity.usage = Some(AgentUsage {
                    input_tokens,
                    output_tokens,
                    total_tokens: input_tokens.saturating_add(output_tokens),
                    model,
                });
            }
            activity.result = event["result"].as_str().map(str::to_string);
        }
        _ => {}
    }
    #[cfg(test)]
    { activity.lines = retained_messages(Some(&event), line, true, "stdout").iter()
        .map(|m| m.readable().strip_suffix('\n').unwrap_or(&m.text).to_string()).collect(); }
    activity
}

/// Presentation retention deliberately does not participate in outcome parsing.
pub(crate) fn retained_messages(event: Option<&Value>, raw: &str, claude: bool, stream: &str) -> Vec<Message> {
    let plain = || {
        let mut message = Message::new(if stream == "stderr" { "error" } else { "plain" }, stream, "", raw);
        message.verbatim = true;
        vec![message]
    };
    if stream == "stderr" { return plain(); }
    let Some(event) = event else { return plain(); };
    let mut records = Vec::new();
    let mut add = |kind: &str, label: &str, text: &str| records.push(Message::new(kind, stream, label, text));
    if claude {
        match event["type"].as_str() {
            Some("assistant" | "user") => {
                for content in event["message"]["content"].as_array().into_iter().flatten() {
                    match content["type"].as_str() {
                        Some("text" | "thinking") => {
                            let key = if content["type"] == "thinking" { "thinking" } else { "text" };
                            if let Some(text) = content[key].as_str() { add("message", "", text); }
                        }
                        Some("tool_use") => {
                            let name = content["name"].as_str().unwrap_or("tool");
                            // Preserve every distinct input string, including less familiar
                            // fields. Nested inputs remain complete JSON, never summaries.
                            if let Some(input) = content["input"].as_object() {
                                for (key, value) in input {
                                    let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                                    add("tool_input", &format!("» {name} ({key}): "), &text);
                                }
                            } else if !content["input"].is_null() {
                                let text = content["input"].as_str().map(str::to_string).unwrap_or_else(|| content["input"].to_string());
                                add("tool_input", &format!("» {name}: "), &text);
                            }
                        }
                        Some("tool_result") => {
                            let kind = if content["is_error"] == true { "error" } else { "tool_output" };
                            if let Some(text) = content["content"].as_str() { add(kind, "« tool: ", text); }
                            else if let Some(parts) = content["content"].as_array() {
                                for part in parts {
                                    let text = part["text"].as_str().map(str::to_string).unwrap_or_else(|| part.to_string());
                                    add(kind, "« tool: ", &text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                let error = event["is_error"] == true || event["subtype"].as_str().is_some_and(|s| s != "success");
                let kind = if error { "error" } else { "result" };
                let label = if error { "✗ result: " } else { "✔ result: " };
                if (error || event["result"].is_null()) && let Some(subtype) = event["subtype"].as_str() { add(kind, label, subtype); }
                if let Some(text) = event["result"].as_str() { add(kind, label, text); }
                if let Some(text) = event["errors"].as_str() { add("error", "✗ diagnostic: ", text); }
                for diagnostic in event["errors"].as_array().into_iter().flatten() {
                    let text = diagnostic.as_str().map(str::to_string).unwrap_or_else(|| diagnostic.to_string());
                    add("error", "✗ diagnostic: ", &text);
                }
                if !event["structured_output"].is_null() { add(kind, "structured result: ", &event["structured_output"].to_string()); }
            }
            Some("error") => {
                if let Some(text) = event["error"]["message"].as_str().or_else(|| event["message"].as_str()).or_else(|| event["error"].as_str()) { add("error", "", text); }
                else { add("error", "", raw); }
            }
            Some("system") if event["subtype"] == "init" => {
                add("status", "", &format!("session started (model {})", event["model"].as_str().unwrap_or("unknown")));
            }
            _ => {}
        }
    } else {
        match event["type"].as_str() {
            Some("thread.started") => add("status", "", "session started"),
            Some("turn.completed") => add("status", "", "turn completed"),
            Some("turn.failed" | "error") => {
                let text = event["message"].as_str().or_else(|| event["error"]["message"].as_str()).or_else(|| event["error"].as_str()).unwrap_or(raw);
                add("error", "", text);
            }
            Some("item.started" | "item.completed") => {
                let item = &event["item"];
                for (key, kind, label) in [("text", "message", ""), ("message", "error", ""), ("command", "tool_input", "» command: "),
                    ("aggregated_output", "tool_output", "« output: "), ("output", "tool_output", "« output: "),
                    ("input", "tool_input", "» input: "), ("arguments", "tool_input", "» arguments: "),
                    ("result", "tool_output", "« result: "), ("error", "error", "✗ error: ")] {
                    if let Some(value) = item.get(key) {
                        let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                        add(if item["type"] == "error" { "error" } else { kind }, label, &text);
                    }
                }
            }
            _ => return plain(),
        }
    }
    records
}

fn codex_usage(line: &str) -> Option<AgentUsage> {
    if line.trim_start().starts_with("» ") {
        return None;
    }
    let lower = line.to_ascii_lowercase();
    let (_, footer) = lower.split_once("tokens used")?;
    // Spaces within a digit group are separators, like commas and underscores:
    // even "10 20" intentionally represents 1020, not two separate counts.
    let number = footer
        .rsplit(|c: char| !c.is_ascii_digit() && !matches!(c, ',' | '_' | ' '))
        .find(|part| part.chars().any(|c| c.is_ascii_digit()))?;
    let total_tokens = number
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse::<i64>()
        .ok()?;
    Some(AgentUsage {
        total_tokens,
        ..AgentUsage::default()
    })
}

pub(crate) fn stream_agent_result_on<R: std::io::Read, W: Sink>(
    input: R,
    log: &Arc<Mutex<W>>,
    state: &Mutex<State>,
    tail_limit: usize,
    claude: bool,
    stream: &str,
) -> Result<AgentResult, String> {
    let mut decoded = AgentResult::default();
    let mut reader = BufReader::new(input);
    let mut bytes = Vec::new();
    let mut tail = String::new();
    let mut result_tail = None;
    let mut usage = None;

    loop {
        bytes.clear();
        let read = std::io::Read::take(&mut reader, 1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| format!("failed to read agent output: {e}"))?;
        if read > 1024 * 1024 {
            return Err("agent event exceeds 1 MiB".into());
        }
        if read == 0 {
            break;
        }

        let lossy = String::from_utf8_lossy(&bytes);
        let line = lossy.trim_end_matches(&['\r', '\n'][..]);
        // Retain raw output as a fallback when Claude never sends a result.
        tail.push_str(line);
        tail.push('\n');
        tail = last_chars(&tail, tail_limit);

        let event: Option<Value> = serde_json::from_str(line).ok();
        if let Some(event) = &event {
            let id = if claude {
                event["session_id"].as_str()
            } else {
                event["thread_id"].as_str()
            };
            if let Some(id) = id {
                if !session_id(id) || decoded.session.as_ref().is_some_and(|old| old != id) {
                    decoded.error = Some("invalid or changed session identity in stream".into());
                } else {
                    decoded.session = Some(id.into());
                }
            }
            if let Some(model) = event["model"]
                .as_str()
                .or_else(|| event["message"]["model"].as_str())
            {
                decoded.effective_model = model.into();
            }
        }
        if claude {
            let activity = claude_activity(line);
            if let Some(result) = activity.result {
                result_tail = Some(result);
            }
            if activity.usage.is_some() {
                usage = activity.usage;
            }
            if let Some(event) = &event {
                if event["type"] == "result" {
                    decoded.completed = event["is_error"] != true
                        && (event["subtype"].is_null() || event["subtype"] == "success");
                    if !decoded.completed {
                        decoded.error = Some(format!(
                            "Claude result failed: {}: {}",
                            event["subtype"],
                            event["result"].as_str().unwrap_or("no diagnostic")
                        ));
                    }
                    if event["structured_output"].is_object() {
                        result_tail = Some(event["structured_output"].to_string());
                    }
                }
            }
        } else if let Some(event) = &event {
            match event["type"].as_str() {
                Some("item.started" | "item.completed") => {
                    let item = &event["item"];
                    if item["type"] == "agent_message" && event["type"] == "item.completed" {
                        if let Some(text) = item["text"].as_str() {
                            result_tail = Some(text.into());
                        }
                    }
                }
                Some("turn.completed") => {
                    decoded.completed = true;
                    if let Some(u) = event["usage"].as_object() {
                        let n = |k| u.get(k).and_then(Value::as_i64).unwrap_or(0).max(0);
                        let input_tokens = n("input_tokens"); // cached_input_tokens is a subset
                        let output_tokens = n("output_tokens");
                        usage = Some(AgentUsage {
                            input_tokens,
                            output_tokens,
                            total_tokens: input_tokens.saturating_add(output_tokens),
                            model: String::new(),
                        });
                    }
                }
                Some("turn.failed" | "error") => {
                    let error = event["message"]
                        .as_str()
                        .or_else(|| event["error"]["message"].as_str())
                        .unwrap_or("provider turn failed");
                    decoded.error = Some(error.into());
                }
                _ => {},
            }
        } else {
            if let Some(candidate) = codex_usage(line) {
                usage = Some(candidate);
            }
        };
        if result_tail.as_ref().is_some_and(|s| s.len() > 128 * 1024) {
            return Err("agent result exceeds 128 KiB".into());
        }
        for message in retained_messages(event.as_ref(), &lossy, claude, stream) {
            log.lock().map_err(|_| "agent log lock was poisoned".to_string())?
                .append(&message).map_err(|e| format!("failed to persist agent log: {e}"))?;
            let line = message.readable();

            let mut s = state.lock().unwrap();
            s.agent_lines += 1;
            let latest = line.trim();
            if !latest.is_empty() {
                s.agent_last_line = latest.chars().take(200).collect();
            }
        }
    }

    decoded.output = result_tail.unwrap_or(tail).trim().to_string();
    if decoded.effective_model.is_empty() {
        decoded.effective_model = usage.as_ref().map(|u| u.model.clone()).unwrap_or_default();
    }
    decoded.model_reported = !decoded.effective_model.is_empty();
    decoded.usage = usage;
    Ok(decoded)
}

#[cfg(test)]
pub(crate) fn stream_agent_result<R: std::io::Read, W: Sink>(
    input: R, log: &Arc<Mutex<W>>, state: &Mutex<State>, tail_limit: usize, claude: bool,
) -> Result<AgentResult, String> {
    stream_agent_result_on(input, log, state, tail_limit, claude, "stdout")
}

#[cfg(test)]
pub(crate) fn stream_agent_output<R: std::io::Read, W: Sink>(
    input: R,
    log: &Arc<Mutex<W>>,
    state: &Mutex<State>,
    tail_limit: usize,
    claude: bool,
) -> Result<(String, Option<AgentUsage>), String> {
    let result = stream_agent_result(input, log, state, tail_limit, claude)?;
    Ok((last_chars(&result.output, tail_limit), result.usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_result_usage_includes_cache_and_selects_largest_model() {
        let event = json!({
            "type": "result", "result": "Done",
            "usage": {
                "input_tokens": 100, "output_tokens": 20,
                "cache_creation_input_tokens": 300, "cache_read_input_tokens": 400
            },
            "modelUsage": {
                "model-a": {"inputTokens": 100, "outputTokens": 20},
                "model-b": {"cacheCreationInputTokens": 300, "cacheReadInputTokens": 400},
                "model-z": {"inputTokens": 1}
            }
        });
        let input = format!("{event}\n{{\"type\":\"unknown\"}}\n");
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = Mutex::new(State::default());
        let (tail, usage) = stream_agent_output(input.as_bytes(), &log, &state, 600, true).unwrap();
        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, 800);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.total_tokens, 820);
        assert_eq!(usage.model, "model-b");
        assert!(!usage.is_empty());
        assert_eq!(tail, "Done");
        assert_eq!(*log.lock().unwrap(), b"\xe2\x9c\x94 result: Done\n");
        assert_eq!(state.lock().unwrap().agent_lines, 1);
    }

    #[test]
    fn claude_usage_fields_are_optional_and_defensive() {
        for fields in [
            json!({}),
            json!({
                "input_tokens": "invalid", "output_tokens": null,
                "cache_creation_input_tokens": false, "cache_read_input_tokens": 1.5
            }),
        ] {
            let event = json!({"type": "result", "usage": fields});
            let usage = claude_activity(&event.to_string()).usage.unwrap();
            assert_eq!(usage.input_tokens, 0);
            assert_eq!(usage.output_tokens, 0);
            assert!(usage.is_empty());
            assert!(usage.model.is_empty());
        }
        let event = json!({"type": "result", "usage": {"output_tokens": 42}});
        let usage = claude_activity(&event.to_string()).usage.unwrap();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 42);
        assert_eq!(usage.total_tokens, 42);
        assert!(usage.model.is_empty());
    }

    #[test]
    fn claude_result_without_usage_returns_none() {
        for line in [
            r#"{"type":"result","result":"Done"}"#,
            r#"{"type":"result","usage":null}"#,
            r#"{"type":"result","usage":"invalid"}"#,
            r#"{"type":"assistant","usage":{"input_tokens":100}}"#,
            "plain output",
        ] {
            assert!(claude_activity(line).usage.is_none(), "{line}");
        }
    }

    #[test]
    fn codex_footer_accepts_case_and_digit_separators() {
        for line in [
            "Tokens used: 1,234,567",
            "TOKENS USED = 1_234_567",
            "total tokens used: 1 234 567 tokens",
            "tokens used: input=10; total=1,234,567",
        ] {
            let usage = codex_usage(line).unwrap();
            assert_eq!(usage.total_tokens, 1_234_567, "{line}");
            assert_eq!(usage.input_tokens, 0);
            assert_eq!(usage.output_tokens, 0);
            assert!(usage.model.is_empty());
        }
        assert_eq!(
            codex_usage("tokens used: 10 20").unwrap().total_tokens,
            1020
        );
    }

    #[test]
    fn codex_stream_uses_last_footer_and_preserves_tail_and_logging() {
        let input = "Working\r\nTokens used: 123\nTOKENS USED: 1,234,567\n\
            \t » Bash: echo tokens used: 999\ntokens used: unavailable\nDone\n";
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = Mutex::new(State::default());
        let (tail, usage) =
            stream_agent_output(input.as_bytes(), &log, &state, 600, false).unwrap();
        assert_eq!(usage.unwrap().total_tokens, 1_234_567);
        let expected = input.replace("\r\n", "\n");
        assert_eq!(tail, expected.trim());
        assert_eq!(*log.lock().unwrap(), input.as_bytes());
        assert_eq!(state.lock().unwrap().agent_lines, 6);
        assert_eq!(state.lock().unwrap().agent_last_line, "Done");
    }

    #[test]
    fn unrelated_or_invalid_codex_lines_return_no_usage() {
        for line in [
            "",
            "Working",
            "1234567",
            "input_tokens: 100",
            "tokens used: unavailable",
            "12: tokens used: unavailable",
            "» Bash: echo Tokens used: 123",
            "  » Bash: echo Tokens used: 123",
            "\t » Bash: echo Tokens used: 123",
            "tokens used: 99999999999999999999999999999999999",
        ] {
            assert!(codex_usage(line).is_none(), "{line}");
            let log = Arc::new(Mutex::new(Vec::new()));
            let state = Mutex::new(State::default());
            let (_, usage) =
                stream_agent_output(line.as_bytes(), &log, &state, 600, false).unwrap();
            assert!(usage.is_none(), "{line}");
        }
    }
}

#[cfg(test)]
#[path = "agent_cli_tests.rs"]
mod cli_tests;
