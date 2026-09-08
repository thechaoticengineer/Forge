//! Application-owned availability catalogue. Discovery is evidence, configured tiers are policy.
use crate::catalogue_process::{Budget, Channel, CommandSpec, Launcher, SystemLauncher};
use crate::util::unix_timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

static CACHE_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const VERSION: u32 = 1;
const MAX_MODELS: usize = 1024;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provider {
    Codex,
    Claude,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    Basic,
    Standard,
    Strong,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub provider: Provider,
    pub model: String,
    pub tier: Tier,
    #[serde(default)]
    pub suitability: Vec<String>,
    #[serde(default)]
    pub limits: BTreeMap<String, u64>,
    #[serde(default)]
    pub relative_cost_preference: Option<u32>,
    #[serde(default = "default_effort")]
    pub effort: String,
}
fn default_effort() -> String {
    "provider_default".into()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub policy_revision: String,
    pub codex_scope: String,
    pub claude_scope: String,
    pub claude_bridge: String,
    pub entries: Vec<Entry>,
    /// Periodic application refresh intervals and the official-metadata TTL.
    /// `metadata_research: false` keeps the service cache-only; configured
    /// tiers route either way.
    #[serde(default = "default_discovery_minutes")]
    pub discovery_refresh_minutes: u64,
    #[serde(default = "default_metadata_minutes")]
    pub metadata_refresh_minutes: u64,
    #[serde(default = "default_metadata_ttl_hours")]
    pub metadata_ttl_hours: u64,
    #[serde(default = "default_metadata_research")]
    pub metadata_research: bool,
}
fn default_discovery_minutes() -> u64 {
    360
}
fn default_metadata_minutes() -> u64 {
    1440
}
fn default_metadata_ttl_hours() -> u64 {
    168
}
fn default_metadata_research() -> bool {
    true
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            policy_revision: "1".into(),
            codex_scope: "default".into(),
            claude_scope: "default".into(),
            claude_bridge: String::new(),
            entries: vec![],
            discovery_refresh_minutes: default_discovery_minutes(),
            metadata_refresh_minutes: default_metadata_minutes(),
            metadata_ttl_hours: default_metadata_ttl_hours(),
            metadata_research: default_metadata_research(),
        }
    }
}
pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/[]".contains(c))
        && !s.starts_with('-')
}
impl Policy {
    pub fn from_settings(settings: &Value) -> Result<Self, String> {
        let p: Self = serde_json::from_value(settings["model_catalogue"].clone())
            .map_err(|e| format!("invalid model_catalogue: {e}"))?;
        if !identifier(&p.policy_revision)
            || !identifier(&p.codex_scope)
            || !identifier(&p.claude_scope)
        {
            return Err(
                "catalogue revision and scopes must be non-empty identifiers (max 200 bytes)"
                    .into(),
            );
        }
        if p.claude_bridge.len() > 4096
            || (!p.claude_bridge.is_empty() && !Path::new(&p.claude_bridge).is_absolute())
        {
            return Err("claude_bridge must be empty or an absolute executable path".into());
        }
        if p.entries.len() > 64 {
            return Err("model registry is limited to 64 entries".into());
        }
        if !(5..=10080).contains(&p.discovery_refresh_minutes)
            || !(15..=10080).contains(&p.metadata_refresh_minutes)
            || !(1..=8760).contains(&p.metadata_ttl_hours)
        {
            return Err(
                "refresh intervals out of range (discovery 5-10080m, metadata 15-10080m, ttl 1-8760h)"
                    .into(),
            );
        }
        let mut keys = BTreeSet::new();
        for e in &p.entries {
            if !identifier(&e.model)
                || !identifier(&e.effort)
                || !keys.insert((e.provider, &e.model))
            {
                return Err("invalid or duplicate provider/model, or invalid effort".into());
            }
            if e.suitability.len() > 16
                || e.suitability.iter().any(|s| s.len() > 256)
                || e.limits.len() > 16
                || e.limits.keys().any(|s| !identifier(s))
                || e.relative_cost_preference.is_some_and(|n| n > 1000)
            {
                return Err(
                    "registry suitability/limits/preferences exceed bounds (preference 0..1000)"
                        .into(),
                );
            }
        }
        Ok(p)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Model {
    pub id: String,
    pub resolved_id: Option<String>,
    pub aliases: Vec<String>,
    pub display_name: Option<String>,
    pub is_default: Option<bool>,
    pub default_effort: Option<String>,
    pub supported_efforts: Option<Vec<String>>,
    pub capabilities: BTreeMap<String, Value>,
    pub pricing: Option<Value>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    MissingExecutable,
    Unsupported,
    Auth,
    Rejected,
    Malformed,
    Io,
    Process,
    Timeout,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub message: String,
}
impl Failure {
    pub fn new(kind: FailureKind) -> Self {
        let message = match kind {
            FailureKind::MissingExecutable => "provider executable is missing",
            FailureKind::Unsupported => {
                "discovery unsupported: install/configure the optional bridge or a supported CLI"
            }
            FailureKind::Auth => "provider authentication failed; use the CLI to sign in",
            FailureKind::Rejected => "provider rejected access",
            FailureKind::Malformed => "invalid or oversized discovery response",
            FailureKind::Io => "discovery IO failed",
            FailureKind::Process => "discovery process failed",
            FailureKind::Timeout => "discovery timed out",
            FailureKind::Cancelled => "discovery cancelled",
        };
        Self {
            kind,
            message: message.into(),
        }
    }
    fn blocks(&self) -> bool {
        matches!(
            self.kind,
            FailureKind::MissingExecutable | FailureKind::Auth | FailureKind::Rejected
        )
    }
}
pub(crate) fn auth_error(s: &str) -> bool {
    let s = s.to_lowercase();
    [
        "unauthorized",
        "authentication_error",
        "authentication failed",
        "not logged in",
        "not authenticated",
        "login required",
        "invalid api key",
        "invalid_api_key",
        "token expired",
        "please log in",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}
fn protocol_error(v: &Value) -> Failure {
    let s = v.to_string().to_lowercase();
    Failure::new(if auth_error(&s) || v["code"] == 401 {
        FailureKind::Auth
    } else if v["code"] == 403 {
        FailureKind::Rejected
    } else if v["code"] == -32601 || s.contains("unsupported") || s.contains("unknown method") {
        FailureKind::Unsupported
    } else {
        FailureKind::Process
    })
}
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(256).collect())
}
fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array()
        .map(|a| a.iter().take(64).filter_map(text).collect())
}
pub(crate) fn normalize(provider: Provider, rows: &Value) -> Result<Vec<Model>, Failure> {
    let rows = rows
        .as_array()
        .filter(|a| a.len() <= MAX_MODELS)
        .ok_or_else(|| Failure::new(FailureKind::Malformed))?;
    let mut models = Vec::new();
    let mut ids = BTreeSet::new();
    for row in rows {
        let id_key = if provider == Provider::Codex {
            "id"
        } else {
            "value"
        };
        let id = row[id_key]
            .as_str()
            .filter(|s| identifier(s))
            .ok_or_else(|| Failure::new(FailureKind::Malformed))?
            .to_owned();
        if !ids.insert(id.clone()) {
            return Err(Failure::new(FailureKind::Malformed));
        }
        let resolved_id = text(
            &row[if provider == Provider::Codex {
                "model"
            } else {
                "resolvedModel"
            }],
        );
        if resolved_id.as_ref().is_some_and(|s| !identifier(s)) {
            return Err(Failure::new(FailureKind::Malformed));
        }
        let mut aliases = strings(&row["aliases"]).unwrap_or_default();
        aliases.retain(|s| identifier(s));
        if resolved_id.as_ref().is_some_and(|s| s != &id) {
            aliases.push(id.clone());
        }
        aliases.sort();
        aliases.dedup();
        let supported_efforts = if provider == Provider::Codex {
            row["supportedReasoningEfforts"].as_array().map(|a| {
                a.iter()
                    .take(64)
                    .filter_map(|e| text(&e["reasoningEffort"]))
                    .collect()
            })
        } else if row["supportsEffort"] == false {
            Some(vec![])
        } else {
            strings(&row["supportedEffortLevels"])
        };
        let mut capabilities = BTreeMap::new();
        for key in [
            "inputModalities",
            "supportsPersonality",
            "supportsEffort",
            "supportsAdaptiveThinking",
            "supportsFastMode",
            "supportsAutoMode",
            "hidden",
        ] {
            if let Some(v) = row
                .get(key)
                .filter(|v| !v.is_null() && v.to_string().len() <= 2048)
            {
                capabilities.insert(key.into(), v.clone());
            }
        }
        models.push(Model {
            id,
            resolved_id,
            aliases,
            display_name: text(&row["displayName"]),
            is_default: row["isDefault"].as_bool(),
            default_effort: text(&row["defaultReasoningEffort"]),
            supported_efforts,
            capabilities,
            pricing: None,
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}
#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub cli_version: Option<String>,
    pub result: Result<Vec<Model>, Failure>,
}
pub(crate) trait Discovery: Send + Sync {
    fn probe(&self, provider: Provider, policy: &Policy, budget: Budget) -> Probe;
}
pub(crate) struct CliDiscovery {
    pub launcher: Arc<dyn Launcher>,
    pub cwd: PathBuf,
}
impl Default for CliDiscovery {
    fn default() -> Self {
        Self {
            launcher: Arc::new(SystemLauncher),
            cwd: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        }
    }
}
impl CliDiscovery {
    fn command(
        &self,
        executable: &str,
        args: &[&str],
        budget: Budget,
    ) -> Result<Box<dyn Channel>, Failure> {
        self.launcher.spawn(
            &CommandSpec {
                executable: executable.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
                cwd: self.cwd.clone(),
            },
            budget,
        )
    }
    fn discover(
        &self,
        provider: Provider,
        policy: &Policy,
        budget: Budget,
        version: &str,
    ) -> Result<Vec<Model>, Failure> {
        if provider == Provider::Claude {
            if policy.claude_bridge.is_empty() {
                return Err(Failure::new(FailureKind::Unsupported));
            }
            let mut c = self
                .command(
                    &policy.claude_bridge,
                    &["--forge-discovery-v1", provider.name(), version],
                    budget,
                )
                .map_err(|e| {
                    if e.kind == FailureKind::MissingExecutable {
                        Failure::new(FailureKind::Unsupported)
                    } else {
                        e
                    }
                })?;
            for _ in 0..4096 {
                let line = c
                    .line()?
                    .ok_or_else(|| Failure::new(FailureKind::Unsupported))?;
                let v: Value = serde_json::from_str(&line)
                    .map_err(|_| Failure::new(FailureKind::Malformed))?;
                if v.get("notification").is_some() {
                    continue;
                }
                if v["bridge_version"] != 1 {
                    return Err(Failure::new(FailureKind::Unsupported));
                }
                if let Some(e) = v.get("error") {
                    return Err(protocol_error(e));
                }
                if v["source"] != "claude_code_initialization"
                    || v["cli_version"] != version
                    || v["sdk_version"] != "0.3.261"
                {
                    return Err(Failure::new(FailureKind::Unsupported));
                }
                let models = normalize(provider, &v["models"])?;
                c.finish()?;
                return Ok(models);
            }
            return Err(Failure::new(FailureKind::Malformed));
        }
        let mut c = self.command("codex", &["app-server"], budget)?;
        rpc(
            &mut *c,
            0,
            "initialize",
            json!({"clientInfo":{"name":"forge_catalogue","version":env!("CARGO_PKG_VERSION")}}),
        )?;
        c.send("{\"method\":\"initialized\",\"params\":{}}\n")?;
        // This control request uses existing CLI authentication, never credential files.
        // Older app-servers may not support it; model/list still provides discovery.
        match rpc(&mut *c, 1, "account/read", json!({"refreshToken": false})) {
            Ok(v) if v["requiresOpenaiAuth"] == true && v["account"].is_null() => {
                return Err(Failure::new(FailureKind::Auth));
            }
            Err(e) if e.kind != FailureKind::Unsupported => return Err(e),
            _ => {}
        }
        let mut cursor = Value::Null;
        let mut cursors = BTreeSet::new();
        let mut models = Vec::new();
        for page in 0..64 {
            let result = rpc(
                &mut *c,
                page + 2,
                "model/list",
                json!({"limit":100,"cursor":cursor,"includeHidden":true}),
            )?;
            models.extend(normalize(provider, &result["data"])?);
            if models.len() > MAX_MODELS {
                return Err(Failure::new(FailureKind::Malformed));
            }
            cursor = result["nextCursor"].clone();
            if cursor.is_null() {
                models.sort_by(|a, b| a.id.cmp(&b.id));
                if models.windows(2).any(|w| w[0].id == w[1].id) {
                    return Err(Failure::new(FailureKind::Malformed));
                }
                return Ok(models);
            }
            if !cursor.is_string() || !cursors.insert(cursor.to_string()) {
                return Err(Failure::new(FailureKind::Malformed));
            }
        }
        Err(Failure::new(FailureKind::Malformed))
    }
}
fn rpc(c: &mut dyn Channel, id: u32, method: &str, params: Value) -> Result<Value, Failure> {
    c.send(&format!(
        "{}\n",
        json!({"id":id,"method":method,"params":params})
    ))?;
    for _ in 0..4096 {
        let line = c
            .line()?
            .ok_or_else(|| Failure::new(FailureKind::Process))?;
        let v: Value =
            serde_json::from_str(&line).map_err(|_| Failure::new(FailureKind::Malformed))?;
        if !v.is_object() {
            return Err(Failure::new(FailureKind::Malformed));
        }
        if v["id"] != id {
            continue;
        }
        if let Some(e) = v.get("error") {
            return Err(protocol_error(e));
        }
        return v
            .get("result")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| Failure::new(FailureKind::Malformed));
    }
    Err(Failure::new(FailureKind::Malformed))
}
impl Discovery for CliDiscovery {
    fn probe(&self, provider: Provider, policy: &Policy, budget: Budget) -> Probe {
        let version = (|| {
            let mut c = self.command(provider.name(), &["--version"], budget.clone())?;
            let line = c
                .line()?
                .ok_or_else(|| Failure::new(FailureKind::Malformed))?;
            c.finish()?;
            if line.len() > 200 || !line.chars().any(|c| c.is_ascii_digit()) {
                return Err(Failure::new(FailureKind::Malformed));
            }
            Ok(line.trim().to_owned())
        })();
        match version {
            Ok(version) => Probe {
                result: self.discover(provider, policy, budget, &version),
                cli_version: Some(version),
            },
            Err(e) => Probe {
                cli_version: None,
                result: Err(e),
            },
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub alias_retargeted: Vec<String>,
    pub capabilities_changed: Vec<String>,
    pub cli_version_changed: bool,
}
fn diff(old: &[Model], new: &[Model]) -> Diff {
    let mut d = Diff::default();
    for m in new {
        if let Some(previous) = old.iter().find(|p| p.id == m.id) {
            if previous.resolved_id != m.resolved_id || previous.aliases != m.aliases {
                d.alias_retargeted.push(m.id.clone());
            }
            if previous.capabilities != m.capabilities
                || previous.supported_efforts != m.supported_efforts
                || previous.default_effort != m.default_effort
                || previous.is_default != m.is_default
            {
                d.capabilities_changed.push(m.id.clone());
            }
        } else {
            d.added.push(m.id.clone());
        }
    }
    d.removed = old
        .iter()
        .filter(|m| !new.iter().any(|n| n.id == m.id))
        .map(|m| m.id.clone())
        .collect();
    d
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cache {
    version: u32,
    provider: Provider,
    scope: String,
    cli_version: String,
    refreshed_unix: i64,
    models: Vec<Model>,
    diff: Diff,
    // Tombstones survive transient failures and subsequent refreshes.
    removed: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize)]
struct ProviderState {
    provider: Provider,
    scope: String,
    status: String,
    cli_version: Option<String>,
    refreshed_unix: Option<i64>,
    cached_cli_version: Option<String>,
    error: Option<Failure>,
    cache_error: Option<String>,
    models: Vec<Model>,
    diff: Diff,
    removed: BTreeSet<String>,
    blocker: Option<Failure>,
    execution_verified: BTreeSet<String>,
    execution_blocked: bool,
    execution_rejections: BTreeSet<String>,
    invalid_efforts: BTreeMap<String, BTreeSet<String>>,
}
impl ProviderState {
    fn new(provider: Provider, scope: String) -> Self {
        Self {
            provider,
            scope,
            status: "pending".into(),
            cli_version: None,
            refreshed_unix: None,
            cached_cli_version: None,
            error: None,
            cache_error: None,
            models: vec![],
            diff: Diff::default(),
            removed: BTreeSet::new(),
            blocker: None,
            execution_verified: BTreeSet::new(),
            execution_blocked: false,
            execution_rejections: BTreeSet::new(),
            invalid_efforts: BTreeMap::new(),
        }
    }

    // Called under the hydration lock; reading the cache stays in the worker.
    fn hydrate_cache(&mut self, cached: Result<Option<Cache>, String>) {
        match cached {
            Ok(Some(c)) if self.refreshed_unix.is_none() => {
                self.status = "cached_stale".into();
                self.models = c.models;
                self.cached_cli_version = Some(c.cli_version.clone());
                self.cli_version = Some(c.cli_version);
                self.refreshed_unix = Some(c.refreshed_unix);
                self.diff = c.diff;
                self.removed = c.removed;
            }
            Err(e) => self.cache_error = Some(e),
            _ => {}
        }
    }

    // Apply to current state so execution evidence recorded during probing survives.
    fn apply_successful_probe(
        &mut self,
        models: Vec<Model>,
        cli_version: Option<String>,
        scope: String,
    ) -> Cache {
        let mut changes = diff(&self.models, &models);
        changes.cli_version_changed =
            self.cached_cli_version.is_some() && self.cached_cli_version != cli_version;
        self.removed.extend(changes.removed.iter().cloned());
        // Also retire explicit wire IDs covered by a removed or retargeted alias.
        for old in &self.models {
            for id in old.aliases.iter().chain(old.resolved_id.iter()) {
                if !models.iter().any(|m| {
                    m.id == *id || m.resolved_id.as_ref() == Some(id) || m.aliases.contains(id)
                }) {
                    self.removed.insert(id.clone());
                }
            }
        }
        for m in &models {
            for id in std::iter::once(&m.id)
                .chain(m.aliases.iter())
                .chain(m.resolved_id.iter())
            {
                self.removed.remove(id);
            }
        }
        self.models = models;
        self.cached_cli_version = cli_version.clone();
        self.cli_version = cli_version;
        self.diff = changes;
        self.refreshed_unix = Some(unix_timestamp());
        self.status = "discovered".into();
        self.error = None;
        if !self.execution_blocked {
            self.blocker = None;
        }
        if self.blocker.is_some() {
            self.status = "unavailable".into();
        }
        Cache {
            version: VERSION,
            provider: self.provider,
            scope,
            cli_version: self.cli_version.clone().unwrap_or_default(),
            refreshed_unix: self.refreshed_unix.unwrap(),
            models: self.models.clone(),
            diff: self.diff.clone(),
            removed: self.removed.clone(),
        }
    }

    fn apply_failed_probe(&mut self, e: Failure, cli_version: Option<String>) {
        if cli_version.is_some()
            && self
                .blocker
                .as_ref()
                .is_some_and(|b| b.kind == FailureKind::MissingExecutable)
        {
            self.blocker = None;
            self.execution_blocked = false;
        }
        if e.blocks() {
            self.blocker = Some(e.clone());
        }
        self.status = if self.blocker.is_some() {
            "unavailable"
        } else if e.kind == FailureKind::Unsupported {
            "unsupported_discovery"
        } else if self.refreshed_unix.is_some() {
            "cached_stale"
        } else {
            "discovery_failed"
        }
        .into();
        self.error = Some(e);
        if cli_version.is_some() {
            self.cli_version = cli_version;
        }
    }
}
#[derive(Default)]
struct State {
    providers: BTreeMap<Provider, ProviderState>,
    running: bool,
    cancel: Option<Arc<AtomicBool>>,
}
pub(crate) struct Catalogue {
    state: Arc<Mutex<State>>,
    discovery: Arc<dyn Discovery>,
    cache_root: PathBuf,
    timeout: Duration,
}
// Keep cache I/O and probing outside the three original state-lock boundaries.
fn refresh_provider(
    state: &Mutex<State>,
    discovery: &dyn Discovery,
    root: &Path,
    provider: Provider,
    policy: &Policy,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
) {
    let key = scope(provider, policy);
    let path = root.join(format!("{key}.json"));
    let cached = read_cache(&path, provider, &key);
    {
        let mut locked = state.lock().unwrap();
        let s = locked.providers.get_mut(&provider).unwrap();
        s.hydrate_cache(cached);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discovery.probe(
            provider,
            policy,
            Budget {
                deadline: Instant::now() + timeout,
                cancel,
            },
        )
    }))
    .unwrap_or(Probe {
        cli_version: None,
        result: Err(Failure::new(FailureKind::Process)),
    });
    let cache = {
        let mut locked = state.lock().unwrap();
        let s = locked.providers.get_mut(&provider).unwrap();
        match result.result {
            Ok(models) => Some(s.apply_successful_probe(models, result.cli_version, key)),
            Err(e) => {
                s.apply_failed_probe(e, result.cli_version);
                None
            }
        }
    };
    if let Some(cache) = cache {
        if let Err(e) = write_cache(&path, &cache) {
            state
                .lock()
                .unwrap()
                .providers
                .get_mut(&provider)
                .unwrap()
                .cache_error = Some(e);
        } else {
            state
                .lock()
                .unwrap()
                .providers
                .get_mut(&provider)
                .unwrap()
                .cache_error = None;
        }
    }
}

// Stable FNV-1a partition key; no secret or credential contents are read or stored.
fn scope(provider: Provider, policy: &Policy) -> String {
    let configured = if provider == Provider::Codex {
        &policy.codex_scope
    } else {
        &policy.claude_scope
    };
    let mut material = format!("v{VERSION}:{provider:?}:{configured}");
    for key in [
        "HOME",
        "PATH",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "OPENAI_BASE_URL",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "AWS_PROFILE",
        "AWS_REGION",
        "ANTHROPIC_VERTEX_PROJECT_ID",
    ] {
        material.push_str(&format!(
            "|{key}={}",
            std::env::var(key).unwrap_or_default()
        ));
    }
    if provider == Provider::Claude {
        material.push_str(&policy.claude_bridge);
    }
    let hash = material.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    format!("{}-{hash:016x}", provider.name())
}
impl Default for Catalogue {
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
            Arc::new(CliDiscovery::default()),
            root.join("forge/models/v1"),
            Duration::from_secs(15),
        )
    }
}
impl Catalogue {
    pub fn new(discovery: Arc<dyn Discovery>, cache_root: PathBuf, timeout: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            discovery,
            cache_root,
            timeout,
        }
    }
    /// Claim in constant time, then load caches and run both probes away from HTTP/project locks.
    pub fn refresh(&self, policy: Policy) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.running {
            return false;
        }
        state.running = true;
        let cancel = Arc::new(AtomicBool::new(false));
        state.cancel = Some(cancel.clone());
        for provider in [Provider::Codex, Provider::Claude] {
            let key = scope(provider, &policy);
            if state
                .providers
                .get(&provider)
                .is_none_or(|s| s.scope != key)
            {
                state
                    .providers
                    .insert(provider, ProviderState::new(provider, key));
            }
        }
        drop(state);
        let state = self.state.clone();
        let discovery = self.discovery.clone();
        let root = self.cache_root.clone();
        let timeout = self.timeout;
        std::thread::spawn(move || {
            std::thread::scope(|threads| {
                for provider in [Provider::Codex, Provider::Claude] {
                    let state = state.clone();
                    let discovery = discovery.clone();
                    let root = root.clone();
                    let cancel = cancel.clone();
                    let policy = &policy;
                    threads.spawn(move || {
                        refresh_provider(
                            &state, discovery.as_ref(), &root, provider, policy, timeout, cancel,
                        );
                    });
                }
            });
            let mut locked = state.lock().unwrap();
            locked.running = false;
            locked.cancel = None;
        });
        true
    }
    pub fn cancel(&self) {
        if let Some(cancel) = &self.state.lock().unwrap().cancel {
            cancel.store(true, Ordering::SeqCst);
        }
    }
    pub fn running(&self) -> bool {
        self.state.lock().unwrap().running
    }
    pub fn summary(&self, policy: &Policy) -> Value {
        let state = self.state.lock().unwrap();
        let providers: Vec<_> = [Provider::Codex, Provider::Claude].iter().map(|p| {
            let configured = policy.entries.iter().filter(|e| e.provider == *p).count();
            match state.providers.get(p).filter(|s| s.scope == scope(*p, policy)) {
                Some(s) => json!({"provider":p,"status":s.status,"cli_version":s.cli_version,"cached_cli_version":s.cached_cli_version,"refreshed_unix":s.refreshed_unix,
                    "error":s.error,"blocker":s.blocker,"cache_error":s.cache_error,"model_count":s.models.len(),"configured_count":configured,
                    "cached_stale":s.status != "discovered" && s.refreshed_unix.is_some(),
                    "changes":{"added":s.diff.added.len(),"removed":s.diff.removed.len(),"alias_retargeted":s.diff.alias_retargeted.len(),
                        "capabilities_changed":s.diff.capabilities_changed.len(),"cli_version_changed":s.diff.cli_version_changed}}),
                None => json!({"provider":p,"status":"pending","configured_count":configured,"model_count":0}),
            }
        }).collect();
        json!({"version":VERSION,"refreshing":state.running,"providers":providers,"provenance":"configured",
            "policy_revision":policy.policy_revision,"configured_count":policy.entries.len(),"details_url":"/api/models"})
    }
    pub fn details(&self, policy: &Policy) -> Value {
        let state = self.state.lock().unwrap();
        let providers: Vec<_> = state
            .providers
            .values()
            .filter(|s| s.scope == scope(s.provider, policy))
            .cloned()
            .collect();
        let options: Vec<_> = policy
            .entries
            .iter()
            .map(|e| {
                selection(
                    state
                        .providers
                        .get(&e.provider)
                        .filter(|s| s.scope == scope(e.provider, policy)),
                    e.provider,
                    &e.model,
                    &e.effort,
                    Some(e),
                    &policy.policy_revision,
                    false,
                )
            })
            .collect();
        json!({"version":VERSION,"refreshing":state.running,"providers":providers,"options":options,"policy_revision":policy.policy_revision})
    }
    /// Existing explicit role settings remain explicit configuration; no model routing occurs here.
    pub fn execution_input(&self, policy: &Policy, provider: Provider, model: &str) -> Value {
        let state = self.state.lock().unwrap();
        let entry = policy
            .entries
            .iter()
            .find(|e| e.provider == provider && e.model == model);
        let effort = entry
            .map(|e| e.effort.as_str())
            .unwrap_or("provider_default");
        selection(
            state
                .providers
                .get(&provider)
                .filter(|s| s.scope == scope(provider, policy)),
            provider,
            model,
            effort,
            entry,
            &policy.policy_revision,
            true,
        )
    }
    pub fn execution_with_effort(&self, policy: &Policy, provider: Provider, model: &str, effort: &str) -> Value {
        let state = self.state.lock().unwrap();
        let entry = policy.entries.iter().find(|e| e.provider == provider && e.model == model);
        selection(state.providers.get(&provider).filter(|s| s.scope == scope(provider, policy)),
            provider, model, effort, entry, &policy.policy_revision, true)
    }
    /// Execution supplies stronger evidence than a discovery failure. Never infer an auth or
    /// model rejection merely from an arbitrary non-zero process exit.
    pub fn reject_effort(&self, policy: &Policy, provider: Provider, model: &str, effort: &str) {
        let mut state = self.state.lock().unwrap();
        let key = scope(provider, policy);
        let s = state
            .providers
            .entry(provider)
            .or_insert_with(|| ProviderState::new(provider, key.clone()));
        if s.scope == key {
            s.invalid_efforts
                .entry(model.into())
                .or_default()
                .insert(effort.into());
        }
    }
    pub fn observe(
        &self,
        policy: &Policy,
        provider: Provider,
        model: &str,
        result: Result<(), Failure>,
    ) {
        let mut state = self.state.lock().unwrap();
        let key = scope(provider, policy);
        let s = state
            .providers
            .entry(provider)
            .or_insert_with(|| ProviderState::new(provider, key.clone()));
        if s.scope != key {
            return;
        } // A run from a previous configuration cannot change the new scope.
        match result {
            Ok(()) => {
                s.execution_verified.insert(model.into());
                s.removed.remove(model);
                s.execution_rejections.remove(model);
            }
            Err(e) if e.kind == FailureKind::Rejected => {
                s.execution_rejections.insert(model.into());
                s.execution_verified.remove(model);
            }
            Err(e) if e.blocks() => {
                s.execution_blocked = true;
                s.blocker = Some(e.clone());
                s.error = Some(e);
                s.status = "unavailable".into();
                s.execution_verified.clear();
            }
            _ => {}
        }
    }
    /// Discovery evidence for metadata research: canonical model IDs with a
    /// fingerprint that changes on retargeting or material capability changes,
    /// plus native reasoning support so discovered facts can win conflicts.
    /// Configured-only entries are included so cache-only research still
    /// covers explicitly configured tiers.
    pub fn metadata_snapshot(&self, policy: &Policy) -> Vec<crate::metadata::Snapshot> {
        let state = self.state.lock().unwrap();
        let mut out = Vec::new();
        let mut known: BTreeSet<(Provider, String)> = BTreeSet::new();
        for (provider, s) in &state.providers {
            if s.scope != scope(*provider, policy) {
                continue;
            }
            for m in &s.models {
                let canonical = m.resolved_id.clone().unwrap_or_else(|| m.id.clone());
                for id in std::iter::once(&m.id)
                    .chain(m.aliases.iter())
                    .chain(m.resolved_id.iter())
                {
                    known.insert((*provider, id.clone()));
                }
                if out.iter().any(|s: &crate::metadata::Snapshot| {
                    s.provider == *provider && s.model == canonical
                }) {
                    continue;
                }
                let mut material = format!(
                    "{}|{:?}|{:?}|{:?}|{:?}",
                    m.id, m.resolved_id, m.aliases, m.supported_efforts, m.default_effort
                );
                for (k, v) in &m.capabilities {
                    material.push_str(&format!("|{k}={v}"));
                }
                out.push(crate::metadata::Snapshot {
                    provider: *provider,
                    model: canonical,
                    discovery_fingerprint: crate::metadata::fingerprint(material.as_bytes()),
                    discovered_reasoning: m.supported_efforts.as_ref().map(|e| !e.is_empty()),
                });
            }
        }
        for e in &policy.entries {
            if !known.contains(&(e.provider, e.model.clone())) {
                out.push(crate::metadata::Snapshot {
                    provider: e.provider,
                    model: e.model.clone(),
                    discovery_fingerprint: "configured-only".into(),
                    discovered_reasoning: None,
                });
                known.insert((e.provider, e.model.clone()));
            }
        }
        out
    }
    pub fn select(&self, policy: &Policy, provider: Provider, model: &str, effort: &str) -> Value {
        let state = self.state.lock().unwrap();
        let entry = policy
            .entries
            .iter()
            .find(|e| e.provider == provider && e.model == model);
        selection(
            state
                .providers
                .get(&provider)
                .filter(|s| s.scope == scope(provider, policy)),
            provider,
            model,
            effort,
            entry,
            &policy.policy_revision,
            false,
        )
    }
}
impl Drop for Catalogue {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn selection(
    s: Option<&ProviderState>,
    provider: Provider,
    id: &str,
    effort: &str,
    entry: Option<&Entry>,
    revision: &str,
    explicit: bool,
) -> Value {
    let m = s.and_then(|s| {
        s.models.iter().find(|m| {
            (id.is_empty() && m.is_default == Some(true))
                || m.id == id
                || m.resolved_id.as_deref() == Some(id)
                || m.aliases.iter().any(|a| a == id)
        })
    });
    let mut error = None;
    let availability = if s.is_some_and(|s| {
        s.blocker.is_some()
            || s.removed.contains(id)
            || s.execution_rejections.contains(id)
            || m.is_some_and(|m| {
                s.execution_rejections.contains(&m.id)
                    || m.aliases
                        .iter()
                        .any(|id| s.execution_rejections.contains(id))
                    || m.resolved_id
                        .as_ref()
                        .is_some_and(|id| s.execution_rejections.contains(id))
            })
    }) {
        error = Some("provider/model is known unavailable");
        "unavailable"
    } else if s.is_some_and(|s| s.execution_verified.contains(id)) {
        "execution_verified"
    } else if m.is_some() && s.is_some_and(|s| s.status == "discovered") {
        "discovered"
    } else if entry.is_some() || explicit {
        "configured_unverified"
    } else {
        error =
            Some("model requires an exact explicit registry entry when discovery is unverified");
        "unverified"
    };
    let mut args: Vec<String> = Vec::new();
    if effort != "provider_default" {
        if s.is_some_and(|s| {
            let rejected = |id: &str| {
                s.invalid_efforts
                    .get(id)
                    .is_some_and(|set| set.contains(effort))
            };
            rejected(id)
                || m.is_some_and(|m| {
                    rejected(&m.id)
                        || m.aliases.iter().any(|id| rejected(id))
                        || m.resolved_id.as_ref().is_some_and(|id| rejected(id))
                })
        }) {
            error = Some("native effort was rejected during execution; use provider_default");
        } else if !m.and_then(|m| m.supported_efforts.as_ref()).is_some_and(|list| list.iter().any(|e| e == effort))
            && !(m.and_then(|m| m.supported_efforts.as_ref()).is_none()
                && entry.is_some_and(|e| e.effort == effort)
                && match provider {
                    Provider::Codex => ["none", "minimal", "low", "medium", "high", "xhigh"].contains(&effort),
                    Provider::Claude => ["low", "medium", "high", "xhigh", "max"].contains(&effort),
                })
        {
            error = Some("unsupported or unknown native effort; use provider_default");
        } else if identifier(effort)
            && (provider != Provider::Claude
                || ["low", "medium", "high", "xhigh", "max"].contains(&effort))
        {
            args = match provider {
                Provider::Codex => vec![
                    "-c".into(),
                    format!("model_reasoning_effort={}", json!(effort)),
                ],
                Provider::Claude => vec!["--effort".into(), effort.into()],
            };
        } else {
            error = Some("invalid native effort");
        }
    }
    if !(explicit && id.is_empty()) && !identifier(id) {
        error = Some("invalid model identifier");
    }
    if error.is_some() {
        args.clear();
    }
    json!({"provider":provider,"model":id,"resolved_id":m.and_then(|m| m.resolved_id.as_ref()),"availability":availability,
        "availability_unverified":matches!(availability,"configured_unverified"|"unverified"),"eligible":error.is_none(),"error":error,
        "effort":effort,"native_effort_args":args,"tier":entry.map(|e| &e.tier),"suitability":entry.map(|e| &e.suitability),
        "limits":entry.map(|e| &e.limits),"relative_cost_preference":entry.and_then(|e| e.relative_cost_preference),"pricing":null,
        "provenance":if entry.is_some() { Some("configured") } else if explicit { Some("configured_role_setting") } else { None },"policy_revision":revision})
}
fn read_cache(path: &Path, provider: Provider, scope: &str) -> Result<Option<Cache>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cache read failed".into()),
    };
    if file.metadata().map_err(|_| "cache metadata failed")?.len() > 4 * 1024 * 1024 {
        return Err("cache exceeds size limit".into());
    }
    let c: Cache = serde_json::from_reader(file).map_err(|_| "cache is corrupt")?;
    if c.version != VERSION
        || c.provider != provider
        || c.scope != scope
        || c.models.len() > MAX_MODELS
        || c.models.iter().any(|m| !identifier(&m.id))
    {
        return Err("cache version/scope/schema mismatch".into());
    }
    Ok(Some(c))
}
pub(crate) fn policy_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".config")
        })
        .join("forge/model-policy.json")
}
pub(crate) fn load_policy(path: &Path) -> Result<Option<Policy>, String> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("could not read model policy".into()),
    };
    if f.metadata()
        .map_err(|_| "could not inspect model policy")?
        .len()
        > 128 * 1024
    {
        return Err("model policy exceeds size limit".into());
    }
    let value: Value = serde_json::from_reader(f).map_err(|_| "model policy is corrupt")?;
    Policy::from_settings(&json!({"model_catalogue":value})).map(Some)
}
pub(crate) fn save_policy(path: &Path, policy: &Policy) -> Result<(), String> {
    write_atomic_json(path, &json!(policy))
}
fn write_cache(path: &Path, cache: &Cache) -> Result<(), String> {
    write_atomic_json(path, &json!(cache))
}
pub(crate) fn write_atomic_json(path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|_| "cache directory creation failed")?;
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        CACHE_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|_| "cache temporary file failed")?;
        f.write_all(&serde_json::to_vec(value).map_err(|_| "cache encoding failed")?)
            .map_err(|_| "cache write failed")?;
        f.sync_all().map_err(|_| "cache sync failed")?;
        std::fs::rename(&tmp, path).map_err(|_| "cache publication failed")?;
        std::fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|_| "cache directory sync failed")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(tmp);
    result
}

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;
