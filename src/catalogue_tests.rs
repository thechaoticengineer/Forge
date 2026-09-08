use super::*;
use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "forge-catalogue-test-{}-{}",
            std::process::id(),
            CACHE_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn budget() -> Budget {
    Budget {
        deadline: Instant::now() + Duration::from_secs(2),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
fn row(id: &str) -> Model {
    normalize(Provider::Codex, &json!([{"id":id,"model":id}]))
        .unwrap()
        .remove(0)
}
fn policy() -> Policy {
    let mut p = Policy::default();
    p.entries.push(Entry {
        provider: Provider::Codex,
        model: "explicit-model".into(),
        tier: Tier::Strong,
        suitability: vec!["contracts".into()],
        limits: BTreeMap::new(),
        relative_cost_preference: Some(2),
        effort: default_effort(),
    });
    p
}
fn wait(c: &Catalogue) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while c.running() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
struct Fixed {
    result: Mutex<Result<Vec<Model>, Failure>>,
    calls: AtomicUsize,
    delay: Duration,
}
impl Fixed {
    fn new(result: Result<Vec<Model>, Failure>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(result),
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
        })
    }
}
impl Discovery for Fixed {
    fn probe(&self, _: Provider, _: &Policy, budget: Budget) -> Probe {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let start = Instant::now();
        while start.elapsed() < self.delay {
            if let Err(e) = budget.check() {
                return Probe {
                    cli_version: Some("fixture 1".into()),
                    result: Err(e),
                };
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Probe {
            cli_version: Some("fixture 1".into()),
            result: self.result.lock().unwrap().clone(),
        }
    }
}

// Each provider reports entry and waits for the test to supply its result.
// Receiving both requests before replying also verifies provider parallelism.
type ProbeRequest = (Provider, Budget, std::sync::mpsc::Sender<Option<Probe>>);
struct Controlled {
    entered: std::sync::mpsc::Sender<ProbeRequest>,
}
impl Controlled {
    fn new() -> (Arc<Self>, std::sync::mpsc::Receiver<ProbeRequest>) {
        let (entered, requests) = std::sync::mpsc::channel();
        (Arc::new(Self { entered }), requests)
    }
}
impl Discovery for Controlled {
    fn probe(&self, provider: Provider, _: &Policy, budget: Budget) -> Probe {
        let (reply, result) = std::sync::mpsc::channel();
        self.entered.send((provider, budget, reply)).unwrap();
        result
            .recv_timeout(Duration::from_secs(3))
            .expect("test must release the provider")
            .expect("injected discovery panic")
    }
}
fn start_controlled_refresh(
    c: &Catalogue,
    p: &Policy,
    requests: &std::sync::mpsc::Receiver<ProbeRequest>,
) -> BTreeMap<Provider, (Budget, std::sync::mpsc::Sender<Option<Probe>>)> {
    assert!(c.refresh(p.clone()));
    let mut pending = BTreeMap::new();
    for _ in 0..2 {
        let (provider, budget, reply) = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(pending.insert(provider, (budget, reply)).is_none());
    }
    pending
}

struct Script {
    lines: VecDeque<String>,
    exchanges: VecDeque<(Value, Vec<Value>)>,
    drops: Arc<AtomicUsize>,
}
impl Channel for Script {
    fn send(&mut self, text: &str) -> Result<(), Failure> {
        let (expected, responses) = self
            .exchanges
            .pop_front()
            .expect("unexpected request (no generation allowed)");
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), expected);
        self.lines.extend(responses.iter().map(Value::to_string));
        Ok(())
    }
    fn line(&mut self) -> Result<Option<String>, Failure> {
        Ok(self.lines.pop_front())
    }
    fn finish(&mut self) -> Result<(), Failure> {
        Ok(())
    }
}
impl Drop for Script {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct FakeLauncher {
    scripts: Mutex<BTreeMap<String, Result<Script, Failure>>>,
    calls: Mutex<Vec<CommandSpec>>,
}
impl Launcher for FakeLauncher {
    fn spawn(&self, s: &CommandSpec, _: Budget) -> Result<Box<dyn Channel>, Failure> {
        self.calls.lock().unwrap().push(s.clone());
        self.scripts
            .lock()
            .unwrap()
            .remove(&format!("{} {}", s.executable, s.args.join(" ")))
            .unwrap_or_else(|| panic!("unexpected process {:?}", s))
            .map(|s| Box::new(s) as Box<dyn Channel>)
    }
}
fn script(
    lines: Vec<String>,
    exchanges: Vec<(Value, Vec<Value>)>,
    drops: &Arc<AtomicUsize>,
) -> Script {
    Script {
        lines: lines.into(),
        exchanges: exchanges.into(),
        drops: drops.clone(),
    }
}
fn init() -> Value {
    json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"forge_catalogue","version":env!("CARGO_PKG_VERSION")}}})
}
fn account() -> Value {
    json!({"id":1,"method":"account/read","params":{"refreshToken":false}})
}
fn list(id: u32, cursor: Value) -> Value {
    json!({"id":id,"method":"model/list","params":{"cursor":cursor,"limit":100,"includeHidden":true}})
}
fn launcher(scripts: BTreeMap<String, Result<Script, Failure>>) -> Arc<FakeLauncher> {
    Arc::new(FakeLauncher {
        scripts: Mutex::new(scripts),
        calls: Mutex::new(vec![]),
    })
}

