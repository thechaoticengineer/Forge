//! Application plan persistence over the existing architecture store.
use super::Ctx;
use crate::util::unix_timestamp;
use serde_json::{Value, json};

impl Ctx {
    pub(crate) fn architecture_store(&self) -> crate::architecture::Store {
        crate::architecture::Store::new(self.forge_path(""))
    }

    pub(crate) fn load_plan(&self) -> Option<Value> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        match self.architecture_store().load() {
            Ok(plan) => plan,
            Err(error) => {
                *self.session.persistence_error.lock().unwrap() = Some(error);
                None
            }
        }
    }

    pub(crate) fn save_plan(&self, plan: &Value) -> Result<(), String> {
        self.publish_plan(plan, false).map(|_| ())
    }

    pub(crate) fn publish_plan(&self, plan: &Value, replacement: bool) -> Result<Value, String> {
        let _guard = self.session.persistence_lock.lock().unwrap();
        let result = (|| {
            let store = self.architecture_store();
            let mut old = store.load()?;
            if let Some(legacy) = old.as_ref().filter(|p| p.get("architecture").is_none()) {
                old = Some(store.publish(legacy.clone(), crate::architecture::checkpoint_default(),
                    json!({"kind": "legacy_import"}))?);
            }
            let mut next = plan.clone();
            // Import legacy metadata on first mutation, never on a read.
            let same = !replacement && old.as_ref().is_some_and(|p|
                next["plan_id"].is_null() || next["plan_id"] == p["plan_id"]);
            if !replacement && old.is_some() && !same { return Err("plan identity does not match the active project plan".into()); }
            let mut cp = crate::architecture::checkpoint_default();
            let mut invalidations = Vec::new();
            if same {
                let old = old.as_ref().unwrap();
                if old.get("architecture").is_some() {
                    cp = store.checkpoint(old)?;
                    for key in ["plan_id", "contract_version", "architecture"] { next[key] = old[key].clone(); }
                    if next["revision"].is_null() { next["revision"] = old["revision"].clone(); }
                }
                let affected = crate::plan::affected_stages(old, &next).map_err(str::to_owned)?;
                if !affected.is_empty() && next["revision"] == old["revision"] {
                    next["revision"] = json!(old["revision"].as_u64().unwrap().checked_add(1).ok_or("revision limit reached")?);
                }
                if next["revision"] != old["revision"] {
                    // Legacy references cannot prove their mutable tails were committed.
                    if cp["session"]["resume_policy"] == "fork_from_checkpoint" { cp["context_status"] = json!("needs_recovery"); }
                    // Exact committed sessions remain usable after an engine-only edit.
                    for key in ["guidance", "agreements"] {
                        if let Some(records) = cp[key].as_object_mut() {
                            for (id, record) in records {
                                let valid = next["stages"].as_array().unwrap().iter().any(|s|
                                    s["id"].to_string() == *id && (s["status"] == "committed" || !affected.contains(&s["id"])));
                                if !valid && record.is_object() {
                                    let trigger = if old["goal"] != next["goal"] { "goal_changed" }
                                        else if !next["stages"].as_array().unwrap().iter().any(|s| s["id"].to_string() == *id) { "stage_removed" }
                                        else { "stage_or_dependency_changed" };
                                    invalidations.push(json!({"kind": key, "stage_id": id, "record_id": record["id"],
                                        "trigger": trigger, "previous_revision": old["revision"], "revision": next["revision"]}));
                                    record["valid"] = json!(false);
                                    record["invalidation_trigger"] = json!(trigger);
                                }
                            }
                        }
                    }
                }
            } else {
                for key in ["plan_id", "revision", "architecture", "contract_version"] { next.as_object_mut().ok_or("invalid plan")?.remove(key); }
            }
            let kind = if replacement { "plan_created" } else if old.as_ref().is_some_and(|p| p.get("architecture").is_none()) { "legacy_import" } else { "plan_saved" };
            let mut reviews = Vec::new();
            let mut removed = Vec::new();
            if let Some(old) = old.as_ref().filter(|_| same) {
                for stage in old["stages"].as_array().unwrap() {
                    if !next["stages"].as_array().unwrap().iter().any(|s| s["id"] == stage["id"]) {
                        removed.push(stage["id"].clone());
                    }
                }
            }
            for stage in next["stages"].as_array().ok_or("invalid stages")? {
                let count = old.as_ref().filter(|_| same).and_then(|p| p["stages"].as_array())
                    .and_then(|stages| stages.iter().find(|s| s["id"] == stage["id"]))
                    .and_then(|s| s["reviews"].as_array()).map_or(0, Vec::len);
                for review in stage["reviews"].as_array().into_iter().flatten().skip(count) {
                    let mut record = review.clone();
                    if record.is_object() {
                        record["version"] = json!(crate::architecture::VERSION);
                        if record["id"].is_null() { record["id"] = json!(crate::architecture::identity()); }
                        if record["policy"].is_null() { record["policy"] = cp["review_policy"].clone(); }
                        record["stage_id"] = stage["id"].clone();
                        if record["role"].is_null() { record["role"] = json!("reviewer"); }
                        reviews.push(record);
                    }
                }
            }
            let mut outcomes = Vec::new();
            if let Some(old) = old.as_ref().filter(|_| same) {
                for stage in next["stages"].as_array().unwrap() {
                    let before = old["stages"].as_array().unwrap().iter().find(|s| s["id"] == stage["id"]);
                    if stage["status"] == "committed" && before.is_none_or(|s| s["status"] != "committed") {
                        let outcome = json!({"stage_id":stage["id"],"revision":next["revision"],"status":"committed","sha":stage["sha"],"review_gate":stage["review_gate"],"review_policy":stage["review_policy"],"acceptance":stage["acceptance"].as_str().unwrap_or("").chars().take(500).collect::<String>(),"unix":unix_timestamp()});
                        outcomes.push(outcome.clone());
                        if !cp["execution_outcomes"].is_array() { cp["execution_outcomes"] = json!([]); }
                        let recent = cp["execution_outcomes"].as_array_mut().unwrap(); recent.push(outcome);
                        if recent.len() > 8 { recent.remove(0); }
                    }
                }
            }
            if old.as_ref() == Some(&next) { return Ok(next); }
            store.publish(next, cp, json!({"kind": kind, "reviews": reviews, "removed_stage_ids": removed, "invalidations": invalidations, "execution_outcomes":outcomes}))
        })();
        *self.session.persistence_error.lock().unwrap() = result.as_ref().err().cloned();
        result
    }
}
