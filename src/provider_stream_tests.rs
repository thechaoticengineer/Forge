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
        ["Inspecting the code.".to_string(), "« tool: ignored".to_string(), "界".repeat(401)]
    );
    assert!(activity.result.is_none());
}

#[test]
fn claude_tool_use_retains_all_fields_and_whitespace() {
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
            "» Read (command): ignored",
            "» Read (file_path): src/main.rs",
            "» Bash (command): cargo build\n  cargo test",
            "» Custom (value): 42",
        ]
    );
}

#[test]
fn claude_tool_fields_keep_unicode_beyond_old_limit() {
    for field in ["file_path", "command", "pattern", "description", "url"] {
        let event = json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "name": "Tool", "input": {field: "界".repeat(161)}
            }]}
        });
        assert_eq!(
            claude_activity(&event.to_string()).lines,
            [format!("» Tool ({field}): {}", "界".repeat(161))]
        );
    }
}

#[test]
fn claude_result_keeps_full_text_for_history() {
    let result = "界".repeat(701);
    let event = json!({"type": "result", "subtype": "success", "result": result});
    let activity = claude_activity(&event.to_string());
    assert_eq!(activity.lines, [format!("✔ result: {result}")]);
    assert_eq!(activity.result.as_deref(), Some(result.as_str()));
}

#[test]
fn claude_error_result_keeps_subtype_and_diagnostic() {
    let event = r#"{"type":"result","is_error":true,"subtype":"error_during_execution","result":"failed"}"#;
    assert_eq!(
        claude_activity(event).lines,
        ["✗ result: error_during_execution", "✗ result: failed"]
    );
    let event = r#"{"type":"result","subtype":"error_max_turns"}"#;
    assert_eq!(claude_activity(event).lines, ["✗ result: error_max_turns"]);
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
        goal_enhancement: Value::Null,
        goal_enhancement_serial: 0,
        model_policy_suggestion: Value::Null,
        model_policy_suggestion_serial: 0,
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
    assert_eq!(output, "plain output");
    assert_eq!(state.agent_lines, 1);
    assert_eq!(state.agent_last_line, "plain output");
}

#[test]
fn raw_stream_preserves_codex_and_stderr_behavior() {
    let input = "  Working  \r\n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone";
    let (tail, output, state) = capture_stream(input, false);
    let expected = "  Working  \n{\"type\":\"result\",\"result\":\"raw JSON\"}\n\nDone\n";
    assert_eq!(tail, expected.trim());
    assert_eq!(output, input);
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

#[test]
fn complete_provider_messages_reach_durable_records_without_changing_outcomes() {
    use crate::agent::{stream_agent_result_on};
    use crate::agent_log::{InvocationLog, page};
    for claude in [false, true] {
        let fixture = crate::test_support::QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let texts = [format!("\r\n  {}\nsecond line\t \r\n", "界🦀".repeat(500)), "\nnext message\r\n  final  ".into()];
        let command = format!("\t{}\r\n  echo done  \n", "λ".repeat(501));
        // Larger than the page budget but below the defensive event limit.
        let output = format!("\n{}\r\n  ", "界".repeat(100_000));
        let diagnostic = format!("  error\n{}\t ", "ø".repeat(600));
        let mut events = Vec::new();
        for text in &texts {
            events.push(if claude { json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}}) }
                else { json!({"type":"item.completed","item":{"type":"agent_message","text":text}}) });
        }
        if claude {
            events.extend([
                json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":command}}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","content":output}]}}),
                json!({"type":"result","is_error":true,"subtype":"error_during_execution","result":diagnostic,"errors":["  additional\r\ndiagnostic \t"],"usage":{"input_tokens":10,"output_tokens":4}}),
            ]);
        } else {
            events.extend([
                json!({"type":"item.completed","item":{"type":"command_execution","command":command,"aggregated_output":output}}),
                json!({"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":4}}),
                json!({"type":"turn.failed","error":{"message":diagnostic}}),
            ]);
        }
        let input = events.iter().map(Value::to_string).collect::<Vec<_>>().join("\r\n");
        let log = Arc::new(Mutex::new(InvocationLog::start(&forge, "header\n").unwrap()));
        let result = stream_agent_result_on(input.as_bytes(), &log, &Mutex::new(State::default()), 100, claude, "stdout").unwrap();
        assert_eq!(result.usage.unwrap().total_tokens, 14);
        assert!(result.error.unwrap().contains(&diagnostic));
        assert_eq!(result.completed, !claude); // existing turn.completed semantics
        assert_eq!(result.output, if claude { diagnostic.trim() } else { texts[1].trim() });
        let first = page(&forge, fixture.app.project(), None, None, 0, 1, false).unwrap();
        let session = first["session"].as_str().unwrap();
        let mut entries = first["entries"].as_array().unwrap().clone();
        let mut cursor = first["next_cursor"].as_u64().unwrap();
        loop {
            let next = page(&forge, fixture.app.project(), Some(session), None, cursor, 100, false).unwrap();
            entries.extend(next["entries"].as_array().unwrap().clone());
            cursor = next["next_cursor"].as_u64().unwrap();
            if next["more"] == false { break; }
        }
        assert_eq!(entries[0]["text"], texts[0]);
        assert_eq!(entries[1]["text"], texts[1]);
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry["sequence"], i);
            assert_eq!(entry["id"], format!("{session}:{i}"));
        }
        for (kind, text) in [("tool_input", &command), ("tool_output", &output), ("error", &diagnostic)] {
            assert!(entries.iter().any(|e| e["kind"] == kind && e["text"] == *text));
        }
        let readable = std::fs::read_to_string(forge.join("agent.log")).unwrap();
        for text in [&texts[0], &texts[1], &command, &output, &diagnostic] { assert!(readable.contains(text)); }
        let empty = page(&forge, fixture.app.project(), Some(session), None, cursor, 5, false).unwrap();
        assert_eq!(empty["next_cursor"], cursor);
        assert_eq!(empty["entries"], json!([]));
    }
}