#[test]
fn both_startup_probes_pagination_notifications_aliases_and_optional_fields() {
    let temp = Temp::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut p = policy();
    p.claude_bridge = "/bridge".into();
    let l=launcher(BTreeMap::from([
        ("codex --version".into(),Ok(script(vec!["codex-cli 0.153.2".into()],vec![],&drops))),
        ("claude --version".into(),Ok(script(vec!["2.1.261 (Claude Code)".into()],vec![],&drops))),
        ("codex app-server".into(),Ok(script(vec![],vec![
            (init(),vec![json!({"method":"notice","params":{}}),json!({"id":0,"result":{"unknown":"fine"}})]),
            (json!({"method":"initialized","params":{}}),vec![]),
            (account(),vec![json!({"id":1,"result":{"requiresOpenaiAuth":true,"account":{"type":"chatgpt"}}})]),
            (list(2,Value::Null),vec![json!({"method":"account/updated"}),json!({"id":99,"result":{}}),json!({"id":2,"result":{"data":[{
                "id":"alias","model":"wire-id","isDefault":true,"defaultReasoningEffort":"high","supportedReasoningEfforts":[{"reasoningEffort":"high","future":5}],"inputModalities":["text"],"future":{}}],"nextCursor":"page2"}})]),
            (list(3,json!("page2")),vec![json!({"id":3,"result":{"data":[{"id":"minimal"}]}})]),
        ],&drops))),
        ("/bridge --forge-discovery-v1 claude 2.1.261 (Claude Code)".into(),Ok(script(vec![json!({"notification":"ready"}).to_string(),json!({
            "bridge_version":1,"source":"claude_code_initialization","sdk_version":"0.3.261","cli_version":"2.1.261 (Claude Code)",
            "models":[{"value":"alias","resolvedModel":"wire-claude","supportedEffortLevels":["high","future"],"supportsAdaptiveThinking":true},{"value":"minimal"}]}).to_string()],vec![],&drops))),
    ]));
    let c = Catalogue::new(
        Arc::new(CliDiscovery {
            launcher: l.clone(),
            cwd: temp.0.clone(),
        }),
        temp.0.clone(),
        Duration::from_secs(2),
    );
    assert!(c.refresh(p.clone()));
    wait(&c);
    assert_eq!(drops.load(Ordering::SeqCst), 4);
    assert!(l.scripts.lock().unwrap().is_empty());
    let details = c.details(&p);
    assert_eq!(
        details["providers"][0]["models"][0]["resolved_id"],
        "wire-id"
    );
    assert_eq!(
        details["providers"][0]["models"][0]["aliases"],
        json!(["alias"])
    );
    assert_eq!(
        details["providers"][0]["models"][1]["supported_efforts"],
        Value::Null
    );
    assert_eq!(
        details["providers"][1]["models"][0]["resolved_id"],
        "wire-claude"
    );
    assert_eq!(
        c.select(&p, Provider::Claude, "wire-claude", "high")["native_effort_args"],
        json!(["--effort", "high"])
    );
    assert_eq!(
        c.select(&p, Provider::Codex, "alias", "high")["native_effort_args"],
        json!(["-c", "model_reasoning_effort=\"high\""])
    );
    for provider in details["providers"].as_array().unwrap() {
        for model in provider["models"].as_array().unwrap() {
            assert!(model["pricing"].is_null());
        }
    }
}
#[test]
fn missing_executable_bridge_malformed_auth_and_unsupported_protocol() {
    let temp = Temp::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let l = launcher(BTreeMap::from([(
        "claude --version".into(),
        Ok(script(vec!["2.1.261 (Claude Code)".into()], vec![], &drops)),
    )]));
    let discovery = CliDiscovery {
        launcher: l,
        cwd: temp.0.clone(),
    };
    assert_eq!(
        discovery
            .probe(Provider::Claude, &policy(), budget())
            .result
            .unwrap_err()
            .kind,
        FailureKind::Unsupported
    );
    let l = launcher(BTreeMap::from([(
        "codex --version".into(),
        Err(Failure::new(FailureKind::MissingExecutable)),
    )]));
    assert_eq!(
        CliDiscovery {
            launcher: l,
            cwd: temp.0.clone()
        }
        .probe(Provider::Codex, &policy(), budget())
        .result
        .unwrap_err()
        .kind,
        FailureKind::MissingExecutable
    );
    for (response, expected) in [
        (
            json!({"id":0,"error":{"code":401,"message":"secret"}}),
            FailureKind::Auth,
        ),
        (
            json!({"id":0,"error":{"code":-32601}}),
            FailureKind::Unsupported,
        ),
        (json!({"id":0,"error":{"code":403}}), FailureKind::Rejected),
        (json!({"id":0,"result":null}), FailureKind::Malformed),
    ] {
        let mut s = script(vec![], vec![(init(), vec![response])], &drops);
        let e = rpc(&mut s, 0, "initialize", init()["params"].clone()).unwrap_err();
        assert_eq!(e.kind, expected);
        assert!(!e.message.contains("secret"));
    }
    let mut s = script(vec!["{broken".into()], vec![(init(), vec![])], &drops);
    assert_eq!(
        rpc(&mut s, 0, "initialize", init()["params"].clone())
            .unwrap_err()
            .kind,
        FailureKind::Malformed
    );
    assert!(normalize(Provider::Codex, &json!([{}, {"id":"x"}])).is_err());
    assert!(normalize(Provider::Claude, &json!([{"value":"x"},{"value":"x"}])).is_err());
}
#[test]
fn bridge_missing_version_mismatch_and_api_lists_are_not_subscription_discovery() {
    let temp = Temp::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut p = policy();
    p.claude_bridge = "/bridge".into();
    for body in [
        None,
        Some(json!({"bridge_version":2})),
        Some(json!({"bridge_version":1,"source":"anthropic_api","models":[]})),
        Some(json!({"bridge_version":1,"error":{"code":401}})),
    ] {
        let expected = if body.as_ref().is_some_and(|b| b["error"]["code"] == 401) {
            FailureKind::Auth
        } else {
            FailureKind::Unsupported
        };
        let l = launcher(BTreeMap::from([
            (
                "claude --version".into(),
                Ok(script(vec!["2.1.261 (Claude Code)".into()], vec![], &drops)),
            ),
            (
                "/bridge --forge-discovery-v1 claude 2.1.261 (Claude Code)".into(),
                body.map(|v| script(vec![v.to_string()], vec![], &drops))
                    .ok_or(Failure::new(FailureKind::MissingExecutable)),
            ),
        ]));
        assert_eq!(
            CliDiscovery {
                launcher: l,
                cwd: temp.0.clone()
            }
            .probe(Provider::Claude, &p, budget())
            .result
            .unwrap_err()
            .kind,
            expected
        );
    }
}
#[test]
fn cached_stale_fallback_diffs_tombstones_and_scope_partition() {
    let temp = Temp::new();
    let fixed = Fixed::new(Ok(vec![row("explicit-model"), row("removed-model")]));
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    let p = policy();
    c.refresh(p.clone());
    wait(&c);
    let path = temp.0.join(format!("{}.json", scope(Provider::Codex, &p)));
    let good = std::fs::read(&path).unwrap();
    *fixed.result.lock().unwrap() = Err(Failure::new(FailureKind::Timeout));
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(std::fs::read(&path).unwrap(), good);
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "provider_default")["availability"],
        "configured_unverified"
    );
    assert_eq!(c.summary(&p)["providers"][0]["status"], "cached_stale");
    let mut changed = row("explicit-model");
    changed.resolved_id = Some("new-wire".into());
    changed.supported_efforts = Some(vec!["high".into()]);
    *fixed.result.lock().unwrap() = Ok(vec![changed.clone(), row("added-model")]);
    c.refresh(p.clone());
    wait(&c);
    let d = &c.details(&p)["providers"][0]["diff"];
    assert_eq!(d["removed"], json!(["removed-model"]));
    assert_eq!(d["added"], json!(["added-model"]));
    assert_eq!(d["alias_retargeted"], json!(["explicit-model"]));
    assert_eq!(d["capabilities_changed"], json!(["explicit-model"]));
    *fixed.result.lock().unwrap() = Err(Failure::new(FailureKind::Io));
    c.refresh(p.clone());
    wait(&c);
    let mut fallback = p.clone();
    let mut removed = fallback.entries[0].clone();
    removed.model = "removed-model".into();
    fallback.entries.push(removed);
    assert_eq!(
        c.select(
            &fallback,
            Provider::Codex,
            "removed-model",
            "provider_default"
        )["eligible"],
        false
    );
    let restarted = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    restarted.refresh(p.clone());
    wait(&restarted);
    assert_eq!(
        restarted.select(
            &fallback,
            Provider::Codex,
            "removed-model",
            "provider_default"
        )["eligible"],
        false
    );
    let mut other = p.clone();
    other.codex_scope = "other-account".into();
    c.refresh(other.clone());
    wait(&c);
    assert_eq!(c.details(&other)["providers"][0]["models"], json!([]));
    assert!(path.exists());
    assert!(
        !temp
            .0
            .join(format!("{}.json", scope(Provider::Codex, &other)))
            .exists()
    );
    assert!(read_cache(&path, Provider::Claude, &scope(Provider::Codex, &p)).is_err());
    assert!(read_cache(&path, Provider::Codex, "different-scope").is_err());
}
#[test]
fn corrupt_cache_and_unknown_fields_do_not_break_refresh() {
    let temp = Temp::new();
    let p = policy();
    let path = temp.0.join(format!("{}.json", scope(Provider::Codex, &p)));
    std::fs::write(&path, b"{").unwrap();
    let fixed = Fixed::new(Err(Failure::new(FailureKind::Io)));
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(
        c.summary(&p)["providers"][0]["cache_error"],
        "cache is corrupt"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{");
    *fixed.result.lock().unwrap() = Ok(vec![row("x")]);
    c.refresh(p.clone());
    wait(&c);
    assert!(
        read_cache(&path, Provider::Codex, &scope(Provider::Codex, &p))
            .unwrap()
            .is_some()
    );
    assert!(c.summary(&p)["providers"][0]["cache_error"].is_null());
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    v["version"] = json!(99);
    std::fs::write(&path, v.to_string()).unwrap();
    assert!(read_cache(&path, Provider::Codex, &scope(Provider::Codex, &p)).is_err());
}
#[test]
fn configured_policy_is_explicit_unverified_never_invented_pricing_or_effort() {
    let temp = Temp::new();
    let fixed = Fixed::new(Err(Failure::new(FailureKind::Unsupported)));
    let p = policy();
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    c.refresh(p.clone());
    wait(&c);
    let option = c.select(&p, Provider::Codex, "explicit-model", "provider_default");
    assert_eq!(option["eligible"], true);
    assert_eq!(option["availability"], "configured_unverified");
    assert_eq!(option["availability_unverified"], true);
    assert_eq!(option["tier"], "strong");
    assert_eq!(option["provenance"], "configured");
    assert_eq!(option["relative_cost_preference"], 2);
    assert!(option["pricing"].is_null());
    assert_eq!(
        c.select(&p, Provider::Codex, "not-configured", "provider_default")["eligible"],
        false
    );
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "ultra")["eligible"],
        false
    );
    for failure in [
        FailureKind::MissingExecutable,
        FailureKind::Auth,
        FailureKind::Rejected,
    ] {
        *fixed.result.lock().unwrap() = Err(Failure::new(failure));
        c.refresh(p.clone());
        wait(&c);
        assert_eq!(
            c.select(&p, Provider::Codex, "explicit-model", "provider_default")["eligible"],
            false
        );
        *fixed.result.lock().unwrap() = Err(Failure::new(FailureKind::Timeout));
        c.refresh(p.clone());
        wait(&c);
        // Fixed returns a successful version probe even when discovery times out:
        // this is evidence that a previously missing executable was installed.
        assert_eq!(
            c.select(&p, Provider::Codex, "explicit-model", "provider_default")["eligible"],
            failure == FailureKind::MissingExecutable
        );
    }
    *fixed.result.lock().unwrap() = Ok(vec![row("explicit-model")]);
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "provider_default")["eligible"],
        true
    );
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "high")["native_effort_args"],
        json!([])
    );
}
#[test]
fn slow_refresh_is_shared_bounded_and_cancellable() {
    let temp = Temp::new();
    let p = policy();
    let fixed = Arc::new(Fixed {
        result: Mutex::new(Ok(vec![])),
        calls: AtomicUsize::new(0),
        delay: Duration::from_secs(5),
    });
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_millis(500));
    let start = Instant::now();
    assert!(c.refresh(p.clone()));
    for _ in 0..100 {
        assert!(!c.refresh(p.clone()));
        assert_eq!(c.summary(&p)["refreshing"], true);
    }
    assert!(start.elapsed() < Duration::from_millis(250));
    while fixed.calls.load(Ordering::SeqCst) < 2 {
        std::thread::sleep(Duration::from_millis(5));
    }
    c.cancel();
    wait(&c);
    assert_eq!(fixed.calls.load(Ordering::SeqCst), 2);
    assert_eq!(c.summary(&p)["providers"][0]["error"]["kind"], "cancelled");
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(c.summary(&p)["providers"][0]["error"]["kind"], "timeout");
}
#[test]
fn settings_validation_bounds_types_and_provenance() {
    let mut settings = crate::plan::default_settings();
    assert!(Policy::from_settings(&settings).is_ok());
    settings["model_catalogue"] = json!(policy());
    assert!(Policy::from_settings(&settings).is_ok());
    for (key, value) in [
        ("tier", json!("best")),
        ("model", json!("--inject")),
        ("effort", json!("high\n-c evil")),
        ("pricing", json!(2)),
        ("relative_cost_preference", json!(-1)),
        ("provenance", json!("official")),
    ] {
        let mut invalid = settings.clone();
        invalid["model_catalogue"]["entries"][0][key] = value;
        assert!(Policy::from_settings(&invalid).is_err(), "{key}");
    }
    let mut invalid = settings.clone();
    invalid["model_catalogue"]["entries"]
        .as_array_mut()
        .unwrap()
        .push(json!(policy().entries[0]));
    assert!(Policy::from_settings(&invalid).is_err());
    settings["model_catalogue"]["claude_bridge"] = json!("relative/bridge");
    assert!(Policy::from_settings(&settings).is_err());
}
#[test]
fn subprocess_timeout_cancellation_stderr_and_group_cleanup() {
    let temp = Temp::new();
    for cancel in [false, true] {
        let b = Budget {
            deadline: Instant::now() + Duration::from_millis(250),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let mut c = SystemLauncher
            .spawn(
                &CommandSpec {
                    executable: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        "echo $$; sleep 60 & echo $!; while :; do echo diagnostic >&2; done".into(),
                    ],
                    cwd: temp.0.clone(),
                },
                b.clone(),
            )
            .unwrap();
        let pid: u32 = c.line().unwrap().unwrap().trim().parse().unwrap();
        let child: u32 = c.line().unwrap().unwrap().trim().parse().unwrap();
        if cancel {
            b.cancel.store(true, Ordering::SeqCst);
        }
        let error = c.line().unwrap_err();
        assert_eq!(
            error.kind,
            if cancel {
                FailureKind::Cancelled
            } else {
                FailureKind::Timeout
            }
        );
        drop(c);
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
        // An adopted grandchild may remain briefly as a zombie, but cannot keep running.
        let deadline = Instant::now() + Duration::from_secs(1);
        while let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) {
            if stat.contains(") Z ") || stat.contains(") X ") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child survived process-group cancellation"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let mut c = SystemLauncher
        .spawn(
            &CommandSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "echo 'authentication failed SECRET' >&2; exit 1".into(),
                ],
                cwd: temp.0.clone(),
            },
            budget(),
        )
        .unwrap();
    let e = c.finish().unwrap_err();
    assert_eq!(e.kind, FailureKind::Auth);
    assert!(!e.message.contains("SECRET"));
    assert_eq!(
        SystemLauncher
            .spawn(
                &CommandSpec {
                    executable: "/no/such/forge-cli".into(),
                    args: vec![],
                    cwd: temp.0.clone()
                },
                budget()
            )
            .err()
            .unwrap()
            .kind,
        FailureKind::MissingExecutable
    );
}

