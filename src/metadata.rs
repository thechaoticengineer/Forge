//! Bounded official-metadata enrichment. Documents are data, never instructions;
//! official pages never grant runtime availability, and configured tiers route
//! whether or not research has run. Only the fixed adapters below are fetched.
use crate::catalogue::{Policy, Provider, identifier, write_atomic_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const VERSION: u32 = 1;
/// Enforceable HTTPS official-host allowlist; every redirect hop is revalidated.
pub(crate) const ALLOWED_HOSTS: [&str; 5] = [
    "developers.openai.com",
    "platform.openai.com",
    "learn.chatgpt.com",
    "code.claude.com",
    "platform.claude.com",
];
/// Routing-relevant fields an official document may provide. Anything else,
/// including quality comparisons, stays unknown rather than fabricated.
const ROUTING_FIELDS: [&str; 5] = [
    "context_window",
    "max_output_tokens",
    "supports_reasoning",
    "lifecycle",
    "pricing",
];
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const BACKOFF_BASE_SECS: i64 = 3600;
const BACKOFF_CAP_SECS: i64 = 24 * 3600;

pub(crate) trait Clock: Send + Sync {
    fn now_unix(&self) -> i64;
}
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        crate::util::unix_timestamp()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FetchRequest {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct FetchResponse {
    pub status: u16,
    pub location: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body: Vec<u8>,
}
/// One HTTPS request without redirect following; validation stays in `retrieve`.
pub(crate) trait Fetch: Send + Sync {
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, String>;
}

pub(crate) struct CurlFetch {
    pub timeout_secs: u32,
}
impl Fetch for CurlFetch {
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, String> {
        let mut cmd = std::process::Command::new("curl");
        // No -L: redirects come back to retrieve() so every hop is validated.
        cmd.args([
            "-sS",
            "-i",
            "--proto",
            "=https",
            "--max-filesize",
            "2097152",
            "--connect-timeout",
            "10",
            "--max-time",
            &self.timeout_secs.to_string(),
            "-H",
            "Accept: application/json",
        ]);
        if let Some(etag) = &request.etag {
            cmd.args(["-H", &format!("If-None-Match: {etag}")]);
        }
        if let Some(modified) = &request.last_modified {
            cmd.args(["-H", &format!("If-Modified-Since: {modified}")]);
        }
        cmd.args(["--url", &request.url]);
        cmd.stdin(std::process::Stdio::null());
        let out = cmd
            .output()
            .map_err(|_| "curl is unavailable".to_string())?;
        if !out.status.success() {
            return Err(format!(
                "curl exited with status {}",
                out.status.code().unwrap_or(-1)
            ));
        }
        parse_http_response(&out.stdout)
    }
}

fn header_value(v: &str) -> Option<String> {
    let v: String = v
        .trim()
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(256)
        .collect();
    (!v.is_empty()).then_some(v)
}
pub(crate) fn parse_http_response(raw: &[u8]) -> Result<FetchResponse, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("malformed http response")?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "malformed http response")?;
    let body = raw[split + 4..].to_vec();
    if body.len() > MAX_BODY {
        return Err("document exceeds size limit".into());
    }
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("malformed http status")?;
    let mut response = FetchResponse {
        status,
        location: None,
        etag: None,
        last_modified: None,
        body,
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "location" => response.location = header_value(value),
            "etag" => response.etag = header_value(value),
            "last-modified" => response.last_modified = header_value(value),
            _ => {}
        }
    }
    Ok(response)
}

pub(crate) fn validate_url(url: &str) -> Result<(), String> {
    if url.len() > 2048
        || !url.is_ascii()
        || url.chars().any(|c| c.is_ascii_control() || c == ' ')
    {
        return Err("invalid url".into());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or("only https:// urls are allowed")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') || authority.contains(':') {
        return Err("urls with credentials or ports are not allowed".into());
    }
    if !ALLOWED_HOSTS.contains(&authority.to_ascii_lowercase().as_str()) {
        return Err("host is not on the official allowlist".into());
    }
    Ok(())
}
fn resolve_redirect(base: &str, location: &str) -> Result<String, String> {
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let rest = base.strip_prefix("https://").unwrap_or(base);
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        return Ok(format!("https://{host}{location}"));
    }
    Err("redirect target must be absolute https or a same-host absolute path".into())
}

