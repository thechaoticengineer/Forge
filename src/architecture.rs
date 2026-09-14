//! Versioned, provider-neutral architecture persistence. The full plan is the
//! commit record: its reference selects ONE immutable checkpoint and event prefix.
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(crate) use crate::durable_json::identity;
use crate::durable_json::{safe_id, sync_dir};

#[path = "architecture_storage.rs"]
mod storage;
#[path = "architecture_validation.rs"]
mod validation;
#[path = "architecture_publication.rs"]
mod publication;
#[path = "architecture_query.rs"]
mod query;

#[cfg(test)]
use crate::durable_json::publish_pretty;

pub(crate) const VERSION: u64 = 1;
// Checkpoints accumulate agreements and completed-stage evidence. Use the
// existing decoded-record memory budget for disk storage too; the 64 KiB
// event/page budget is not a valid bound for an entire plan checkpoint.
const CHECKPOINT_LIMIT: usize = storage::EXPANDED_LIMIT;
// One turn may carry up to 16 decisions, each validated at 1 KiB of summary,
// 4 KiB of rationale and 8 alternatives of 1 KiB each — roughly 336 KiB before
// agreements and risks. A smaller line budget rejects turns the validator
// accepts, so the log line has to hold what a legal turn can produce.
const EVENT_LIMIT: usize = 512 * 1024;
const HISTORY_PAGE_BYTES: usize = 256 * 1024;

/// Guidance and agreement records carry a `relevant_inputs` fingerprint that
/// copies the stage text a prompt already contains, and it grows with every
/// dependency. Strip it before a checkpoint enters a provider turn; storage
/// and every validity comparison keep the complete value.
pub(crate) fn prompt_checkpoint(checkpoint: &Value) -> Value {
    let mut checkpoint = checkpoint.clone();
    for group in ["guidance", "agreements"] {
        let Some(records) = checkpoint.get_mut(group).and_then(Value::as_object_mut) else { continue };
        for record in records.values_mut() {
            if let Some(record) = record.as_object_mut() { record.remove("relevant_inputs"); }
        }
    }
    checkpoint
}

fn read_json(path: &Path) -> Result<Value, String> {
    crate::durable_json::read_json(
        path,
        |path, e| format!("{}: {e}", path.display()),
        |path, e| format!("{}: {e}", path.display()),
    )
}

pub(crate) fn checkpoint_default() -> Value {
    json!({"version": VERSION, "context_status": "inactive", "session": null,
        "summary": "", "recent_decisions": [], "guidance": {}, "agreements": {},
        "review_policy": {"version": VERSION, "required_roles": ["architect", "reviewer"], "scope": "code_or_contract",
            "rationale": "Conservative default; engine classifies each implementation snapshot before review."}})
}

