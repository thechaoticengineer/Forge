use crate::agent::{claude_activity, stream_agent_output};
use crate::app::State;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

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
        model_selection: Value::Null,
        architect_activity: Value::Null,
        role_usage: Value::Null,
        agent_started_unix: 0,
        agent_lines: 0,
        agent_last_line: String::new(),
    });
    let (tail, _) = stream_agent_output(input.as_bytes(), &log, &state, 600, claude).unwrap();
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

#[test]
fn claude_long_context_init_and_wire_model_are_the_same_identity() {
    let input = [
        json!({"type":"system","subtype":"init","model":"claude-opus-5[1m]"}),
        json!({"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"Review complete"}]}}),
        json!({"type":"result","subtype":"success","is_error":false,"result":"approved",
            "usage":{"input_tokens":10,"output_tokens":2},"modelUsage":{"claude-opus-5":{"inputTokens":10,"outputTokens":2}}})
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    let log = Arc::new(Mutex::new(Vec::new()));
    let result = crate::agent::stream_agent_result(input.as_bytes(), &log, &Mutex::new(State::default()), 4096, true).unwrap();
    assert!(result.completed && result.model_reported);
    // Preserve provider-reported evidence; compare the context suffix at identity boundaries.
    assert_eq!(result.effective_model,"claude-opus-5");
    assert!(crate::agent::same_model("claude","claude-opus-5[1m]",&result.effective_model));
    assert!(!crate::agent::same_model("claude","claude-fable-5-1[1m]",&result.effective_model));
}

#[test]
fn model_identity_equivalence_is_limited_to_claude_context_suffix() {
    use crate::agent::same_model;
    assert!(same_model("claude","claude-opus-5","claude-opus-5[1m]"));
    assert!(same_model("codex","gpt-6-astra","gpt-6-astra"));
    for (provider, expected, reported) in [
        ("claude","claude-opus-5[1m]","claude-opus-4"),
        ("claude","claude-opus-5[1m]","claude-sonnet-5"),
        ("claude","claude-opus-5[1m]","claude-opus-5[2m]"),
        ("claude","opus[1m]","claude-opus-5"),
        ("claude","claude-opus-5","<synthetic>"),
        ("codex","claude-opus-5[1m]","claude-opus-5"),
        ("claude","",""),
        ("claude","claude-opus-5[1m]",""),
    ] { assert!(!same_model(provider,expected,reported),"{provider}: {expected} / {reported}"); }
}
