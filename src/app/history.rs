//! Project event logging and bounded history, chat, and report readers.
use super::{Ctx, FORGE_DIR};
use crate::util::{clock_hms, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// Append a history event for a project without a live session; used for
/// events that must outlive an engine restart, like self-update completion.
pub(super) fn log_project_event(project: &str, kind: &str, text: &str) {
    let dir = PathBuf::from(project).join(FORGE_DIR);
    let _ = fs::create_dir_all(&dir);
    let entry = json!({"t": clock_hms(), "unix": unix_timestamp(), "kind": kind, "text": text});
    append_history_event(project, &entry, kind, text);
}

fn append_history_event(project: &str, entry: &Value, kind: &str, text: &str) {
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PathBuf::from(project).join(FORGE_DIR).join("history.jsonl"))
    {
        let _ = writeln!(f, "{entry}");
    }
    println!("[{kind}] {text}");
}

impl Ctx {
    pub(crate) fn log_event(&self, kind: &str, text: &str) {
        self.ensure_forge_dir();
        let t = clock_hms();
        let mut entry = json!({"t": t, "unix": unix_timestamp(), "kind": kind, "text": text});
        {
            // Callers release the session state lock before logging.
            let s = self.session.state.lock().unwrap();
            if !s.goal.is_empty() {
                entry["goal"] = json!(s.goal.chars().take(120).collect::<String>());
            }
            if let Some(stage) = s.current_stage {
                entry["stage"] = json!(stage);
            }
        }
        append_history_event(self.project(), &entry, kind, text);
    }

    fn read_jsonl_tail(&self, name: &str, keep: usize) -> Value {
        let items = (|| -> std::io::Result<Vec<Value>> {
            let mut file = fs::File::open(self.forge_path(name))?;
            let metadata = file.metadata()?;
            let size = metadata.len();
            // history.jsonl is append-only. Source identity survives append and
            // restart; replacement/rotation gets a new inode/birth generation.
            // Byte offsets distinguish even byte-identical adjacent records.
            let birth = metadata.created().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |t| t.as_nanos());
            let source = format!("{:x}-{:x}-{birth:x}", metadata.dev(), metadata.ino());
            let start = size.saturating_sub(2 * 1024 * 1024);
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::new();
            file.take(2 * 1024 * 1024).read_to_end(&mut bytes)?;
            let mut offset = start;
            if start > 0 {
                let boundary = bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |p| p + 1);
                bytes.drain(..boundary);
                offset += boundary as u64;
            }
            let mut items = Vec::new();
            for line in bytes.split_inclusive(|b| *b == b'\n') {
                // Preserve legacy readers' acceptance of a complete final JSON
                // object without a newline; incomplete JSON is never published.
                if let Ok(mut entry) = serde_json::from_str::<Value>(&String::from_utf8_lossy(line)) {
                    if name == "history.jsonl" && entry.is_object() {
                        entry["id"] = json!(format!("history:{source}:{offset:x}"));
                    }
                    items.push(entry);
                }
                offset += line.len() as u64;
            }
            Ok(items)
        })().unwrap_or_default();
        let skip = items.len().saturating_sub(keep);
        Value::Array(items.into_iter().skip(skip).collect())
    }

    pub(crate) fn read_history(&self) -> Value {
        self.read_jsonl_tail("history.jsonl", 400)
    }

    pub(crate) fn read_chat(&self) -> Value {
        self.read_jsonl_tail("chat.jsonl", 100)
    }

    pub(crate) fn read_reports(&self) -> Value {
        self.read_jsonl_tail("reports.jsonl", 100)
    }
}