#[test]
fn execution_evidence_dominates_lists_and_rejected_efforts_never_emit_arguments() {
    let temp = Temp::new();
    let p = policy();
    let mut m = row("explicit-model");
    m.supported_efforts = Some(vec!["high".into()]);
    let fixed = Fixed::new(Ok(vec![m]));
    let c = Catalogue::new(fixed, temp.0.clone(), Duration::from_secs(2));
    assert_eq!(
        c.execution_input(&p, Provider::Claude, "explicit-role-model")["availability"],
        "configured_unverified"
    );
    assert_eq!(
        c.execution_input(&p, Provider::Claude, "")["eligible"],
        true
    );
    c.refresh(p.clone());
    wait(&c);
    c.reject_effort(&p, Provider::Codex, "explicit-model", "high");
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "high")["native_effort_args"],
        json!([])
    );
    assert_eq!(
        c.select(&p, Provider::Codex, "explicit-model", "high")["eligible"],
        false
    );
    c.observe(
        &p,
        Provider::Codex,
        "explicit-model",
        Err(Failure::new(FailureKind::Rejected)),
    );
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(
        c.execution_input(&p, Provider::Codex, "explicit-model")["eligible"],
        false
    );
    c.observe(&p, Provider::Codex, "explicit-model", Ok(()));
    assert_eq!(
        c.execution_input(&p, Provider::Codex, "explicit-model")["availability"],
        "execution_verified"
    );
    c.observe(
        &p,
        Provider::Codex,
        "explicit-model",
        Err(Failure::new(FailureKind::Auth)),
    );
    c.refresh(p.clone());
    wait(&c);
    assert_eq!(
        c.execution_input(&p, Provider::Codex, "explicit-model")["eligible"],
        false
    );
}

