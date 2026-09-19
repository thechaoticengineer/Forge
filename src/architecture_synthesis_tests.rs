//! Storage, validation, survival and derived current work of the project
//! synthesis. Role injection across a whole plan is covered in growth_tests.rs.
use super::*;
use crate::architecture::{Store, checkpoint_default};
use crate::test_support::QueueTest;
use std::fs;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-synthesis-{}", crate::durable_json::identity()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn source() -> Value {
    json!({"plan_id": "plan-1", "checkpoint": "token-1", "sha": "0123abcd", "goal": "Ship the greeting service", "unix": 10})
}

fn texts(prefix: &str, count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("{prefix} {n} holds")).collect()
}

fn output(constraints: &[String], interfaces: &[String], decisions: &[String]) -> Value {
    json!({"constraints": constraints, "interfaces": interfaces, "decisions": decisions, "retired": []})
}

fn doc(constraints: &[String], interfaces: &[String], decisions: &[String]) -> Result<Value, String> {
    build(&output(constraints, interfaces, decisions), None, source())
}

fn plan() -> Value {
    json!({"goal": "goal", "stages": [{"id": 1, "title": "one", "instructions": "work", "acceptance": "test", "commit": "feat: one"}]})
}

fn base() -> Value {
    doc(&texts("constraint", 2), &texts("interface", 1), &texts("decision", 1)).unwrap()
}

#[test]
fn ids_are_stable_kind_prefixed_content_hashes() {
    let first = base();
    assert_eq!(first, base(), "the same output must derive the same document");
    let text = first["constraints"][0]["text"].as_str().unwrap();
    assert_eq!(first["constraints"][0]["id"], crate::prompt_view::constraint_id(text), "constraints share the prompt constraint IDs");
    assert!(first["interfaces"][0]["id"].as_str().unwrap().starts_with("i-"));
    assert!(first["decisions"][0]["id"].as_str().unwrap().starts_with("d-"));
    assert_ne!(first["constraints"][0]["id"], first["constraints"][1]["id"]);
    // The same text in two kinds is two distinct entries.
    let same = vec!["shared text".to_string()];
    let both = doc(&same, &same, &[]).unwrap();
    assert_ne!(both["constraints"][0]["id"], both["interfaces"][0]["id"]);
    // An ID the engine did not derive from the text is rejected.
    let mut forged = base();
    forged["constraints"][0]["id"] = json!("c-00000000");
    assert!(validate(&forged, None).unwrap_err().contains("engine-derived ID"));
}

#[test]
fn per_kind_counts_are_limited() {
    for (kind, max) in [("constraints", MAX_CONSTRAINTS), ("interfaces", MAX_INTERFACES), ("decisions", MAX_DECISIONS)] {
        let entries = |n| texts("e", n);
        let make = |n: usize| {
            let mut out = output(&[], &[], &[]);
            out[kind] = json!(entries(n));
            build(&out, None, source())
        };
        assert!(make(max).is_ok(), "{max} {kind} must be accepted: {:?}", make(max).err());
        let error = make(max + 1).unwrap_err();
        assert!(error.contains(&format!("over the limit of {max}")), "{kind}: {error}");
    }
}

#[test]
fn entry_texts_are_limited_to_300_bytes_and_non_empty() {
    // 150 two-byte characters are exactly 300 bytes.
    let at_limit = "é".repeat(150);
    assert!(doc(std::slice::from_ref(&at_limit), &[], &[]).is_ok());
    let error = doc(&[format!("{at_limit}a")], &[], &[]).unwrap_err();
    assert!(error.contains("301 bytes, over the 300-byte limit"), "{error}");
    assert!(doc(&[], &["  ".into()], &[]).unwrap_err().contains("empty"));
    let duplicate = vec!["same".to_string(), "same".to_string()];
    assert!(doc(&duplicate, &[], &[]).unwrap_err().contains("duplicate synthesis ID"));
}

#[test]
fn the_serialized_document_is_limited_to_8192_bytes() {
    let long = |prefix: &str, n: usize| -> Vec<String> { (0..n).map(|i| format!("{prefix}{i:02}{}", "x".repeat(280))).collect() };
    // Every count is within its limit; only the total size is not.
    let error = doc(&long("c", 20), &long("i", 10), &[]).unwrap_err();
    assert!(error.contains("over the 8192-byte limit"), "{error}");
    let mut fits = 0;
    while doc(&long("c", fits + 1), &[], &[]).is_ok() { fits += 1; }
    let largest = doc(&long("c", fits), &[], &[]).unwrap();
    let bytes = serde_json::to_vec_pretty(&largest).unwrap().len();
    assert!(bytes <= MAX_BYTES && bytes > MAX_BYTES - 400, "the largest accepted document is {bytes} bytes");
}

