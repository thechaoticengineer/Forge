use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "forge-metadata-test-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn store(&self) -> PathBuf {
        self.0.join("metadata.json")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct FakeClock(AtomicI64);
impl Clock for FakeClock {
    fn now_unix(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct FakeFetch {
    responses: Mutex<BTreeMap<String, VecDeque<Result<FetchResponse, String>>>>,
    calls: Mutex<Vec<FetchRequest>>,
    delay: Duration,
}
impl FakeFetch {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(BTreeMap::new()),
            calls: Mutex::new(vec![]),
            delay: Duration::ZERO,
        })
    }
    fn push(&self, url: &str, response: Result<FetchResponse, String>) {
        self.responses
            .lock()
            .unwrap()
            .entry(url.into())
            .or_default()
            .push_back(response);
    }
    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
impl Fetch for FakeFetch {
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, String> {
        std::thread::sleep(self.delay);
        self.calls.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .get_mut(&request.url)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| panic!("unexpected research request to {}", request.url))
    }
}
fn ok(body: &Value, etag: &str) -> Result<FetchResponse, String> {
    Ok(FetchResponse {
        status: 200,
        location: None,
        etag: Some(etag.into()),
        last_modified: Some("Mon, 01 Sep 2026 00:00:00 GMT".into()),
        body: body.to_string().into_bytes(),
    })
}
fn not_modified() -> Result<FetchResponse, String> {
    Ok(FetchResponse {
        status: 304,
        location: None,
        etag: None,
        last_modified: None,
        body: vec![],
    })
}
fn redirect(location: &str) -> Result<FetchResponse, String> {
    Ok(FetchResponse {
        status: 308,
        location: Some(location.into()),
        etag: None,
        last_modified: None,
        body: vec![],
    })
}
fn doc(models: Value) -> Value {
    json!({"models": models})
}
fn snap(model: &str, fp: &str) -> Snapshot {
    Snapshot {
        provider: Provider::Codex,
        model: model.into(),
        discovery_fingerprint: fp.into(),
        discovered_reasoning: None,
    }
}
fn service(fetch: Arc<FakeFetch>, clock: Arc<FakeClock>, temp: &Temp) -> Service {
    Service::new(fetch, clock, temp.store())
}
fn wait(s: &Service) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while s.running() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn run(s: &Service, policy: &Policy, snaps: Vec<Snapshot>) {
    assert!(s.refresh(policy, snaps));
    wait(s);
}
const CODEX_URL: &str = "https://learn.chatgpt.com/docs/models.md";
const CLAUDE_URL: &str = "https://platform.claude.com/docs/en/models/overview.md";