#[test]
fn policy_storage_is_versioned_validated_and_atomic() {
    let temp = Temp::new();
    let path = temp.0.join("settings/policy.json");
    assert!(load_policy(&path).unwrap().is_none());
    save_policy(&path, &policy()).unwrap();
    assert_eq!(load_policy(&path).unwrap().unwrap(), policy());
    std::fs::write(&path, b"{").unwrap();
    assert!(load_policy(&path).is_err());
    let invalid = temp.0.join("directory");
    std::fs::create_dir(&invalid).unwrap();
    assert!(save_policy(&invalid, &policy()).is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2); // No abandoned temporary file.
}

#[test]
fn repeated_cursor_and_missing_auth_are_not_successful_catalogues() {
    let temp = Temp::new();
    let drops = Arc::new(AtomicUsize::new(0));
    for auth in [false, true] {
        let mut exchanges = vec![
            (init(), vec![json!({"id":0,"result":{}})]),
            (json!({"method":"initialized","params":{}}), vec![]),
            (
                account(),
                vec![json!({"id":1,"result":{"requiresOpenaiAuth":auth,"account":null}})],
            ),
        ];
        if !auth {
            exchanges.push((
                list(2, Value::Null),
                vec![json!({"id":2,"result":{"data":[],"nextCursor":"again"}})],
            ));
            exchanges.push((
                list(3, json!("again")),
                vec![json!({"id":3,"result":{"data":[],"nextCursor":"again"}})],
            ));
        }
        let l = launcher(BTreeMap::from([
            (
                "codex --version".into(),
                Ok(script(vec!["codex-cli 0.153.2".into()], vec![], &drops)),
            ),
            (
                "codex app-server".into(),
                Ok(script(vec![], exchanges, &drops)),
            ),
        ]));
        let result = CliDiscovery {
            launcher: l,
            cwd: temp.0.clone(),
        }
        .probe(Provider::Codex, &policy(), budget());
        assert_eq!(
            result.result.unwrap_err().kind,
            if auth {
                FailureKind::Auth
            } else {
                FailureKind::Malformed
            }
        );
    }
    assert_eq!(drops.load(Ordering::SeqCst), 4);
}