#[test]
fn the_source_names_a_plan_and_a_sha() {
    for (key, value) in [("plan_id", Value::Null), ("sha", Value::Null), ("sha", json!("")), ("plan_id", json!("synthesis.json")),
        ("unix", json!("now")), ("checkpoint", json!(3))] {
        let mut source = source();
        source[key] = value.clone();
        assert!(build(&output(&[], &[], &[]), None, source).is_err(), "source {key} = {value} must be rejected");
    }
    let mut unknown = base();
    unknown["extra"] = json!(1);
    assert!(validate(&unknown, None).unwrap_err().contains("unknown field"));
    let mut version = base();
    version["version"] = json!(2);
    assert!(validate(&version, None).is_err());
}

#[test]
fn old_entries_are_kept_byte_for_byte_or_retired_with_a_reason() {
    let old = base();
    let (kept, dropped) = (old["constraints"][0]["text"].as_str().unwrap().to_string(), old["constraints"][1]["id"].clone());
    let rest = |constraints: Vec<String>| output(&constraints, &texts("interface", 1), &texts("decision", 1));

    // Dropped without retirement.
    let error = build(&rest(vec![kept.clone()]), Some(&old), source()).unwrap_err();
    assert!(error.contains("dropped without being retired"), "{error}");

    // Retired with a reason.
    let mut retiring = rest(vec![kept.clone()]);
    retiring["retired"] = json!([{"id": dropped, "reason": "stage 2 replaced it"}]);
    let replacement = build(&retiring, Some(&old), source()).unwrap();
    assert_eq!(replacement["retired"][0]["text"], old["constraints"][1]["text"], "a retirement copies the old text");
    assert_eq!(replacement["constraints"][0], old["constraints"][0], "a kept entry keeps its ID and text");

    // An empty reason is no retirement.
    retiring["retired"][0]["reason"] = json!(" ");
    assert!(build(&retiring, Some(&old), source()).is_err());

    // Changed text is a drop plus an add, so the old ID needs retiring.
    let changed = vec![format!("{kept} (revised)"), old["constraints"][1]["text"].as_str().unwrap().into()];
    let error = build(&rest(changed.clone()), Some(&old), source()).unwrap_err();
    assert!(error.contains(old["constraints"][0]["id"].as_str().unwrap()), "{error}");
    let mut revised = rest(changed);
    revised["retired"] = json!([{"id": old["constraints"][0]["id"], "reason": "reworded"}]);
    assert!(build(&revised, Some(&old), source()).is_ok());

    // Retirements name only previous active entries, never a kept one.
    let mut unknown = rest(texts("constraint", 2));
    unknown["retired"] = json!([{"id": "c-00000000", "reason": "unknown"}]);
    assert!(build(&unknown, Some(&old), source()).unwrap_err().contains("not an entry of the previous synthesis"));
    let mut both = replacement.clone();
    both["retired"] = json!([{"id": old["constraints"][0]["id"], "text": kept, "reason": "still active"}]);
    assert!(validate(&both, Some(&old)).unwrap_err().contains("both active and retired"));
    // Without a previous document there is nothing to retire.
    assert!(validate(&replacement, None).is_err());

    // `retired` covers only the latest replacement: the next one drops it.
    let next = build(&rest(vec![kept]), Some(&replacement), source()).unwrap();
    assert_eq!(next["retired"], json!([]));
}

#[test]
fn load_and_save_validate_the_stored_file() {
    let temp = Temp::new();
    assert_eq!(load(&temp.0).unwrap(), None);
    let first = base();
    save(&temp.0, &first).unwrap();
    assert_eq!(path(&temp.0), temp.0.join("architecture").join("synthesis.json"));
    assert_eq!(load(&temp.0).unwrap(), Some(first.clone()));

    // An invalid replacement leaves the stored bytes unchanged.
    let bytes = fs::read(path(&temp.0)).unwrap();
    let dropped = doc(&texts("constraint", 1), &texts("interface", 1), &texts("decision", 1)).unwrap();
    assert!(save(&temp.0, &dropped).unwrap_err().contains("dropped without being retired"));
    let mut oversized = first.clone();
    oversized["constraints"][0]["text"] = json!("x".repeat(400));
    assert!(save(&temp.0, &oversized).is_err());
    assert_eq!(fs::read(path(&temp.0)).unwrap(), bytes);

    // An invalid stored file is an error, never silently empty.
    fs::write(path(&temp.0), b"{\"version\":1}").unwrap();
    assert!(load(&temp.0).is_err());
    fs::write(path(&temp.0), b"{").unwrap();
    assert!(load(&temp.0).is_err());
}