#[test]
fn zero_research_on_fresh_unchanged_startup_and_selective_refresh() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000_000)));
    let fetch = FakeFetch::new();
    let policy = Policy::default();
    let body = doc(json!([
        {"id": "model-a", "context_window": 400000, "max_output_tokens": 128000,
         "reasoning": true, "lifecycle": "available",
         "pricing": {"currency": "USD", "unit": "per_million_tokens", "basis": "api_list_rate",
             "as_of": "2026-09-01", "input": 1.25, "output": 10.0}},
        {"id": "model-b", "lifecycle": "available"}
    ]));
    fetch.push(CODEX_URL, ok(&body, "v1"));
    let s = service(fetch.clone(), clock.clone(), &temp);
    // Two models, one source: coalesced into a single request.
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 1);
    let details = s.details(&[snap("model-a", "fp1")]);
    let a = &details["records"][0];
    assert_eq!(a["model"], "model-a");
    assert_eq!(a["provenance"], "official");
    assert_eq!(a["source_url"], CODEX_URL);
    assert_eq!(a["verified_unix"], 1_000_000);
    assert_eq!(a["fields"]["context_window"], 400000);
    assert_eq!(a["pricing"]["label"], "api_list_rate");
    assert_eq!(a["pricing"]["currency"], "USD");
    assert_eq!(a["pricing"]["basis"], "api_list_rate");
    assert_eq!(a["pricing"]["as_of"], "2026-09-01");
    let b = &details["records"][1];
    assert_eq!(b["pricing"], Value::Null); // Unknown costs stay null.
    assert!(b["unknown"].as_array().unwrap().contains(&json!("pricing")));

    // Fresh unchanged restart: persisted store, matching fingerprints, valid
    // TTL — zero research requests.
    let restarted_fetch = FakeFetch::new();
    let restarted = service(restarted_fetch.clone(), clock.clone(), &temp);
    clock.0.store(1_000_600, Ordering::SeqCst);
    run(&restarted, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(restarted_fetch.calls(), 0);
    assert_eq!(restarted.summary()["records"], 2);

    // A materially changed model refetches selectively, with a conditional
    // request carrying the stored validators.
    restarted_fetch.push(
        CODEX_URL,
        ok(&doc(json!([{"id": "model-a", "context_window": 272000}, {"id": "model-b"}])), "v2"),
    );
    run(&restarted, &policy, vec![snap("model-a", "fp2"), snap("model-b", "fp1")]);
    assert_eq!(restarted_fetch.calls(), 1);
    let request = restarted_fetch.calls.lock().unwrap()[0].clone();
    assert_eq!(request.etag.as_deref(), Some("v1"));
    assert!(request.last_modified.is_some());
    let details = restarted.details(&[]);
    assert_eq!(details["records"][0]["fields"]["context_window"], 272000);
    assert_eq!(details["records"][0]["unknown"].as_array().unwrap().len(), 4);

    // TTL expiry revalidates; 304 keeps facts and fingerprint, bumps only
    // freshness — timestamps alone never look like a material change.
    let before = restarted.details(&[])["records"][0].clone();
    clock
        .0
        .store(1_000_600 + policy.metadata_ttl_hours as i64 * 3600, Ordering::SeqCst);
    restarted_fetch.push(CODEX_URL, not_modified());
    run(&restarted, &policy, vec![snap("model-a", "fp2"), snap("model-b", "fp1")]);
    assert_eq!(restarted_fetch.calls(), 2);
    let after = restarted.details(&[])["records"][0].clone();
    assert_eq!(after["fields"], before["fields"]);
    assert_eq!(after["fingerprint"], before["fingerprint"]);
    assert!(after["verified_unix"].as_i64().unwrap() > before["verified_unix"].as_i64().unwrap());
}

#[test]
fn newly_interesting_model_forces_full_body_instead_of_conditional_304() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000_000)));
    let fetch = FakeFetch::new();
    let policy = Policy::default();
    // The source lists both models, but only model-a is interesting at first.
    let body = doc(json!([
        {"id": "model-a", "lifecycle": "available"},
        {"id": "model-b", "lifecycle": "available"}
    ]));
    fetch.push(CODEX_URL, ok(&body, "v1"));
    let s = service(fetch.clone(), clock.clone(), &temp);
    run(&s, &policy, vec![snap("model-a", "fp1")]);
    assert_eq!(fetch.calls(), 1);
    assert_eq!(s.details(&[])["records"].as_array().unwrap().len(), 1);

    // model-b becomes interesting (newly discovered or newly configured). The
    // stored parse never looked it up, so a conditional 304 could not answer;
    // the validators must be omitted so the unchanged document returns a body
    // and real fields instead of a fabricated absence.
    clock.0.store(1_000_600, Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&body, "v1"));
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 2);
    let request = fetch.calls.lock().unwrap()[1].clone();
    assert_eq!(request.etag, None);
    assert_eq!(request.last_modified, None);
    let details = s.details(&[]);
    assert_eq!(details["negative"].as_array().unwrap().len(), 0);
    let b = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "model-b")
        .unwrap();
    assert_eq!(b["fields"]["lifecycle"], "available");

    // With every interesting model accounted for again, revalidation goes
    // back to conditional requests.
    clock
        .0
        .store(1_000_600 + policy.metadata_ttl_hours as i64 * 3600, Ordering::SeqCst);
    fetch.push(CODEX_URL, not_modified());
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 3);
    let request = fetch.calls.lock().unwrap()[2].clone();
    assert_eq!(request.etag.as_deref(), Some("v1"));
    assert_eq!(s.details(&[])["negative"].as_array().unwrap().len(), 0);
}

