//! Git snapshot identity, changed-path classification, and review-scope policy.
use crate::util::digest;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;

fn git_bytes(root: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(out.stdout)
}
fn parse_git_path_list(bytes: &[u8]) -> Result<Vec<String>, String> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            String::from_utf8(p.to_vec()).map_err(|_| "non-UTF8 path requires manual review".into())
        })
        .collect()
}
fn is_implementation_path(path: &str) -> bool {
    path != ".forge" && !path.starts_with(".forge/")
}

/// A raw layout identity and a normalized final tree are both necessary: git add
/// changes the former legitimately, but must never change reviewed content.
pub(super) fn review_snapshot(root: &str) -> Result<Value, String> {
    review_snapshot_against(root, "HEAD")
}

// Keep deleted paths from the reviewed parent in the content digest during finalization.
pub(super) fn review_snapshot_against(root: &str, subject_head: &str) -> Result<Value, String> {
    let head = String::from_utf8(git_bytes(root, &["rev-parse", "HEAD"])?)
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let names = parse_git_path_list(&git_bytes(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?)?;
    let mut names: std::collections::BTreeSet<_> = names
        .into_iter()
        .chain(parse_git_path_list(&git_bytes(
            root,
            &["ls-tree", "-r", "--name-only", "-z", subject_head],
        )?)?)
        .filter(|p| is_implementation_path(p))
        .collect();
    let mut content = Vec::new();
    for path in std::mem::take(&mut names) {
        let full = PathBuf::from(root).join(&path);
        let (mode, bytes) = match fs::symlink_metadata(&full) {
            Ok(m) if m.is_symlink() => (
                0o120000,
                fs::read_link(&full)
                    .map_err(|e| e.to_string())?
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            ),
            Ok(m) if m.is_file() => (
                if m.permissions().mode() & 0o111 != 0 {
                    0o100755
                } else {
                    0o100644
                },
                fs::read(&full).map_err(|e| e.to_string())?,
            ),
            Ok(_) => {
                return Err(format!(
                    "unsupported directory/submodule in snapshot: {path}"
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (0, vec![]),
            Err(e) => return Err(e.to_string()),
        };
        let hash = digest(&bytes)?;
        content.extend_from_slice(format!("{}:{path}:{mode}:{hash}\n", path.len()).as_bytes());
    }
    let head_ref = String::from_utf8_lossy(&git_bytes(
        root,
        &["rev-parse", "--symbolic-full-name", "HEAD"],
    )?)
    .trim()
    .to_string();
    content.extend_from_slice(head_ref.as_bytes());
    let index = git_bytes(
        root,
        &["ls-files", "--stage", "-z", "--", ".", ":(exclude).forge"],
    )?;
    let staged = git_bytes(
        root,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?;
    let unstaged = git_bytes(
        root,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?;
    let content_hash = digest(&content)?;
    let mut raw = head.as_bytes().to_vec();
    for part in [&index, &staged, &unstaged, &content] {
        raw.extend_from_slice(&(part.len() as u64).to_le_bytes());
        raw.extend_from_slice(part);
    }
    // Isolated index computes the tree git add will produce without touching the real index.
    let temp = std::env::temp_dir().join(format!(
        "forge-review-index-{}",
        crate::architecture::identity()
    ));
    let tree = (|| {
        for args in [
            vec!["read-tree", "HEAD"],
            // Naming an ignored .forge even in an exclude pathspec makes git add fail.
            // Reset runtime paths afterward, using this same isolated index.
            vec!["add", "-A"],
            vec!["reset", "-q", "HEAD", "--", ".forge"],
        ] {
            let out = Command::new("git")
                .args(args)
                .env("GIT_INDEX_FILE", &temp)
                .current_dir(root)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
        }
        let out = Command::new("git")
            .arg("write-tree")
            .env("GIT_INDEX_FILE", &temp)
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("cannot compute reviewed tree".into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })();
    let _ = fs::remove_file(temp);
    Ok(json!({"head":head,"fingerprint":digest(&raw)?,"content":content_hash,"tree":tree?}))
}
pub(super) fn dual_review_policy(reason: &str) -> Value {
    json!({"version":1,"required_roles":["architect","reviewer"],"scope":"code_or_contract","rationale":reason})
}
pub(super) fn classify_review_scope(root: &str, stage: &Value, promoted: bool) -> Result<Value, String> {
    if promoted {
        return Ok(dual_review_policy("Dual review retained for this attempt"));
    }
    let intent = format!(
        "{} {}",
        stage["title"].as_str().unwrap_or(""),
        stage["instructions"].as_str().unwrap_or("")
    )
    .to_lowercase();
    let risky = |text: &str| {
        let lower = text.to_lowercase();
        [
            "api",
            "schema",
            "interface",
            "contract",
            "architect",
            "security",
            "must",
            "shall",
            "required",
            "guarantee",
            "decision",
            "config",
            "build",
            "executable",
            "```",
            "~~~",
            "<script",
            "<!--",
            "\n    ",
            "\t",
            "`",
            "permission",
            "authentication",
            "authorization",
            "always",
            "never",
            "choose",
            "chosen",
            "=",
            "{",
            "}",
            "print(",
            "#!/",
            "import ",
            "export ",
        ]
        .iter()
        .any(|word| lower.contains(word))
    };
    if !["documentation", "prose", "spelling", "typo", "explanation"]
        .iter()
        .any(|s| intent.contains(s))
        || risky(&intent)
    {
        return Ok(dual_review_policy("Stage intent is code, contractual, or uncertain"));
    }
    let mut changed = parse_git_path_list(&git_bytes(
        root,
        &[
            "diff",
            "HEAD",
            "--name-only",
            "-z",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?)?;
    changed.extend(parse_git_path_list(&git_bytes(
        root,
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--",
            ".",
            ":(exclude).forge",
        ],
    )?)?);
    let untracked = parse_git_path_list(&git_bytes(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?;
    // New/deleted/renamed documents, executable bits, and unknown formats are uncertain.
    if untracked.iter().any(|p| is_implementation_path(p)) {
        return Ok(dual_review_policy(
            "Untracked implementation content requires dual review",
        ));
    }
    if changed.is_empty() {
        return Ok(dual_review_policy("Empty or uncertain implementation diff"));
    }
    for path in changed {
        if !(path.ends_with(".md") || path.ends_with(".txt") || path.ends_with(".rst")) {
            return Ok(dual_review_policy("Mixed or non-prose file changes"));
        }
        let Ok(m) = fs::symlink_metadata(PathBuf::from(root).join(&path)) else {
            return Ok(dual_review_policy("Deleted or unreadable document"));
        };
        if !m.is_file() || m.mode() & 0o111 != 0 {
            return Ok(dual_review_policy("Non-ordinary document mode"));
        }
        let diff = git_bytes(
            root,
            &[
                "diff",
                "HEAD",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=3",
                "--",
                &path,
            ],
        )?;
        let mut diff = diff;
        diff.extend(git_bytes(
            root,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                &path,
            ],
        )?);
        diff.extend(git_bytes(
            root,
            &["diff", "--no-ext-diff", "--no-textconv", "--", &path],
        )?);
        let diff = String::from_utf8(diff).map_err(|_| "non-text diff".to_string())?;
        if diff.contains("new file mode")
            || diff.contains("old mode")
            || diff.contains("deleted file")
            || risky(&diff)
        {
            return Ok(dual_review_policy(
                "Full diff contains executable, normative, architectural, or uncertain content",
            ));
        }
    }
    Ok(
        json!({"version":1,"required_roles":["reviewer"],"scope":"ordinary_documentation","rationale":"Prose intent and full diff contain only existing non-executable documents without detected contractual or normative content; independent scope verification required"}),
    )
}

