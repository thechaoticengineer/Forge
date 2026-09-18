use super::*;
use crate::app::reassessment;
use crate::durable_json::{publish_pretty, publish_pretty_checked};
use crate::util::unix_timestamp;
use std::fs::OpenOptions;
use std::io::Write;

impl Store {
    /// Persist an inactive contract record; callers supply a checkpoint-forked
    /// session reference separately. This never invokes a provider or changes gates.
    #[allow(dead_code)]
    pub(crate) fn record(&self, plan: &Value, kind: &str, record: Value) -> Result<Value, String> {
        let current = self.load()?.ok_or("no plan")?;
        if current != *plan {
            return Err("stale architecture record".into());
        }
        validation::validate_record_identity(plan, &record)?;
        let mut cp = self.checkpoint(plan)?;
        match validation::validate_record(plan, &cp, kind, &record)? {
            validation::ValidatedRecord::Decision(d) => {
                let recent = cp["recent_decisions"].as_array_mut().unwrap();
                if let Some(supersedes) = &d.supersedes {
                    for previous in recent.iter_mut().filter(|p| p["id"] == *supersedes) {
                        previous["status"] = json!("superseded");
                    }
                }
                recent.retain(|p| p["id"] != d.id);
                recent.push(json!({"id": d.id, "status": record["status"], "summary": d.summary.chars().take(240).collect::<String>()}));
                if recent.len() > 8 {
                    recent.remove(0);
                }
            }
            validation::ValidatedRecord::Model(m) => {
                if matches!(m.kind, crate::contracts::SelectionKind::Agreement) {
                    let mut active = record.clone();
                    active["valid"] = json!(true);
                    cp["agreements"][m.stage_id.to_string()] = active;
                } else if matches!(m.kind, crate::contracts::SelectionKind::Invalidation) {
                    let active = &mut cp["agreements"][m.stage_id.to_string()];
                    if active.is_object() {
                        active["valid"] = json!(false);
                        active["invalidation_trigger"] = json!(m.trigger);
                    }
                }
            }
            validation::ValidatedRecord::Guidance(g) => {
                cp["guidance"][g.stage_id.to_string()] = record.clone();
            }
            validation::ValidatedRecord::Review => {}
        }
        self.publish(plan.clone(), cp, json!({"kind": kind, "record": record}))
    }