#[derive(Debug)]
pub(crate) enum Document {
    NotModified,
    Fresh {
        body: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}
/// Follow at most MAX_REDIRECTS hops, validating each URL against the
/// allowlist, with one bounded retry per hop for transport errors.
pub(crate) fn retrieve(
    fetch: &dyn Fetch,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Document, String> {
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        validate_url(&url)?;
        let request = FetchRequest {
            url: url.clone(),
            etag: etag.map(str::to_owned),
            last_modified: last_modified.map(str::to_owned),
        };
        let response = fetch
            .fetch(&request)
            .or_else(|_| fetch.fetch(&request))?;
        match response.status {
            200 => {
                if response.body.len() > MAX_BODY {
                    return Err("document exceeds size limit".into());
                }
                return Ok(Document::Fresh {
                    body: response.body,
                    etag: response.etag,
                    last_modified: response.last_modified,
                });
            }
            304 => return Ok(Document::NotModified),
            301 | 302 | 303 | 307 | 308 => {
                let location = response.location.ok_or("redirect without location")?;
                url = resolve_redirect(&url, &location)?;
            }
            status => return Err(format!("http status {status}")),
        }
    }
    Err("too many redirects".into())
}

/// The documented source adapters: one official index document per provider —
/// the markdown model pages both providers actually publish (no machine-readable
/// JSON index exists). Documents that do not match a documented shape are
/// unsupported and produce backoff state, never guesses.
pub(crate) fn source_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "https://learn.chatgpt.com/docs/models.md",
        Provider::Claude => "https://platform.claude.com/docs/en/models/overview.md",
    }
}
/// An API list rate is stored only when complete: currency, unit, billing
/// basis, source date and the explicit API label. Anything less stays unknown;
/// a CLI subscription charge is never inferred from it.
fn pricing(v: &Value) -> Option<Value> {
    let currency = v["currency"]
        .as_str()
        .filter(|s| (1..=8).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase()))?;
    let unit = v["unit"].as_str().filter(|s| (1..=64).contains(&s.len()))?;
    let basis = v["basis"].as_str().filter(|s| (1..=64).contains(&s.len()))?;
    let as_of = v["as_of"].as_str().filter(|s| (1..=32).contains(&s.len()))?;
    let input = v["input"].as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
    let output = v["output"].as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
    Some(json!({"label": "api_list_rate", "currency": currency, "unit": unit,
        "basis": basis, "as_of": as_of, "input": input, "output": output}))
}
/// Documented shapes: a JSON index {"models":[{"id","context_window",
/// "max_output_tokens","reasoning","lifecycle","pricing":{...}}]}, or the
/// providers' official markdown model pages. Fields are optional; absent or
/// unparseable values remain unknown rather than guessed.
fn parse_document(body: &[u8]) -> Result<BTreeMap<String, BTreeMap<String, Value>>, String> {
    if body.len() > MAX_BODY {
        return Err("document exceeds size limit".into());
    }
    let Ok(doc) = serde_json::from_slice::<Value>(body) else {
        let text = std::str::from_utf8(body).map_err(|_| "unsupported document format")?;
        let models = parse_markdown(text);
        return if models.is_empty() {
            Err("unsupported document format".into())
        } else {
            Ok(models)
        };
    };
    let rows = doc["models"]
        .as_array()
        .filter(|a| a.len() <= 512)
        .ok_or("unsupported document format")?;
    let mut models = BTreeMap::new();
    for row in rows {
        let Some(id) = row["id"].as_str().filter(|s| identifier(s)) else {
            continue;
        };
        let mut fields = BTreeMap::new();
        for key in ["context_window", "max_output_tokens"] {
            if let Some(n) = row[key].as_u64() {
                fields.insert(key.to_string(), json!(n));
            }
        }
        if let Some(b) = row["reasoning"].as_bool() {
            fields.insert("supports_reasoning".into(), json!(b));
        }
        if let Some(s) = row["lifecycle"]
            .as_str()
            .filter(|s| (1..=64).contains(&s.len()) && s.is_ascii())
        {
            fields.insert("lifecycle".into(), json!(s));
        }
        if let Some(p) = pricing(&row["pricing"]) {
            fields.insert("pricing".into(), p);
        }
        models.insert(id.to_string(), fields);
    }
    Ok(models)
}