#[test]
fn cli_version_changes_are_reported_against_last_good_cache() {
    let temp = Temp::new();
    let p = policy();
    let fixed = Fixed::new(Ok(vec![row("model")]));
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    c.refresh(p.clone());
    wait(&c);
    let key = scope(Provider::Codex, &p);
    let path = temp.0.join(format!("{key}.json"));
    let mut cache = read_cache(&path, Provider::Codex, &key).unwrap().unwrap();
    cache.cli_version = "older-cli".into();
    write_cache(&path, &cache).unwrap();
    let restarted = Catalogue::new(fixed, temp.0.clone(), Duration::from_secs(2));
    restarted.refresh(p.clone());
    wait(&restarted);
    assert_eq!(
        restarted.summary(&p)["providers"][0]["changes"]["cli_version_changed"],
        true
    );
}

#[test]
fn overlapping_refresh_preserves_execution_evidence_and_joins_both_providers() {
    let temp = Temp::new();
    let p = policy();
    let (discovery, requests) = Controlled::new();
    let c = Catalogue::new(discovery, temp.0.clone(), Duration::from_secs(2));
    let mut pending = start_controlled_refresh(&c, &p, &requests);
    let mut other_scope = p.clone();
    other_scope.codex_scope = "must-not-replace-running-scope".into();
    assert!(!c.refresh(other_scope));
    c.observe(
        &p,
        Provider::Codex,
        "explicit-model",
        Err(Failure::new(FailureKind::Auth)),
    );
    c.observe(
        &p,
        Provider::Claude,
        "explicit-model",
        Err(Failure::new(FailureKind::Rejected)),
    );
    c.reject_effort(&p, Provider::Claude, "explicit-model", "high");
    c.observe(&p, Provider::Claude, "verified-model", Ok(()));
    let mut model = row("explicit-model");
    model.supported_efforts = Some(vec!["high".into()]);
    pending
        .remove(&Provider::Codex)
        .unwrap()
        .1
        .send(Some(Probe {
            cli_version: Some("fixture 1".into()),
            result: Ok(vec![model.clone()]),
        }))
        .unwrap();
    let key = scope(Provider::Codex, &p);
    let path = temp.0.join(format!("{key}.json"));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Publication by one worker does not release the outer single-flight claim.
    assert!(!c.refresh(p.clone()));
    assert!(c.running());
    let cache = read_cache(&path, Provider::Codex, &key).unwrap().unwrap();
    assert_eq!(cache.models, vec![model.clone()]);
    assert_eq!(cache.scope, key);
    pending
        .remove(&Provider::Claude)
        .unwrap()
        .1
        .send(Some(Probe {
            cli_version: Some("fixture 1".into()),
            result: Ok(vec![model]),
        }))
        .unwrap();
    wait(&c);
    let details = c.details(&p);
    assert_eq!(details["providers"][0]["status"], "unavailable");
    assert_eq!(details["providers"][0]["blocker"]["kind"], "auth");
    assert_eq!(details["providers"][0]["execution_blocked"], true);
    assert!(details["providers"][0]["error"].is_null());
    assert!(details["providers"][0]["cache_error"].is_null());
    assert_eq!(
        c.execution_input(&p, Provider::Codex, "explicit-model")["eligible"],
        false
    );
    assert_eq!(
        c.execution_input(&p, Provider::Claude, "explicit-model")["eligible"],
        false
    );
    let effort = c.execution_with_effort(&p, Provider::Claude, "explicit-model", "high");
    assert_eq!(effort["eligible"], false);
    assert_eq!(effort["native_effort_args"], json!([]));
    assert_eq!(
        c.execution_input(&p, Provider::Claude, "verified-model")["availability"],
        "execution_verified"
    );
    c.observe(&p, Provider::Claude, "explicit-model", Ok(()));
    assert_eq!(
        c.execution_input(&p, Provider::Claude, "explicit-model")["availability"],
        "execution_verified"
    );
    assert!(c.state.lock().unwrap().cancel.is_none());
}

