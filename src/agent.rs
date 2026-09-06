use crate::app::State;
use crate::util::last_chars;
use serde_json::Value;
use std::io::{BufRead as _, BufReader};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct ClaudeActivity {
    pub(crate) lines: Vec<String>,
    pub(crate) result: Option<String>,
}

pub(crate) fn claude_activity(line: &str) -> ClaudeActivity {
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

pub(crate) fn stream_agent_output<R: std::io::Read, W: std::io::Write>(
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