#[test]
fn the_file_name_can_never_be_a_plan_directory() {
    assert!(!crate::durable_json::safe_id(FILE), "{FILE} must be rejected as a plan identity");
    let temp = Temp::new();
    let mut plan = plan();
    plan["plan_id"] = json!(FILE);
    let error = Store::new(temp.0.clone()).publish(plan, checkpoint_default(), json!({"kind": "created"})).unwrap_err();
    assert!(error.contains("invalid plan identity"), "{error}");
    assert!(!path(&temp.0).exists());
}

#[test]
fn the_synthesis_survives_reset_and_archiving_of_a_plan_identity() {
    let temp = Temp::new();
    let store = Store::new(temp.0.clone());
    let first = store.publish(plan(), checkpoint_default(), json!({"kind": "created"})).unwrap();
    save(&temp.0, &base()).unwrap();
    let bytes = fs::read(path(&temp.0)).unwrap();

    // Publishing a new plan identity archives the previous one.
    let mut next = plan();
    next["goal"] = json!("another goal");
    let second = store.publish(next, checkpoint_default(), json!({"kind": "created"})).unwrap();
    assert_ne!(first["plan_id"], second["plan_id"]);
    let archived = temp.0.join("architecture").join(first["plan_id"].as_str().unwrap()).join("archived.json");
    assert!(archived.exists(), "the first plan must be archived");
    assert_eq!(fs::read(path(&temp.0)).unwrap(), bytes, "archiving must not touch the synthesis");

    store.reset().unwrap();
    assert!(store.load().unwrap().is_none(), "reset removes the active plan");
    assert_eq!(fs::read(path(&temp.0)).unwrap(), bytes, "reset must not touch the synthesis");
    assert_eq!(load(&temp.0).unwrap(), Some(base()));
}

// ---------------------------------------------------------------- current work

fn write_feature(root: &Path, slug: &str, milestones: &str) {
    let dir = root.join("docs/features").join(slug);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), format!("# Feature {slug}\n")).unwrap();
    fs::write(dir.join("scenarios.md"), "## S1: One\n").unwrap();
    fs::write(dir.join("decisions.md"), "## D1: A decision\n").unwrap();
    fs::write(dir.join("milestones.md"), milestones).unwrap();
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() { out.extend(files(&path)); } else { out.push(path); }
    }
    out.sort();
    out
}

#[test]
fn current_work_stays_within_its_cap_with_omitted_counts() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    let mut queue = json!({"items": []});
    for n in 0..30 {
        crate::plan::mutate_queue(&mut queue, "add", &json!({"goal": format!("goal {n} {}", "q".repeat(400))})).unwrap();
    }
    ctx.save_queue(&queue);
    for n in 0..12 {
        write_feature(&test.path, &format!("feature-{n:02}"), "## M1: First\n\nStatus: implemented\nCovers: S1\n\n## M2: Second\n\nCovers: S1\n");
    }
    let stages: Vec<Value> = (1..=60).map(|id| json!({"id": id, "title": format!("{id} {}", "t".repeat(500)), "status": "pending"})).collect();
    let plan = json!({"plan_id": "plan-1", "goal": "g".repeat(1000), "status": "approved", "stages": stages});

    let before = files(&test.path.join(".forge"));
    let work = current_work(ctx, Some(&plan));
    assert_eq!(files(&test.path.join(".forge")), before, "current work is never stored");
    let bytes = serde_json::to_vec(&work).unwrap().len();
    assert!(bytes <= CURRENT_WORK_MAX_BYTES, "current work is {bytes} bytes");
    assert_eq!(work["plan"]["goal"].as_str().unwrap().chars().count(), 300);
    let count = |key: &str| work[key].as_array().unwrap().len() as u64;
    assert!(count("queue") <= MAX_QUEUE_ITEMS as u64 && count("features") <= MAX_FEATURES as u64);
    assert_eq!(count("queue") + work["omitted"]["queue"].as_u64().unwrap(), 30);
    assert_eq!(count("features") + work["omitted"]["features"].as_u64().unwrap(), 12);
    let stages = work["plan"]["stages"].as_array().unwrap();
    assert_eq!(stages.len() as u64 + work["omitted"]["stages"].as_u64().unwrap(), 60);
    assert!(work["omitted"]["stages"].as_u64().unwrap() > 0, "sixty long stages cannot all fit");
    for stage in stages { assert!(stage["title"].as_str().unwrap().chars().count() <= 120); }
    for item in work["queue"].as_array().unwrap() { assert!(item["goal"].as_str().unwrap().chars().count() <= 200); }

    // Multi-byte text is cut at character boundaries and still fits.
    let wide = json!({"plan_id": "plan-1", "goal": "\u{1F600}\u{0}".repeat(400), "status": "approved",
        "stages": (1..=40).map(|id| json!({"id": id, "title": "\u{0}é".repeat(200), "status": "pending"})).collect::<Vec<_>>()});
    let work = current_work(ctx, Some(&wide));
    assert!(serde_json::to_vec(&work).unwrap().len() <= CURRENT_WORK_MAX_BYTES);

    // Derived on every call: a queue change shows up in the next read.
    ctx.save_queue(&json!({"items": [{"id": 7, "goal": "only goal", "status": "queued"}]}));
    let work = current_work(ctx, None);
    assert_eq!(work["queue"], json!([{"id": 7, "goal": "only goal", "status": "queued"}]));
    assert_eq!(work["plan"], Value::Null);
}

