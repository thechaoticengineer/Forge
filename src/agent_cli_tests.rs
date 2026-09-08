use super::*;
use crate::app::{App, Ctx};
use crate::catalogue::{Discovery, Model, Policy, Probe, Provider};
use crate::catalogue_process::Budget;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const ID: &str = "11111111-2222-4333-8444-555555555555";
struct Discovered;
impl Discovery for Discovered {
    fn probe(&self, _: Provider, _: &Policy, _: Budget) -> Probe {
        let model: Model = serde_json::from_value(
            json!({"id":"exact-model","model":"exact-model","aliases":[],
            "supported_efforts":["high","max"],"capabilities":{}}),
        )
        .unwrap();
        Probe {
            cli_version: Some("fixture".into()),
            result: Ok(vec![model]),
        }
    }
}
struct Cli {
    root: PathBuf,
    ctx: Ctx,
}
impl Cli {
    fn new(provider: &str, body: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "forge-cli-test-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(&root).unwrap();
        let script = root.join(provider);
        fs::write(&script, format!(r#"#!/usr/bin/env python3
import sys, json, os, subprocess, time
if '--help' in sys.argv:
    print('--sandbox read-only --ignore-user-config --ignore-rules --json resume --safe-mode --tools --permission-mode dontAsk --strict-mcp-config --settings --resume')
    sys.exit(0)
with open('argv.json','w') as f: json.dump(sys.argv[1:],f)
{body}
"#)).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let mut settings = crate::plan::default_settings();
        settings[format!("test_cli_{provider}")] = json!(script);
        settings["test_codex_home"] = json!(root.join("codex-home"));
        let mut app = App::new(root.to_str().unwrap(), settings);
        app.catalogue = crate::catalogue::Catalogue::new(
            Arc::new(Discovered),
            root.join("cache"),
            Duration::from_secs(2),
        );
        app.refresh_catalogue();
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.catalogue.running() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let app = Arc::new(app);
        let ctx = app.context(root.to_str().unwrap());
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }
    fn args(&self) -> Vec<String> {
        serde_json::from_slice(&fs::read(self.root.join("argv.json")).unwrap()).unwrap()
    }
}
impl Drop for Cli {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn request(provider: &str) -> AgentRequest<'_> {
    AgentRequest {
        role: "architect",
        provider,
        model: "exact-model",
        effort: "high",
        session: Some(ID),
        prompt: "Inspect; output JSON",
    }
}
fn pair(args: &[String], a: &str, b: &str) -> bool {
    args.windows(2).any(|p| p[0] == a && p[1] == b)
}

// Own the request in an unscoped worker so a pipe-cleanup regression cannot
// trap the test in a scoped thread join. Only fixture-recorded PIDs are killed.
fn run_bounded(fixture: &Cli, provider: &str, prompt: String) -> AgentResult {
    let ctx = fixture.ctx.clone();
    let provider = provider.to_string();
    let (send, receive) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        let result = ctx.run_agent(&AgentRequest { prompt: &prompt, ..request(&provider) });
        let _ = send.send(result);
    });
    match receive.recv_timeout(Duration::from_secs(5)) {
        Ok(result) => {
            task.join().unwrap();
            result.unwrap()
        }
        Err(error) => {
            fixture.ctx.session.stop_requested.store(true, Ordering::SeqCst);
            for (file, group) in [("parent.pid", true), ("descendant.pid", false)] {
                if let Some(pid) = fs::read_to_string(fixture.root.join(file)).ok()
                    .and_then(|pid| pid.parse::<i32>().ok()).filter(|pid| *pid > 0)
                {
                    unsafe { libc::kill(if group { -pid } else { pid }, libc::SIGKILL); }
                }
            }
            // Give cleanup a bounded chance to finish, even on a failing test.
            let _ = receive.recv_timeout(Duration::from_secs(2));
            panic!("fake CLI did not finish within five seconds: {error}");
        }
    }
}

