//! Shared durable JSON file primitives and the repository's distinct publication policies.
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static IDS: AtomicU64 = AtomicU64::new(0);
static CACHE_NONCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn identity() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        IDS.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn read_json<ReadError, DecodeError>(
    path: &Path,
    read_error: ReadError,
    decode_error: DecodeError,
) -> Result<Value, String>
where
    ReadError: FnOnce(&Path, &std::io::Error) -> String,
    DecodeError: FnOnce(&Path, &serde_json::Error) -> String,
{
    let bytes = fs::read(path).map_err(|error| read_error(path, &error))?;
    serde_json::from_slice(&bytes).map_err(|error| decode_error(path, &error))
}

pub(crate) fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

/// Publish architecture-owned JSON with read-back verification and rollback.
pub(crate) fn publish_pretty(path: &Path, value: &Value) -> Result<(), String> {
    publish_pretty_checked(path, value, false)
}

/// The checked policy exposes the existing post-rename sync failure seam.
pub(crate) fn publish_pretty_checked(
    path: &Path,
    value: &Value,
    fail_sync: bool,
) -> Result<(), String> {
    let parent = path.parent().ok_or("missing parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".{}.tmp", identity()));
    let backup = parent.join(format!(".{}.rollback", identity()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        if read_json(
            &tmp,
            |path, e| format!("{}: {e}", path.display()),
            |path, e| format!("{}: {e}", path.display()),
        )? != *value
        {
            return Err("snapshot readback mismatch".into());
        }
        let existed = path.exists();
        if existed {
            fs::hard_link(path, &backup).map_err(|e| e.to_string())?;
        }
        sync_dir(parent)?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        let synced = if fail_sync {
            Err("injected publication sync error".into())
        } else {
            sync_dir(parent)
        };
        if let Err(error) = synced {
            // A reported failure restores the old publication. A process crash
            // instead selects whichever complete commit record survived rename.
            let restored = if existed {
                fs::rename(&backup, path)
            } else {
                fs::remove_file(path)
            };
            restored.map_err(|e| {
                format!("{error}; publication outcome uncertain, rollback failed: {e}")
            })?;
            sync_dir(parent).map_err(|e| {
                format!("{error}; restored publication but rollback durability uncertain: {e}")
            })?;
            return Err(error);
        }
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    // If rollback itself failed, retain its durable recovery record.
    if result
        .as_ref()
        .err()
        .is_none_or(|e| !e.contains("outcome uncertain"))
    {
        let _ = fs::remove_file(backup);
    }
    result
}

/// Publish private cache/settings JSON with compact encoding and mode 0600.
pub(crate) fn publish_private_compact(path: &Path, value: &Value) -> Result<(), String> {
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).map_err(|_| "cache directory creation failed")?;
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        CACHE_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|_| "cache temporary file failed")?;
        f.write_all(&serde_json::to_vec(value).map_err(|_| "cache encoding failed")?)
            .map_err(|_| "cache write failed")?;
        f.sync_all().map_err(|_| "cache sync failed")?;
        fs::rename(&tmp, path).map_err(|_| "cache publication failed")?;
        sync_dir(dir).map_err(|_| "cache directory sync failed")?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("forge-durable-json-{}", identity()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn policies_preserve_pretty_and_private_compact_bytes() {
        let temp = Temp::new();
        let value = json!({"label":"value","nested":[1,2]});
        let pretty = temp.0.join("pretty.json");
        publish_pretty(&pretty, &value).unwrap();
        assert_eq!(
            fs::read(&pretty).unwrap(),
            serde_json::to_vec_pretty(&value).unwrap()
        );

        let compact = temp.0.join("compact.json");
        publish_private_compact(&compact, &value).unwrap();
        assert_eq!(
            fs::read(&compact).unwrap(),
            serde_json::to_vec(&value).unwrap()
        );
        assert_eq!(
            fs::metadata(&compact).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
    }

    #[test]
    fn read_error_formatters_preserve_caller_specific_text() {
        let temp = Temp::new();
        let missing = temp.0.join("missing.json");
        let error = read_json(
            &missing,
            |path, e| format!("{}: {e}", path.display()),
            |path, e| format!("invalid record {}: {e}", path.display()),
        )
        .unwrap_err();
        assert!(error.starts_with(&format!("{}: ", missing.display())));

        let corrupt = temp.0.join("corrupt.json");
        fs::write(&corrupt, b"{").unwrap();
        let error = read_json(
            &corrupt,
            |path, e| format!("{}: {e}", path.display()),
            |path, e| format!("invalid record {}: {e}", path.display()),
        )
        .unwrap_err();
        assert!(error.starts_with(&format!("invalid record {}: ", corrupt.display())));
    }
}
