//! Claude subscription limits, read through an initialization-only SDK control request.
//! Cached globally across projects; API polling never starts provider processes.
use crate::catalogue_process::{Budget, CommandSpec, Launcher, SystemLauncher};
use crate::util::unix_timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Condvar, Mutex, atomic::AtomicBool};
use std::time::{Duration, Instant};

const FRESH_SECONDS: i64 = 300;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Window {
    name: String,
    model: Option<String>,
    used_percent: Option<f64>,
    resets_at: Option<String>,
    resets_unix: Option<i64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Usage {
    available: bool,
    windows: Vec<Window>,
    extra_usage_enabled: Option<bool>,
}
#[derive(Default)]
struct Cache {
    bridge: String,
    attempted: i64,
    checked: i64,
    refreshing: bool,
    usage: Option<Usage>,
    error: Option<String>,
}
#[derive(Clone)]
pub(crate) struct Service {
    cache: Arc<Mutex<Cache>>,
    fetch_lock: Arc<Mutex<()>>,
    refreshed: Arc<Condvar>,
    launcher: Arc<dyn Launcher>,
}
impl Default for Service {
    fn default() -> Self {
        Self { cache: Default::default(), fetch_lock: Default::default(), refreshed: Default::default(), launcher: Arc::new(SystemLauncher) }
    }
}
impl Service {
    pub fn snapshot(&self) -> Value {
        let c = self.cache.lock().unwrap();
        let stale = c.checked > 0 && unix_timestamp() - c.checked >= FRESH_SECONDS;
        let status = if c.usage.as_ref().is_some_and(|u| u.available) {
            if stale || c.error.is_some() { "stale" } else { "available" }
        } else if c.attempted == 0 { "pending" } else { "unavailable" };
        json!({"status":status,"refreshing":c.refreshing,"checked_unix":c.checked,
            "error":c.error,"windows":c.usage.as_ref().map(|u| &u.windows),
            "extra_usage_enabled":c.usage.as_ref().and_then(|u| u.extra_usage_enabled)})
    }

    /// A short cooldown also coalesces manual refreshes. No account data is persisted.
    pub fn refresh(&self, bridge: String, manual: bool) -> bool {
        if !self.claim(&bridge, if manual { 15 } else { FRESH_SECONDS }) { return false; }
        let service = self.clone();
        std::thread::spawn(move || service.fetch(&bridge));
        true
    }

    fn claim(&self, bridge: &str, max_age: i64) -> bool {
        let mut c = self.cache.lock().unwrap();
        if c.refreshing { return false; }
        if c.bridge == bridge && c.attempted > 0 && unix_timestamp() - c.attempted < max_age { return false; }
        if c.bridge != bridge { *c = Cache { bridge: bridge.into(), ..Cache::default() }; }
        c.refreshing = true;
        true
    }

    fn fetch(&self, bridge: &str) {
        let _fetch = self.fetch_lock.lock().unwrap();
        let result = probe(self.launcher.as_ref(), bridge);
        let mut c = self.cache.lock().unwrap();
        c.attempted = unix_timestamp();
        c.refreshing = false;
        match result {
            Ok(usage) => { c.checked = c.attempted; c.usage = Some(usage); c.error = None; }
            Err(error) => c.error = Some(error),
        }
        self.refreshed.notify_all();
    }

    /// Refresh before a real Claude invocation (at most once a minute).
    /// Missing evidence does not invent a zero balance or permanently lock out a model.
    pub fn check(&self, bridge: &str, model: &str) -> Result<(), String> {
        if self.claim(bridge, 60) { self.fetch(bridge); }
        // Wait for a startup/background check, bounded independently of the transport.
        // API reads only take the cache lock and stay responsive during this wait.
        let (c, _) = self.refreshed.wait_timeout_while(self.cache.lock().unwrap(),
            Duration::from_secs(16), |c| c.refreshing).unwrap();
        if c.bridge != bridge || c.checked == 0 || unix_timestamp() - c.checked >= FRESH_SECONDS { return Ok(()); }
        if let Some(usage) = &c.usage
            && let Some(window) = exhausted(usage, model, unix_timestamp()) {
            return Err(format!("{} quota exhausted (0% remaining); resets {}. Change model or wait for the reset.",
                window.name, window.resets_at.as_deref().unwrap_or("at the provider's reset time")));
        }
        Ok(())
    }
}

fn exhausted<'a>(usage: &'a Usage, model: &str, now: i64) -> Option<&'a Window> {
    // Enabled or unknown overage may still allow this invocation. Never enable it here.
    if !usage.available || usage.extra_usage_enabled != Some(false) { return None; }
    usage.windows.iter().find(|w| {
        w.used_percent.is_some_and(|n| n >= 100.0)
            && w.resets_unix.is_some_and(|reset| reset > now)
            && w.model.as_ref().is_none_or(|name| model.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|part| part.eq_ignore_ascii_case(name)))
    })
}

fn probe(launcher: &dyn Launcher, bridge: &str) -> Result<Usage, String> {
    if bridge.is_empty() { return Err("Claude usage unavailable: configure the Claude bridge".into()); }
    let budget = Budget { deadline: Instant::now() + Duration::from_secs(15), cancel: Arc::new(AtomicBool::new(false)) };
    let command = |executable: &str, args: Vec<String>| CommandSpec {
        executable: executable.into(), args, cwd: std::env::temp_dir(),
    };
    let failure = |_| "Claude usage check failed (CLI, authentication, timeout or unsupported version)".to_string();
    let mut version = launcher.spawn(&command("claude", vec!["--version".into()]), budget.clone()).map_err(failure)?;
    let version_text = version.line().map_err(failure)?.ok_or("Claude version unavailable")?;
    version.finish().map_err(failure)?;
    let version_text = version_text.trim();
    let mut channel = launcher.spawn(&command(bridge, vec!["--forge-usage-v1".into(), "claude".into(), version_text.into()]), budget).map_err(failure)?;
    let line = channel.line().map_err(failure)?.ok_or("Claude usage response missing")?;
    let value: Value = serde_json::from_str(&line).map_err(|_| "Invalid Claude usage response")?;
    channel.finish().map_err(failure)?;
    if value["bridge_version"] != 1 || value["source"] != "claude_code_usage"
        || value["sdk_version"] != "0.3.261" || value["cli_version"] != version_text || value.get("error").is_some() {
        return Err("Claude usage bridge unavailable or unsupported".into());
    }
    let usage: Usage = serde_json::from_value(value["usage"].clone()).map_err(|_| "Invalid Claude usage data")?;
    if usage.windows.len() > 40 || usage.windows.iter().any(|w|
        w.name.len() > 100 || w.model.as_ref().is_some_and(|m| m.len() > 64)
            || w.resets_at.as_ref().is_some_and(|s| s.len() > 64)
            || w.used_percent.is_some_and(|p| !p.is_finite() || !(0.0..=100.0).contains(&p))) {
        return Err("Invalid Claude usage windows".into());
    }
    Ok(usage)
}

#[cfg(test)]
#[path = "quota_tests.rs"]
mod tests;