#[test]
fn features_in_progress_come_from_milestones_and_plan_links_once_each() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    write_feature(&test.path, "done", "## M1: First\n\nStatus: implemented\nCovers: S1\n");
    write_feature(&test.path, "fresh", "## M1: First\n\nCovers: S1\n");
    write_feature(&test.path, "linked", "## M1: First\n\nCovers: S1\n\n## M2: Second\n\nCovers: S1\n");
    write_feature(&test.path, "partial", "## M1: First\n\nStatus: implemented\nCovers: S1\n\n## M2: Second\n\nCovers: S1\n");
    crate::feature_state::append_plan_link(ctx, &json!({"slug": "linked", "milestone": "M1", "title": "First"}), "goal").unwrap();
    crate::feature_state::append_plan_link(ctx, &json!({"slug": "linked", "milestone": "M2", "title": "Second"}), "goal").unwrap();
    crate::feature_state::set_plan_link_status(ctx, "linked", "M2", "planned", Some("plan-2"), None).unwrap();
    let plan = json!({"plan_id": "plan-2", "goal": "goal", "status": "approved", "stages": [],
        "feature": {"slug": "linked", "milestone": "M2", "title": "Second", "scenario_ids": ["S1"]}});

    let features = current_work(ctx, Some(&plan))["features"].clone();
    assert_eq!(features, json!([
        {"slug": "linked", "title": "Second", "milestone": "M2", "status": "current plan"},
        {"slug": "linked", "title": "Feature linked", "milestone": "M1", "status": "planning"},
        {"slug": "partial", "title": "Feature partial", "milestone": "M2", "status": "in progress"},
    ]), "implemented and unstarted features are not in progress");
}

// ---------------------------------------------------------------- role views

#[test]
fn role_views_omit_retired_entries_and_carry_the_advisory_note() {
    let test = QueueTest::new(false);
    let ctx = &test.app;
    assert_eq!(ctx.synthesis_section(), None, "no section without a stored synthesis");
    let context = ctx.synthesis_context(None);
    assert_eq!(context["what_holds"], Value::Null);
    assert!(context["current_work"].is_object() && context["note"].as_str().unwrap().contains("advisory"));

    let old = base();
    ctx.save_project_synthesis(&old).unwrap();
    let mut out = output(&texts("constraint", 1), &texts("interface", 1), &texts("decision", 1));
    out["retired"] = json!([{"id": old["constraints"][1]["id"], "reason": "no longer true"}]);
    let current = build(&out, Some(&old), source()).unwrap();
    ctx.save_project_synthesis(&current).unwrap();

    let section = ctx.synthesis_section().unwrap();
    assert!(section.contains("constraint 1 holds") && section.contains("commit 0123abcd") && section.contains("check every entry against the repository code"));
    assert!(!section.contains("constraint 2 holds") && !section.contains("retired") && !section.contains("current_work"), "{section}");
    let context = ctx.synthesis_context(None);
    assert_eq!(context["what_holds"], what_holds(&current));
    assert!(context["what_holds"].get("retired").is_none());

    // An invalid stored file never breaks a prompt: the section is left out.
    fs::write(path(&ctx.forge_path("")), b"{").unwrap();
    assert_eq!(ctx.synthesis_section(), None);
    assert_eq!(ctx.synthesis_context(None)["what_holds"], Value::Null);
}
