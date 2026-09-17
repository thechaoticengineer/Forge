//! Business tests for milestone M4 (pen.dev integration) scenarios S20-S26.
//! See docs/features/feature-specs/scenarios.md.

use crate::app::{App, Ctx};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

// ---------------------------------------------------------------- sha256
// A tiny, self-contained SHA-256 so fixture PNG bytes can be predicted
// without adding a crate dependency. Only used by tests.

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(temp1);
            d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[test]
fn sha256_matches_known_vectors() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

// ---------------------------------------------------------- design helpers

/// Builds a minimal `.pen` JSON document with the given top-level frames.
fn design_json(frames: &[(&str, &str)]) -> String {
    json!({
        "children": frames.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>()
    }).to_string()
}

/// The fake pen CLI's deterministic PNG bytes for a given `.pen` file's
/// content and exported frame id.
fn expected_png(pen_bytes: &[u8], id: &str) -> Vec<u8> {
    format!("PNG:{}:{id}", sha256_hex(pen_bytes)).into_bytes()
}

fn python3_path() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("python3"))
        .find(|p| p.is_file())
        .expect("python3 must be on PATH to build the fake pen fixture")
}

// -------------------------------------------------------------- fake pen

const PEN_SCRIPT_BODY: &str = r##"
import sys, os, json, hashlib, re

def _here():
    return os.path.realpath(__file__)

def _root():
    dist_dir = os.path.dirname(_here())
    return os.path.normpath(os.path.join(dist_dir, "..", "..", "..", "..", ".."))

def _log(args):
    with open(os.path.join(_root(), "calls.log"), "a") as f:
        f.write(json.dumps(args) + "\n")

def _status():
    mode_path = os.path.join(_root(), "status-mode")
    mode = "ok"
    if os.path.exists(mode_path):
        with open(mode_path) as f:
            text = f.read().strip()
        if text:
            mode = text
    if mode == "exit1":
        sys.stderr.write("Error: could not reach pen.dev\n")
        return 1
    if mode == "logged-out":
        sys.stdout.write("Not logged in. Run pen login.\n")
        return 0
    sys.stdout.write("Logged in as fixture-user (session active)\n")
    return 0

_EXPORT_RE = re.compile(r'Export\(\[(?P<ids>[^\]]*)\],"png","(?P<dir>[^"]*)"\)')

def _interactive(rest):
    in_path = None
    out_path = None
    i = 0
    while i < len(rest):
        if rest[i] == "--in" and i + 1 < len(rest):
            in_path = rest[i + 1]
            i += 2
        elif rest[i] == "--out" and i + 1 < len(rest):
            out_path = rest[i + 1]
            i += 2
        else:
            i += 1
    try:
        with open(in_path, "rb") as f:
            raw = f.read()
        doc = json.loads(raw.decode("utf-8"))
    except Exception:
        sys.stderr.write("Error: cannot open " + str(in_path) + "\n")
        return 1
    children = doc.get("children", [])
    for line in sys.stdin:
        line = line.rstrip("\n")
        if "Get(" in line:
            for child in children:
                sys.stdout.write("forge-frame " + str(child["id"]) + " " + str(child["name"]) + "\n")
            sys.stdout.flush()
        elif "Export(" in line:
            m = _EXPORT_RE.search(line)
            if not m:
                sys.stdout.write("Error: cannot parse export call\n")
                sys.stdout.flush()
                continue
            ids_raw = m.group("ids").strip()
            ids = [part.strip().strip('"') for part in ids_raw.split(",")] if ids_raw else []
            out_dir = m.group("dir")
            if ids == ["document"]:
                sys.stdout.write("Error: exporting [\"document\"] is not supported\n")
            else:
                digest = hashlib.sha256(raw).hexdigest()
                for frame_id in ids:
                    payload = ("PNG:" + digest + ":" + frame_id).encode("utf-8")
                    with open(os.path.join(out_dir, frame_id + ".png"), "wb") as out_f:
                        out_f.write(payload)
            sys.stdout.flush()
        elif line.startswith("save()"):
            if out_path:
                with open(out_path, "wb") as f:
                    f.write(raw)
        elif line.startswith("exit()"):
            return 0
    return 0

def main():
    args = sys.argv[1:]
    _log(args)
    if args[:1] == ["status"]:
        return _status()
    if args[:1] == ["interactive"]:
        return _interactive(args[1:])
    sys.stderr.write("Error: unsupported command\n")
    return 1

if __name__ == "__main__":
    sys.exit(main())
"##;

const MISE_SCRIPT_BODY: &str = r##"
import sys, os

def _here():
    return os.path.realpath(__file__)

def _root():
    bin_dir = os.path.dirname(_here())
    return os.path.normpath(os.path.join(bin_dir, "..", ".."))

def _index_path():
    return os.path.join(_root(), "pkg", "node_modules", "@pen.dev", "cli", "dist", "index.mjs")

def main():
    prog = os.path.basename(sys.argv[0])
    if prog == "mise":
        if sys.argv[1:3] == ["which", "pen"]:
            sys.stdout.write(_index_path() + "\n")
            return 0
        sys.stderr.write("Error: unsupported mise command\n")
        return 1
    target = _index_path()
    os.execv(target, [target] + sys.argv[1:])

if __name__ == "__main__":
    sys.exit(main())
"##;

/// A temporary fixture providing a fake `pen` CLI (and a fake `mise` shim
/// variant), following the FeatureFixture pattern: a unique temp root
/// removed on Drop. The fake pen never contacts the real pen.dev service;
/// it appends each invocation's argv to `<root>/calls.log`.
struct PenCli {
    root: PathBuf,
}

impl PenCli {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-pen-cli-{}", crate::architecture::identity()));
        let cli_dir = root.join("pkg/node_modules/@pen.dev/cli/dist");
        fs::create_dir_all(&cli_dir).unwrap();
        fs::create_dir_all(cli_dir.join("out/skills/pen-dev")).unwrap();
        fs::write(
            cli_dir.join("out/skills/pen-dev/SKILL.md"),
            "# pen.dev CLI skill\n\nDrive `pen interactive --in <file.pen> --out <file.pen>` from the shell: \
             send `execute({ input: '<js>' })` calls on stdin, then `save()` and `exit()`.\n",
        ).unwrap();

