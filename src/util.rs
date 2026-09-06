use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Resolve existing ancestors too, so a clone destination has a stable key
/// before its directory exists (including under a symlinked projects root).
pub(crate) fn canonical_project(path: &str) -> String {
    fn resolve(path: &std::path::Path) -> PathBuf {
        if let Ok(path) = fs::canonicalize(path) {
            return path;
        }
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => resolve(parent).join(name),
            _ => path.to_path_buf(),
        }
    }
    let path = PathBuf::from(path);
    let path = if path.is_absolute() { path } else {
        std::env::current_dir().unwrap().join(path)
    };
    resolve(&path).display().to_string()
}

pub(crate) fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

pub(crate) fn clock_hms() -> String {
    Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub(crate) fn last_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(limit)).collect()
}

/// Substitute template segments once so placeholders in user content stay literal.
pub(crate) fn fill_template(template: &str, pairs: &[(&str, &str)]) -> String {
    template.split_inclusive('}').map(|part| {
        for &(key, value) in pairs {
            if let Some(prefix) = part.strip_suffix(key) {
                return format!("{prefix}{value}");
            }
        }
        part.to_string()
    }).collect()
}
