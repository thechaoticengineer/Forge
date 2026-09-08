//! Codex exec JSONL can omit the model. Read provider-written turn metadata,
//! restricted to the exact thread and bytes appended during this invocation.
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MAX_LINE: u64 = 1024 * 1024;
const MAX_APPEND: u64 = 64 * 1024 * 1024;

pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))
}

pub(crate) struct Snapshot {
    root: PathBuf,
    files: HashMap<PathBuf, Metadata>,
}

fn files(root: &Path) -> Result<HashMap<PathBuf, Metadata>, String> {
    let mut found = HashMap::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut entries = 0;
    while let Some((dir, depth)) = pending.pop() {
        let listing = match fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir == root => continue,
            Err(e) => return Err(format!("cannot inspect Codex sessions: {e}")),
        };
        for entry in listing {
            entries += 1;
            if entries > 100_000 {
                return Err("Codex session lookup exceeds 100000 entries".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            // Native layout: sessions/YYYY/MM/DD/rollout-...-UUID.jsonl.
            // Never follow nested symlinks or traverse arbitrary deeper trees.
            if kind.is_dir() && depth < 3 {
                pending.push((path, depth + 1));
            } else if kind.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with("rollout-") && s.ends_with(".jsonl"))
            {
                found.insert(path, entry.metadata().map_err(|e| e.to_string())?);
            }
        }
    }
    Ok(found)
}

fn line(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    let size = reader
        .take(MAX_LINE + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if size == 0 {
        return Ok(None);
    }
    if size as u64 > MAX_LINE {
        return Err("Codex session event exceeds 1 MiB".into());
    }
    if bytes.last() != Some(&b'\n') {
        return Err("incomplete Codex session event".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("invalid Codex session event: {e}"))
}

impl Snapshot {
    pub(crate) fn capture(home: &Path) -> Result<Self, String> {
        let root = home.join("sessions");
        Ok(Self {
            files: files(&root)?,
            root,
        })
    }

    pub(crate) fn model(&self, session: &str, project: &str) -> Result<String, String> {
        if !super::session_id(session) {
            return Err("invalid Codex session identity".into());
        }
        let suffix = format!("-{session}.jsonl");
        let matching: Vec<_> = files(&self.root)?
            .into_iter()
            .filter(|(path, _)| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with(&suffix))
            })
            .collect();
        if matching.len() != 1 {
            return Err("exact Codex session file missing or ambiguous".into());
        }
        let (path, _) = &matching[0];
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| e.to_string())?;
        let current = file.metadata().map_err(|e| e.to_string())?;
        if !current.is_file() {
            return Err("Codex session is not a regular file".into());
        }
        let offset = if let Some(old) = self.files.get(path) {
            if old.ino() != current.ino() || old.dev() != current.dev() || current.len() < old.len()
            {
                return Err("Codex session replaced or truncated during invocation".into());
            }
            old.len()
        } else {
            0
        };
        if current.len() - offset > MAX_APPEND {
            return Err("Codex session append exceeds 64 MiB".into());
        }
        let mut reader = BufReader::new(file);
        let meta = line(&mut reader)?.ok_or("empty Codex session")?;
        if meta["type"] != "session_meta" || meta["payload"]["id"] != session {
            return Err("Codex session metadata identity mismatch".into());
        }
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(reader.take(current.len() - offset));
        let mut turn = None;
        let mut model = None;
        let mut completed = false;
        while let Some(event) = line(&mut reader)? {
            let payload = &event["payload"];
            if event["type"] == "event_msg" && payload["type"] == "task_started" {
                let id = payload["turn_id"]
                    .as_str()
                    .filter(|id| super::session_id(id))
                    .ok_or("invalid Codex turn identity")?;
                if turn.is_some() {
                    return Err("multiple Codex turns during invocation".into());
                }
                turn = Some(id.to_string());
            } else if event["type"] == "turn_context" {
                if turn.as_deref() != payload["turn_id"].as_str() || turn.is_none() || completed {
                    return Err("Codex context does not belong to the current turn".into());
                }
                if payload["cwd"].as_str() != Some(project) {
                    return Err("Codex turn project mismatch".into());
                }
                let reported = payload["model"]
                    .as_str()
                    .filter(|s| crate::catalogue::identifier(s))
                    .ok_or("Codex turn model missing or invalid")?;
                if model.as_deref().is_some_and(|old| old != reported) {
                    return Err("Codex model changed during turn".into());
                }
                model = Some(reported.to_string());
            } else if event["type"] == "event_msg" && payload["type"] == "task_complete" {
                if turn.is_none() || turn.as_deref() != payload["turn_id"].as_str() {
                    return Err("Codex completion turn mismatch".into());
                }
                completed = true;
            } else if event["type"] == "event_msg" && payload["type"] == "turn_aborted" {
                return Err("Codex session turn aborted".into());
            }
        }
        if !completed {
            return Err("no completed Codex turn appended during invocation".into());
        }
        model.ok_or_else(|| "current Codex turn omitted model metadata".into())
    }
}

#[cfg(test)]
#[path = "codex_session_tests.rs"]
mod tests;
