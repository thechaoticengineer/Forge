//! Complete presentation records, independent of provider outcome decoding.
//!
//! Each invocation has immutable numbered JSON files. `commit.json` publishes
//! only durable whole records; readers address files directly, without rescanning
//! the log. A shared filesystem lock orders startup, both streams, and readers.
//! A durable pending marker makes interrupted dual-file publication explicit.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(crate) const PAGE_BYTES: usize = 256 * 1024;
pub(crate) const PAGE_LIMIT: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Message {
    pub kind: String,
    pub stream: String,
    pub label: String,
    pub text: String,
    /// Plain streams already contain their actual transport terminators.
    pub verbatim: bool,
}
impl Message {
    pub(crate) fn new(kind: &str, stream: &str, label: &str, text: &str) -> Self {
        Self {
            kind: kind.into(),
            stream: stream.into(),
            label: label.into(),
            text: text.into(),
            verbatim: false,
        }
    }
    pub(crate) fn readable(&self) -> String {
        if self.verbatim {
            self.text.clone()
        } else if self.label.is_empty() {
            format!("{}\n", self.text)
        } else {
            format!("{}{}\n", self.label, self.text)
        }
    }
}

pub(crate) trait Sink {
    fn append(&mut self, message: &Message) -> Result<(), String>;
}
#[cfg(test)]
impl Sink for Vec<u8> {
    fn append(&mut self, message: &Message) -> Result<(), String> {
        self.extend_from_slice(message.readable().as_bytes());
        Ok(())
    }
}