        let python3 = python3_path();
        let index = cli_dir.join("index.mjs");
        fs::write(&index, format!("#!{}\n{PEN_SCRIPT_BODY}", python3.display())).unwrap();
        fs::set_permissions(&index, fs::Permissions::from_mode(0o755)).unwrap();

        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&index, bin.join("pen")).unwrap();

        let mise_bin = root.join("mise/bin");
        fs::create_dir_all(&mise_bin).unwrap();
        let mise = mise_bin.join("mise");
        fs::write(&mise, format!("#!{}\n{MISE_SCRIPT_BODY}", python3.display())).unwrap();
        fs::set_permissions(&mise, fs::Permissions::from_mode(0o755)).unwrap();

        let shims = root.join("mise/shims");
        fs::create_dir_all(&shims).unwrap();
        std::os::unix::fs::symlink(&mise, shims.join("pen")).unwrap();

        Self { root }
    }

    fn direct_search_path(&self) -> OsString {
        OsString::from(self.root.join("bin"))
    }

    fn mise_search_path(&self) -> OsString {
        OsString::from(self.root.join("mise/shims"))
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("pkg/node_modules/@pen.dev/cli/dist/index.mjs")
    }

    fn mise_path(&self) -> PathBuf {
        self.root.join("mise/bin/mise")
    }

    fn skill_path(&self) -> PathBuf {
        self.root.join("pkg/node_modules/@pen.dev/cli/dist/out/skills/pen-dev/SKILL.md")
    }

    fn set_status_mode(&self, mode: &str) {
        fs::write(self.root.join("status-mode"), mode).unwrap();
    }

    fn calls(&self) -> Vec<Value> {
        let log = self.root.join("calls.log");
        if !log.exists() {
            return vec![];
        }
        fs::read_to_string(log).unwrap().lines().filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

impl Drop for PenCli {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// --------------------------------------------------- fixture self-checks
// These are NOT scenario tests and must not be #[ignore]d: they prove the
// fake CLI itself behaves as documented, independent of the M4 engine work.

#[test]
fn fixture_fake_pen_reports_status_by_mode() {
    let cli = PenCli::new();
    let out = std::process::Command::new(cli.index_path()).arg("status").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Logged in"));

    cli.set_status_mode("exit1");
    let out = std::process::Command::new(cli.index_path()).arg("status").output().unwrap();
    assert!(!out.status.success());

    cli.set_status_mode("logged-out");
    let out = std::process::Command::new(cli.index_path()).arg("status").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Not logged in. Run pen login."));

    let calls = cli.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c[0] == "status"));
}

#[test]
fn fixture_fake_pen_interactive_lists_frames_and_exports_png() {
    let cli = PenCli::new();
    let pen_json = design_json(&[("f1", "Main Screen")]);
    let tmp = std::env::temp_dir().join(format!("forge-pen-fixture-check-{}", crate::architecture::identity()));
    fs::create_dir_all(&tmp).unwrap();
    let in_path = tmp.join("screen.pen");
    let out_path = tmp.join("screen.out.pen");
    fs::write(&in_path, &pen_json).unwrap();
    let export_dir = tmp.join("out");
    fs::create_dir_all(&export_dir).unwrap();

    let mut child = std::process::Command::new(cli.index_path())
        .args(["interactive", "--in", in_path.to_str().unwrap(), "--out", out_path.to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "Get((n,c)=>{{c.skipChildren();Print(n.id,n.name)}})").unwrap();
        writeln!(stdin, "Export([\"f1\"],\"png\",\"{}\")", export_dir.display()).unwrap();
        writeln!(stdin, "save()").unwrap();
        writeln!(stdin, "exit()").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("forge-frame f1 Main Screen"), "{stdout}");

    let png = fs::read(export_dir.join("f1.png")).unwrap();
    assert_eq!(png, expected_png(pen_json.as_bytes(), "f1"));
    assert_eq!(fs::read(&out_path).unwrap(), pen_json.as_bytes());

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn fixture_fake_pen_rejects_missing_input_and_document_export() {
    let cli = PenCli::new();
    let out = std::process::Command::new(cli.index_path())
        .args(["interactive", "--in", "/nonexistent/screen.pen", "--out", "/nonexistent/out.pen"])
        .output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot open"));

    let pen_json = design_json(&[("f1", "Main Screen")]);
    let tmp = std::env::temp_dir().join(format!("forge-pen-fixture-check-{}", crate::architecture::identity()));
    fs::create_dir_all(&tmp).unwrap();
    let in_path = tmp.join("screen.pen");
    fs::write(&in_path, &pen_json).unwrap();
    let export_dir = tmp.join("out");
    fs::create_dir_all(&export_dir).unwrap();

    let mut child = std::process::Command::new(cli.index_path())
        .args(["interactive", "--in", in_path.to_str().unwrap(), "--out", tmp.join("out.pen").to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "Export([\"document\"],\"png\",\"{}\")", export_dir.display()).unwrap();
        writeln!(stdin, "exit()").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("not supported"));
    assert!(!export_dir.join("document.png").exists());

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn fixture_mise_shim_resolves_and_forwards_to_pen() {
    let cli = PenCli::new();
    let out = std::process::Command::new(cli.mise_path()).args(["which", "pen"]).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), cli.index_path().display().to_string());

    let shim_pen = PathBuf::from(cli.mise_search_path()).join("pen");
    let out = std::process::Command::new(&shim_pen).arg("status").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Logged in"));

    let calls = cli.calls();
    assert!(calls.iter().any(|c| c[0] == "status"), "{calls:?}");
}

// ------------------------------------------------------------- fixture

fn pen_paragraph(prompt: &str) -> Option<String> {
    prompt.split("\n\n").find(|p| p.contains("pen interactive --in")).map(str::to_string)
}