fn successful_result_script(provider: &str) -> String {
    if provider == "codex" {
        format!(r#"print(json.dumps({{'type':'thread.started','thread_id':'{ID}'}}))
print(json.dumps({{'type':'turn.completed'}}))"#)
    } else {
        format!(r#"print(json.dumps({{'type':'result','subtype':'success','session_id':'{ID}','result':'ok'}}))"#)
    }
}

#[test]
fn prompt_stdin_threshold_is_strictly_greater_than_32768_bytes() {
    for provider in ["codex", "claude"] {
        for bytes in [32768, 32769] {
            let fixture = Cli::new(provider, &format!(r#"
with open('parent.pid','w') as f: f.write(str(os.getpid()))
with open('received-stdin','wb') as f: f.write(sys.stdin.buffer.read())
{}
"#, successful_result_script(provider)));
            // Multibyte text verifies the threshold measures bytes, not characters.
            let prompt = format!("{}{}", "ż".repeat(16384), if bytes == 32769 { "x" } else { "" });
            assert_eq!(prompt.len(), bytes);
            let result = run_bounded(&fixture, provider, prompt.clone());
            assert!(result.completed);
            assert_eq!(result.session.as_deref(), Some(ID));
            let mut expected: Vec<String> = command(&request(provider)).unwrap().get_args()
                .map(|arg| arg.to_str().unwrap().to_string()).collect();
            expected.pop();
            let input = fs::read(fixture.root.join("received-stdin")).unwrap();
            if bytes == 32768 {
                expected.push(prompt);
                assert!(input.is_empty());
            } else {
                if provider == "codex" { expected.push("-".into()); }
                assert_eq!(input, prompt.as_bytes());
            }
            assert_eq!(fixture.args(), expected);
        }
    }
}

#[test]
fn successful_parent_exit_kills_descendants_holding_output_pipes() {
    for provider in ["codex", "claude"] {
        let fixture = Cli::new(provider, &format!(r#"
with open('parent.pid','w') as f: f.write(str(os.getpid()))
# The descendant inherits both output pipes and outlives its successful parent.
child = subprocess.Popen(['sleep','60'])
with open('descendant.pid','w') as f: f.write(str(child.pid))
{}
"#, successful_result_script(provider)));
        let result = run_bounded(&fixture, provider, request(provider).prompt.into());
        assert!(result.completed);
        assert_eq!(result.session.as_deref(), Some(ID));
        assert!(fixture.ctx.session.state.lock().unwrap().agent_role.is_empty());
        let pid = fs::read_to_string(fixture.root.join("descendant.pid")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            if stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z") { break; }
            assert!(Instant::now() < deadline, "descendant survived successful parent exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn large_prompts_use_stdin_while_output_is_drained() {
    for provider in ["codex", "claude"] {
        let terminal = if provider == "codex" {
            format!(r#"print(json.dumps({{'type':'thread.started','thread_id':'{ID}'}}))
print(json.dumps({{'type':'turn.completed'}}))"#)
        } else {
            format!(r#"print(json.dumps({{'type':'result','subtype':'success','session_id':'{ID}','result':'ok'}}))"#)
        };
        let f = Cli::new(provider, &format!(r#"
# Fill both output pipes before consuming stdin: a synchronous writer deadlocks.
for _ in range(128):
    print('x' * 1024, flush=True)
    print('y' * 1024, file=sys.stderr, flush=True)
with open('received-prompt','wb') as output: output.write(sys.stdin.buffer.read())
{terminal}
"#));
        let prompt = "Large context: żółw `literal` $(literal)\n".repeat(8000);
        f.ctx.run_agent(&AgentRequest { prompt: &prompt, ..request(provider) }).unwrap();
        assert_eq!(fs::read(f.root.join("received-prompt")).unwrap(), prompt.as_bytes());
        assert!(f.args().iter().all(|a| a.len() < 32 * 1024));
        assert_eq!(f.args().last().is_some_and(|a| a == "-"), provider == "codex");
    }
}

// Shape observed in Codex CLI 0.153.4: exec stdout lacks model, while the
// exact rollout records task_started, turn_context and task_complete.
fn codex_rollout_script(model: &str) -> String {
    format!(r#"
from pathlib import Path
import uuid
session = '{ID}'
turn = str(uuid.uuid4())
path = Path(os.environ['CODEX_HOME']) / 'sessions/2026/09/08' / ('rollout-fixture-' + session + '.jsonl')
path.parent.mkdir(parents=True, exist_ok=True)
events = []
if not path.exists():
    events.append({{'type':'session_meta','payload':{{'id':session,'cwd':os.getcwd()}}}})
events.extend([
    {{'type':'event_msg','payload':{{'type':'task_started','turn_id':turn}}}},
    {{'type':'turn_context','payload':{{'turn_id':turn,'cwd':os.getcwd(),'model':'{model}'}}}},
    {{'type':'event_msg','payload':{{'type':'task_complete','turn_id':turn}}}}
])
with path.open('a') as f:
    for event in events: f.write(json.dumps(event) + '\n')
print(json.dumps({{'type':'thread.started','thread_id':session}}))
print(json.dumps({{'type':'item.completed','item':{{'type':'agent_message','text':answer if 'answer' in globals() else '{{}}'}}}}))
print(json.dumps({{'type':'turn.completed','usage':{{'input_tokens':10,'output_tokens':5}}}}))
"#)
}

#[test]
fn codex_session_metadata_supplies_model_for_fresh_and_resumed_invocations() {
    let fixture = Cli::new("codex", &codex_rollout_script("exact-model"));
    for session in [None, Some(ID)] {
        let mut req = request("codex");
        req.session = session;
        let output = fixture.ctx.run_agent(&req).unwrap();
        assert!(output.model_reported);
        assert_eq!(output.effective_model, "exact-model");
        assert_eq!(output.usage.unwrap().model, "exact-model");
    }
}

#[test]
fn codex_stream_model_takes_precedence_over_rollout_fallback() {
    let fixture = Cli::new("codex", &format!("{}\nprint(json.dumps({{'type':'model','model':'stream-model'}}))", codex_rollout_script("rollout-model")));
    let output = fixture.ctx.run_agent(&request("codex")).unwrap();
    assert!(output.model_reported);
    assert_eq!(output.effective_model, "stream-model");
}

#[test]
fn architect_publishes_with_codex_rollout_model_and_retains_plan_on_substitution() {
    let answer = r#"
context = json.JSONDecoder().raw_decode(sys.argv[-1].split('Context:\n', 1)[1])[0]
plan = context['plan']
answer = json.dumps({'version':1,'plan_id':plan['plan_id'],'revision':plan['revision'],
    'checkpoint':{'summary':'Preserve contracts.','constraints':[],'completed_interfaces':[]},
    'decisions':[],'guidance':[{'stage_id':i,'text':'Check interfaces.'} for i in context['required_stage_ids']],
    'unresolved_risks':[],'resolved_risks':[],
    'model_evaluations':[dict(stage_id=s['id'],agree=True,rationale='Checked interfaces.',
        **{k:s['model_proposal'][k] for k in ('risk','complexity','task')})
        for s in plan['stages'] if s['id'] in context['required_model_stage_ids']]})
"#;
    let fixture = Cli::new("codex", &format!("{answer}\n{}", codex_rollout_script("exact-model")));
    {
        let mut settings = fixture.ctx.app.settings.lock().unwrap();
        settings["architect"] = json!("codex");
        settings["architect_model"] = json!("exact-model");
        settings["planner"] = json!("mock");
        settings["implementer"] = json!("mock");
        settings["reviewer"] = json!("mock");
        settings["automatic_routing"] = json!(false);
        let mut policy = Policy::default();
        policy.entries.push(serde_json::from_value(json!({"provider":"codex","model":"exact-model","tier":"strong"})).unwrap());
        settings["model_catalogue"] = json!(policy);
    }
    let candidate = json!({"goal":"check model verification","status":"draft","stages":[
        {"id":1,"title":"API","instructions":"Preserve API","acceptance":"tests pass","commit":"fix: API","status":"pending"}]});
    let plan = fixture.ctx.architect_publish(candidate, None, "initial").unwrap();
    assert_eq!(fixture.ctx.session.state.lock().unwrap().architect_activity["status"], "ready");
    assert_eq!(fixture.ctx.architecture_store().checkpoint(&plan).unwrap()["effective_model"]["model"], "exact-model");
    let before = fs::read(fixture.ctx.forge_path("plan.json")).unwrap();
    let script = fixture.root.join("codex");
    let body = fs::read_to_string(&script).unwrap().replace("'model':'exact-model'", "'model':'substituted-model'");
    fs::write(&script, body).unwrap();
    let mut revised = plan.clone();
    revised["stages"][0]["instructions"] = json!("Preserve and verify API");
    let revised = crate::plan::edit_plan(&plan, &json!({"plan":revised})).unwrap();
    let error = fixture.ctx.architect_publish(revised, Some(&plan), "revision").unwrap_err();
    assert_eq!(error, "architect effective model changed: expected exact-model, reported substituted-model");
    assert_eq!(fs::read(fixture.ctx.forge_path("plan.json")).unwrap(), before);
}

#[test]
fn codex_fixture_exact_resume_effort_permissions_decoding_usage_and_readable_logs() {
    let fixture = Cli::new(
        "codex",
        &format!(
            r#"
child = subprocess.Popen(['sleep','60'])
with open('descendant.pid','w') as f: f.write(str(child.pid))
print(json.dumps({{'type':'thread.started','thread_id':'{ID}'}}))
print(json.dumps({{'type':'item.started','item':{{'type':'command_execution','command':'rg interface src'}}}}))
print(json.dumps({{'type':'item.completed','item':{{'type':'agent_message','text':'{{"answer":"ok"}}'}}}}))
print(json.dumps({{'type':'turn.completed','usage':{{'input_tokens':100,'cached_input_tokens':80,'output_tokens':20}}}}))
print('diagnostic on stderr', file=sys.stderr)
"#
        ),
    );
    let output = fixture.ctx.run_agent(&request("codex")).unwrap();
    assert_eq!(output.session.as_deref(), Some(ID));
    assert_eq!(output.output, "{\"answer\":\"ok\"}");
    assert_eq!(output.effective_model, "exact-model");
    assert!(!output.model_reported, "requested model fallback is not a provider report");
    assert_eq!(output.usage.unwrap().total_tokens, 120);
    let args = fixture.args();
    assert!(pair(&args, "resume", ID));
    assert!(pair(&args, "-m", "exact-model"));
    assert!(pair(&args, "--sandbox", "read-only"));
    assert!(pair(&args, "-c", "approval_policy=\"never\""));
    assert!(pair(&args, "-c", "model_reasoning_effort=\"high\""));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("dangerously") || a == "--last" || a == "--continue")
    );
    let log = fs::read_to_string(fixture.ctx.forge_path("agent.log")).unwrap();
    assert!(log.contains("rg interface src"));
    assert!(log.contains("diagnostic on stderr"));
    assert!(!log.contains("thread.started"));
    assert_eq!(
        fixture.ctx.session.state.lock().unwrap().role_usage["architect"]["codex"]["total_tokens"],
        120
    );
    let pid = fs::read_to_string(fixture.root.join("descendant.pid")).unwrap();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"));
}

#[test]
fn claude_fixture_resume_native_effort_structured_result_and_cache_usage() {
    let fixture = Cli::new(
        "claude",
        &format!(
            r#"
print(json.dumps({{'type':'system','subtype':'init','session_id':'{ID}','model':'effective-claude'}}))
print(json.dumps({{'type':'result','subtype':'success','session_id':'{ID}','is_error':False,'structured_output':{{'answer':'yes'}},'usage':{{'input_tokens':10,'cache_read_input_tokens':30,'cache_creation_input_tokens':40,'output_tokens':20}}}}))
"#
        ),
    );
    let mut req = request("claude");
    req.effort = "max";
    let result = fixture.ctx.run_agent(&req).unwrap();
    assert_eq!(result.output, "{\"answer\":\"yes\"}");
    assert_eq!(result.session.as_deref(), Some(ID));
    assert_eq!(result.effective_model, "effective-claude");
    assert!(result.model_reported);
    assert_eq!(result.usage.unwrap().total_tokens, 100);
    let args = fixture.args();
    assert!(pair(&args, "--resume", ID));
    assert!(pair(&args, "--effort", "max"));
    assert!(pair(&args, "--model", "exact-model"));
    assert!(pair(&args, "--permission-mode", "dontAsk"));
    assert!(pair(&args, "--tools", "Read,Glob,Grep"));
    assert!(args.contains(&"--safe-mode".into()));
    assert!(args.contains(&"--strict-mcp-config".into()));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("dangerously") || a == "--continue")
    );
}

#[test]
fn reviewers_and_qa_are_always_fresh_and_unknown_effort_is_never_mapped() {
    for provider in ["codex", "claude"] {
        let mut req = request(provider);
        req.role = "reviewer";
        assert!(command(&req).is_err());
        req.session = None;
        let cmd = command(&req).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args
                .iter()
                .any(|a| a == "resume" || a == "--resume" || a == "--last" || a == "--continue")
        );
        req.role = "chat";
        let cmd = command(&req).unwrap();
        assert!(
            !cmd.get_args()
                .any(|a| a.to_string_lossy().contains("dangerously"))
        );
    }
    let fixture = Cli::new("codex", "raise Exception('must not launch')");
    let mut req = request("codex");
    req.effort = "ultra-unknown";
    assert!(
        fixture
            .ctx
            .run_agent(&req)
            .unwrap_err()
            .contains("unknown native effort")
    );
    assert!(!fixture.root.join("argv.json").exists());
}

#[test]
fn failed_result_wrong_session_and_incomplete_stream_fail_closed() {
    for body in [
        format!("print(json.dumps({{'type':'result','session_id':'{ID}','subtype':'error_during_execution','is_error':True,'result':'failed'}}))"),
        "print(json.dumps({'type':'result','session_id':'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee','subtype':'success','result':'{}'}))".into(),
        "print('a diagnostic without a result')".into(),
    ] {
        let f = Cli::new("claude", &body); assert!(f.ctx.run_agent(&request("claude")).is_err());
    }
}

#[test]
fn missing_permission_capabilities_prevent_paid_invocation() {
    let f = Cli::new("claude", "raise Exception('must not launch')");
    fs::write(
        f.root.join("claude"),
        "#!/bin/sh\necho 'old CLI without safe tools'\n",
    )
    .unwrap();
    assert!(
        f.ctx
            .run_agent(&request("claude"))
            .unwrap_err()
            .contains("capability failure")
    );
    assert!(!f.root.join("argv.json").exists());
}

#[test]
fn stop_and_reader_failure_kill_descendants_and_close_pipes() {
    for reader_failure in [false, true] {
        let body = format!(
            r#"
child = subprocess.Popen(['sleep','60'])
with open('descendant.pid','w') as f: f.write(str(child.pid))
{}
time.sleep(60)
"#,
            if reader_failure {
                "print('x' * (1024 * 1024 + 2), flush=True)"
            } else {
                "print('working', flush=True)"
            }
        );
        let f = Cli::new("codex", &body);
        let start = Instant::now();
        let prompt = "context".repeat(40_000);
        let req = AgentRequest { prompt: &prompt, ..request("codex") };
        let result = std::thread::scope(|scope| {
            let task = scope.spawn(|| f.ctx.run_agent(&req));
            while !f.root.join("descendant.pid").exists() {
                assert!(start.elapsed() < Duration::from_secs(3));
                std::thread::sleep(Duration::from_millis(10));
            }
            if !reader_failure {
                f.ctx.session.stop_requested.store(true, Ordering::SeqCst);
            }
            task.join().unwrap()
        });
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(4));
        let pid = fs::read_to_string(f.root.join("descendant.pid")).unwrap();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"));
        assert!(f.ctx.session.state.lock().unwrap().agent_role.is_empty());
    }
}