    pub(crate) fn publish(&self, plan: Value, cp: Value, event: Value) -> Result<Value, String> {
        self.publish_at(plan, cp, event, "")
    }
    pub(super) fn publish_at(
        &self,
        mut plan: Value,
        cp: Value,
        payload: Value,
        stop: &str,
    ) -> Result<Value, String> {
        if !payload["kind"].as_str().is_some_and(|s| !s.is_empty()) {
            return Err("invalid event payload".into());
        }
        if !plan["stages"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_object))
        {
            return Err("invalid plan".into());
        }
        let old = self.load()?;
        let id = plan["plan_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(identity);
        let dir = self.directory(&id)?;
        let same = old.as_ref().is_some_and(|p| p["plan_id"] == id);
        if let Some(old) = &old {
            if old.get("architecture").is_some() && !same {
                self.archive(old)?;
            }
        }
        if plan.get("contract_version").is_some_and(|v| *v != VERSION) {
            return Err("unsupported plan contract".into());
        }
        let revision = plan["revision"].as_u64().unwrap_or(1);
        if revision == 0 {
            return Err("invalid revision".into());
        }
        if same {
            let previous = old.as_ref().unwrap();
            if plan["architecture"] != previous["architecture"] {
                return Err("stale architectural checkpoint".into());
            }
            let rev = previous["revision"].as_u64().unwrap();
            if revision < rev || revision > rev.checked_add(1).ok_or("revision limit")? {
                return Err("stale plan revision".into());
            }
        } else if plan.get("architecture").is_some() || dir.join("archived.json").exists() {
            return Err("cannot reuse an archived plan identity".into());
        }
        plan["contract_version"] = json!(VERSION);
        plan["plan_id"] = json!(id);
        plan["revision"] = json!(revision);
        validation::validate_checkpoint(&cp, &plan)?;
        fs::create_dir_all(dir.join("checkpoints")).map_err(|e| e.to_string())?;
        sync_dir(&self.root)?;
        if let Some(parent) = self.root.parent() {
            sync_dir(parent)?;
        }
        sync_dir(&self.root.join("architecture"))?;
        sync_dir(&dir)?;
        let token = identity();
        let start = if same {
            old.as_ref().unwrap()["architecture"]["event_end"]
                .as_u64()
                .unwrap()
        } else {
            0
        };
        // Plan verdicts are retrieved only through history. Admit them only if
        // their lossless stored form fits one existing page; expansion keeps its
        // separate 4 MiB limit. Reject before publishing any event or reference.
        let event_limit = if payload["kind"] == "plan_review" {
            HISTORY_PAGE_BYTES
        } else { EVENT_LIMIT };
        // Older reassessment history moves, complete, into this event. A batch
        // that does not fit shrinks; the remainder drains on later publications.
        let saved = old.as_ref().filter(|_| same);
        let mut batch = reassessment::ARCHIVE_MAX_ENTRIES;
        let bytes = loop {
            let mut bounded = plan.clone();
            let archived = reassessment::archive_history(&mut bounded, saved, batch, reassessment::ARCHIVE_MAX_BYTES);
            let moved: usize = archived.iter().map(|a| a["entries"].as_array().map_or(0, Vec::len)).sum();
            let mut payload = payload.clone();
            if moved > 0 {
                payload["archived_reassessment_history"] = json!(archived);
            }
            let event = json!({"version": VERSION, "id": identity(), "plan_id": id,
                "revision": revision, "unix": unix_timestamp(), "checkpoint": token, "payload": payload});
            let stored = storage::pack(&event, event_limit - 1, "architecture event").and_then(|stored| {
                let mut bytes = serde_json::to_vec(&stored).map_err(|e| e.to_string())?;
                bytes.push(b'\n');
                if bytes.len() > EVENT_LIMIT {
                    return Err("oversized architecture event".into());
                }
                Ok(bytes)
            });
            match stored {
                Ok(bytes) => {
                    plan = bounded;
                    break bytes;
                }
                Err(_) if moved > 0 => batch = moved - 1,
                Err(error) => return Err(error),
            }
        };
        let end = start
            .checked_add(bytes.len() as u64)
            .ok_or("event position overflow")?;
        let mut log = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(dir.join("events.jsonl"))
            .map_err(|e| e.to_string())?;
        // Only uncommitted crash residue is truncated. Committed bytes never change.
        log.set_len(start).map_err(|e| e.to_string())?;
        log.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        if stop == "partial_event" {
            log.write_all(&bytes[..bytes.len() / 2])
                .map_err(|e| e.to_string())?;
            log.sync_all().map_err(|e| e.to_string())?;
            return Err("injected partial event".into());
        }
        log.write_all(&bytes).map_err(|e| e.to_string())?;
        log.sync_all().map_err(|e| e.to_string())?;
        sync_dir(&dir)?;
        if stop == "events" {
            return Err("injected interruption after events".into());
        }
        plan["contract_version"] = json!(VERSION);
        plan["plan_id"] = json!(id);
        plan["revision"] = json!(revision);
        plan["architecture"] = json!({"checkpoint": token, "event_start": start, "event_end": end});
        let mut references = if same {
            old.as_ref().unwrap()["architecture"]["review_history"]
                .as_object()
                .cloned()
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };
        let review_dir = dir.join("reviews");
        // Files and indexes are durable before the checkpoint and sole plan publication.
        // Removed stages retain their references. Unchanged arrays reuse immutable files.
        for stage in plan["stages"].as_array_mut().unwrap() {
            if let Some(records) = stage["reviews"].as_array() {
                let stage_id = stage["id"].to_string();
                let unchanged = same
                    && old.as_ref().unwrap()["stages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|s| s["id"] == stage["id"] && s["reviews"] == stage["reviews"]);
                if !unchanged || !references.contains_key(&stage_id) {
                    references.insert(
                        stage_id.clone(),
                        crate::review_history::write(&review_dir, records)?,
                    );
                }
                stage["reviews"] = json!({"$forge_reviews": references[&stage_id]["file"]});
            }
        }
        if !references.is_empty() {
            plan["architecture"]["review_history"] = json!(references);
        }
        if let Some(records) = plan["plan_review"]["reviews"].as_array() {
            let previous = old.as_ref().filter(|_| same);
            let reference = if previous.is_some_and(|p| p["plan_review"]["reviews"] == json!(records)
                && p["architecture"]["plan_review_history"].is_object()) {
                previous.unwrap()["architecture"]["plan_review_history"].clone()
            } else { crate::review_history::write_plan(&review_dir, records)? };
            plan["plan_review"]["reviews"] = json!({"$forge_reviews":reference["file"]});
            plan["architecture"]["plan_review_history"] = reference;
        }
        sync_dir(&dir)?;
        if stop == "reviews" {
            return Err("injected interruption after review files".into());
        }
        let stored_cp = storage::pack(&cp, CHECKPOINT_LIMIT, "architectural checkpoint")?;
        let bundle = json!({"version": VERSION, "plan": plan, "checkpoint": stored_cp});
        if stop == "partial_snapshot" {
            fs::write(
                dir.join("checkpoints").join(format!(".{token}.tmp")),
                b"{\"version\":",
            )
            .map_err(|e| e.to_string())?;
            return Err("injected partial snapshot".into());
        }
        publish_pretty(
            &dir.join("checkpoints").join(format!("{token}.json")),
            &bundle,
        )?;
        // This validates the persisted snapshot and its event before publication.
        self.checkpoint(&plan)?;
        if stop == "checkpoint" {
            return Err("injected interruption after checkpoint".into());
        }
        let logical_plan = self.hydrate(plan.clone())?;
        publish_pretty_checked(
            &self.root.join("plan.json"),
            &plan,
            stop == "publication_error",
        )?;
        if stop == "publication" {
            return Err("injected crash after committed publication".into());
        }
        Ok(logical_plan)
    }
    fn archive(&self, plan: &Value) -> Result<(), String> {
        self.checkpoint(plan)?;
        let dir = self.directory(plan["plan_id"].as_str().ok_or("missing identity")?)?;
        let bundle = read_json(&dir.join("checkpoints").join(format!("{}.json",
            plan["architecture"]["checkpoint"].as_str().ok_or("missing checkpoint")?)))?;
        publish_pretty(&dir.join("archived.json"), &bundle["plan"])
    }
    pub(crate) fn reset(&self) -> Result<(), String> {
        if let Some(plan) = self.load()? {
            let plan = if plan.get("architecture").is_none() {
                self.publish(plan, checkpoint_default(), json!({"kind": "legacy_import"}))?
            } else {
                plan
            };
            self.archive(&plan)?;
            fs::remove_file(self.root.join("plan.json")).map_err(|e| e.to_string())?;
            sync_dir(&self.root)?;
        }
        Ok(())
    }
}
