use crate::app::State;
use crate::util::last_chars;
use serde_json::Value;
use std::io::{BufRead as _, BufReader};
use std::sync::{Arc, Mutex};

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
    pub(crate) lines: Vec<String>,
    pub(crate) result: Option<String>,
    pub(crate) usage: Option<AgentUsage>,
}

pub(crate) fn claude_activity(line: &str) -> ClaudeActivity {
    let event: Value = match serde_json::from_str(line) {
        Ok(event) => event,
        Err(_) => {
            return ClaudeActivity {
                lines: vec![line.to_string()],
                result: None,
                usage: None,
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
            if let Some(usage) = event["usage"].as_object() {
                let tokens = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
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

pub(crate) fn stream_agent_output<R: std::io::Read, W: std::io::Write>(
    input: R,
    log: &Arc<Mutex<W>>,
    state: &Mutex<State>,
    tail_limit: usize,
    claude: bool,
) -> Result<(String, Option<AgentUsage>), String> {
    let mut reader = BufReader::new(input);
    let mut bytes = Vec::new();
    let mut tail = String::new();
    let mut result_tail = None;
    let mut usage = None;

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
            if activity.usage.is_some() {
                usage = activity.usage;
            }
            activity.lines
        } else {
            if let Some(candidate) = codex_usage(line) {
                usage = Some(candidate);
            }
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

    Ok((result_tail.unwrap_or(tail).trim().to_string(), usage))
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
        assert_eq!(codex_usage("tokens used: 10 20").unwrap().total_tokens, 1020);
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
        assert_eq!(*log.lock().unwrap(), expected.as_bytes());
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