#[test]
fn document_change_while_out_of_interest_reconciles_stale_negatives_and_records() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000_000)));
    let fetch = FakeFetch::new();
    let policy = Policy::default();
    // v1 lacks model-b -> negative entry; model-c gains a record; etag v1.
    let v1 = doc(json!([
        {"id": "model-a", "lifecycle": "available"},
        {"id": "model-c", "lifecycle": "available"}
    ]));
    fetch.push(CODEX_URL, ok(&v1, "v1"));
    let s = service(fetch.clone(), clock.clone(), &temp);
    run(
        &s,
        &policy,
        vec![snap("model-a", "fp1"), snap("model-b", "fp1"), snap("model-c", "fp1")],
    );
    assert_eq!(s.details(&[])["negative"].as_array().unwrap().len(), 1);

    // model-b and model-c leave the interest set; the document changes to v2,
    // which now lists model-b and deprecates model-c. A TTL-due refresh for
    // model-a alone must reconcile both against the full parse — otherwise the
    // stored etag would keep vouching for facts v2 contradicts.
    let v2 = doc(json!([
        {"id": "model-a", "lifecycle": "available"},
        {"id": "model-b", "lifecycle": "available"},
        {"id": "model-c", "lifecycle": "deprecated"}
    ]));
    let t2 = 1_000_000 + policy.metadata_ttl_hours as i64 * 3600;
    clock.0.store(t2, Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&v2, "v2"));
    run(&s, &policy, vec![snap("model-a", "fp1")]);
    assert_eq!(fetch.calls(), 2);
    let details = s.details(&[]);
    assert_eq!(details["negative"].as_array().unwrap().len(), 0);
    let c = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "model-c")
        .unwrap();
    assert_eq!(c["fields"]["lifecycle"], "deprecated");
    assert_eq!(c["verified_unix"], t2);

    // model-b returns: with the stale negative gone it forces a full body and
    // gains real fields, instead of a conditional 304 re-fabricating an
    // absence the current document contradicts.
    clock.0.store(t2 + 600, Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&v2, "v2"));
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 3);
    let request = fetch.calls.lock().unwrap()[2].clone();
    assert_eq!(request.etag, None);
    assert_eq!(request.last_modified, None);
    let details = s.details(&[]);
    assert_eq!(details["negative"].as_array().unwrap().len(), 0);
    let b = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "model-b")
        .unwrap();
    assert_eq!(b["fields"]["lifecycle"], "available");
}

#[test]
fn both_providers_fetch_their_own_sources() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(5_000)));
    let fetch = FakeFetch::new();
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "codex-model"}])), "c1"));
    fetch.push(CLAUDE_URL, ok(&doc(json!([{"id": "claude-model", "reasoning": false}])), "a1"));
    let s = service(fetch.clone(), clock, &temp);
    let mut claude = snap("claude-model", "fp1");
    claude.provider = Provider::Claude;
    claude.discovered_reasoning = Some(true);
    run(&s, &Policy::default(), vec![snap("codex-model", "fp1"), claude.clone()]);
    assert_eq!(fetch.calls(), 2);
    // Discovered native support wins over descriptive metadata; the conflict
    // is surfaced, not silently overwritten.
    let details = s.details(&[claude]);
    let record = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "claude-model")
        .unwrap();
    assert_eq!(record["conflicts"][0]["field"], "supports_reasoning");
    assert_eq!(record["conflicts"][0]["effective"], true);
    assert_eq!(record["fields"]["supports_reasoning"], false);
}

#[test]
fn negative_cache_backoff_and_known_missing_fields_do_not_refetch() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(100_000)));
    let fetch = FakeFetch::new();
    let policy = Policy::default();
    // model-a present without pricing/limits; model-b absent from the source.
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-a"}])), "v1"));
    let s = service(fetch.clone(), clock.clone(), &temp);
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 1);
    let negative = s.details(&[])["negative"].clone();
    assert_eq!(negative[0]["model"], "model-b");
    assert_eq!(negative[0]["attempts"], 1);
    let next = negative[0]["next_attempt_unix"].as_i64().unwrap();
    assert_eq!(next, 100_000 + backoff(1));

    // Already-known missing fields and a not-yet-due negative entry cause no
    // research at all.
    clock.0.store(100_010, Ordering::SeqCst);
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 1);

    // Once the backoff expires the retry happens and the backoff grows.
    clock.0.store(next, Ordering::SeqCst);
    fetch.push(CODEX_URL, not_modified());
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    assert_eq!(fetch.calls(), 2);
    let negative = s.details(&[])["negative"].clone();
    assert_eq!(negative[0]["attempts"], 2);
    assert_eq!(negative[0]["next_attempt_unix"], next + backoff(2));
    assert!(backoff(2) > backoff(1));
    assert_eq!(backoff(30), 24 * 3600); // capped

    // When the source finally lists the model the negative entry clears.
    clock.0.store(next + backoff(2), Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-a"}, {"id": "model-b"}])), "v2"));
    run(&s, &policy, vec![snap("model-a", "fp1"), snap("model-b", "fp1")]);
    let details = s.details(&[]);
    assert_eq!(details["negative"].as_array().unwrap().len(), 0);
    assert_eq!(details["records"].as_array().unwrap().len(), 2);
}