#[test]
fn controlled_cancellation_and_probe_panic_release_the_refresh_claim() {
    let temp = Temp::new();
    let p = policy();
    let (discovery, requests) = Controlled::new();
    let c = Catalogue::new(discovery, temp.0.clone(), Duration::from_secs(2));
    let pending = start_controlled_refresh(&c, &p, &requests);
    assert!(Arc::ptr_eq(
        &pending[&Provider::Codex].0.cancel,
        &pending[&Provider::Claude].0.cancel
    ));
    c.cancel();
    assert!(!c.refresh(p.clone()));
    for (_, (budget, reply)) in pending {
        assert_eq!(budget.check().unwrap_err().kind, FailureKind::Cancelled);
        reply
            .send(Some(Probe {
                cli_version: None,
                result: budget.check().map(|()| vec![]),
            }))
            .unwrap();
    }
    wait(&c);
    for provider in c.summary(&p)["providers"].as_array().unwrap() {
        assert_eq!(provider["status"], "discovery_failed");
        assert_eq!(provider["error"]["kind"], "cancelled");
    }
    assert!(c.state.lock().unwrap().cancel.is_none());
    let pending = start_controlled_refresh(&c, &p, &requests);
    for (provider, (budget, reply)) in pending {
        assert!(budget.check().is_ok());
        reply
            .send(if provider == Provider::Codex {
                None
            } else {
                Some(Probe {
                    cli_version: Some("fixture 1".into()),
                    result: Ok(vec![row("model")]),
                })
            })
            .unwrap();
    }
    wait(&c);
    let summary = c.summary(&p);
    assert_eq!(summary["providers"][0]["error"]["kind"], "process");
    assert_eq!(summary["providers"][0]["status"], "discovery_failed");
    assert_eq!(summary["providers"][1]["status"], "discovered");
    assert!(c.state.lock().unwrap().cancel.is_none());
}