struct Lock(File);
impl Lock {
    fn acquire(root: &Path, exclusive: bool) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("lock"))?;
        if unsafe {
            libc::flock(
                file.as_raw_fd(),
                if exclusive {
                    libc::LOCK_EX
                } else {
                    libc::LOCK_SH
                },
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn read(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("invalid agent log record {}: {e}", path.display()))
}
fn atomic(path: &Path, value: &Value) -> Result<(), String> {
    crate::architecture::atomic_json(path, value)
}
fn pending(dir: &Path) -> Result<(), String> {
    atomic(
        &dir.join("pending.json"),
        &json!({"publication":"incomplete"}),
    )
}
fn finish(dir: &Path) -> Result<(), String> {
    fs::remove_file(dir.join("pending.json")).map_err(|e| e.to_string())?;
    sync_dir(dir)
}
fn check(dir: &Path) -> Result<(), String> {
    if dir.join("pending.json").exists() {
        Err("agent log publication incomplete; preserved files require recovery".into())
    } else {
        Ok(())
    }
}
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn record(session: &str, sequence: u64, message: &Message) -> Value {
    json!({"version":1,"session":session,"id":format!("{session}:{sequence}"),"sequence":sequence,
        "kind":message.kind,"stream":message.stream,"label":message.label,"text":message.text})
}
fn record_path(dir: &Path, sequence: u64) -> PathBuf {
    dir.join(format!("{sequence:020}.json"))
}
fn new_session(root: &Path, legacy: bool) -> Result<(String, PathBuf), String> {
    let session = crate::architecture::identity();
    let dir = root.join(&session);
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    atomic(
        &dir.join("commit.json"),
        &json!({"version":1,"session":session,"count":0,"legacy":legacy}),
    )?;
    sync_dir(root)?;
    Ok((session, dir))
}

/// Root publication is a resumable transaction, distinct from message pending
/// markers. Its immutable journal also retains the pre-publication evidence.
#[derive(Serialize, Deserialize)]
struct Startup {
    version: u64,
    startup: String,
    legacy: Option<String>,
}
impl Startup {
    fn journal(&self, root: &Path) -> PathBuf {
        root.join(format!(".startup-{}", self.startup))
    }
}

fn same_inode(a: &Path, b: &Path) -> Result<bool, String> {
    let a = fs::metadata(a).map_err(|e| e.to_string())?;
    let b = fs::metadata(b).map_err(|e| e.to_string())?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

// Older startup markers had no provenance. Recognize their header replacement
// by inode, and an already completed legacy copy by its exact opaque record.
// Never infer message boundaries or rewrite an existing archive/commit.
fn needs_legacy_archive(forge: &Path, root: &Path) -> Result<bool, String> {
    let log = forge.join("agent.log");
    if root.join("current.json").exists() || !log.exists() {
        return Ok(false);
    }
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !safe_id(&entry.file_name().to_string_lossy())
            || !entry.path().join("readable.log").exists()
        {
            continue;
        }
        if same_inode(&log, &entry.path().join("readable.log"))? {
            return Ok(false);
        }
        let commit = read(&entry.path().join("commit.json"))?;
        if commit["legacy"] == true && commit["count"] == 1 && check(&entry.path()).is_ok() {
            let text = fs::read_to_string(&log).map_err(|e| e.to_string())?;
            if read(&record_path(&entry.path(), 0))?["text"] == text {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn preserve_link(source: &Path, destination: &Path) -> Result<(), String> {
    if source.exists() {
        fs::hard_link(source, destination).map_err(|e| e.to_string())?;
        File::open(destination)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// All callers hold the exclusive root lock, including recovery. Hooks allow
// tests to interrupt the actual publication path at durable boundaries.
fn publish_startup(
    forge: &Path,
    root: &Path,
    startup: &Startup,
    hook: &mut impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let journal = startup.journal(root);
    let dir = root.join(&startup.startup);
    let commit = read(&dir.join("commit.json"))?;
    if commit != json!({"version":1,"session":startup.startup,"count":0,"legacy":false}) {
        return Err("invalid agent log startup target".into());
    }
    check(&dir)?; // Startup recovery must never repair a message publication.
    if let Some(id) = &startup.legacy {
        let destination = root.join(id);
        if !destination.exists() {
            // Stage outside the source namespace. A retry reuses the reserved
            // ID and snapshot even if agent.log already contains the new header.
            let staging = journal.join("legacy");
            fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
            if !staging.join("readable.log").exists() {
                preserve_link(&journal.join("previous.log"), &staging.join("readable.log"))?;
            }
            let text =
                fs::read_to_string(staging.join("readable.log")).map_err(|e| e.to_string())?;
            atomic(
                &record_path(&staging, 0),
                &record(id, 0, &Message::new("legacy", "legacy", "", &text)),
            )?;
            atomic(
                &staging.join("commit.json"),
                &json!({"version":1,"session":id,"count":1,"legacy":true}),
            )?;
            sync_dir(&staging)?;
            sync_dir(&journal)?;
            hook("legacy_staged")?;
            fs::rename(&staging, &destination).map_err(|e| e.to_string())?;
        }
        // Sync both sides even when a previous rename survived an interruption.
        sync_dir(&journal)?;
        sync_dir(root)?;
    }
    hook("legacy_ready")?;
    let replacement = root.join("agent.log.next");
    match fs::symlink_metadata(&replacement) {
        Ok(_) => {
            // Inspect the directory entry itself, including dangling symlinks.
            // Preserve every collision without overwriting recovery evidence.
            fs::rename(
                &replacement,
                journal.join(format!("stale-{}", crate::architecture::identity())),
            )
            .map_err(|e| e.to_string())?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    sync_dir(&journal)?;
    sync_dir(root)?;
    hook("stale_preserved")?;
    fs::hard_link(dir.join("readable.log"), &replacement).map_err(|e| e.to_string())?;
    sync_dir(root)?;
    hook("replacement_linked")?;
    fs::rename(&replacement, forge.join("agent.log")).map_err(|e| e.to_string())?;
    sync_dir(forge)?;
    sync_dir(root)?;
    hook("compatibility_renamed")?;
    atomic(
        &root.join("current.json"),
        &json!({"version":1,"session":startup.startup}),
    )?;
    hook("current_published")?;
    finish(root)
}

pub(crate) struct InvocationLog {
    root: PathBuf,
    dir: PathBuf,
    session: String,
    count: u64,
    readable: File,
    failed: bool,
}
impl InvocationLog {
    pub(crate) fn start(forge: &Path, header: &str) -> Result<Self, String> {
        Self::start_with_hook(forge, header, &mut |_| Ok(()))
    }

    fn start_with_hook(
        forge: &Path,
        header: &str,
        hook: &mut impl FnMut(&str) -> Result<(), String>,
    ) -> Result<Self, String> {
        let root = forge.join("agent-records");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let _lock = Lock::acquire(&root, true).map_err(|e| e.to_string())?;
        // Current-format interruptions complete the recorded transaction first.
        // An old marker without provenance is safely superseded below, retaining
        // its raw bytes and both old pointers in the new immutable journal.
        if root.join("pending.json").exists() {
            let value = read(&root.join("pending.json"))?;
            if value.get("startup").is_some() {
                let startup: Startup =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                if startup.version != 1
                    || !safe_id(&startup.startup)
                    || startup.legacy.as_deref().is_some_and(|id| !safe_id(id))
                    || read(&startup.journal(&root).join("startup.json"))? != value
                {
                    return Err("invalid agent log startup journal".into());
                }
                atomic(
                    &root.join(&startup.startup).join("startup-interrupted.json"),
                    &value,
                )?;
                hook("recovery_started")?;
                publish_startup(forge, &root, &startup, hook)?;
                hook("recovery_completed")?;
            }
        }
        let legacy = if needs_legacy_archive(forge, &root)? {
            Some(crate::architecture::identity())
        } else {
            None
        };
        let (session, dir) = new_session(&root, false)?;
        let mut readable = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.join("readable.log"))
            .map_err(|e| e.to_string())?;
        readable
            .write_all(header.as_bytes())
            .and_then(|_| readable.sync_all())
            .map_err(|e| e.to_string())?;
        sync_dir(&dir)?;
        let startup = Startup {
            version: 1,
            startup: session.clone(),
            legacy,
        };
        let journal = startup.journal(&root);
        fs::create_dir(&journal).map_err(|e| e.to_string())?;
        preserve_link(&forge.join("agent.log"), &journal.join("previous.log"))?;
        preserve_link(
            &root.join("current.json"),
            &journal.join("previous-current.json"),
        )?;
        preserve_link(
            &root.join("pending.json"),
            &journal.join("previous-pending.json"),
        )?;
        let value = serde_json::to_value(&startup).map_err(|e| e.to_string())?;
        atomic(&journal.join("startup.json"), &value)?;
        sync_dir(&journal)?;
        sync_dir(&root)?;
        hook("prepared")?;
        atomic(&root.join("pending.json"), &value)?;
        hook("marked")?;
        publish_startup(forge, &root, &startup, hook)?;
        Ok(Self {
            root,
            dir,
            session,
            count: 0,
            readable,
            failed: false,
        })
    }
}
impl Sink for InvocationLog {
    fn append(&mut self, message: &Message) -> Result<(), String> {
        if self.failed {
            return Err("agent log writer previously failed".into());
        }
        let result = (|| {
            let _lock = Lock::acquire(&self.root, true).map_err(|e| e.to_string())?;
            check(&self.root)?;
            check(&self.dir)?;
            if read(&self.root.join("current.json"))?["session"] != self.session {
                return Err("agent log invocation replaced".into());
            }
            pending(&self.dir)?;
            // Readable bytes reach disk before the immutable record and cursor.
            self.readable
                .write_all(message.readable().as_bytes())
                .and_then(|_| self.readable.sync_all())
                .map_err(|e| e.to_string())?;
            atomic(
                &record_path(&self.dir, self.count),
                &record(&self.session, self.count, message),
            )?;
            atomic(
                &self.dir.join("commit.json"),
                &json!({"version":1,"session":self.session,"count":self.count+1,"legacy":false}),
            )?;
            finish(&self.dir)?;
            self.count += 1;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

/// `session` is the caller's live generation; `archive` pins historical reads.
/// Cursor = next immutable sequence (zero based), always scoped to session.
/// `history` lists source identities; use archive=<id> to read any listed source.
pub(crate) fn page(
    forge: &Path,
    project: &str,
    session: Option<&str>,
    archive: Option<&str>,
    cursor: u64,
    limit: usize,
    history: bool,
) -> Result<Value, String> {
    let root = forge.join("agent-records");
    let limit = limit.clamp(1, PAGE_LIMIT);
    if let Some(id) = archive
        && !safe_id(id)
    {
        return Err("invalid agent log archive".into());
    }
    let _lock = if root.exists() {
        Some(Lock::acquire(&root, false).map_err(|e| e.to_string())?)
    } else {
        None
    };
    if history {
        let mut ids = Vec::new();
        if root.exists() {
            for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let id = entry.file_name().to_string_lossy().into_owned();
                if safe_id(&id) && entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        let sources: Vec<_> = ids
            .iter()
            .skip(usize::try_from(cursor).unwrap_or(usize::MAX))
            .take(limit)
            .map(|id| {
                let dir = root.join(id);
                let mut source = read(&dir.join("commit.json"))?;
                source["incomplete"] = json!(dir.join("pending.json").exists());
                source["startup_interrupted"] =
                    json!(dir.join("startup-interrupted.json").exists());
                Ok::<_, String>(source)
            })
            .collect::<Result<_, _>>()?;
        let next = cursor.saturating_add(sources.len() as u64);
        return Ok(
            json!({"version":1,"project":project,"sources":sources,"next_cursor":next,"more":next < ids.len() as u64}),
        );
    }
    if archive.is_none() && root.exists() {
        check(&root)?;
    }
    let current = if root.join("current.json").exists() {
        Some(
            read(&root.join("current.json"))?["session"]
                .as_str()
                .filter(|s| safe_id(s))
                .ok_or("invalid agent log session")?
                .to_string(),
        )
    } else {
        None
    };
    let selected = archive.map(str::to_string).or(current.clone());
    let identity = selected.as_deref().unwrap_or("legacy");
    let reset = session.is_some_and(|s| s != identity);
    if cursor > 0 && session.is_none() && archive.is_none() {
        return Err("agent log cursor requires session".into());
    }
    let cursor = if reset { 0 } else { cursor };
    let mut entries = Vec::new();
    let count;
    let legacy;
    if let Some(id) = &selected {
        let dir = root.join(id);
        check(&dir)?;
        let commit = read(&dir.join("commit.json"))?;
        if commit["version"] != 1 || commit["session"] != *id {
            return Err("invalid agent log commit".into());
        }
        legacy = commit["legacy"]
            .as_bool()
            .ok_or("invalid agent log legacy flag")?;
        count = commit["count"].as_u64().ok_or("invalid agent log count")?;
        if cursor > count {
            return Err("agent log cursor exceeds committed boundary".into());
        }
        let mut bytes = 0;
        for seq in cursor..count {
            let value = read(&record_path(&dir, seq))?;
            if value["version"] != 1
                || value["session"] != *id
                || value["sequence"] != seq
                || value["id"] != format!("{id}:{seq}")
                || ["text", "kind", "stream", "label"]
                    .iter()
                    .any(|key| !value[*key].is_string())
            {
                return Err(format!("invalid agent log record at sequence {seq}"));
            }
            let size = value.to_string().len();
            if !entries.is_empty() && (entries.len() >= limit || bytes + size > PAGE_BYTES) {
                break;
            }
            entries.push(value);
            bytes += size;
            if entries.len() >= limit || bytes >= PAGE_BYTES {
                break;
            }
        }
    } else {
        // Legacy text is an opaque source, never inferred newline boundaries.
        // Re-fetch from cursor zero while a pre-upgrade engine may still append.
        legacy = true;
        count = u64::from(forge.join("agent.log").exists());
        if cursor > count {
            return Err("agent log cursor exceeds committed boundary".into());
        }
        if cursor == 0 && count == 1 {
            let text = fs::read_to_string(forge.join("agent.log")).map_err(|e| e.to_string())?;
            entries.push(record(
                "legacy",
                0,
                &Message::new("legacy", "legacy", "", &text),
            ));
        }
    }
    let next = cursor + entries.len() as u64;
    Ok(
        json!({"version":1,"project":project,"session":identity,"current_session":current,
        "reset":reset,"entries":entries,"next_cursor":next,"more":next<count,
        "legacy":legacy}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::QueueTest;

    #[test]
    fn page_budgets_keep_indivisible_records_and_stable_cursors() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut writer = InvocationLog::start(&forge, "header\n").unwrap();
        let texts = [
            "α\nβ  ".into(),
            "界".repeat(PAGE_BYTES),
            "next\nlast\t".into(),
        ];
        for text in &texts {
            writer
                .append(&Message::new("message", "stdout", "", text))
                .unwrap();
        }
        let first = page(&forge, "project", None, None, 0, PAGE_LIMIT, false).unwrap();
        assert_eq!(first["entries"].as_array().unwrap().len(), 1);
        assert_eq!(first["next_cursor"], 1);
        assert_eq!(first["more"], true);
        let id = first["session"].as_str().unwrap();
        let second = page(&forge, "project", Some(id), None, 1, PAGE_LIMIT, false).unwrap();
        assert_eq!(second["entries"].as_array().unwrap().len(), 1);
        assert_eq!(second["entries"][0]["text"], texts[1]);
        assert!(second.to_string().len() > PAGE_BYTES);
        let third = page(&forge, "project", Some(id), None, 2, PAGE_LIMIT, false).unwrap();
        assert_eq!(third["entries"][0]["text"], texts[2]);
        assert_eq!(third["next_cursor"], 3);
        assert_eq!(third["more"], false);
        writer
            .append(&Message::new("message", "stdout", "", "appended\nexact "))
            .unwrap();
        let fourth = page(&forge, "project", Some(id), None, 3, 1, false).unwrap();
        assert_eq!(fourth["entries"][0]["sequence"], 3);
        assert_eq!(fourth["entries"][0]["text"], "appended\nexact ");
        assert!(page(&forge, "project", None, None, 1, 10, false).is_err());
        assert!(page(&forge, "project", Some(id), None, 999, 10, false).is_err());
        // Count bound is independent of the soft byte budget.
        for _ in 0..PAGE_LIMIT {
            writer
                .append(&Message::new("message", "stdout", "", "x"))
                .unwrap();
        }
        let bounded = page(&forge, "project", Some(id), None, 3, usize::MAX, false).unwrap();
        assert_eq!(bounded["entries"].as_array().unwrap().len(), PAGE_LIMIT);
        assert_eq!(bounded["more"], true);
    }

    #[test]
    fn legacy_archive_replacement_and_restart_preserve_every_available_byte() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let legacy = format!("\r\n  {}\n\nno known boundaries  \t", "界".repeat(40_000));
        fs::write(forge.join("agent.log"), &legacy).unwrap();
        let initial = page(&forge, "project", None, None, 0, 10, false).unwrap();
        assert_eq!(initial["entries"].as_array().unwrap().len(), 1);
        assert_eq!(initial["entries"][0]["kind"], "legacy");
        assert_eq!(initial["entries"][0]["text"], legacy);
        let mut writer = InvocationLog::start(&forge, "first\n").unwrap();
        writer
            .append(&Message::new("message", "stdout", "", "first\nmessage  "))
            .unwrap();
        let first = page(&forge, "project", Some("legacy"), None, 1, 10, false).unwrap();
        assert_eq!(first["reset"], true);
        let first_id = first["session"].as_str().unwrap().to_string();
        drop(writer);
        // Reopening the reader (as after an engine restart) retains generation and IDs.
        assert_eq!(
            page(&forge, "project", Some(&first_id), None, 0, 10, false).unwrap()["entries"],
            first["entries"]
        );
        let mut replacement = InvocationLog::start(&forge, "second\n").unwrap();
        replacement
            .append(&Message::new(
                "message",
                "stdout",
                "",
                &"larger new invocation".repeat(100),
            ))
            .unwrap();
        let reset = page(&forge, "project", Some(&first_id), None, 1, 10, false).unwrap();
        assert_eq!(reset["reset"], true);
        assert_ne!(reset["session"], first_id);
        assert_eq!(reset["entries"][0]["sequence"], 0);
        let archived = page(&forge, "project", None, Some(&first_id), 0, 10, false).unwrap();
        assert_eq!(archived["entries"], first["entries"]);
        assert_eq!(
            fs::read_to_string(
                forge
                    .join("agent-records")
                    .join(&first_id)
                    .join("readable.log")
            )
            .unwrap(),
            "first\nfirst\nmessage  \n"
        );
        let sources = page(&forge, "project", None, None, 0, 100, true).unwrap();
        assert_eq!(sources["sources"].as_array().unwrap().len(), 3);
        let legacy_id = sources["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["legacy"] == true)
            .unwrap()["session"]
            .as_str()
            .unwrap();
        let archived_legacy = page(&forge, "project", None, Some(legacy_id), 0, 10, false).unwrap();
        assert_eq!(archived_legacy["entries"][0]["text"], legacy);
        assert_eq!(
            fs::read_to_string(
                forge
                    .join("agent-records")
                    .join(legacy_id)
                    .join("readable.log")
            )
            .unwrap(),
            legacy
        );
        let list_page = page(&forge, "project", None, None, 0, 1, true).unwrap();
        assert_eq!(list_page["sources"].as_array().unwrap().len(), 1);
        assert_eq!(list_page["next_cursor"], 1);
        assert_eq!(list_page["more"], true);
    }

    #[test]
    fn stale_writer_cannot_publish_into_replacement_invocation() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut old = InvocationLog::start(&forge, "old\n").unwrap();
        let _new = InvocationLog::start(&forge, "new\n").unwrap();
        assert!(
            old.append(&Message::new("message", "stdout", "", "stale"))
                .unwrap_err()
                .contains("replaced")
        );
        assert_eq!(
            fs::read_to_string(forge.join("agent.log")).unwrap(),
            "new\n"
        );
        assert_eq!(
            page(&forge, "project", None, None, 0, 100, false).unwrap()["next_cursor"],
            0
        );
    }

    #[test]
    fn unfinished_writes_do_not_advance_cursor_and_corruption_is_explicit() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut writer = InvocationLog::start(&forge, "").unwrap();
        writer
            .append(&Message::new("message", "stdout", "", "committed"))
            .unwrap();
        let id = writer.session.clone();
        // A torn/unpublished trailing write is outside commit.json's boundary.
        fs::write(record_path(&writer.dir, 1), b"{\"text\":\"unfinished").unwrap();
        let result = page(&forge, "project", Some(&id), None, 1, 10, false).unwrap();
        assert_eq!(result["next_cursor"], 1);
        assert_eq!(result["entries"], json!([]));
        fs::write(record_path(&writer.dir, 0), b"malformed interior").unwrap();
        assert!(
            page(&forge, "project", Some(&id), None, 0, 10, false)
                .unwrap_err()
                .contains("invalid agent log record")
        );
        // The durable pending marker distinguishes an interrupted dual-file
        // publication from an idle stream, including across process restart.
        pending(&writer.dir).unwrap();
        drop(writer);
        assert!(
            page(&forge, "project", Some(&id), None, 1, 10, false)
                .unwrap_err()
                .contains("publication incomplete")
        );
    }

    #[test]
    fn readers_wait_for_complete_publication_and_then_see_committed_boundary() {
        use std::sync::mpsc;
        use std::time::Duration;
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let writer = InvocationLog::start(&forge, "").unwrap();
        let lock = Lock::acquire(&writer.root, true).unwrap();
        pending(&writer.dir).unwrap();
        fs::write(record_path(&writer.dir, 0), b"unfinished").unwrap();
        let (begun_tx, begun_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                begun_tx.send(()).unwrap();
                done_tx
                    .send(page(&forge, "project", None, None, 0, 10, false))
                    .unwrap();
            });
            begun_rx.recv().unwrap();
            assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
            // Complete the publication while retaining the same exclusive lock.
            atomic(
                &record_path(&writer.dir, 0),
                &record(
                    &writer.session,
                    0,
                    &Message::new("message", "stdout", "", "whole\nrecord"),
                ),
            )
            .unwrap();
            atomic(
                &writer.dir.join("commit.json"),
                &json!({"version":1,"session":writer.session,"count":1,"legacy":false}),
            )
            .unwrap();
            finish(&writer.dir).unwrap();
            drop(lock);
            let page = done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert_eq!(page["next_cursor"], 1);
            assert_eq!(page["entries"][0]["text"], "whole\nrecord");
        });
    }

    #[test]
    fn record_publication_failure_is_reported_and_does_not_publish_cursor() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut writer = InvocationLog::start(&forge, "header\n").unwrap();
        // Inject a real filesystem rename failure after readable persistence.
        fs::create_dir(record_path(&writer.dir, 0)).unwrap();
        assert!(
            writer
                .append(&Message::new(
                    "message",
                    "stdout",
                    "",
                    "retained despite failure"
                ))
                .is_err()
        );
        assert_eq!(read(&writer.dir.join("commit.json")).unwrap()["count"], 0);
        assert!(
            fs::read_to_string(forge.join("agent.log"))
                .unwrap()
                .contains("retained despite failure")
        );
        assert!(
            page(&forge, "project", None, None, 0, 10, false)
                .unwrap_err()
                .contains("publication incomplete")
        );
        assert!(
            writer
                .append(&Message::new("message", "stdout", "", "no retry"))
                .unwrap_err()
                .contains("previously failed")
        );
    }

    fn interrupt_start(forge: &Path, point: &str) {
        let error = InvocationLog::start_with_hook(forge, "interrupted header\n", &mut |step| {
            if step == point {
                Err(format!("interrupted at {point}"))
            } else {
                Ok(())
            }
        })
        .err()
        .expect("injection point reached");
        assert_eq!(error, format!("interrupted at {point}"));
    }

    fn sources(forge: &Path) -> Vec<Value> {
        page(forge, "project", None, None, 0, 100, true).unwrap()["sources"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn assert_legacy(forge: &Path, expected: &str) -> String {
        let legacy: Vec<_> = sources(forge)
            .into_iter()
            .filter(|s| s["legacy"] == true)
            .collect();
        assert_eq!(legacy.len(), 1, "no duplicated legacy sources");
        let id = legacy[0]["session"].as_str().unwrap();
        let entry = page(forge, "project", None, Some(id), 0, 1, false).unwrap();
        assert_eq!(entry["entries"][0]["text"], expected);
        assert_eq!(entry["entries"][0]["id"], format!("{id}:0"));
        assert_eq!(entry["next_cursor"], 1);
        assert_eq!(
            fs::read_to_string(forge.join("agent-records").join(id).join("readable.log")).unwrap(),
            expected
        );
        assert_eq!(
            page(forge, "project", None, Some(id), 1, 1, false).unwrap()["entries"],
            json!([])
        );
        id.to_owned()
    }

    fn assert_fresh_writer(forge: &Path, old_session: &str) {
        let mut writer = InvocationLog::start(forge, "recovered\n").unwrap();
        assert!(!writer.root.join("pending.json").exists());
        assert!(!writer.root.join("agent.log.next").exists());
        assert_eq!(
            read(&writer.root.join("current.json")).unwrap()["session"],
            writer.session
        );
        assert!(same_inode(&forge.join("agent.log"), &writer.dir.join("readable.log")).unwrap());
        for text in ["  recovered 界\r\nfirst\t", "\nsecond message  "] {
            writer
                .append(&Message::new("message", "stdout", "", text))
                .unwrap();
        }
        let reset = page(forge, "project", Some(old_session), None, 999, 1, false).unwrap();
        assert_eq!(reset["reset"], true);
        assert_eq!(reset["entries"][0]["text"], "  recovered 界\r\nfirst\t");
        assert_eq!(reset["next_cursor"], 1);
        let next = page(forge, "project", Some(&writer.session), None, 1, 1, false).unwrap();
        assert_eq!(next["reset"], false);
        assert_eq!(next["entries"][0]["text"], "\nsecond message  ");
        assert_eq!(next["next_cursor"], 2);
        assert_eq!(next["more"], false);
        assert_eq!(
            fs::read_to_string(forge.join("agent.log")).unwrap(),
            "recovered\n  recovered 界\r\nfirst\t\n\nsecond message  \n"
        );
    }

    #[test]
    fn startup_interruptions_recover_and_preserve_sources_and_cursors() {
        for structured in [false, true] {
            for point in [
                "prepared",
                "marked",
                "legacy_staged",
                "legacy_ready",
                "stale_preserved",
                "replacement_linked",
                "compatibility_renamed",
                "current_published",
            ] {
                if structured && point == "legacy_staged" {
                    continue;
                }
                let fixture = QueueTest::new(false);
                let forge = fixture.app.forge_path("");
                let legacy = format!("\r\n  {}\nno boundaries\t  ", "界".repeat(401));
                fs::write(forge.join("agent.log"), &legacy).unwrap();
                let mut old = structured.then(|| {
                    let mut writer = InvocationLog::start(&forge, "previous\n").unwrap();
                    for text in ["full previous\nmessage  ", "second\r\nmessage\t"] {
                        writer
                            .append(&Message::new("message", "stdout", "", text))
                            .unwrap();
                    }
                    writer
                });
                let old_records = old.as_ref().map(|w| page(&forge, "project", None, Some(&w.session), 0, 10, false).unwrap()["entries"].clone());
                let before = fs::read(forge.join("agent.log")).unwrap();
                interrupt_start(&forge, point);
                if point != "prepared" {
                    assert!(
                        page(&forge, "project", None, None, 0, 10, false)
                            .unwrap_err()
                            .contains("publication incomplete")
                    );
                }
                let archives_before = sources(&forge);
                let root = forge.join("agent-records");
                // All already published record files and commits stay byte exact.
                let preserved: Vec<_> = archives_before
                    .iter()
                    .flat_map(|source| {
                        fs::read_dir(root.join(source["session"].as_str().unwrap()))
                            .unwrap()
                            .map(|e| e.unwrap().path())
                            .filter(|p| p.is_file())
                            .map(|p| {
                                let bytes = fs::read(&p).unwrap();
                                (p, bytes)
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                assert_fresh_writer(&forge, old.as_ref().map_or("legacy", |w| &w.session));
                for (path, bytes) in preserved {
                    assert_eq!(fs::read(path).unwrap(), bytes);
                }
                let legacy_id = assert_legacy(&forge, &legacy);
                for source in archives_before.iter().filter(|s| s["legacy"] == true) {
                    assert_eq!(source["session"], legacy_id);
                }
                if let Some(writer) = &mut old {
                    assert_eq!(fs::read(writer.dir.join("readable.log")).unwrap(), before);
                    assert_eq!(
                        page(&forge, "project", None, Some(&writer.session), 0, 10, false).unwrap()
                            ["entries"],
                        old_records.unwrap()
                    );
                    assert!(
                        writer
                            .append(&Message::new("plain", "stdout", "", "stale"))
                            .unwrap_err()
                            .contains("replaced")
                    );
                }
                if point != "prepared" {
                    assert!(
                        sources(&forge)
                            .iter()
                            .any(|s| s["startup_interrupted"] == true && s["count"] == 0)
                    );
                }
            }
        }
    }

    #[test]
    fn interrupted_recovery_is_retryable_even_during_first_legacy_migration() {
        for first_point in [
            "marked",
            "legacy_staged",
            "replacement_linked",
            "compatibility_renamed",
            "current_published",
        ] {
            for recovery_point in [
                "recovery_started",
                "legacy_ready",
                "stale_preserved",
                "replacement_linked",
                "compatibility_renamed",
                "current_published",
                "recovery_completed",
            ] {
                let fixture = QueueTest::new(false);
                let forge = fixture.app.forge_path("");
                let legacy = "\r\n original legacy 界\n\t exact  ";
                fs::write(forge.join("agent.log"), legacy).unwrap();
                interrupt_start(&forge, first_point);
                let root = forge.join("agent-records");
                let pending_before = fs::read(root.join("pending.json")).unwrap();
                let startup: Startup = serde_json::from_slice(&pending_before).unwrap();
                let reserved_id = startup.legacy.as_deref().unwrap();
                interrupt_start(&forge, recovery_point);
                if recovery_point != "recovery_completed" {
                    assert_eq!(fs::read(root.join("pending.json")).unwrap(), pending_before);
                    assert!(page(&forge, "project", None, None, 0, 10, false).is_err());
                }
                assert_eq!(
                    fs::read_to_string(startup.journal(&root).join("previous.log")).unwrap(),
                    legacy
                );
                assert_fresh_writer(&forge, &startup.startup);
                assert_eq!(assert_legacy(&forge, legacy), reserved_id);
                assert_eq!(
                    read(&root.join(&startup.startup).join("commit.json")).unwrap()["count"],
                    0
                );
            }
        }
    }

    #[test]
    fn old_startup_markers_and_stale_replacements_are_preserved_and_superseded() {
        for renamed in [false, true] {
            for published in [false, true] {
                for stale_link in [false, true] {
                    let fixture = QueueTest::new(false);
                    let forge = fixture.app.forge_path("");
                    let legacy = "old-version legacy\r\n  界\t";
                    fs::write(forge.join("agent.log"), legacy).unwrap();
                    let root = forge.join("agent-records");
                    fs::create_dir(&root).unwrap();
                    // Reproduce the prior implementation: copied legacy archive,
                    // new header inode, then a marker with no target provenance.
                    let (legacy_id, legacy_dir) = new_session(&root, true).unwrap();
                    fs::copy(forge.join("agent.log"), legacy_dir.join("readable.log")).unwrap();
                    atomic(
                        &record_path(&legacy_dir, 0),
                        &record(&legacy_id, 0, &Message::new("legacy", "legacy", "", legacy)),
                    )
                    .unwrap();
                    atomic(
                        &legacy_dir.join("commit.json"),
                        &json!({"version":1,"session":legacy_id,"count":1,"legacy":true}),
                    )
                    .unwrap();
                    let (id, dir) = new_session(&root, false).unwrap();
                    fs::write(dir.join("readable.log"), "header only\n").unwrap();
                    pending(&root).unwrap();
                    let old_marker = fs::read(root.join("pending.json")).unwrap();
                    if renamed {
                        fs::hard_link(dir.join("readable.log"), root.join("agent.log.next"))
                            .unwrap();
                        fs::rename(root.join("agent.log.next"), forge.join("agent.log")).unwrap();
                    }
                    if published {
                        atomic(
                            &root.join("current.json"),
                            &json!({"version":1,"session":id}),
                        )
                        .unwrap();
                    }
                    if stale_link {
                        fs::hard_link(dir.join("readable.log"), root.join("agent.log.next"))
                            .unwrap();
                    } else {
                        fs::write(root.join("agent.log.next"), "unrelated collision 界\n").unwrap();
                    }
                    let stale = fs::read(root.join("agent.log.next")).unwrap();
                    // Also interrupt the conversion of the older marker itself.
                    interrupt_start(&forge, "marked");
                    let startup: Startup =
                        serde_json::from_value(read(&root.join("pending.json")).unwrap()).unwrap();
                    let journal = startup.journal(&root);
                    assert_eq!(
                        fs::read(journal.join("previous-pending.json")).unwrap(),
                        old_marker
                    );
                    assert_fresh_writer(&forge, &id);
                    assert_eq!(assert_legacy(&forge, legacy), legacy_id);
                    assert!(fs::read_dir(&journal).unwrap().any(|e| {
                        let e = e.unwrap();
                        e.file_name().to_string_lossy().starts_with("stale-")
                            && fs::read(e.path()).unwrap() == stale
                    }));
                    assert_eq!(
                        fs::read_to_string(dir.join("readable.log")).unwrap(),
                        "header only\n"
                    );
                }
            }
        }
    }

    #[test]
    fn dangling_replacement_is_preserved_across_interrupted_recovery() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let old = InvocationLog::start(&forge, "previous\n").unwrap();
        interrupt_start(&forge, "marked");
        let startup: Startup =
            serde_json::from_value(read(&old.root.join("pending.json")).unwrap()).unwrap();
        let target = Path::new("missing target 界\n");
        std::os::unix::fs::symlink(target, old.root.join("agent.log.next")).unwrap();
        interrupt_start(&forge, "stale_preserved");
        let evidence = fs::read_dir(startup.journal(&old.root))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("stale-")
            })
            .unwrap();
        assert_eq!(fs::read_link(&evidence).unwrap(), target);
        assert_fresh_writer(&forge, &old.session);
        assert_eq!(fs::read_link(evidence).unwrap(), target);
        assert_eq!(
            fs::read_to_string(old.dir.join("readable.log")).unwrap(),
            "previous\n"
        );
    }

    #[test]
    fn startup_journal_recovery_restores_http_feed_and_preserves_pinned_archives() {
        use crate::test_support::api_request;
        for point in [
            "marked",
            "replacement_linked",
            "compatibility_renamed",
            "current_published",
        ] {
            let fixture = QueueTest::new(false);
            let forge = fixture.app.forge_path("");
            let mut old = InvocationLog::start(&forge, "previous\n").unwrap();
            old.append(&Message::new(
                "message",
                "stdout",
                "",
                "previous 界\r\n  exact\t",
            ))
            .unwrap();
            let get = |url: &str| api_request(&fixture.app.app, "GET", url, json!({}));
            let original = get("/api/agent_records").1;
            let pinned = format!("/api/agent_records?archive={}", old.session);
            interrupt_start(&forge, point);
            assert_eq!(get("/api/agent_records").0, 500);
            let compatible = if matches!(point, "marked" | "replacement_linked") {
                "previous\nprevious 界\r\n  exact\t\n"
            } else {
                "interrupted header\n"
            };
            assert_eq!(
                get("/api/agent_log"),
                (200, json!({"log":compatible,"size":compatible.len()}))
            );
            let (status, archived) = get(&pinned);
            assert_eq!(status, 200);
            assert_eq!(archived["entries"], original["entries"]);
            // A second interruption during recovery still leaves pinned history readable.
            interrupt_start(&forge, "current_published");
            assert_eq!(get("/api/agent_records").0, 500);
            assert_eq!(get(&pinned).1["entries"], original["entries"]);
            let mut next = InvocationLog::start(&forge, "recovered\n").unwrap();
            let text = "\nnew 界\r\n  complete\t";
            next.append(&Message::new("message", "stdout", "", text))
                .unwrap();
            let (status, reset) = get(&format!(
                "/api/agent_records?session={}&cursor=1",
                old.session
            ));
            assert_eq!(status, 200);
            assert_eq!(reset["reset"], true);
            assert_eq!(reset["session"], next.session);
            assert_eq!(reset["entries"][0]["text"], text);
            assert_eq!(reset["next_cursor"], 1);
            let readable = format!("recovered\n{text}\n");
            assert_eq!(
                get("/api/agent_log"),
                (200, json!({"log":readable,"size":readable.len()}))
            );
            assert_eq!(get(&pinned).1["entries"], original["entries"]);
        }
    }

    #[test]
    fn recovery_preserves_incomplete_message_evidence_without_advancing_history() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut writer = InvocationLog::start(&forge, "previous\n").unwrap();
        writer
            .append(&Message::new("message", "stdout", "", "committed\ntext  "))
            .unwrap();
        pending(&writer.dir).unwrap();
        writer
            .readable
            .write_all(b"unpublished readable\n")
            .unwrap();
        writer.readable.sync_all().unwrap();
        fs::write(record_path(&writer.dir, 1), "partial JSON").unwrap();
        let files: Vec<_> = fs::read_dir(&writer.dir)
            .unwrap()
            .map(|e| {
                let path = e.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        interrupt_start(&forge, "compatibility_renamed");
        interrupt_start(&forge, "current_published");
        assert_fresh_writer(&forge, &writer.session);
        for (path, bytes) in files {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        assert!(
            page(&forge, "project", None, Some(&writer.session), 0, 10, false)
                .unwrap_err()
                .contains("publication incomplete")
        );
        let source = sources(&forge)
            .into_iter()
            .find(|s| s["session"] == writer.session)
            .unwrap();
        assert_eq!(source["incomplete"], true);
        assert_eq!(source["count"], 1);
    }

    #[test]
    fn real_startup_io_failure_recovers_after_obstruction_is_removed() {
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let old = InvocationLog::start(&forge, "previous\n").unwrap();
        // An actual current.json publication failure, after agent.log rename.
        let result = InvocationLog::start_with_hook(&forge, "next\n", &mut |step| {
            if step == "compatibility_renamed" {
                fs::rename(
                    old.root.join("current.json"),
                    old.root.join("saved-current.json"),
                )
                .unwrap();
                fs::create_dir(old.root.join("current.json")).unwrap();
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(old.root.join("pending.json").exists());
        assert_eq!(
            fs::read_to_string(old.dir.join("readable.log")).unwrap(),
            "previous\n"
        );
        // No marker deletion: fix only the injected environmental IO failure.
        fs::remove_dir(old.root.join("current.json")).unwrap();
        fs::rename(
            old.root.join("saved-current.json"),
            old.root.join("current.json"),
        )
        .unwrap();
        assert_fresh_writer(&forge, &old.session);
    }

    #[test]
    fn startup_crash_child() {
        let Ok(path) = std::env::var("FORGE_LOG_CRASH_TEST_PATH") else {
            return;
        };
        let point = std::env::var("FORGE_LOG_CRASH_TEST_POINT").unwrap();
        let _ = InvocationLog::start_with_hook(Path::new(&path), "crash header\n", &mut |step| {
            if step == point {
                // Exit only this isolated test subprocess without Rust cleanup,
                // leaving the OS to release flock and close writable handles.
                std::process::exit(77);
            }
            Ok(())
        });
        panic!("crash point was not reached");
    }

    #[test]
    fn process_exit_during_startup_and_recovery_keeps_durable_legacy_identity() {
        let crash = |forge: &Path, point: &str| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "agent_log::tests::startup_crash_child"])
                .env("FORGE_LOG_CRASH_TEST_PATH", forge)
                .env("FORGE_LOG_CRASH_TEST_POINT", point)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(77),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        };
        for point in [
            "marked",
            "legacy_staged",
            "replacement_linked",
            "compatibility_renamed",
            "current_published",
        ] {
            let fixture = QueueTest::new(false);
            let forge = fixture.app.forge_path("");
            let text = "\r\n process exit 界\n legacy  \t";
            fs::write(forge.join("agent.log"), text).unwrap();
            crash(&forge, point);
            let root = forge.join("agent-records");
            let startup: Startup =
                serde_json::from_value(read(&root.join("pending.json")).unwrap()).unwrap();
            crash(&forge, "compatibility_renamed");
            crash(&forge, "current_published");
            assert_fresh_writer(&forge, &startup.startup);
            assert_eq!(assert_legacy(&forge, text), startup.legacy.unwrap());
        }
    }

    #[test]
    fn recovery_excludes_readers_and_replaced_writers_until_publication_finishes() {
        use std::sync::mpsc;
        use std::time::Duration;
        let fixture = QueueTest::new(false);
        let forge = fixture.app.forge_path("");
        let mut old = InvocationLog::start(&forge, "old\n").unwrap();
        interrupt_start(&forge, "replacement_linked");
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (read_tx, read_rx) = mpsc::channel();
        let (write_tx, write_rx) = mpsc::channel();
        let (begun_tx, begun_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let forge_ref = &forge;
            let recovering = scope.spawn(move || {
                let mut paused = false;
                InvocationLog::start_with_hook(forge_ref, "new\n", &mut |point| {
                    if point == "compatibility_renamed" && !paused {
                        paused = true;
                        held_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    Ok(())
                })
                .unwrap()
            });
            held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let begun_read = begun_tx.clone();
            scope.spawn(move || {
                begun_read.send(()).unwrap();
                read_tx
                    .send(page(forge_ref, "project", None, None, 0, 10, false))
                    .unwrap();
            });
            scope.spawn(move || {
                begun_tx.send(()).unwrap();
                write_tx
                    .send(old.append(&Message::new("plain", "stdout", "", "stale")))
                    .unwrap();
            });
            for _ in 0..2 {
                begun_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            assert!(read_rx.recv_timeout(Duration::from_millis(30)).is_err());
            assert!(write_rx.recv_timeout(Duration::from_millis(30)).is_err());
            release_tx.send(()).unwrap();
            let new = recovering.join().unwrap();
            let result = read_rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert_eq!(result["session"], new.session);
            assert_eq!(result["next_cursor"], 0);
            assert!(
                write_rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap_err()
                    .contains("replaced")
            );
            assert_eq!(
                fs::read_to_string(forge.join("agent.log")).unwrap(),
                "new\n"
            );
        });
    }
}