#[test]
fn allowlist_rejects_off_list_urls_and_validates_every_redirect() {
    for url in [
        "http://developers.openai.com/x",
        "https://evil.example/x",
        "https://developers.openai.com.evil.example/x",
        "https://developers.openai.com:8443/x",
        "https://user@developers.openai.com/x",
        "https://developers.openai.com/x y",
        "ftp://developers.openai.com/x",
    ] {
        assert!(validate_url(url).is_err(), "{url}");
    }
    assert!(validate_url("https://platform.claude.com/docs/models.json").is_ok());
    assert!(validate_url("https://LEARN.chatgpt.com/models").is_ok());

    // An allowed redirect chain is followed; every hop is validated.
    let fetch = FakeFetch::new();
    fetch.push("https://platform.openai.com/old", redirect("https://developers.openai.com/new"));
    fetch.push("https://developers.openai.com/new", redirect("/newer"));
    fetch.push("https://developers.openai.com/newer", ok(&json!({"models": []}), "v1"));
    assert!(matches!(
        retrieve(&*fetch, "https://platform.openai.com/old", None, None),
        Ok(Document::Fresh { .. })
    ));
    // Off-allowlist redirect targets are rejected without being fetched.
    let fetch = FakeFetch::new();
    fetch.push("https://platform.openai.com/old", redirect("https://evil.example/steal"));
    assert!(retrieve(&*fetch, "https://platform.openai.com/old", None, None).is_err());
    assert_eq!(fetch.calls(), 1);
    let fetch = FakeFetch::new();
    fetch.push("https://platform.openai.com/old", redirect("//evil.example/steal"));
    assert!(retrieve(&*fetch, "https://platform.openai.com/old", None, None).is_err());
    // Redirect loops stop at the hop limit.
    let fetch = FakeFetch::new();
    for _ in 0..8 {
        fetch.push("https://platform.openai.com/loop", redirect("/loop"));
    }
    assert_eq!(
        retrieve(&*fetch, "https://platform.openai.com/loop", None, None).unwrap_err(),
        "too many redirects"
    );
    assert!(retrieve(&*fetch, "https://evil.example/", None, None).is_err());
}

#[test]
fn curl_response_parsing_is_a_pure_fixture() {
    let raw = b"HTTP/2 200\r\nContent-Type: application/json\r\nETag: \"abc\"\r\nLast-Modified: Mon, 01 Sep 2026 00:00:00 GMT\r\n\r\n{\"models\":[]}";
    let r = parse_http_response(raw).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.etag.as_deref(), Some("\"abc\""));
    assert!(r.last_modified.is_some());
    assert_eq!(r.body, b"{\"models\":[]}");
    let r = parse_http_response(b"HTTP/1.1 308 Permanent Redirect\r\nLocation: https://developers.openai.com/x\r\n\r\n").unwrap();
    assert_eq!(r.status, 308);
    assert_eq!(r.location.as_deref(), Some("https://developers.openai.com/x"));
    assert!(parse_http_response(b"garbage").is_err());
    assert!(parse_http_response(b"HTTP/2 nope\r\n\r\n").is_err());
    // Header values are sanitized: control characters cannot be smuggled.
    let r = parse_http_response(b"HTTP/2 200\r\nETag: a\x01b\r\n\r\nx").unwrap();
    assert_eq!(r.etag.as_deref(), Some("ab"));
}