/// Offline engine fixture: a real git repository driven through the real
/// implementer/fixer/reviewer/architect prompt path, with execution mocked
/// via `test_fake_providers`, following the pattern in src/lifecycle_tests.rs.
struct Fixture {
    root: PathBuf,
    ctx: Ctx,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("forge-pen-dev-test-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect": "per_stage", "reviewer": "per_stage"});
        settings["planner"] = json!("mock");
        settings["architect"] = json!("mock");
        settings["test_fake_providers"] = json!(true);
        settings["auto_push"] = json!(false);
        settings["model_catalogue"]["entries"] = json!([
            {"provider": "codex", "model": "small", "tier": "strong", "relative_cost_preference": 1},
            {"provider": "claude", "model": "other", "tier": "strong", "relative_cost_preference": 1},
        ]);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            ctx.git(&args).unwrap();
        }
        fs::write(root.join("README.md"), "A demo project.\n").unwrap();
        // mock_edits writes files but never creates parent directories, so the
        // feature's design folder must already exist for stages that edit it.
        fs::create_dir_all(root.join("docs/features/demo/design")).unwrap();
        fs::write(root.join("docs/features/demo/design/.gitkeep"), "").unwrap();
        ctx.git(&["add", "README.md", "docs/features/demo/design/.gitkeep"]).unwrap();
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        Self { root, ctx }
    }

    fn set(&self, key: &str, value: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = value;
    }

    fn proposal(id: i64, provider: &str, model: &str) -> Value {
        json!({"stage_id": id, "proposal": {"provider": provider, "model": model, "native_effort": "provider_default",
            "risk": "simple", "complexity": "simple", "task": "functionality",
            "rationale": "Configured adequacy meets the stage risk."}})
    }

    fn stage(id: i64, title: &str, instructions: &str, acceptance: &str) -> Value {
        json!({"id": id, "title": title, "instructions": instructions, "acceptance": acceptance,
            "commit": format!("feat: {title}"), "depends_on": [], "status": "pending", "rounds": 0})
    }

    fn verdict(approved: bool) -> Value {
        json!({"approved": approved,
            "issues": if approved { Vec::<String>::new() } else { vec!["Address the outstanding request".to_string()] }})
    }

    fn publish_ready(&self, goal: &str, stages: Vec<Value>) -> Value {
        self.ctx.architect_publish(json!({"goal": goal, "status": "ready", "stages": stages}), None, "fixture draft").unwrap()
    }

    fn plan(&self) -> Value {
        self.ctx.load_plan().unwrap()
    }

    fn role_prompts(&self, role: &str) -> Vec<String> {
        self.ctx.app.settings.lock().unwrap()["mock_agent_requests"].as_array().into_iter().flatten()
            .filter(|r| r["role"] == role)
            .map(|r| r["prompt"].as_str().unwrap().to_string()).collect()
    }

    fn implementer_prompts(&self) -> Vec<String> {
        self.role_prompts("implementer")
    }

    fn fixer_prompts(&self) -> Vec<String> {
        self.role_prompts("fixer")
    }

    fn mock_prompts(&self, key: &str) -> Vec<String> {
        self.ctx.app.settings.lock().unwrap().get(key).and_then(Value::as_array).cloned().unwrap_or_default()
            .into_iter().map(|v| v.as_str().unwrap().to_string()).collect()
    }

    fn reviewer_prompts(&self) -> Vec<String> {
        self.mock_prompts("mock_reviewer_prompts")
    }

    fn architect_prompts(&self) -> Vec<String> {
        self.mock_prompts("mock_architect_prompts")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// -------------------------------------------------------- scenario tests

#[test]
fn s20_editing_agents_get_pen_instructions_for_design_stages() {
    // S20: Editing agents get pen.dev instructions when a stage involves designs
    for (text, expected) in [
        ("Edit docs/features/demo/design/screen.pen using pen interactive.", true),
        ("Add mockups under docs/features/a/design/ for review.", true),
        ("open file and change the greeting", false),
        ("redesign/ this API surface", false),
        ("pending", false),
    ] {
        assert_eq!(crate::pen::references_designs(text), expected, "{text}");
    }

    // No MCP configuration may be added for a design prompt: command()
    // arguments must be identical to a plain prompt, apart from the prompt
    // argument itself.
    for role in ["implementer", "fixer"] {
        let design = crate::agent::command(&crate::agent::AgentRequest {
            role, provider: "claude", model: "other", effort: "provider_default",
            session: None, prompt: "Edit docs/features/demo/design/screen.pen with pen interactive.",
        }).unwrap();
        let plain = crate::agent::command(&crate::agent::AgentRequest {
            role, provider: "claude", model: "other", effort: "provider_default",
            session: None, prompt: "Append a friendly line to README.md.",
        }).unwrap();
        let design_args: Vec<_> = design.get_args().map(|a| a.to_owned()).collect();
        let plain_args: Vec<_> = plain.get_args().map(|a| a.to_owned()).collect();
        assert_eq!(design_args.len(), plain_args.len(), "{role}");
        assert_eq!(
            &design_args[..design_args.len() - 1], &plain_args[..plain_args.len() - 1],
            "{role}: no MCP configuration may be added for a design prompt"
        );
    }

    let mut pen_paragraphs: Vec<(String, String)> = Vec::new();
    for (provider, model) in [("claude", "other"), ("codex", "small")] {
        for variant in ["direct", "mise"] {
            let cli = PenCli::new();
            let search_path = if variant == "direct" { cli.direct_search_path() } else { cli.mise_search_path() };
            let skill = fs::canonicalize(cli.skill_path()).unwrap();

            let f = Fixture::new();
            f.set("test_pen_search_path", json!(search_path.to_string_lossy()));
            f.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, provider, model)]}]));
            f.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
                "Create docs/features/demo/design/screen.pen using pen interactive.",
                "docs/features/demo/design/screen.pen exists and its PNG matches")]);
            f.set("mock_verdicts", json!([Fixture::verdict(false)]));
            f.ctx.run_worker();

            let implementer_prompts = f.implementer_prompts();
            let fixer_prompts = f.fixer_prompts();
            assert!(!implementer_prompts.is_empty(), "{provider}/{variant}: implementer must run");
            assert!(!fixer_prompts.is_empty(), "{provider}/{variant}: a fix round must run");
            for prompt in implementer_prompts.iter().chain(fixer_prompts.iter()) {
                for text in ["pen interactive --in", "--out", "execute(", "save()"] {
                    assert!(prompt.contains(text), "{provider}/{variant}: missing {text:?}: {prompt}");
                }
                assert!(
                    prompt.contains(&skill.display().to_string()),
                    "{provider}/{variant}: missing skill path {}: {prompt}", skill.display()
                );
            }
            if variant == "direct" {
                let paragraph = pen_paragraph(&implementer_prompts[0]).unwrap_or_else(|| {
                    panic!("{provider}: implementer prompt must contain a pen instructions paragraph")
                });
                pen_paragraphs.push((provider.to_string(), paragraph));
            }
        }
    }
    assert_eq!(pen_paragraphs.len(), 2);
    assert_eq!(
        pen_paragraphs[0].1, pen_paragraphs[1].1,
        "pen instructions must be identical for {} and {}", pen_paragraphs[0].0, pen_paragraphs[1].0
    );

    // A stage referencing only the design/ folder (no specific .pen file)
    // also gets pen instructions.
    let cli = PenCli::new();
    let f = Fixture::new();
    f.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    f.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    f.publish_ready("Sketch a new mockup", vec![Fixture::stage(1, "Sketch a new mockup",
        "Add a new mockup under docs/features/demo/design/ using pen.dev.",
        "A new .pen file exists under docs/features/demo/design/")]);
    f.ctx.run_worker();
    let implementer_prompts = f.implementer_prompts();
    assert!(!implementer_prompts.is_empty());
    assert!(
        implementer_prompts[0].contains("pen interactive --in"),
        "a design/ folder reference alone must trigger pen instructions: {}", implementer_prompts[0]
    );
}

