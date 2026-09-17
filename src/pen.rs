//! pen.dev integration for milestone M4: design detection, pen CLI discovery,
//! bundled skill resolution and headless PNG export of changed `.pen` files.
//!
//! Every child process gets the explicit search path as its PATH, runs in its
//! own process group with a bounded timeout, and has its output retained only
//! up to a fixed limit. Export never writes to the source `.pen` file: the
//! interactive session's `--out` target is a private scratch file and `save()`
//! is never sent. See docs/features/feature-specs/scenarios.md S20-S26.

use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MISE_WHICH_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(120);
/// Bytes of stdout/stderr kept per stream; the remainder is drained and dropped.
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Characters of combined output quoted in error messages.
const TAIL_CHARS: usize = 2000;
const FRAME_TOKEN: &str = "forge-frame";

const PEN_MISSING: &str = "pen.dev export blocked: the pen CLI is not installed or not on PATH. \
Install it with `npm install -g @pen.dev/cli`, then resume the run.";
const PEN_NO_SESSION: &str = "pen.dev export blocked: `pen status` reports no active session. \
Run `pen login`, then resume the run.";

/// Whether free text (stage instructions or acceptance) references a `.pen`
/// file or a feature's `design/` folder.
#[allow(dead_code)]
pub(crate) fn references_designs(text: &str) -> bool {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let pen = text.match_indices(".pen").any(|(i, m)| bytes.get(i + m.len()).is_none_or(|b| !ident(*b)));
    let design = text.match_indices("design/").any(|(i, _)| i == 0 || !ident(bytes[i - 1]));
    pen || design
}

/// Whether a stage's instructions or acceptance criteria reference designs.
#[allow(dead_code)]
pub(crate) fn stage_references_designs(stage: &Value) -> bool {
    ["instructions", "acceptance"]
        .iter()
        .any(|key| stage.get(key).and_then(Value::as_str).is_some_and(references_designs))
}

/// Look up `pen` in an explicit PATH-like string, never the process PATH.
#[allow(dead_code)]
pub(crate) fn find_pen(search_path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("pen"))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Resolve the pen.dev CLI's bundled SKILL.md next to the real `pen` entry point.
/// Follows direct symlinks and mise shims; never runs `pen` itself.
#[allow(dead_code)]
pub(crate) fn resolve_skill(search_path: &OsStr) -> Option<PathBuf> {
    let found = find_pen(search_path)?;
    let canonical = fs::canonicalize(&found).ok()?;
    let entry = if is_mise(&found, &canonical) {
        let out = run_bounded(
            Command::new(&canonical).args(["which", "pen"]).env("PATH", search_path),
            None,
            MISE_WHICH_TIMEOUT,
        )
        .ok()?;
        if !out.success {
            return None;
        }
        let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if resolved.is_empty() {
            return None;
        }
        fs::canonicalize(resolved).ok()?
    } else {
        canonical
    };
    skill_beside_entry(&entry)
}

fn is_mise(found: &Path, canonical: &Path) -> bool {
    let in_shims = found.parent().is_some_and(|dir| {
        dir.file_name() == Some(OsStr::new("shims"))
            && dir.parent().and_then(Path::file_name) == Some(OsStr::new("mise"))
    });
    in_shims || canonical.file_name() == Some(OsStr::new("mise"))
}

fn skill_beside_entry(entry: &Path) -> Option<PathBuf> {
    const SKILL: &str = "out/skills/pen-dev/SKILL.md";
    let direct = entry.parent()?.join(SKILL);
    if direct.is_file() {
        return fs::canonicalize(direct).ok();
    }
    entry.ancestors().skip(1).find_map(|dir| {
        let manifest: Value = serde_json::from_slice(&fs::read(dir.join("package.json")).ok()?).ok()?;
        if manifest.get("name").and_then(Value::as_str) != Some("@pen.dev/cli") {
            return None;
        }
        let skill = dir.join("dist").join(SKILL);
        skill.is_file().then(|| fs::canonicalize(skill).ok()).flatten()
    })
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum ExportError {
    Unavailable(String),
    Failed(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Unavailable(message) | ExportError::Failed(message) => {
                write!(f, "{message}")
            }
        }
    }
}

/// A top-level frame name turned into the PNG filename fragment: ASCII
/// lowercased, each non-alphanumeric character replaced by `-`.
#[allow(dead_code)]
pub(crate) fn frame_slug(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect()
}