#[test]
fn concurrent_plain_stdout_stderr_keep_terminators_and_final_fragments() {
    use crate::agent::stream_agent_result_on;
    use crate::agent_log::{InvocationLog, page};
    let fixture = crate::test_support::QueueTest::new(false);
    let forge = fixture.app.forge_path("");
    let log = Arc::new(Mutex::new(InvocationLog::start(&forge, "").unwrap()));
    let stdout = "  stdout 界 \t\r\n\r\nlast stdout  ";
    let stderr = "\tstderr 🦀\r\n  next\nlast error\t";
    let state = Mutex::new(State::default());
    std::thread::scope(|scope| {
        let a = scope.spawn(|| stream_agent_result_on(stdout.as_bytes(), &log, &state, 600, true, "stdout").unwrap());
        let b = scope.spawn(|| stream_agent_result_on(stderr.as_bytes(), &log, &state, 600, false, "stderr").unwrap());
        a.join().unwrap(); b.join().unwrap();
    });
    let page = page(&forge, fixture.app.project(), None, None, 0, 100, false).unwrap();
    let entries = page["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 6);
    for (stream, text) in [("stdout", stdout), ("stderr", stderr)] {
        let found = entries.iter().filter(|e| e["stream"] == stream).map(|e| e["text"].as_str().unwrap()).collect::<String>();
        assert_eq!(found, text);
    }
    assert!(entries.iter().filter(|e| e["stream"] == "stderr").all(|e| e["kind"] == "error"));
    let all = entries.iter().map(|e| e["text"].as_str().unwrap()).collect::<String>();
    assert_eq!(std::fs::read_to_string(forge.join("agent.log")).unwrap(), all);
}

#[test]
fn retention_keeps_defensive_event_and_result_limits() {
    use crate::agent::stream_agent_result;
    for claude in [false, true] {
        let state = Mutex::new(State::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let input = "x".repeat(1024 * 1024 + 1);
        assert!(stream_agent_result(input.as_bytes(), &log, &state, 100, claude).unwrap_err().contains("1 MiB"));
        let text = "x".repeat(128 * 1024 + 1);
        let event = if claude { json!({"type":"result","result":text}) }
            else { json!({"type":"item.completed","item":{"type":"agent_message","text":text}}) };
        assert!(stream_agent_result(event.to_string().as_bytes(), &log, &state, 100, claude).unwrap_err().contains("128 KiB"));
        assert!(log.lock().unwrap().is_empty());
    }
}

#[test]
fn provider_error_shapes_and_json_stderr_retain_original_diagnostics() {
    use crate::agent::{retained_messages, stream_agent_result_on};
    let diagnostic = "\r\n  detailed error\n\ttrailing  ";
    for (claude, event) in [
        (true, json!({"type":"error","error":diagnostic})),
        (true, json!({"type":"error","error":{"message":diagnostic}})),
        (false, json!({"type":"error","error":diagnostic})),
        (false, json!({"type":"item.completed","item":{"type":"error","message":diagnostic}})),
    ] {
        let records = retained_messages(Some(&event), &event.to_string(), claude, "stdout");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, diagnostic);
        assert_eq!(records[0].kind, "error");
    }
    let input = format!("  {}  \r\n", json!({"type":"item.completed","item":{"type":"agent_message","text":diagnostic}}));
    let log = Arc::new(Mutex::new(Vec::new()));
    stream_agent_result_on(input.as_bytes(), &log, &Mutex::new(State::default()), 600, false, "stderr").unwrap();
    assert_eq!(*log.lock().unwrap(), input.as_bytes());
}