/// Markdown extraction is limited to two exact, bounded patterns the official
/// pages publish: comparison tables keyed by a "Claude API ID" row (context
/// window and max output per column) and `slug="…"` model attributes, which
/// name models without providing routing facts. Prose is never interpreted.
fn parse_markdown(text: &str) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut models = BTreeMap::new();
    let mut rest = text;
    while let Some(i) = rest.find("slug=\"") {
        rest = &rest[i + 6..];
        let Some(j) = rest.find('"') else { break };
        let id = &rest[..j];
        if identifier(id) && models.len() < 512 {
            models.entry(id.to_string()).or_default();
        }
        rest = &rest[j + 1..];
    }
    let mut table: Vec<Vec<String>> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('|') && line.ends_with('|') && line.len() > 1 {
            let cells: Vec<String> = line[1..line.len() - 1]
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            // Alignment separators carry no data.
            if !cells
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, ':' | '-')))
            {
                table.push(cells);
            }
        } else if !table.is_empty() {
            table_facts(&table, &mut models);
            table.clear();
        }
    }
    table_facts(&table, &mut models);
    models
}
/// One comparison table: the "Claude API ID" row names each column's model;
/// only exactly recognized rows contribute fields.
fn table_facts(table: &[Vec<String>], models: &mut BTreeMap<String, BTreeMap<String, Value>>) {
    let ids: Vec<Option<String>> = match table.iter().find(|r| r.first().is_some_and(|c| plain_label(c) == "Claude API ID")) {
        Some(row) => row[1..]
            .iter()
            .map(|c| {
                let id = c.trim_matches('`');
                (c.starts_with('`') && c.ends_with('`') && identifier(id)).then(|| id.to_string())
            })
            .collect(),
        None => return,
    };
    for row in table {
        let field = match row.first().map(|c| plain_label(c)).as_deref() {
            Some("Context window") => "context_window",
            Some("Max output") => "max_output_tokens",
            _ => continue,
        };
        for (id, cell) in ids.iter().zip(&row[1..]) {
            if let (Some(id), Some(n)) = (id, token_count(cell)) {
                if models.len() < 512 || models.contains_key(id) {
                    models.entry(id.clone()).or_default().insert(field.into(), json!(n));
                }
            }
        }
    }
}
/// A label cell is plain text or a single markdown link around it.
fn plain_label(cell: &str) -> String {
    match (cell.strip_prefix('['), cell.split_once("](")) {
        (Some(_), Some((label, _))) => label[1..].to_string(),
        _ => cell.to_string(),
    }
}
/// Exact token counts like "200K tokens" or "1M tokens"; anything else stays unknown.
fn token_count(cell: &str) -> Option<u64> {
    let v = cell.strip_suffix(" tokens")?;
    let (digits, mult) = match v.strip_suffix('M') {
        Some(n) => (n, 1_000_000),
        None => (v.strip_suffix('K')?, 1_000),
    };
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(mult)
}

pub(crate) fn fingerprint(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}
pub(crate) fn backoff(failures: u32) -> i64 {
    BACKOFF_BASE_SECS
        .saturating_mul(1i64 << failures.saturating_sub(1).min(20))
        .min(BACKOFF_CAP_SECS)
}
fn truncated(s: &str) -> String {
    s.chars().take(200).collect()
}