/// Repository-relative `.pen` files added, modified, renamed or untracked
/// since `base` (index or worktree), excluding `.forge/` and deleted files.
#[allow(dead_code)]
pub(crate) fn changed_pen_files(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "-z", "--diff-filter=d", base, "--"],
        vec!["ls-files", "-z", "--others", "--exclude-standard"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        for name in out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
            let name = String::from_utf8_lossy(name).into_owned();
            if name.ends_with(".pen") && !name.starts_with(".forge/") && root.join(&name).is_file() {
                files.insert(name);
            }
        }
    }
    Ok(files.into_iter().collect())
}

/// Fails with an actionable Unavailable message unless `pen` is installed and
/// `pen status` reports an active session.
fn check_session(root: &Path, search_path: &OsStr) -> Result<PathBuf, ExportError> {
    let pen = find_pen(search_path).ok_or_else(|| ExportError::Unavailable(PEN_MISSING.to_string()))?;
    let result = run_bounded(
        Command::new(&pen).arg("status").current_dir(root).env("PATH", search_path),
        None,
        STATUS_TIMEOUT,
    );
    let no_session = |detail: String| ExportError::Unavailable(format!("{PEN_NO_SESSION}\n{detail}"));
    let out = result.map_err(no_session)?;
    let text = out.combined();
    let lower = text.to_lowercase();
    let logged_out = ["not logged in", "no active session", "logged out", "unauthenticated", "pen login"]
        .iter()
        .any(|marker| lower.contains(marker));
    if out.timed_out || !out.success || logged_out {
        let reason = if out.timed_out { "`pen status` timed out" } else { "`pen status` output" };
        return Err(no_session(format!("{reason}: {}", tail(&text))));
    }
    Ok(pen)
}

/// Export one `.pen` file to PNGs next to it; returns the repository-relative
/// PNG paths written. The source `.pen` file is never modified.
#[allow(dead_code)]
pub(crate) fn export_design(root: &Path, pen_file: &str, search_path: &OsStr) -> Result<Vec<String>, ExportError> {
    let failed = |message: String| ExportError::Failed(format!("pen.dev export of {pen_file} failed: {message}"));
    let relative = Path::new(pen_file);
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(failed("the path must be repository-relative".into()));
    }
    let stem = relative
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.strip_suffix(".pen"))
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| failed("not a .pen file".into()))?;
    let source = root.join(relative);
    let dest_dir = source.parent().ok_or_else(|| failed("no parent directory".into()))?.to_path_buf();
    let rel_dir = relative.parent().unwrap_or(Path::new(""));
    let pen = find_pen(search_path).ok_or_else(|| ExportError::Unavailable(PEN_MISSING.to_string()))?;

    let scratch = ScratchDir::new().map_err(|e| failed(format!("cannot create a temporary directory: {e}")))?;
    let scratch_str = scratch.0.to_str().filter(|s| !s.contains(['\'', '"', '\\', '\n', '\r']))
        .ok_or_else(|| failed(format!("unsupported temporary directory path {}", scratch.0.display())))?
        .to_string();
    let interactive = |input: &str| {
        let out = run_bounded(
            Command::new(&pen)
                .arg("interactive")
                .arg("--in")
                .arg(&source)
                .arg("--out")
                .arg(scratch.0.join("scratch.pen"))
                .current_dir(root)
                .env("PATH", search_path),
            Some(input.as_bytes().to_vec()),
            INTERACTIVE_TIMEOUT,
        )
        .map_err(&failed)?;
        if out.timed_out {
            return Err(failed(format!("`pen interactive` timed out: {}", tail(&out.combined()))));
        }
        if !out.success {
            return Err(failed(format!("`pen interactive` exited with an error: {}", tail(&out.combined()))));
        }
        Ok(out)
    };

    let listing = interactive(concat!(
        r#"execute({ input: 'Get((n,c)=>{c.skipChildren();Print("forge-frame",n.id,n.name)})' })"#,
        "\nexit()\n"
    ))?;
    let frames: Vec<(String, String)> =
        String::from_utf8_lossy(&listing.stdout).lines().filter_map(parse_frame_line).collect();
    let names = png_names(stem, &frames).map_err(|e| failed(format!("{e}: {}", tail(&listing.combined()))))?;

    let mut script = String::new();
    for (id, _) in &frames {
        script.push_str(&format!("execute({{ input: 'Export([\"{id}\"],\"png\",\"{scratch_str}\")' }})\n"));
    }
    script.push_str("exit()\n");
    let exported = interactive(&script)?;
    for (id, _) in &frames {
        let png = scratch.0.join(format!("{id}.png"));
        if !fs::metadata(&png).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Err(failed(format!(
                "frame {id} produced no PNG: {}",
                tail(&exported.combined())
            )));
        }
    }

    for ((id, _), name) in frames.iter().zip(&names) {
        install(&scratch.0.join(format!("{id}.png")), &dest_dir.join(name))
            .map_err(|e| failed(format!("cannot install {name}: {e}")))?;
    }
    remove_stale_exports(&dest_dir, stem, &names).map_err(|e| failed(format!("cannot remove stale exports: {e}")))?;

    Ok(names.iter().map(|name| rel_dir.join(name).to_string_lossy().into_owned()).collect())
}