#[test]
fn failed_probe_statuses_recover_only_applicable_missing_executable_blockers() {
    use FailureKind::*;
    for (cached, blocker, version, failure, status, remaining_blocker) in [
        (false, None, None, Io, "discovery_failed", None),
        (
            false,
            None,
            Some("fixture 2"),
            Unsupported,
            "unsupported_discovery",
            None,
        ),
        (true, None, Some("fixture 2"), Io, "cached_stale", None),
        (
            true,
            None,
            Some("fixture 2"),
            Unsupported,
            "unsupported_discovery",
            None,
        ),
        (
            true,
            Some(MissingExecutable),
            None,
            Io,
            "unavailable",
            Some(MissingExecutable),
        ),
        (
            true,
            Some(MissingExecutable),
            Some("fixture 2"),
            Io,
            "cached_stale",
            None,
        ),
        (
            true,
            Some(MissingExecutable),
            Some("fixture 2"),
            Unsupported,
            "unsupported_discovery",
            None,
        ),
        (
            true,
            Some(Auth),
            Some("fixture 2"),
            Io,
            "unavailable",
            Some(Auth),
        ),
        (
            true,
            Some(MissingExecutable),
            Some("fixture 2"),
            Auth,
            "unavailable",
            Some(Auth),
        ),
        (
            true,
            None,
            Some("fixture 2"),
            Rejected,
            "unavailable",
            Some(Rejected),
        ),
    ] {
        let temp = Temp::new();
        let p = policy();
        let key = scope(Provider::Codex, &p);
        let path = temp.0.join(format!("{key}.json"));
        let good = if cached {
            let seed = Catalogue::new(
                Fixed::new(Ok(vec![row("explicit-model")])),
                temp.0.clone(),
                Duration::from_secs(2),
            );
            assert!(seed.refresh(p.clone()));
            wait(&seed);
            Some(std::fs::read(&path).unwrap())
        } else {
            None
        };
        let (discovery, requests) = Controlled::new();
        let c = Catalogue::new(discovery, temp.0.clone(), Duration::from_secs(2));
        let pending = start_controlled_refresh(&c, &p, &requests);
        if let Some(kind) = blocker {
            c.observe(
                &p,
                Provider::Codex,
                "explicit-model",
                Err(Failure::new(kind)),
            );
        }
        for (_, (_, reply)) in pending {
            reply
                .send(Some(Probe {
                    cli_version: version.map(str::to_owned),
                    result: Err(Failure::new(failure)),
                }))
                .unwrap();
        }
        wait(&c);
        let details = c.details(&p);
        let state = &details["providers"][0];
        assert_eq!(
            state["status"], status,
            "{blocker:?}, {version:?}, {failure:?}"
        );
        assert_eq!(state["blocker"]["kind"], json!(remaining_blocker));
        assert_eq!(state["error"]["kind"], json!(failure));
        assert_eq!(
            state["cached_cli_version"],
            if cached {
                json!("fixture 1")
            } else {
                Value::Null
            }
        );
        assert_eq!(
            state["cli_version"],
            json!(version.or(if cached { Some("fixture 1") } else { None }))
        );
        assert_eq!(
            c.execution_input(&p, Provider::Codex, "explicit-model")["eligible"],
            remaining_blocker.is_none()
        );
        if let Some(good) = good {
            assert_eq!(std::fs::read(&path).unwrap(), good);
            let cache: Value = serde_json::from_slice(&good).unwrap();
            for field in ["models", "diff", "removed", "refreshed_unix"] {
                assert_eq!(state[field], cache[field], "{field}");
            }
        } else {
            assert!(!path.exists());
            assert_eq!(state["models"], json!([]));
        }
        // CLI version comparisons must still use the last successful discovery.
        let pending = start_controlled_refresh(&c, &p, &requests);
        for (_, (_, reply)) in pending {
            reply
                .send(Some(Probe {
                    cli_version: Some("fixture 2".into()),
                    result: Ok(vec![row("explicit-model")]),
                }))
                .unwrap();
        }
        wait(&c);
        assert_eq!(
            c.summary(&p)["providers"][0]["changes"]["cli_version_changed"],
            cached
        );
        let retained_execution_blocker =
            blocker.is_some() && !(blocker == Some(MissingExecutable) && version.is_some());
        assert_eq!(
            c.execution_input(&p, Provider::Codex, "explicit-model")["eligible"],
            !retained_execution_blocker
        );
        assert_eq!(
            read_cache(&path, Provider::Codex, &key)
                .unwrap()
                .unwrap()
                .cli_version,
            "fixture 2"
        );
    }
}