#[test]
fn s21_stages_without_designs_are_unchanged() {
    // S21: Stages without designs are unchanged
    let cli = PenCli::new();

    let plain = Fixture::new();
    plain.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    plain.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    plain.publish_ready("Improve greeting", vec![Fixture::stage(1, "Fix greeting",
        "Append a friendly line to README.md.", "README.md has the friendly line")]);
    plain.ctx.run_worker();
    let plan = plain.plan();
    assert_eq!(plan["stages"][0]["status"], "committed", "{}", plain.ctx.read_history());
    for prompt in plain.implementer_prompts().iter()
        .chain(plain.fixer_prompts().iter())
        .chain(plain.reviewer_prompts().iter())
        .chain(plain.architect_prompts().iter())
    {
        assert!(!prompt.contains("pen interactive"), "{prompt}");
        assert!(!prompt.contains("pen.dev"), "{prompt}");
    }
    assert!(cli.calls().is_empty(), "pen must never be invoked for a stage without designs: {:?}", cli.calls());

    // Contrast: the same pen fixture IS invoked for a stage that changes a
    // .pen file, proving the isolation above is not merely because nothing
    // ever calls pen. This stage only wires prompts, not the export that
    // will drive `pen` for real (a later stage); export_design already
    // exists (stage 2) so this constructs that export directly against the
    // committed snapshot, the same way a later stage's export wiring will.
    let design = Fixture::new();
    design.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    design.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    design.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
        "Create docs/features/demo/design/screen.pen using pen interactive.",
        "docs/features/demo/design/screen.pen exists and its PNG matches")]);
    design.set("mock_edits", json!([{"docs/features/demo/design/screen.pen": design_json(&[("f1", "Main")])}]));
    design.ctx.run_worker();
    assert_eq!(design.plan()["stages"][0]["status"], "committed", "{}", design.ctx.read_history());
    crate::pen::export_design(&design.root, "docs/features/demo/design/screen.pen", &cli.direct_search_path()).unwrap();
    assert!(!cli.calls().is_empty(), "pen must be invoked for a stage that changes a .pen file");
}

#[test]
#[ignore = "M4 pending: S22 not implemented yet"]
fn s22_png_exported_for_every_changed_pen_file() {
    // S22: A PNG is exported for every changed .pen file
    let cli = PenCli::new();
    let f = Fixture::new();
    f.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    f.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    f.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
        "Create docs/features/demo/design/screen.pen using pen interactive.",
        "docs/features/demo/design/screen.pen exists and its PNG matches")]);
    f.set("mock_edits", json!([
        {"docs/features/demo/design/screen.pen": design_json(&[("f1", "Main Screen")])},
        {"docs/features/demo/design/screen.pen": design_json(&[("f1", "Main Screen Updated")])},
    ]));
    // Force a fix round so both the implementer's and the fixer's changes
    // to the .pen file are exercised; the fixer's version is what commits.
    f.set("mock_verdicts", json!([Fixture::verdict(false)]));
    f.ctx.run_worker();

    let plan = f.plan();
    assert_eq!(plan["stages"][0]["status"], "committed", "{}", f.ctx.read_history());
    let sha = plan["stages"][0]["sha"].as_str().expect("stage must record its commit sha").to_string();

    // The PNG exists next to the .pen file and is part of the same commit
    // as the .pen change (checked by the plan review's snapshot identity
    // guarantee, since export happens before review or commit — D12).
    let changed = f.ctx.git(&["show", "--name-only", "--pretty=format:", &sha]).unwrap();
    assert!(changed.contains("docs/features/demo/design/screen.pen"), "{changed}");
    assert!(
        changed.contains("docs/features/demo/design/screen.png"),
        "PNG and .pen must be committed together: {changed}"
    );

    let committed_pen = f.ctx.git(&["show", &format!("{sha}:docs/features/demo/design/screen.pen")]).unwrap();
    let committed_png = f.ctx.git(&["show", &format!("{sha}:docs/features/demo/design/screen.png")]).unwrap_or_default();
    assert_eq!(
        committed_png.into_bytes(),
        expected_png(committed_pen.as_bytes(), "f1"),
        "the exported PNG must match the committed .pen content"
    );
}