/// Export every `.pen` file changed since `base`. Spawns no process when no
/// `.pen` file changed.
#[allow(dead_code)]
pub(crate) fn export_changed(root: &Path, base: &str, search_path: &OsStr) -> Result<Vec<String>, ExportError> {
    let files = changed_pen_files(root, base)
        .map_err(|e| ExportError::Failed(format!("pen.dev export failed: cannot list changed .pen files: {e}")))?;
    if files.is_empty() {
        return Ok(vec![]);
    }
    check_session(root, search_path)?;
    let mut written = Vec::new();
    let mut failures = Vec::new();
    for file in &files {
        match export_design(root, file, search_path) {
            Ok(paths) => written.extend(paths),
            Err(unavailable @ ExportError::Unavailable(_)) => return Err(unavailable),
            Err(ExportError::Failed(message)) => failures.push(format!("- {file}: {message}")),
        }
    }
    if failures.is_empty() {
        Ok(written)
    } else {
        Err(ExportError::Failed(format!(
            "pen.dev export failed for {} of {} changed design(s):\n{}",
            failures.len(),
            files.len(),
            failures.join("\n")
        )))
    }
}

/// Parses `... forge-frame <id> <name>` with the marker as a whole token.
fn parse_frame_line(line: &str) -> Option<(String, String)> {
    let mut offset = 0;
    for token in line.split_whitespace() {
        let start = offset + line[offset..].find(token)?;
        offset = start + token.len();
        if token == FRAME_TOKEN {
            let rest = line[offset..].trim_start();
            let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            return Some((id.to_string(), name.trim().to_string()));
        }
    }
    None
}

/// PNG file names for the frames, validating ids and derived names.
fn png_names(stem: &str, frames: &[(String, String)]) -> Result<Vec<String>, String> {
    if frames.is_empty() {
        return Err("no top-level frames".into());
    }
    let mut ids = BTreeSet::new();
    let mut names = Vec::new();
    for (id, name) in frames {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(format!("invalid frame id {id:?}"));
        }
        if !ids.insert(id.as_str()) {
            return Err(format!("duplicate frame id {id:?}"));
        }
        let file = if frames.len() == 1 {
            format!("{stem}.png")
        } else {
            let slug = frame_slug(name);
            if slug.is_empty() {
                return Err(format!("frame {id} has an empty name"));
            }
            format!("{stem}.{slug}.png")
        };
        if names.contains(&file) {
            return Err(format!("several frames export to the same file {file}"));
        }
        names.push(file);
    }
    Ok(names)
}

/// Copies to a temporary name in the destination directory, then renames.
fn install(from: &Path, to: &Path) -> std::io::Result<()> {
    let dir = to.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let name = to.file_name().ok_or(std::io::ErrorKind::InvalidInput)?.to_string_lossy();
    let temp = dir.join(format!(".{name}.forge-{}.tmp", crate::durable_json::identity()));
    let result = fs::copy(from, &temp).and_then(|_| fs::rename(&temp, to));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Whether `name` is `<stem>.png` or `<stem>.<slug>.png` for this design,
/// not another design's `<stem>.<x>.pen`-backed export.
fn owns_export(dir: &Path, stem: &str, name: &str) -> bool {
    let Some(middle) = name.strip_prefix(stem).and_then(|rest| rest.strip_suffix(".png")) else { return false };
    match middle.strip_prefix('.') {
        None => middle.is_empty(),
        Some(slug) => {
            !slug.is_empty()
                && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !dir.join(format!("{stem}.{slug}.pen")).exists()
        }
    }
}

/// Removes `<stem>.png` / `<stem>.<slug>.png` files not just written, keeping
/// `<stem>.<x>.png` when `<stem>.<x>.pen` exists (another design's export).
fn remove_stale_exports(dir: &Path, stem: &str, written: &[String]) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if written.contains(&name) || !entry.file_type()?.is_file() {
            continue;
        }
        if owns_export(dir, stem, &name) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Repository-relative PNG paths currently exported for a `.pen` file,
/// following [`export_design`]'s naming rule and excluding files that belong
/// to another design's `.pen` in the same directory.
#[allow(dead_code)]
pub(crate) fn exported_pngs(root: &Path, pen_file: &str) -> Vec<String> {
    let relative = Path::new(pen_file);
    let Some(stem) = relative.file_name().and_then(OsStr::to_str).and_then(|name| name.strip_suffix(".pen")) else {
        return vec![];
    };
    let rel_dir = relative.parent().unwrap_or(Path::new(""));
    let dir = root.join(rel_dir);
    let Ok(entries) = fs::read_dir(&dir) else { return vec![] };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| owns_export(&dir, stem, name))
        .collect();
    names.sort();
    names.into_iter().map(|name| rel_dir.join(name).to_string_lossy().into_owned()).collect()
}