#[test]
fn failures_and_unsupported_documents_preserve_last_known_good_with_backoff() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(50_000)));
    let fetch = FakeFetch::new();
    let policy = Policy::default();
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-a", "lifecycle": "available"}])), "v1"));
    let s = service(fetch.clone(), clock.clone(), &temp);
    run(&s, &policy, vec![snap("model-a", "fp1")]);

    // Transport failure after a fingerprint change: one bounded retry, then
    // source backoff; the last-known-good record survives untouched.
    fetch.push(CODEX_URL, Err("connection reset".into()));
    fetch.push(CODEX_URL, Err("connection reset".into()));
    clock.0.store(50_100, Ordering::SeqCst);
    run(&s, &policy, vec![snap("model-a", "fp2")]);
    assert_eq!(fetch.calls(), 3);
    let details = s.details(&[]);
    assert_eq!(details["records"][0]["fields"]["lifecycle"], "available");
    let source = &details["sources"][0];
    assert_eq!(source["failures"], 1);
    assert_eq!(source["error"], "connection reset");
    assert_eq!(source["next_attempt_unix"], 50_100 + backoff(1));

    // While the source is backing off nothing is fetched at all.
    clock.0.store(50_200, Ordering::SeqCst);
    run(&s, &policy, vec![snap("model-a", "fp2")]);
    assert_eq!(fetch.calls(), 3);

    // An unsupported document also backs off instead of fabricating facts.
    clock.0.store(50_100 + backoff(1), Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&json!({"unexpected": true}), "v2"));
    run(&s, &policy, vec![snap("model-a", "fp2")]);
    let details = s.details(&[]);
    assert_eq!(details["sources"][0]["error"], "unsupported document format");
    assert_eq!(details["sources"][0]["failures"], 2);
    assert_eq!(details["records"][0]["fields"]["lifecycle"], "available");

    // A model that discovery no longer reports is retained for audit.
    clock.0.store(400_000, Ordering::SeqCst);
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-c"}])), "v3"));
    run(&s, &policy, vec![snap("model-c", "fp1")]);
    let details = s.details(&[]);
    let removed = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "model-a")
        .unwrap();
    assert_eq!(removed["removed"], true);
    assert_eq!(removed["fields"]["lifecycle"], "available");
    // An empty snapshot (failed discovery, no cache) does not flag removals.
    run(&s, &policy, vec![]);
    let details = s.details(&[]);
    let kept = details["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "model-c")
        .unwrap();
    assert_eq!(kept["removed"], false);
}

#[test]
fn incomplete_pricing_stays_unknown_and_never_becomes_a_subscription_charge() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
    let fetch = FakeFetch::new();
    // Missing billing basis: the whole rate is unknown, never guessed.
    fetch.push(
        CODEX_URL,
        ok(&doc(json!([{"id": "model-a",
            "pricing": {"currency": "USD", "unit": "per_million_tokens",
                "as_of": "2026-09-01", "input": 1.0, "output": 2.0}}])), "v1"),
    );
    let s = service(fetch.clone(), clock, &temp);
    run(&s, &Policy::default(), vec![snap("model-a", "fp1")]);
    let record = &s.details(&[])["records"][0];
    assert_eq!(record["pricing"], Value::Null);
    assert!(record["unknown"].as_array().unwrap().contains(&json!("pricing")));
    assert!(!record.to_string().contains("subscription"));
}

#[test]
fn slow_research_never_blocks_summary_or_details() {
    let temp = Temp::new();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
    let fetch = Arc::new(FakeFetch {
        responses: Mutex::new(BTreeMap::new()),
        calls: Mutex::new(vec![]),
        delay: Duration::from_millis(300),
    });
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-a"}])), "v1"));
    let s = service(fetch.clone(), clock, &temp);
    assert!(s.refresh(&Policy::default(), vec![snap("model-a", "fp1")]));
    assert!(!s.refresh(&Policy::default(), vec![snap("model-a", "fp1")]));
    let start = Instant::now();
    for _ in 0..20 {
        let _ = s.summary();
        let _ = s.details(&[]);
    }
    assert!(start.elapsed() < Duration::from_millis(200));
    wait(&s);
    assert_eq!(s.summary()["records"], 1);
}