#[test]
fn refresh_diffs_current_models_and_retires_then_reactivates_aliases() {
    let temp = Temp::new();
    let p = policy();
    let mut original = row("explicit-model");
    original.aliases = vec!["old-alias".into()];
    original.resolved_id = Some("old-wire".into());
    let fixed = Fixed::new(Ok(vec![original.clone()]));
    let c = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    assert!(c.refresh(p.clone()));
    wait(&c);
    let key = scope(Provider::Codex, &p);
    let path = temp.0.join(format!("{key}.json"));
    let mut disk = read_cache(&path, Provider::Codex, &key).unwrap().unwrap();
    disk.models = vec![row("unrelated-disk-model")];
    disk.cli_version = "unrelated-disk-version".into();
    disk.removed.insert("disk-tombstone".into());
    write_cache(&path, &disk).unwrap();
    let mut changed = original.clone();
    changed.aliases = vec!["new-alias".into()];
    changed.resolved_id = Some("new-wire".into());
    *fixed.result.lock().unwrap() = Ok(vec![changed.clone()]);
    assert!(c.refresh(p.clone()));
    wait(&c);
    let good = std::fs::read(&path).unwrap();
    let cache: Value = serde_json::from_slice(&good).unwrap();
    assert_eq!(cache["models"], json!([changed]));
    assert_eq!(
        cache["diff"],
        json!({"added":[], "removed":[], "alias_retargeted":["explicit-model"], "capabilities_changed":[], "cli_version_changed":false})
    );
    assert_eq!(cache["removed"], json!(["old-alias", "old-wire"]));
    *fixed.result.lock().unwrap() = Err(Failure::new(FailureKind::Io));
    let restarted = Catalogue::new(fixed.clone(), temp.0.clone(), Duration::from_secs(2));
    assert!(restarted.refresh(p.clone()));
    wait(&restarted);
    assert_eq!(std::fs::read(&path).unwrap(), good);
    let details = restarted.details(&p);
    for field in ["models", "diff", "removed", "refreshed_unix"] {
        assert_eq!(details["providers"][0][field], cache[field]);
    }
    for id in ["old-alias", "old-wire"] {
        assert_eq!(
            restarted.execution_input(&p, Provider::Codex, id)["eligible"],
            false
        );
    }
    *fixed.result.lock().unwrap() = Ok(vec![original]);
    assert!(restarted.refresh(p.clone()));
    wait(&restarted);
    for id in ["explicit-model", "old-alias", "old-wire"] {
        assert_eq!(
            restarted.execution_input(&p, Provider::Codex, id)["availability"],
            "discovered"
        );
        assert_eq!(
            restarted.execution_input(&p, Provider::Codex, id)["eligible"],
            true
        );
    }
    for id in ["new-alias", "new-wire"] {
        assert_eq!(
            restarted.execution_input(&p, Provider::Codex, id)["eligible"],
            false
        );
    }
    assert_eq!(
        json!(
            read_cache(&path, Provider::Codex, &key)
                .unwrap()
                .unwrap()
                .removed
        ),
        json!(["new-alias", "new-wire"])
    );
}

#[test]
fn cache_publication_failure_is_reported_and_cleared_by_next_success() {
    let temp = Temp::new();
    let p = policy();
    let key = scope(Provider::Codex, &p);
    let path = temp.0.join(format!("{key}.json"));
    std::fs::create_dir(&path).unwrap();
    let c = Catalogue::new(
        Fixed::new(Ok(vec![row("explicit-model")])),
        temp.0.clone(),
        Duration::from_secs(2),
    );
    assert!(c.refresh(p.clone()));
    wait(&c);
    assert_eq!(
        c.summary(&p)["providers"][0]["cache_error"],
        "cache publication failed"
    );
    assert_eq!(
        c.execution_input(&p, Provider::Codex, "explicit-model")["availability"],
        "discovered"
    );
    std::fs::remove_dir(&path).unwrap();
    assert!(c.refresh(p.clone()));
    wait(&c);
    assert!(c.summary(&p)["providers"][0]["cache_error"].is_null());
    assert_eq!(
        read_cache(&path, Provider::Codex, &key)
            .unwrap()
            .unwrap()
            .models,
        vec![row("explicit-model")]
    );
}
