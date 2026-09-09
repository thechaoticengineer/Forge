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

pub(crate) fn digest(bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let mut child = std::process::Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bytes)
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("snapshot hashing failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .ok_or("missing digest")?
        .into())
}

/// Agents sometimes surround their final JSON with prose or a Markdown fence.
/// Return the first balanced value that actually parses, so a faithful answer
/// is not discarded over its wrapper. Falls back to the trimmed output so a
/// genuinely malformed response still produces a diagnosable parse error.
pub(crate) fn json_payload(output: &str) -> &str {
    let output = output.trim();
    if parses(output) {
        return output;
    }
    let bytes = output.as_bytes();
    let mut attempts = 0;
    for start in 0..bytes.len() {
        if bytes[start] != b'{' && bytes[start] != b'[' {
            continue;
        }
        attempts += 1;
        if attempts > 32 {
            break;
        }
        if let Some(end) = balanced_end(bytes, start)
            && parses(&output[start..end])
        {
            return &output[start..end];
        }
    }
    output
}

fn parses(text: &str) -> bool {
    serde_json::from_str::<serde::de::IgnoredAny>(text).is_ok()
}

/// Index just past the bracket closing the value that opens at `start`,
/// ignoring brackets inside strings.
fn balanced_end(bytes: &[u8], start: usize) -> Option<usize> {
    let (mut depth, mut string, mut escaped) = (0usize, false, false);
    for (idx, &byte) in bytes.iter().enumerate().skip(start) {
        if string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => string = false,
                _ => {},
            }
            continue;
        }
        match byte {
            b'"' => string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx + 1);
                }
            },
            _ => {},
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::json_payload;

    #[test]
    fn plain_json_and_whitespace_are_returned_unchanged() {
        assert_eq!(json_payload("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(json_payload("\n  [1,2]\t "), "[1,2]");
    }

    #[test]
    fn prose_preamble_and_fences_are_stripped_around_the_payload() {
        assert_eq!(json_payload("I explored the card.\n\n{\"stages\":[]}"), "{\"stages\":[]}");
        assert_eq!(json_payload("Done:\n```json\n{\"stages\":[]}\n```\n"), "{\"stages\":[]}");
        assert_eq!(json_payload("{\"stages\":[]}\n\nThat is the plan."), "{\"stages\":[]}");
    }

    #[test]
    fn braces_in_prose_and_strings_do_not_truncate_the_payload() {
        assert_eq!(
            json_payload("Replaced `{key}` with a value.\n{\"text\":\"a } inside\",\"n\":[1]}"),
            "{\"text\":\"a } inside\",\"n\":[1]}"
        );
        assert_eq!(json_payload("{\"escaped\":\"\\\"}\"}"), "{\"escaped\":\"\\\"}\"}");
    }

    #[test]
    fn malformed_output_falls_back_to_the_trimmed_text() {
        assert_eq!(json_payload("  no json here  "), "no json here");
        assert_eq!(json_payload("{\"unclosed\": 1"), "{\"unclosed\": 1");
    }
}