/// What discovery currently knows about one model; the fingerprint changes on
/// retargeting or material capability changes and triggers selective research.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub provider: Provider,
    pub model: String,
    pub discovery_fingerprint: String,
    pub discovered_reasoning: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Record {
    pub provider: Provider,
    pub model: String,
    pub fields: BTreeMap<String, Value>,
    /// Routing fields this source verifiably did not provide; they stay
    /// unknown and do not retrigger research until the record itself is due.
    pub unknown: BTreeSet<String>,
    pub source_url: String,
    pub verified_unix: i64,
    pub fingerprint: String,
    pub discovery_fingerprint: String,
    pub provenance: String,
    #[serde(default)]
    pub removed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Negative {
    pub provider: Provider,
    pub model: String,
    pub error: String,
    pub attempts: u32,
    pub last_attempt_unix: i64,
    pub next_attempt_unix: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SourceState {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub fingerprint: Option<String>,
    pub checked_unix: Option<i64>,
    pub error: Option<String>,
    pub failures: u32,
    pub next_attempt_unix: i64,
}
#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    records: Vec<Record>,
    negative: Vec<Negative>,
    sources: BTreeMap<String, SourceState>,
}

#[derive(Default)]
struct State {
    loaded: bool,
    records: Vec<Record>,
    negative: Vec<Negative>,
    sources: BTreeMap<String, SourceState>,
    running: bool,
    last_refresh_unix: Option<i64>,
    last_requests: u64,
    store_error: Option<String>,
}
pub(crate) struct Service {
    state: Arc<Mutex<State>>,
    fetch: Arc<dyn Fetch>,
    pub(crate) clock: Arc<dyn Clock>,
    path: PathBuf,
}
impl Default for Service {
    fn default() -> Self {
        let root = std::env::var_os("XDG_CACHE_HOME")
            .filter(|p| Path::new(p).is_absolute())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
                    .join(".cache")
            });
        Self::new(
            Arc::new(CurlFetch { timeout_secs: 20 }),
            Arc::new(SystemClock),
            root.join("forge/models/v1/metadata.json"),
        )
    }
}
impl Service {
    pub fn new(fetch: Arc<dyn Fetch>, clock: Arc<dyn Clock>, path: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            fetch,
            clock,
            path,
        }
    }
    fn ensure_loaded(state: &mut State, path: &Path) {
        if state.loaded {
            return;
        }
        state.loaded = true;
        match read_store(path) {
            Ok(Some(file)) => {
                state.records = file.records;
                state.negative = file.negative;
                state.sources = file.sources;
            }
            Ok(None) => {}
            Err(e) => state.store_error = Some(e),
        }
    }
    pub fn running(&self) -> bool {
        self.state.lock().unwrap().running
    }
    /// Claim in constant time; all network work runs on its own thread so slow
    /// or failing research never blocks state access or project execution.
    pub fn refresh(&self, policy: &Policy, snapshots: Vec<Snapshot>) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.running {
            return false;
        }
        Self::ensure_loaded(&mut state, &self.path);
        state.running = true;
        drop(state);
        let state = self.state.clone();
        let fetch = self.fetch.clone();
        let clock = self.clock.clone();
        let path = self.path.clone();
        let ttl_secs = policy.metadata_ttl_hours as i64 * 3600;
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_pass(&state, &*fetch, &*clock, &path, ttl_secs, &snapshots)
            }));
            let mut locked = state.lock().unwrap();
            locked.running = false;
            if result.is_err() {
                locked.store_error = Some("metadata refresh crashed".into());
            }
        });
        true
    }
    pub fn summary(&self) -> Value {
        let mut state = self.state.lock().unwrap();
        Self::ensure_loaded(&mut state, &self.path);
        let unknown_pricing = state
            .records
            .iter()
            .filter(|r| !r.fields.contains_key("pricing"))
            .count();
        let source_errors = state
            .sources
            .values()
            .filter(|s| s.error.is_some())
            .count();
        json!({"records": state.records.len(), "unknown_pricing": unknown_pricing,
            "negative": state.negative.len(), "source_errors": source_errors,
            "refreshing": state.running, "last_refresh_unix": state.last_refresh_unix,
            "last_requests": state.last_requests, "store_error": state.store_error,
            "provenance": "official"})
    }
    pub fn details(&self, snapshots: &[Snapshot]) -> Value {
        let mut state = self.state.lock().unwrap();
        Self::ensure_loaded(&mut state, &self.path);
        let records: Vec<Value> = state
            .records
            .iter()
            .map(|r| {
                let snap = snapshots
                    .iter()
                    .find(|s| s.provider == r.provider && s.model == r.model);
                let mut conflicts = vec![];
                if let (Some(official), Some(discovered)) = (
                    r.fields.get("supports_reasoning").and_then(Value::as_bool),
                    snap.and_then(|s| s.discovered_reasoning),
                ) && official != discovered
                {
                    conflicts.push(json!({"field": "supports_reasoning",
                        "official": official, "discovered": discovered, "effective": discovered,
                        "note": "discovered native support wins over descriptive metadata"}));
                }
                json!({"provider": r.provider, "model": r.model, "fields": r.fields,
                    "unknown": r.unknown,
                    "pricing": r.fields.get("pricing").cloned().unwrap_or(Value::Null),
                    "source_url": r.source_url, "verified_unix": r.verified_unix,
                    "fingerprint": r.fingerprint, "provenance": r.provenance,
                    "removed": r.removed, "conflicts": conflicts})
            })
            .collect();
        json!({"version": VERSION, "refreshing": state.running, "records": records,
            "negative": state.negative, "sources": state.sources.values().collect::<Vec<_>>(),
            "last_refresh_unix": state.last_refresh_unix, "last_requests": state.last_requests,
            "store_error": state.store_error,
            "note": "official metadata never grants runtime availability"})
    }
}