#[test]
fn s23_export_names_follow_top_level_frames() {
    // S23: Export file names follow the design's top-level frames
    assert_eq!(crate::pen::frame_slug("Main Screen"), "main-screen");
    assert_eq!(crate::pen::frame_slug("Settings/Dialog"), "settings-dialog");

    let cli = PenCli::new();
    let root = std::env::temp_dir().join(format!("forge-pen-export-test-{}", crate::architecture::identity()));
    fs::create_dir_all(root.join("design")).unwrap();
    std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).output().unwrap();
    let pen_file = "design/screen.pen";
    let search_path = cli.direct_search_path();

    // One frame -> <name>.png
    fs::write(root.join(pen_file), design_json(&[("f1", "Main Screen")])).unwrap();
    let out = crate::pen::export_design(&root, pen_file, &search_path).unwrap();
    assert_eq!(out, vec!["design/screen.png".to_string()]);
    let bytes = fs::read(root.join(pen_file)).unwrap();
    assert_eq!(fs::read(root.join("design/screen.png")).unwrap(), expected_png(&bytes, "f1"));

    // Several frames -> one <name>.<frame-slug>.png each; the previous
    // single-frame PNG is removed because it no longer matches a frame.
    fs::write(root.join(pen_file), design_json(&[("f1", "Main Screen"), ("f2", "Settings/Dialog")])).unwrap();
    let mut out = crate::pen::export_design(&root, pen_file, &search_path).unwrap();
    out.sort();
    assert_eq!(out, vec!["design/screen.main-screen.png".to_string(), "design/screen.settings-dialog.png".to_string()]);
    assert!(!root.join("design/screen.png").exists(), "stale single-frame PNG must be removed");

    // A stale export belonging to this file that no longer matches any
    // frame is removed; unrelated files are kept.
    fs::write(root.join("design/other.png"), b"keep-other").unwrap();
    fs::write(root.join("design/extra.pen"), design_json(&[("f1", "Frame")])).unwrap();
    fs::write(root.join("design/extra.png"), b"keep-extra").unwrap();
    fs::write(root.join(pen_file), design_json(&[("f1", "Main Screen")])).unwrap();
    let out = crate::pen::export_design(&root, pen_file, &search_path).unwrap();
    assert_eq!(out, vec!["design/screen.png".to_string()]);
    assert!(!root.join("design/screen.settings-dialog.png").exists());
    assert!(!root.join("design/screen.main-screen.png").exists());
    assert!(root.join("design/other.png").exists(), "unrelated file must be kept");
    assert!(root.join("design/extra.png").exists(), "another design's export must be kept");

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "M4 pending: S24 not implemented yet"]
fn s24_missing_or_unauthenticated_pen_blocks_run() {
    // S24: A missing or unauthenticated pen CLI blocks with an actionable message
    struct Case {
        label: &'static str,
        with_pen: bool,
        status_mode: Option<&'static str>,
        expect: &'static str,
    }
    let cases = [
        Case { label: "missing pen", with_pen: false, status_mode: None, expect: "@pen.dev/cli" },
        Case { label: "not logged in (error)", with_pen: true, status_mode: Some("exit1"), expect: "pen login" },
        Case { label: "not logged in (message)", with_pen: true, status_mode: Some("logged-out"), expect: "pen login" },
    ];
    for case in cases {
        let cli = PenCli::new();
        let empty = std::env::temp_dir().join(format!("forge-pen-empty-{}", crate::architecture::identity()));
        fs::create_dir_all(&empty).unwrap();
        let search_path = if case.with_pen {
            if let Some(mode) = case.status_mode { cli.set_status_mode(mode); }
            cli.direct_search_path()
        } else {
            OsString::from(&empty)
        };

        let f = Fixture::new();
        f.set("test_pen_search_path", json!(search_path.to_string_lossy()));
        f.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
        f.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
            "Create docs/features/demo/design/screen.pen using pen interactive.",
            "docs/features/demo/design/screen.pen exists and its PNG matches")]);
        f.set("mock_edits", json!([{"docs/features/demo/design/screen.pen": design_json(&[("f1", "Main")])}]));
        let head_before = f.ctx.git(&["rev-parse", "HEAD"]).unwrap();

        f.ctx.run_worker();

        let plan = f.plan();
        assert_ne!(plan["stages"][0]["status"], "committed", "case {}: {}", case.label, f.ctx.read_history());
        let head_after = f.ctx.git(&["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head_before, head_after, "case {}: HEAD must stay unchanged when pen blocks the run", case.label);
        let status = f.ctx.git(&["status", "--porcelain"]).unwrap();
        assert!(status.contains("screen.pen"), "case {}: .pen work must stay in the worktree: {status}", case.label);
        assert_eq!(f.ctx.session.state.lock().unwrap().phase, "blocked", "case {}", case.label);
        let history = f.ctx.read_history().to_string();
        assert!(history.contains(case.expect), "case {}: block message must mention the fix: {history}", case.label);
        assert!(f.reviewer_prompts().is_empty(), "case {}: no reviewer must run when pen blocks", case.label);
        if case.with_pen {
            assert!(
                cli.calls().iter().all(|c| c[0] == "status"),
                "case {}: only `pen status` may be invoked: {:?}", case.label, cli.calls()
            );
        }
        let _ = fs::remove_dir_all(&empty);
    }
}

#[test]
#[ignore = "M4 pending: S25 not implemented yet"]
fn s25_failed_export_returns_to_agent() {
    // S25: A failed export goes back to the agent
    let cli = PenCli::new();
    let f = Fixture::new();
    f.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    f.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    f.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
        "Create docs/features/demo/design/screen.pen using pen interactive.",
        "docs/features/demo/design/screen.pen exists and its PNG matches")]);
    f.set("mock_edits", json!([
        {"docs/features/demo/design/screen.pen": "not valid pen json"},
        {"docs/features/demo/design/screen.pen": design_json(&[("f1", "Main")])},
    ]));
    f.ctx.run_worker();

    let fixer_prompts = f.fixer_prompts();
    assert!(
        fixer_prompts.iter().any(|p| p.contains("cannot open") && p.contains("docs/features/demo/design/screen.pen")),
        "the fixer must receive the export failure like a failing check: {fixer_prompts:?}"
    );
    let plan = f.plan();
    assert_eq!(plan["stages"][0]["status"], "committed", "{}", f.ctx.read_history());
    let sha = plan["stages"][0]["sha"].as_str().unwrap().to_string();
    let changed = f.ctx.git(&["show", "--name-only", "--pretty=format:", &sha]).unwrap();
    assert!(changed.contains("docs/features/demo/design/screen.png"), "{changed}");

    // An export that keeps failing through the whole fix budget blocks the
    // commit rather than letting a broken design through.
    let cli2 = PenCli::new();
    let f2 = Fixture::new();
    f2.set("max_fix_rounds", json!(1));
    f2.set("test_pen_search_path", json!(cli2.direct_search_path().to_string_lossy()));
    f2.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    f2.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
        "Create docs/features/demo/design/screen.pen using pen interactive.",
        "docs/features/demo/design/screen.pen exists and its PNG matches")]);
    f2.set("mock_edits", json!([
        {"docs/features/demo/design/screen.pen": "still not valid pen json"},
        {"docs/features/demo/design/screen.pen": "still not valid pen json either"},
    ]));
    f2.ctx.run_worker();
    let plan2 = f2.plan();
    assert_ne!(plan2["stages"][0]["status"], "committed", "{}", f2.ctx.read_history());
}