#[test]
fn scheduler_runs_separate_validated_intervals_without_polling() {
    let policy = Policy::default();
    let scheduler = Scheduler::new(10_000);
    // Metadata has never run: due on the first tick (a fresh unchanged store
    // then makes zero requests). Discovery ran at startup: not due yet.
    assert_eq!(scheduler.due(10_030, &policy), (false, true));
    scheduler.mark_metadata(10_030);
    assert_eq!(scheduler.due(10_060, &policy), (false, false));
    let discovery_due = 10_000 + policy.discovery_refresh_minutes as i64 * 60;
    assert_eq!(scheduler.due(discovery_due - 1, &policy), (false, false));
    assert_eq!(scheduler.due(discovery_due, &policy), (true, false));
    scheduler.mark_discovery(discovery_due);
    assert_eq!(scheduler.due(discovery_due + 60, &policy), (false, false));
    let metadata_due = 10_030 + policy.metadata_refresh_minutes as i64 * 60;
    assert_eq!(scheduler.due(metadata_due, &policy).1, true);

    // Interval settings are validated.
    for (key, value) in [
        ("discovery_refresh_minutes", json!(1)),
        ("metadata_refresh_minutes", json!(0)),
        ("metadata_ttl_hours", json!(9000)),
        ("metadata_research", json!("yes")),
    ] {
        let mut settings = crate::plan::default_settings();
        settings["model_catalogue"][key] = value;
        assert!(Policy::from_settings(&settings).is_err(), "{key}");
    }
    let mut settings = crate::plan::default_settings();
    settings["model_catalogue"]["metadata_refresh_minutes"] = json!(60);
    assert_eq!(
        Policy::from_settings(&settings).unwrap().metadata_refresh_minutes,
        60
    );
}

#[test]
fn corrupt_store_is_reported_and_research_rebuilds_it() {
    let temp = Temp::new();
    std::fs::write(temp.store(), b"{").unwrap();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000)));
    let fetch = FakeFetch::new();
    let s = service(fetch.clone(), clock, &temp);
    assert_eq!(s.summary()["store_error"], "metadata store is corrupt");
    fetch.push(CODEX_URL, ok(&doc(json!([{"id": "model-a"}])), "v1"));
    run(&s, &Policy::default(), vec![snap("model-a", "fp1")]);
    assert_eq!(s.summary()["store_error"], Value::Null);
    assert_eq!(s.summary()["records"], 1);
    let reread = read_store(&temp.store()).unwrap().unwrap();
    assert_eq!(reread.records.len(), 1);
    assert_eq!(reread.records[0].provenance, "official");
}

#[test]
fn catalogue_snapshot_covers_discovered_and_configured_only_models() {
    use crate::catalogue::{Catalogue, Discovery, Entry, Probe, Tier};
    use crate::catalogue_process::Budget;
    struct Fixed(Vec<crate::catalogue::Model>);
    impl Discovery for Fixed {
        fn probe(&self, provider: Provider, _: &Policy, _: Budget) -> Probe {
            Probe {
                cli_version: Some("fixture 1".into()),
                result: if provider == Provider::Codex {
                    Ok(self.0.clone())
                } else {
                    Ok(vec![])
                },
            }
        }
    }
    let temp = Temp::new();
    let rows = json!([
        {"id": "alias", "model": "wire-id", "supportedReasoningEfforts": [{"reasoningEffort": "high"}]},
        {"id": "plain"}
    ]);
    let models = crate::catalogue::normalize(Provider::Codex, &rows).unwrap();
    let mut policy = Policy::default();
    policy.entries.push(Entry {
        provider: Provider::Codex,
        model: "alias".into(), // covered by discovery; must not duplicate
        tier: Tier::Strong,
        suitability: vec![],
        limits: BTreeMap::new(),
        relative_cost_preference: None,
        effort: "provider_default".into(),
    });
    policy.entries.push(Entry {
        provider: Provider::Claude,
        model: "configured-only".into(),
        tier: Tier::Basic,
        suitability: vec![],
        limits: BTreeMap::new(),
        relative_cost_preference: None,
        effort: "provider_default".into(),
    });
    let c = Catalogue::new(
        Arc::new(Fixed(models)),
        temp.0.clone(),
        Duration::from_secs(2),
    );
    c.refresh(policy.clone());
    let deadline = Instant::now() + Duration::from_secs(3);
    while c.running() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let snaps = c.metadata_snapshot(&policy);
    let names: Vec<(Provider, &str)> = snaps
        .iter()
        .map(|s| (s.provider, s.model.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            (Provider::Codex, "wire-id"),
            (Provider::Codex, "plain"),
            (Provider::Claude, "configured-only"),
        ]
    );
    let wire = snaps.iter().find(|s| s.model == "wire-id").unwrap();
    assert_eq!(wire.discovered_reasoning, Some(true));
    assert_eq!(
        snaps.iter().find(|s| s.model == "configured-only").unwrap().discovery_fingerprint,
        "configured-only"
    );
    // Capability changes move the fingerprint; unchanged discovery does not.
    assert_eq!(c.metadata_snapshot(&policy), snaps);
}