fn bump_negative(list: &mut Vec<Negative>, provider: Provider, model: &str, now: i64, error: &str) {
    if let Some(n) = list
        .iter_mut()
        .find(|n| n.provider == provider && n.model == model)
    {
        n.attempts = n.attempts.saturating_add(1);
        n.last_attempt_unix = now;
        n.next_attempt_unix = now + backoff(n.attempts);
        n.error = truncated(error);
    } else {
        list.push(Negative {
            provider,
            model: model.into(),
            error: truncated(error),
            attempts: 1,
            last_attempt_unix: now,
            next_attempt_unix: now + backoff(1),
        });
    }
}

fn run_pass(
    state: &Arc<Mutex<State>>,
    fetch: &dyn Fetch,
    clock: &dyn Clock,
    path: &Path,
    ttl_secs: i64,
    snapshots: &[Snapshot],
) {
    let now = clock.now_unix();
    let (mut records, mut negative, mut sources) = {
        let s = state.lock().unwrap();
        (s.records.clone(), s.negative.clone(), s.sources.clone())
    };
    // A source URL that is no longer an adapter's target would otherwise keep
    // its stored freshness and error state — and its errors — forever.
    sources.retain(|url, _| {
        [Provider::Codex, Provider::Claude]
            .iter()
            .any(|p| source_url(*p) == url)
    });
    // Selective research: only new/retargeted/materially changed models, newly
    // missing routing fields, TTL-expired records, or due negative retries.
    let mut by_url: BTreeMap<&'static str, Vec<&Snapshot>> = BTreeMap::new();
    let mut due_urls: BTreeSet<&'static str> = BTreeSet::new();
    // Sources where a 304 cannot answer what is due: a model the stored parse
    // never looked up, or a routing field it never classified. Conditional
    // validators are omitted there so an unchanged document still yields a
    // body instead of a fabricated absence.
    let mut full_body_urls: BTreeSet<&'static str> = BTreeSet::new();
    for snap in snapshots {
        let url = source_url(snap.provider);
        by_url.entry(url).or_default().push(snap);
        let entry = negative
            .iter()
            .find(|n| n.provider == snap.provider && n.model == snap.model);
        let (need, needs_body) = if let Some(n) = entry {
            (now >= n.next_attempt_unix, false)
        } else if let Some(r) = records
            .iter()
            .find(|r| r.provider == snap.provider && r.model == snap.model)
        {
            let unclassified = ROUTING_FIELDS
                .iter()
                .any(|f| !r.fields.contains_key(*f) && !r.unknown.contains(*f));
            let need = r.discovery_fingerprint != snap.discovery_fingerprint
                || now - r.verified_unix >= ttl_secs
                || unclassified;
            (need, unclassified)
        } else {
            (true, true)
        };
        if need {
            due_urls.insert(url);
            if needs_body {
                full_body_urls.insert(url);
            }
        }
    }
    // Coalesce per source, and honor source-level backoff after failures.
    due_urls.retain(|url| {
        sources
            .get(*url)
            .is_none_or(|s| now >= s.next_attempt_unix)
    });
    let mut requests = 0u64;
    for url in &due_urls {
        let interest = &by_url[url];
        let source = sources.entry(url.to_string()).or_insert_with(|| SourceState {
            url: url.to_string(),
            ..SourceState::default()
        });
        requests += 1;
        let conditional = !full_body_urls.contains(url);
        match retrieve(
            fetch,
            url,
            source.etag.as_deref().filter(|_| conditional),
            source.last_modified.as_deref().filter(|_| conditional),
        ) {
            Ok(Document::NotModified) => {
                source.checked_unix = Some(now);
                source.error = None;
                source.failures = 0;
                source.next_attempt_unix = 0;
                // Unchanged document: facts stay valid, only freshness and the
                // discovery fingerprint advance. Nothing material changed.
                for snap in interest {
                    if let Some(r) = records
                        .iter_mut()
                        .find(|r| r.provider == snap.provider && r.model == snap.model)
                    {
                        r.verified_unix = now;
                        r.discovery_fingerprint = snap.discovery_fingerprint.clone();
                    } else {
                        // Recordless models here always carry a negative entry
                        // already (a never-looked-up model forces a full body
                        // above), and every Fresh parse reconciles negatives
                        // against the full document the stored validators
                        // vouch for, so this confirms a known absence against
                        // the unchanged document rather than fabricating one.
                        bump_negative(
                            &mut negative,
                            snap.provider,
                            &snap.model,
                            now,
                            "model is not present in the official source",
                        );
                    }
                }
            }
            Ok(Document::Fresh {
                body,
                etag,
                last_modified,
            }) => match parse_document(&body) {
                Ok(models) => {
                    source.etag = etag;
                    source.last_modified = last_modified;
                    source.fingerprint = Some(fingerprint(&body));
                    source.checked_unix = Some(now);
                    source.error = None;
                    source.failures = 0;
                    source.next_attempt_unix = 0;
                    // Reconcile every stored fact for this source against the
                    // full parse, not just the current interest set: a negative
                    // or record left stale across a document change would let a
                    // later 304 "confirm" it against a version that says
                    // otherwise.
                    negative.retain(|n| {
                        source_url(n.provider) != *url || !models.contains_key(&n.model)
                    });
                    for r in records.iter_mut().filter(|r| r.source_url == *url) {
                        if let Some(fields) = models.get(&r.model) {
                            r.unknown = ROUTING_FIELDS
                                .iter()
                                .filter(|f| !fields.contains_key(**f))
                                .map(|f| f.to_string())
                                .collect();
                            r.fields = fields.clone();
                            r.verified_unix = now;
                            r.fingerprint = fingerprint(&body);
                        }
                    }
                    for snap in interest {
                        match models.get(&snap.model) {
                            Some(fields) => {
                                let unknown: BTreeSet<String> = ROUTING_FIELDS
                                    .iter()
                                    .filter(|f| !fields.contains_key(**f))
                                    .map(|f| f.to_string())
                                    .collect();
                                let updated = Record {
                                    provider: snap.provider,
                                    model: snap.model.clone(),
                                    fields: fields.clone(),
                                    unknown,
                                    source_url: url.to_string(),
                                    verified_unix: now,
                                    fingerprint: fingerprint(&body),
                                    discovery_fingerprint: snap.discovery_fingerprint.clone(),
                                    provenance: "official".into(),
                                    removed: false,
                                };
                                match records.iter_mut().find(|r| {
                                    r.provider == snap.provider && r.model == snap.model
                                }) {
                                    Some(r) => *r = updated,
                                    None => records.push(updated),
                                }
                            }
                            None => bump_negative(
                                &mut negative,
                                snap.provider,
                                &snap.model,
                                now,
                                "model is not present in the official source",
                            ),
                        }
                    }
                }
                Err(e) => {
                    // Unsupported document: backoff, retain last-known-good.
                    source.failures = source.failures.saturating_add(1);
                    source.error = Some(truncated(&e));
                    source.next_attempt_unix = now + backoff(source.failures);
                }
            },
            Err(e) => {
                source.failures = source.failures.saturating_add(1);
                source.error = Some(truncated(&e));
                source.next_attempt_unix = now + backoff(source.failures);
            }
        }
    }
    // Models gone from discovery keep last-known-good records for audit, but
    // only when the provider actually reported a model list this pass.
    let covered: BTreeSet<Provider> = snapshots.iter().map(|s| s.provider).collect();
    for r in &mut records {
        if covered.contains(&r.provider) {
            r.removed = !snapshots
                .iter()
                .any(|s| s.provider == r.provider && s.model == r.model);
        }
    }
    let file = json!(StoreFile {
        version: VERSION,
        records: records.clone(),
        negative: negative.clone(),
        sources: sources.clone(),
    });
    let write = write_atomic_json(path, &file);
    let mut locked = state.lock().unwrap();
    locked.records = records;
    locked.negative = negative;
    locked.sources = sources;
    locked.last_refresh_unix = Some(now);
    locked.last_requests = requests;
    locked.store_error = write.err();
}