#[test]
fn s26_reviewers_review_designs_through_pngs() {
    // S26: Reviewers review designs through the exported PNGs

    // Stage-level reviewer, run alongside a non-design stage for contrast.
    // Export wiring lands in a later stage, so the PNG is constructed here
    // as part of the implementer edit, the same way a later stage's engine
    // export will place it before the reviewer runs.
    let cli = PenCli::new();
    let f = Fixture::new();
    f.set("test_pen_search_path", json!(cli.direct_search_path().to_string_lossy()));
    f.set("mock_routing_planner_outputs", json!([{"proposals": [
        Fixture::proposal(1, "claude", "other"),
        Fixture::proposal(2, "claude", "other"),
    ]}]));
    f.publish_ready("Improve greeting and add a mockup", vec![
        Fixture::stage(1, "Fix greeting", "Append a friendly line to README.md.", "README.md has the friendly line"),
        Fixture::stage(2, "Add screen mockup", "Create docs/features/demo/design/screen.pen using pen interactive.",
            "docs/features/demo/design/screen.pen exists and its PNG matches"),
    ]);
    let screen_pen = design_json(&[("f1", "Main")]);
    let screen_png = String::from_utf8(expected_png(screen_pen.as_bytes(), "f1")).unwrap();
    f.set("mock_edits", json!([
        {"README.md": "Hello, friendly world!\n"},
        {"docs/features/demo/design/screen.pen": screen_pen, "docs/features/demo/design/screen.png": screen_png},
    ]));
    f.ctx.run_worker();

    let plan = f.plan();
    assert_eq!(plan["status"], "done", "{}", f.ctx.read_history());

    let reviewer_prompts = f.reviewer_prompts();
    let design_review = reviewer_prompts.iter().find(|p| p.contains("STAGE: Add screen mockup"))
        .expect("a stage reviewer prompt for the design stage must exist");
    let plain_review = reviewer_prompts.iter().find(|p| p.contains("STAGE: Fix greeting"))
        .expect("a stage reviewer prompt for the non-design stage must exist");

    for text in ["docs/features/demo/design/screen.pen", "docs/features/demo/design/screen.png", "not required to run"] {
        assert!(design_review.contains(text), "stage reviewer prompt missing {text:?}: {design_review}");
    }
    assert!(!design_review.contains("pen interactive --in"), "reviewers must not get editing instructions: {design_review}");
    for text in ["docs/features/demo/design/screen.pen", "docs/features/demo/design/screen.png"] {
        assert!(!plain_review.contains(text), "a non-design review must not mention designs: {plain_review}");
    }

    // Plan-level reviewer, deferred to the end by a per-plan reviewer cadence.
    let plan_cli = PenCli::new();
    let plan_fixture = Fixture::new();
    plan_fixture.set("review_cadence", json!({"architect": "per_stage", "reviewer": "per_plan"}));
    plan_fixture.set("test_pen_search_path", json!(plan_cli.direct_search_path().to_string_lossy()));
    plan_fixture.set("mock_routing_planner_outputs", json!([{"proposals": [Fixture::proposal(1, "claude", "other")]}]));
    plan_fixture.publish_ready("Add a screen mockup", vec![Fixture::stage(1, "Add screen mockup",
        "Create docs/features/demo/design/screen.pen using pen interactive.",
        "docs/features/demo/design/screen.pen exists and its PNG matches")]);
    let plan_screen_pen = design_json(&[("f1", "Main")]);
    let plan_screen_png = String::from_utf8(expected_png(plan_screen_pen.as_bytes(), "f1")).unwrap();
    plan_fixture.set("mock_edits", json!([
        {"docs/features/demo/design/screen.pen": plan_screen_pen, "docs/features/demo/design/screen.png": plan_screen_png},
    ]));
    plan_fixture.ctx.run_worker();
    let plan2 = plan_fixture.plan();
    assert_eq!(plan2["status"], "done", "{}", plan_fixture.ctx.read_history());

    let plan_review_prompts = plan_fixture.reviewer_prompts();
    let plan_review = plan_review_prompts.iter().find(|p| p.contains("COMMIT RANGE"))
        .expect("a plan reviewer prompt must exist");
    for text in ["docs/features/demo/design/screen.pen", "docs/features/demo/design/screen.png", "not required to run"] {
        assert!(plan_review.contains(text), "plan reviewer prompt missing {text:?}: {plan_review}");
    }
    assert!(!plan_review.contains("pen interactive --in"), "reviewers must not get editing instructions: {plan_review}");
}

// ------------------------------------------------------ pen module units
// Focused unit tests for src/pen.rs; not scenario tests.

use crate::pen::ExportError;