#[test]
fn markdown_comparison_table_yields_exact_ids_and_token_counts() {
    let doc = "\
# Models overview\n\
\n\
| Feature | Claude A | Claude B | Claude C |\n\
| :------ | :------- | :------- | :------- |\n\
| Description | prose | prose | prose |\n\
| [Pricing](https://platform.claude.com/docs/pricing) | $10 / MTok | $5 / MTok | $2 / MTok |\n\
| Claude API ID | `model-a` | `model-b` | not-an-id |\n\
| [Context window](https://platform.claude.com/docs/context) | 1M tokens | 200K tokens | 200K tokens |\n\
| Max output | 128K tokens | 64K tokens | ~64K tokens |\n\
| Reliable knowledge cutoff | Jun 2026 | May 2026 | May 2026 |\n";
    let models = parse_document(doc.as_bytes()).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models["model-a"]["context_window"], json!(1_000_000));
    assert_eq!(models["model-a"]["max_output_tokens"], json!(128_000));
    assert_eq!(models["model-b"]["context_window"], json!(200_000));
    assert_eq!(models["model-b"]["max_output_tokens"], json!(64_000));
    // Unrecognized labels, unbackticked IDs and non-exact counts stay unknown.
    assert!(!models["model-a"].contains_key("pricing"));
}

#[test]
fn markdown_slug_attributes_name_models_without_fields() {
    let doc = "\
## Recommended models\n\
<ModelDetails name=\"gpt-x\" slug=\"gpt-x\" description=\"prose\" />\n\
<ModelDetails slug=\"gpt-y-mini\" slug=\"bad id\" />\n";
    let models = parse_document(doc.as_bytes()).unwrap();
    assert_eq!(
        models.keys().collect::<Vec<_>>(),
        vec!["gpt-x", "gpt-y-mini"]
    );
    assert!(models.values().all(BTreeMap::is_empty));
}

#[test]
fn markdown_without_documented_patterns_is_unsupported() {
    for doc in [
        "# Models\nJust prose about models.\n",
        "| Feature | A |\n| :-- | :-- |\n| Context window | 1M tokens |\n",
        "\u{fffd}\u{fffd}",
    ] {
        assert_eq!(
            parse_document(doc.as_bytes()).unwrap_err(),
            "unsupported document format"
        );
    }
}


#[test]
fn stale_source_state_from_a_previous_adapter_url_is_pruned_on_refresh() {
    let temp = Temp::new();
    std::fs::write(
        temp.store(),
        json!({"version": 1, "records": [], "negative": [], "sources": {
            "https://developers.openai.com/codex/models.json": {
                "url": "https://developers.openai.com/codex/models.json",
                "etag": null, "last_modified": null, "fingerprint": null,
                "checked_unix": 999_000, "error": "http status 404",
                "failures": 1, "next_attempt_unix": 2_000_000}}})
        .to_string(),
    )
    .unwrap();
    let clock = Arc::new(FakeClock(AtomicI64::new(1_000_000)));
    let s = service(FakeFetch::new(), clock, &temp);
    assert_eq!(s.summary()["source_errors"], 1);
    run(&s, &Policy::default(), vec![]);
    assert_eq!(s.summary()["source_errors"], 0);
    assert!(s.details(&[])["sources"].as_array().unwrap().is_empty());
}