fn read_store(path: &Path) -> Result<Option<StoreFile>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("metadata store read failed".into()),
    };
    if file
        .metadata()
        .map_err(|_| "metadata store metadata failed")?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("metadata store exceeds size limit".into());
    }
    let store: StoreFile =
        serde_json::from_reader(file).map_err(|_| "metadata store is corrupt")?;
    if store.version != VERSION
        || store.records.len() > 4096
        || store.records.iter().any(|r| !identifier(&r.model))
    {
        return Err("metadata store version/schema mismatch".into());
    }
    Ok(Some(store))
}

/// Application-owned periodic work: discovery and metadata refresh on separate
/// validated intervals, driven by the engine, never by panel polling.
pub(crate) struct Scheduler {
    last: Mutex<(i64, i64)>,
}
impl Scheduler {
    pub fn new(now: i64) -> Self {
        Self {
            last: Mutex::new((now, 0)),
        }
    }
    pub fn due(&self, now: i64, policy: &Policy) -> (bool, bool) {
        let last = self.last.lock().unwrap();
        let discovery = now - last.0 >= policy.discovery_refresh_minutes as i64 * 60;
        let metadata = last.1 == 0 || now - last.1 >= policy.metadata_refresh_minutes as i64 * 60;
        (discovery, metadata)
    }
    pub fn mark_discovery(&self, now: i64) {
        self.last.lock().unwrap().0 = now;
    }
    pub fn mark_metadata(&self, now: i64) {
        self.last.lock().unwrap().1 = now;
    }
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