/// All methods are called under the project's persistence mutex. No provider
/// writes these files. Candidate provider turns must fork the saved checkpoint.
pub(crate) struct Store {
    root: PathBuf,
}
impl Store {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn directory(&self, id: &str) -> Result<PathBuf, String> {
        if !safe_id(id) {
            return Err("invalid plan identity".into());
        }
        Ok(self.root.join("architecture").join(id))
    }
    pub(crate) fn load(&self) -> Result<Option<Value>, String> {
        self.load_raw()?.map(|p| self.hydrate(p)).transpose()
    }
    // Polling and pagination never hydrate the full review arrays.
    pub(crate) fn load_raw(&self) -> Result<Option<Value>, String> {
        self.load_raw_inner(false)
    }
    pub(crate) fn load_state(&self) -> Result<Option<Value>, String> {
        self.load_raw_inner(true)
    }
    fn load_raw_inner(&self, polling: bool) -> Result<Option<Value>, String> {
        let path = self.root.join("plan.json");
        if !path.exists() {
            return Ok(None);
        }
        let plan = read_json(&path)?;
        if !plan["stages"]
            .as_array()
            .is_some_and(|s| s.iter().all(Value::is_object))
        {
            return Err("invalid plan stages".into());
        }
        if plan.get("architecture").is_some() {
            self.checkpoint_inner(&plan, polling)?;
        } else if plan.get("plan_id").is_some() || plan.get("contract_version").is_some() {
            return Err("incomplete plan identity contract".into());
        }
        Ok(Some(plan))
    }
    fn review_dir(&self, plan: &Value) -> Result<PathBuf, String> {
        Ok(self
            .directory(plan["plan_id"].as_str().ok_or("missing plan identity")?)?
            .join("reviews"))
    }
    fn hydrate(&self, mut plan: Value) -> Result<Value, String> {
        let refs = plan["architecture"]["review_history"].clone();
        if plan["plan_review"]["reviews"]["$forge_reviews"].is_string() {
            plan["plan_review"]["reviews"] = crate::review_history::all(
                &self.review_dir(&plan)?, &plan["architecture"]["plan_review_history"])?;
        }
        if refs.is_null() { return Ok(plan); }
        let dir = self.review_dir(&plan)?;
        for stage in plan["stages"].as_array_mut().ok_or("invalid stages")? {
            if stage["reviews"].is_object() && stage["reviews"].get("$forge_reviews").is_some() {
                stage["reviews"] =
                    crate::review_history::all(&dir, &refs[stage["id"].to_string()])?;
            }
        }
        Ok(plan)
    }
    pub(crate) fn checkpoint(&self, plan: &Value) -> Result<Value, String> {
        self.checkpoint_inner(plan, false)
    }
    fn checkpoint_inner(&self, plan: &Value, polling: bool) -> Result<Value, String> {
        let id = plan["plan_id"].as_str().ok_or("missing plan identity")?;
        let token = plan["architecture"]["checkpoint"]
            .as_str()
            .filter(|s| safe_id(s))
            .ok_or("invalid checkpoint reference")?;
        if plan["contract_version"] != VERSION || plan["revision"].as_u64().unwrap_or(0) == 0 {
            return Err("unsupported plan contract".into());
        }
        let dir = self.directory(id)?;
        let mut bundle = read_json(&dir.join("checkpoints").join(format!("{token}.json")))?;
        bundle["checkpoint"] = storage::unpack(bundle["checkpoint"].take(), CHECKPOINT_LIMIT, "architectural checkpoint")?;
        if bundle["version"] != VERSION
            || (bundle["plan"] != *plan && (polling || self.hydrate(bundle["plan"].clone())? != *plan))
        {
            return Err("plan/checkpoint mismatch".into());
        }
        let end = plan["architecture"]["event_end"]
            .as_u64()
            .ok_or("invalid event position")?;
        let file = File::open(dir.join("events.jsonl")).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() < end {
            return Err("truncated architectural history".into());
        }
        // Verify the last committed event; pages validate every event they expose.
        let start = plan["architecture"]["event_start"]
            .as_u64()
            .filter(|s| *s <= end)
            .ok_or("invalid event start")?;
        if end - start > EVENT_LIMIT as u64 {
            return Err("oversized event".into());
        }
        if polling {
            // Publication verified the complete event before selecting this
            // checkpoint. Polling checks its boundary, never its verdict payload.
            if end == start { return Err("empty committed event".into()); }
            let mut file = file;
            file.seek(SeekFrom::Start(end - 1)).map_err(|e| e.to_string())?;
            let mut boundary = [0];
            file.read_exact(&mut boundary).map_err(|e| e.to_string())?;
            if boundary != [b'\n'] { return Err("unterminated committed event".into()); }
            validation::validate_reviews(self, &bundle["plan"])?;
            validation::validate_checkpoint(&bundle["checkpoint"], plan)?;
            return Ok(bundle["checkpoint"].clone());
        }
        let mut file = file;
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; (end - start) as usize];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.last() != Some(&b'\n') {
            return Err("unterminated committed event".into());
        }
        let event = storage::unpack(serde_json::from_slice(&bytes).map_err(|e| e.to_string())?, EVENT_LIMIT - 1, "architecture event")?;
        if event["checkpoint"] != token
            || event["plan_id"] != id
            || event["revision"] != plan["revision"]
            || event["version"] != VERSION
        {
            return Err("event/checkpoint mismatch".into());
        }
        validation::validate_reviews(self, &bundle["plan"])?;
        validation::validate_checkpoint(&bundle["checkpoint"], plan)?;
        Ok(bundle["checkpoint"].clone())
    }
}

#[cfg(test)]
pub(crate) fn agreement_fixture(plan: &Value, index: usize) -> Value {
    let id = plan["stages"][index]["id"].as_i64().unwrap();
    json!({"version": 1, "id": format!("agreement-{id}"), "kind": "agreement", "valid": true,
        "proposal_ids": ["planner-1", "architect-1"], "agreement_id": format!("agreement-{id}"),
        "plan_id": plan["plan_id"], "revision": plan["revision"], "stage_id": id,
        "relevant_inputs": crate::plan::stage_inputs(plan, index),
        "effective": {"provider": "provider", "model": "model", "native_effort": null},
        "planner_reason": "bounded task", "architect_reason": "low design risk",
        "provenance": {"capability_policy_version": "policy-1", "catalogue_revision": "catalogue-2",
            "official_sources": [], "checked_unix": 10},
        "availability": "unverified", "trigger": "initial_assignment", "superseded_agreement": null, "unix": 10})
}

#[cfg(test)]
#[path = "architecture_test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "architecture_validation_tests.rs"]
mod validation_tests;
#[cfg(test)]
#[path = "architecture_publication_tests.rs"]
mod publication_tests;
#[cfg(test)]
#[path = "architecture_query_tests.rs"]
mod query_tests;
