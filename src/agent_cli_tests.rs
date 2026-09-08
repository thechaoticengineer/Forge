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
        let result = std::thread::scope(|scope| {
            let task = scope.spawn(|| f.ctx.run_agent(&request("codex")));
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