fn tail(text: &str) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= TAIL_CHARS {
        return text.to_string();
    }
    let rest: String = text.chars().skip(count - TAIL_CHARS).collect();
    format!("...{rest}")
}

/// A private (0700) temporary directory removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("forge-pen-export-{}", crate::durable_json::identity()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Output {
    success: bool,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Output {
    fn combined(&self) -> String {
        format!("{}{}", String::from_utf8_lossy(&self.stdout), String::from_utf8_lossy(&self.stderr))
    }
}

/// Runs a command in its own process group with piped, size-limited output
/// and a deadline; the whole group is killed on timeout and after exit.
fn run_bounded(command: &mut Command, input: Option<Vec<u8>>, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start {:?}: {e}", command.get_program()))?;
    let kill_group = |pid: u32| unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    };
    let writer = child.stdin.take().zip(input).map(|(mut stdin, bytes)| {
        std::thread::spawn(move || {
            // The child may exit without reading; a broken pipe is not an error here.
            let _ = stdin.write_all(&bytes);
        })
    });
    let reader = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            if let Some(mut stream) = stream {
                let mut buf = [0u8; 8192];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
            kept
        })
    };
    let stdout = reader(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let stderr = reader(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));

    let deadline = Instant::now() + timeout;
    let pid = child.id();
    let (success, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.success(), false),
            Ok(None) if Instant::now() >= deadline => {
                kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break (false, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {:?}: {e}", command.get_program()));
            }
        }
    };
    // A descendant still holding a pipe after the child exited belongs to our
    // process group (which therefore still exists); kill it at the deadline.
    let finished = |handles: &[&std::thread::JoinHandle<Vec<u8>>], until: Instant| {
        while handles.iter().any(|h| !h.is_finished()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        handles.iter().all(|h| h.is_finished())
    };
    if !finished(&[&stdout, &stderr], deadline) {
        kill_group(pid);
        finished(&[&stdout, &stderr], Instant::now() + Duration::from_secs(1));
    }
    let collect = |handle: std::thread::JoinHandle<Vec<u8>>| {
        if handle.is_finished() { handle.join().unwrap_or_default() } else { Vec::new() }
    };
    if let Some(writer) = writer.filter(|w| w.is_finished()) {
        let _ = writer.join();
    }
    Ok(Output { success, timed_out, stdout: collect(stdout), stderr: collect(stderr) })
}

/// Shell-based pen.dev editing instructions for implementer/fixer prompts.
/// Identical for every caller: it takes no provider and carries no MCP
/// configuration, only the resolved skill path (if any) varies.
pub(crate) fn editing_instructions(skill: Option<&Path>) -> String {
    let skill_paragraph = match skill {
        Some(path) => format!(
            "Read the pen.dev CLI's bundled skill at {} before editing for the full command reference.",
            path.display()
        ),
        None => crate::prompts::PEN_EDITING_NO_SKILL.to_string(),
    };
    format!("\n\n{}\n\n{skill_paragraph}", crate::prompts::PEN_EDITING_PARAGRAPH)
}

/// Reviewer pointers to a snapshot's changed designs and their exported
/// PNGs. Returns an empty string when there are no changed designs; never
/// contains editing instructions.
pub(crate) fn reviewer_instructions(designs: &[(String, Vec<String>)]) -> String {
    if designs.is_empty() {
        return String::new();
    }
    let mut section = format!("\n\n{}\n", crate::prompts::PEN_REVIEW_INTRO);
    for (pen_file, pngs) in designs {
        if pngs.is_empty() {
            section.push_str(&format!("- {pen_file} (no exported PNG found)\n"));
        } else {
            section.push_str(&format!("- {pen_file} -> {}\n", pngs.join(", ")));
        }
    }
    section
}