/// A temporary git repository with one initial commit, removed on Drop.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("forge-pen-repo-{}", crate::architecture::identity()));
        fs::create_dir_all(&root).unwrap();
        let repo = Self { root };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Fixture"]);
        repo.git(&["config", "user.email", "fixture@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        fs::write(repo.root.join("README.md"), "demo\n").unwrap();
        for (path, content) in files {
            repo.write(path, content);
        }
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", "initial"]);
        repo
    }

    fn write(&self, path: &str, content: &str) {
        let full = self.root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git").args(args).current_dir(&self.root).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn empty_search_dir() -> (PathBuf, OsString) {
    let dir = std::env::temp_dir().join(format!("forge-pen-empty-{}", crate::architecture::identity()));
    fs::create_dir_all(&dir).unwrap();
    let path = OsString::from(&dir);
    (dir, path)
}

#[test]
fn pen_references_designs_matches_pen_files_and_design_folders() {
    for text in ["x.pen", "see design/screen.pen.", "docs/features/a/design/", "design/ first", "(ui.pen)"] {
        assert!(crate::pen::references_designs(text), "{text}");
    }
    for text in ["open file", "redesign/ the API", "pending", "notes.pens", "my_design/ folder", "design folder"] {
        assert!(!crate::pen::references_designs(text), "{text}");
    }
    assert!(crate::pen::stage_references_designs(&json!({"instructions": "Tidy up.", "acceptance": "screen.pen exported"})));
    assert!(crate::pen::stage_references_designs(&json!({"instructions": "Add docs/features/x/design/ mockups"})));
    assert!(!crate::pen::stage_references_designs(&json!({"instructions": "Tidy up.", "acceptance": "Done"})));
}

#[test]
fn pen_frame_slug_replaces_each_non_alphanumeric_character() {
    assert_eq!(crate::pen::frame_slug("Main  Screen"), "main--screen");
    assert_eq!(crate::pen::frame_slug("A1_b"), "a1-b");
    assert_eq!(crate::pen::frame_slug("Café!"), "caf--");
    assert_eq!(crate::pen::frame_slug(""), "");
}

#[test]
fn pen_find_pen_uses_only_the_explicit_search_path() {
    let (dir, empty) = empty_search_dir();
    assert_eq!(crate::pen::find_pen(&empty), None);
    fs::write(dir.join("pen"), "not executable").unwrap();
    assert_eq!(crate::pen::find_pen(&empty), None, "a non-executable file is not pen");

    let cli = PenCli::new();
    let joined = std::env::join_paths([dir.clone(), PathBuf::from(cli.direct_search_path())]).unwrap();
    assert_eq!(crate::pen::find_pen(&joined), Some(PathBuf::from(cli.direct_search_path()).join("pen")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pen_editing_instructions_are_provider_neutral_and_stable() {
    // editing_instructions takes no provider argument, so the same resolved
    // skill always yields byte-identical text for both providers.
    let cli = PenCli::new();
    let skill = crate::pen::resolve_skill(&cli.direct_search_path());
    let claude = crate::pen::editing_instructions(skill.as_deref());
    let codex = crate::pen::editing_instructions(skill.as_deref());
    assert_eq!(claude, codex, "the editing section must not depend on the caller");
    for text in ["pen interactive --in", "--out", "execute(", "save()", "exit()"] {
        assert!(claude.contains(text), "{text}: {claude}");
    }
    assert!(claude.contains(&skill.unwrap().display().to_string()));
    assert!(claude.contains("or any MCP server"), "{claude}");

    let none = crate::pen::editing_instructions(None);
    assert!(none.contains("@pen.dev/cli"), "{none}");
    assert!(none.contains("dist/out/skills/pen-dev/SKILL.md"), "{none}");
}

#[test]
fn pen_reviewer_instructions_list_designs_without_editing_instructions() {
    let section = crate::pen::reviewer_instructions(&[
        ("docs/features/demo/design/screen.pen".to_string(), vec!["docs/features/demo/design/screen.png".to_string()]),
    ]);
    for text in ["docs/features/demo/design/screen.pen", "docs/features/demo/design/screen.png", "not required to run"] {
        assert!(section.contains(text), "{text}: {section}");
    }
    assert!(!section.contains("pen interactive --in"), "{section}");
    assert_eq!(crate::pen::reviewer_instructions(&[]), "");
}

#[test]
fn pen_resolve_skill_follows_direct_symlink_without_running_pen() {
    let cli = PenCli::new();
    let skill = crate::pen::resolve_skill(&cli.direct_search_path());
    assert_eq!(skill, Some(fs::canonicalize(cli.skill_path()).unwrap()));
    assert!(cli.calls().is_empty(), "pen must not run: {:?}", cli.calls());
}

#[test]
fn pen_resolve_skill_follows_mise_shim_without_running_pen() {
    let cli = PenCli::new();
    let skill = crate::pen::resolve_skill(&cli.mise_search_path());
    assert_eq!(skill, Some(fs::canonicalize(cli.skill_path()).unwrap()));
    assert!(cli.calls().is_empty(), "pen must not run: {:?}", cli.calls());
}

#[test]
fn pen_resolve_skill_returns_none_without_skill_file() {
    let cli = PenCli::new();
    fs::remove_file(cli.skill_path()).unwrap();
    assert_eq!(crate::pen::resolve_skill(&cli.direct_search_path()), None);
    assert_eq!(crate::pen::resolve_skill(&cli.mise_search_path()), None);
    let (dir, empty) = empty_search_dir();
    assert_eq!(crate::pen::resolve_skill(&empty), None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pen_resolve_skill_falls_back_to_package_root() {
    // An entry point nested below dist/ finds the skill via package.json.
    let root = std::env::temp_dir().join(format!("forge-pen-pkg-{}", crate::architecture::identity()));
    let pkg = root.join("lib/@pen.dev/cli");
    fs::create_dir_all(pkg.join("dist/bin")).unwrap();
    fs::create_dir_all(pkg.join("dist/out/skills/pen-dev")).unwrap();
    fs::write(pkg.join("package.json"), r#"{"name": "@pen.dev/cli"}"#).unwrap();
    fs::write(pkg.join("dist/out/skills/pen-dev/SKILL.md"), "skill").unwrap();
    let entry = pkg.join("dist/bin/index.mjs");
    fs::write(&entry, "#!/bin/false\n").unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    std::os::unix::fs::symlink(&entry, root.join("bin/pen")).unwrap();

    let skill = crate::pen::resolve_skill(root.join("bin").as_os_str());
    assert_eq!(skill, Some(fs::canonicalize(pkg.join("dist/out/skills/pen-dev/SKILL.md")).unwrap()));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn pen_changed_pen_files_lists_modified_added_and_untracked_designs() {
    let repo = Repo::new(&[("a.pen", "{}"), ("b.pen", "{}"), ("c.pen", "{}"), ("old.pen", "{}")]);
    let base = repo.head();
    repo.write("a.pen", "{\"changed\": true}");
    fs::remove_file(repo.root.join("b.pen")).unwrap();
    repo.write("design/d.pen", "{}");
    repo.write("e.pen", "{}");
    repo.git(&["add", "e.pen"]);
    repo.git(&["mv", "old.pen", "new.pen"]);
    repo.write(".forge/state.pen", "{}");
    repo.write("README.md", "changed\n");
    repo.write("notes.pen.txt", "x");

    let files = crate::pen::changed_pen_files(&repo.root, &base).unwrap();
    assert_eq!(files, vec!["a.pen", "design/d.pen", "e.pen", "new.pen"]);

    // A committed change relative to an older base is still listed.
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-qm", "next"]);
    let files = crate::pen::changed_pen_files(&repo.root, &base).unwrap();
    assert_eq!(files, vec!["a.pen", "design/d.pen", "e.pen", "new.pen"]);
    assert!(crate::pen::changed_pen_files(&repo.root, "HEAD").unwrap().is_empty());
    assert!(crate::pen::changed_pen_files(&repo.root, "not-a-commit").is_err());
}

#[test]
fn pen_export_changed_without_pen_changes_spawns_nothing() {
    let cli = PenCli::new();
    let repo = Repo::new(&[("design/screen.pen", &design_json(&[("f1", "Main")]))]);
    repo.write("README.md", "changed\n");
    let out = crate::pen::export_changed(&repo.root, &repo.head(), &cli.direct_search_path()).unwrap();
    assert!(out.is_empty());
    assert!(cli.calls().is_empty(), "no process may run: {:?}", cli.calls());
    let (dir, empty) = empty_search_dir();
    assert!(crate::pen::export_changed(&repo.root, &repo.head(), &empty).unwrap().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pen_export_changed_reports_unavailable_pen_actionably() {
    let repo = Repo::new(&[]);
    let base = repo.head();
    repo.write("design/screen.pen", &design_json(&[("f1", "Main")]));

    let (dir, empty) = empty_search_dir();
    match crate::pen::export_changed(&repo.root, &base, &empty) {
        Err(ExportError::Unavailable(message)) => assert!(message.contains("npm install -g @pen.dev/cli"), "{message}"),
        other => panic!("expected Unavailable, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);

    for mode in ["exit1", "logged-out"] {
        let cli = PenCli::new();
        cli.set_status_mode(mode);
        match crate::pen::export_changed(&repo.root, &base, &cli.direct_search_path()) {
            Err(ExportError::Unavailable(message)) => assert!(message.contains("pen login"), "{mode}: {message}"),
            other => panic!("{mode}: expected Unavailable, got {other:?}"),
        }
        assert_eq!(cli.calls(), vec![json!(["status"])], "{mode}: only pen status may run");
    }
    assert!(!repo.root.join("design/screen.png").exists());
}

#[test]
fn pen_export_changed_fails_for_unreadable_design_naming_the_file() {
    let cli = PenCli::new();
    let repo = Repo::new(&[]);
    let base = repo.head();
    repo.write("design/broken.pen", "not valid pen json");
    repo.write("design/good.pen", &design_json(&[("f1", "Main")]));
    match crate::pen::export_changed(&repo.root, &base, &cli.direct_search_path()) {
        Err(ExportError::Failed(message)) => {
            assert!(message.contains("design/broken.pen"), "{message}");
            assert!(message.contains("cannot open"), "{message}");
            assert!(!message.contains("design/good.pen"), "{message}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(repo.root.join("design/broken.pen")).unwrap(), "not valid pen json");
    assert!(!repo.root.join("design/broken.png").exists());
}

#[test]
fn pen_export_design_rejects_missing_or_unusable_frames() {
    let cli = PenCli::new();
    let repo = Repo::new(&[]);
    let search_path = cli.direct_search_path();
    for (frames, expected) in [
        (vec![], "no top-level frames"),
        (vec![("a/b", "Main")], "invalid frame id"),
        (vec![("f1", "A B"), ("f2", "A-B")], "same file"),
        (vec![("f1", "Main"), ("f1", "Other")], "duplicate frame id"),
    ] {
        repo.write("design/screen.pen", &design_json(&frames));
        match crate::pen::export_design(&repo.root, "design/screen.pen", &search_path) {
            Err(ExportError::Failed(message)) => {
                assert!(message.contains("design/screen.pen"), "{message}");
                assert!(message.contains(expected), "{expected}: {message}");
            }
            other => panic!("{expected}: expected Failed, got {other:?}"),
        }
    }
    assert!(fs::read_dir(repo.root.join("design")).unwrap().all(|e| {
        !e.unwrap().file_name().to_string_lossy().ends_with(".png")
    }));
}

#[test]
fn pen_export_design_removes_only_its_own_stale_pngs() {
    let cli = PenCli::new();
    let pen_json = design_json(&[("f1", "Main Screen"), ("f2", "Settings")]);
    let repo = Repo::new(&[]);
    repo.write("design/screen.pen", &pen_json);
    for (name, content) in [
        ("screen.png", "stale single"),
        ("screen.old-frame.png", "stale frame"),
        ("screen.extra.png", "other design"),
        ("screen.extra.pen", "{}"),
        ("screen.Upper.png", "not an export name"),
        ("screen.png.bak", "backup"),
        ("other.png", "unrelated"),
        ("screens.png", "different stem"),
    ] {
        repo.write(&format!("design/{name}"), content);
    }

    let mut out = crate::pen::export_design(&repo.root, "design/screen.pen", &cli.direct_search_path()).unwrap();
    out.sort();
    assert_eq!(out, vec!["design/screen.main-screen.png", "design/screen.settings.png"]);
    assert_eq!(fs::read_to_string(repo.root.join("design/screen.pen")).unwrap(), pen_json, "source must be untouched");
    assert_eq!(fs::read(repo.root.join("design/screen.settings.png")).unwrap(), expected_png(pen_json.as_bytes(), "f2"));

    let mut remaining: Vec<String> = fs::read_dir(repo.root.join("design")).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    remaining.sort();
    assert_eq!(remaining, vec![
        "other.png", "screen.Upper.png", "screen.extra.pen", "screen.extra.png", "screen.main-screen.png",
        "screen.pen", "screen.png.bak", "screen.settings.png", "screens.png",
    ]);
    let interactive: Vec<_> = cli.calls().into_iter().filter(|c| c[0] == "interactive").collect();
    assert_eq!(interactive.len(), 2, "one listing and one export session");
    for call in interactive {
        assert_ne!(call[4], json!(repo.root.join("design/screen.pen").to_string_lossy()), "--out must not be the source");
    }
}
