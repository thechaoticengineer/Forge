use super::*;
use serde_json::json;
use std::io::Write;

const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const TURN: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "forge-codex-session-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(root.join("sessions/2026/09/08")).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join(format!(
            "sessions/2026/09/08/rollout-fixture-{SESSION}.jsonl"
        ))
    }
    fn write(&self, events: &[Value]) {
        let mut file = File::options()
            .create(true)
            .append(true)
            .open(self.path())
            .unwrap();
        for event in events {
            writeln!(file, "{event}").unwrap();
        }
    }
    fn events(&self, model: &str) -> Vec<Value> {
        vec![
            json!({"type":"session_meta","payload":{"id":SESSION}}),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":TURN}}),
            json!({"type":"turn_context","payload":{"turn_id":TURN,"cwd":self.0,"model":model}}),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":TURN}}),
        ]
    }
    fn model(&self, snapshot: &Snapshot) -> Result<String, String> {
        snapshot.model(SESSION, self.0.to_str().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resumed_turn_uses_only_new_bytes_and_never_old_model_metadata() {
    let f = Fixture::new();
    f.write(&f.events("old-model"));
    let snapshot = Snapshot::capture(&f.0).unwrap();
    assert!(f.model(&snapshot).unwrap_err().contains("no completed"));
    f.write(&f.events("new-model")[1..]);
    assert_eq!(f.model(&snapshot).unwrap(), "new-model");
}

#[test]
fn metadata_identity_project_turn_and_completion_are_required() {
    for case in [
        "session",
        "project",
        "context-turn",
        "complete-turn",
        "missing-model",
        "invalid-model",
        "missing-start",
        "incomplete",
        "multiple-turns",
        "changed-model",
        "aborted",
    ] {
        let f = Fixture::new();
        let snapshot = Snapshot::capture(&f.0).unwrap();
        let mut events = f.events("exact-model");
        match case {
            "session" => events[0]["payload"]["id"] = json!(TURN),
            "project" => events[2]["payload"]["cwd"] = json!("/another-project"),
            "context-turn" => events[2]["payload"]["turn_id"] = json!(SESSION),
            "complete-turn" => events[3]["payload"]["turn_id"] = json!(SESSION),
            "missing-model" => events[2]["payload"]["model"] = Value::Null,
            "invalid-model" => events[2]["payload"]["model"] = json!("bad model\n"),
            "missing-start" => {
                events.remove(1);
            }
            "incomplete" => {
                events.pop();
            }
            "multiple-turns" => events.extend(f.events("exact-model")[1..].iter().cloned()),
            "changed-model" => events.insert(3, f.events("changed-model")[2].clone()),
            "aborted" => events.push(json!({"type":"event_msg","payload":{"type":"turn_aborted"}})),
            _ => unreachable!(),
        }
        f.write(&events);
        assert!(f.model(&snapshot).is_err(), "accepted {case}");
    }
}

#[test]
fn lookup_refuses_other_threads_duplicates_symlinks_and_replaced_files() {
    for case in [
        "other-thread",
        "duplicate",
        "symlink",
        "replaced",
        "truncated",
    ] {
        let f = Fixture::new();
        f.write(&f.events("old-model"));
        let snapshot = Snapshot::capture(&f.0).unwrap();
        let path = f.path();
        match case {
            "other-thread" => {
                fs::rename(
                    &path,
                    path.with_file_name(format!("rollout-other-{TURN}.jsonl")),
                )
                .unwrap();
            }
            "duplicate" => {
                fs::copy(
                    &path,
                    path.with_file_name(format!("rollout-other-{SESSION}.jsonl")),
                )
                .unwrap();
            }
            "symlink" => {
                let target = f.0.join("elsewhere");
                fs::rename(&path, &target).unwrap();
                std::os::unix::fs::symlink(target, &path).unwrap();
            }
            "replaced" => {
                fs::rename(&path, f.0.join("old-file")).unwrap();
                f.write(&f.events("new-model"));
            }
            "truncated" => fs::write(&path, "{}\n").unwrap(),
            _ => unreachable!(),
        }
        assert!(f.model(&snapshot).is_err(), "accepted {case}");
    }
}

#[test]
fn malformed_partial_and_oversized_metadata_fail_closed() {
    for case in ["malformed", "partial", "large-line", "large-append"] {
        let f = Fixture::new();
        let snapshot = Snapshot::capture(&f.0).unwrap();
        f.write(&f.events("exact-model"));
        let mut file = File::options().append(true).open(f.path()).unwrap();
        match case {
            "malformed" => writeln!(file, "not JSON").unwrap(),
            "partial" => write!(file, "{{}}").unwrap(),
            "large-line" => {
                file.write_all(&vec![b' '; MAX_LINE as usize]).unwrap();
                writeln!(file, "{{}}").unwrap();
            }
            "large-append" => file.set_len(MAX_APPEND + 1).unwrap(),
            _ => unreachable!(),
        }
        assert!(f.model(&snapshot).is_err(), "accepted {case}");
    }
}
