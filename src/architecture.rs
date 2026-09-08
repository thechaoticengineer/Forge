//! Versioned, provider-neutral architecture persistence. The full plan is the
//! commit record: its reference selects ONE immutable checkpoint and event prefix.
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const VERSION: u64 = 1;
const CHECKPOINT_LIMIT: usize = 64 * 1024;
const EVENT_LIMIT: usize = 64 * 1024;
static IDS: AtomicU64 = AtomicU64::new(0);

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
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
pub(crate) fn atomic_json(path: &Path, value: &Value) -> Result<(), String> {
    atomic_json_checked(path, value, false)
}
fn atomic_json_checked(path: &Path, value: &Value, fail_sync: bool) -> Result<(), String> {
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
        if read_json(&tmp)? != *value {
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

pub(crate) fn checkpoint_default() -> Value {
    json!({"version": VERSION, "context_status": "inactive", "session": null,
        "summary": "", "recent_decisions": [], "guidance": {}, "agreements": {},
        "review_policy": {"version": VERSION, "required_roles": ["reviewer"], "scope": "all",
            "rationale": "Existing review path; architect review gates are not activated in stage one."}})
}
fn validate_checkpoint(cp: &Value, plan: &Value) -> Result<(), String> {
    if cp["version"] != VERSION
        || !cp["summary"].is_string()
        || !cp["guidance"].is_object()
        || !cp["agreements"].is_object()
        || !cp["recent_decisions"]
            .as_array()
            .is_some_and(|a| a.len() <= 8)
        || !matches!(
            cp["context_status"].as_str(),
            Some("inactive" | "ready" | "needs_recovery")
        )
        || serde_json::to_vec(cp).map_err(|e| e.to_string())?.len() > CHECKPOINT_LIMIT
    {
        return Err("invalid or oversized architectural checkpoint".into());
    }
    let policy: crate::contracts::ReviewPolicy =
        serde_json::from_value(cp["review_policy"].clone()).map_err(|e| e.to_string())?;
    if policy.version != VERSION
        || policy.rationale.trim().is_empty()
        || policy.scope.trim().is_empty()
        || policy.required_roles.is_empty()
    {
        return Err("invalid review policy".into());
    }
    for d in cp["recent_decisions"].as_array().unwrap() {
        if !d["id"].as_str().is_some_and(safe_id)
            || !d["summary"].is_string()
            || !matches!(
                d["status"].as_str(),
                Some("proposed" | "accepted" | "rejected" | "superseded")
            )
        {
            return Err("invalid decision summary".into());
        }
    }
    for key in ["guidance", "agreements"] {
        for (stage_key, record) in cp[key].as_object().unwrap() {
            let stage_id = record["stage_id"]
                .as_i64()
                .filter(|id| *id > 0)
                .ok_or("invalid checkpoint stage identity")?;
            let revision = record["revision"]
                .as_u64()
                .filter(|r| *r > 0 && *r <= plan["revision"].as_u64().unwrap_or(0))
                .ok_or("invalid checkpoint record revision")?;
            if record
                .get("plan_id")
                .is_some_and(|id| *id != plan["plan_id"])
                || stage_key != &stage_id.to_string()
                || record["version"] != VERSION
                || !record["id"].as_str().is_some_and(safe_id)
                || !record["valid"].is_boolean()
                || !record["unix"].as_i64().is_some_and(|t| t >= 0)
            {
                return Err("invalid checkpoint record contract".into());
            }
            let inputs = &record["relevant_inputs"];
            if !inputs.is_object()
                || !inputs["dependencies"]
                    .as_array()
                    .is_some_and(|deps| deps.iter().all(|id| id.as_i64().is_some_and(|id| id > 0)))
                || ["goal", "title", "instructions", "acceptance", "commit"]
                    .iter()
                    .any(|key| !inputs[*key].is_string())
            {
                return Err("invalid checkpoint record inputs".into());
            }
            let index = plan["stages"]
                .as_array()
                .ok_or("invalid stages")?
                .iter()
                .position(|stage| stage["id"] == stage_id);
            if record["valid"] == true {
                let index = index.ok_or("active checkpoint record for missing stage")?;
                if *inputs != crate::plan::stage_inputs(plan, index) {
                    return Err("active checkpoint record inputs are stale".into());
                }
            } else if index.is_none()
                && (revision >= plan["revision"].as_u64().unwrap()
                    || record["invalidation_trigger"] != "stage_removed")
            {
                return Err(
                    "missing stage requires an explicitly invalid historical record".into(),
                );
            }
            if key == "agreements" {
                let model = validate_model(record)?;
                if model.plan_id != plan["plan_id"].as_str().unwrap_or("")
                    || !matches!(model.kind, crate::contracts::SelectionKind::Agreement)
                {
                    return Err("checkpoint agreement identity/kind mismatch".into());
                }
            } else {
                let guidance: crate::contracts::Guidance =
                    serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
                if guidance.text.trim().is_empty() {
                    return Err("empty checkpoint guidance".into());
                }
            }
        }
    }
    if !cp["session"].is_null() {
        let _: crate::contracts::SessionReference =
            serde_json::from_value(cp["session"].clone()).map_err(|e| e.to_string())?;
        let s = &cp["session"];
        for key in ["provider", "reference", "checkpoint_reference"] {
            if !s[key].as_str().is_some_and(|v| !v.is_empty()) {
                return Err("invalid exact session reference".into());
            }
        }
        if s["resume_policy"] == "exact_if_committed" {
            if !s["reference"].as_str().is_some_and(crate::agent::session_id)
                || !cp["last_turn"].as_str().is_some_and(safe_id)
                || cp["last_turn"] != s["checkpoint_reference"]
                || cp["effective_model"]["provider"] != s["provider"]
                || !cp["effective_model"]["model"].as_str().is_some_and(|s| !s.is_empty()) {
                return Err("invalid committed architect session".into());
            }
            for key in ["constraints", "completed_interfaces"] {
                if !cp[key].as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 1000))) {
                    return Err("invalid saved architect constraints/interfaces".into());
                }
            }
            let mut ids = std::collections::BTreeSet::new();
            if !cp["unresolved_risks"].as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(|r|
                r["id"].as_str().is_some_and(|id| safe_id(id) && ids.insert(id.to_string())) &&
                r["text"].as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 1000))) {
                return Err("invalid saved architect risks".into());
            }
        }
        if s["resume_policy"] != "fork_from_checkpoint" && s["resume_policy"] != "exact_if_committed" {
            return Err("unsafe session resume policy".into());
        }
    }
    Ok(())
}

fn validate_model(record: &Value) -> Result<crate::contracts::ModelRecord, String> {
    use crate::contracts::SelectionKind;
    let m: crate::contracts::ModelRecord =
        serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
    if m.version != VERSION
        || !safe_id(&m.id)
        || !safe_id(&m.plan_id)
        || m.revision == 0
        || m.stage_id <= 0
        || m.unix < 0
        || m.provenance.checked_unix < 0
        || m.effective.provider.trim().is_empty()
        || m.effective.model.trim().is_empty()
        || !record["effective"]
            .as_object()
            .is_some_and(|m| m.contains_key("native_effort"))
        || m.effective
            .native_effort
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
        || m.provenance.capability_policy_version.trim().is_empty()
        || m.provenance.catalogue_revision.trim().is_empty()
        || m.trigger.trim().is_empty()
        || m.proposal_ids.iter().any(|id| !safe_id(id))
        || m.agreement_id.as_ref().is_some_and(|id| !safe_id(id))
        || m.superseded_agreement
            .as_ref()
            .is_some_and(|id| !safe_id(id) || *id == m.id)
    {
        return Err("incomplete model selection metadata".into());
    }
    if matches!(m.kind, SelectionKind::Agreement)
        && (m.planner_reason.trim().is_empty()
            || m.architect_reason.trim().is_empty()
            || m.proposal_ids.is_empty()
            || m.agreement_id.as_ref() != Some(&m.id))
    {
        return Err("agreement requires proposals, identity and both participants' reasons".into());
    }
    Ok(m)
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
            self.checkpoint(&plan)?;
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
        let Some(refs) = plan["architecture"].get("review_history").cloned() else {
            return Ok(plan);
        };
        let dir = self.review_dir(&plan)?;
        for stage in plan["stages"].as_array_mut().ok_or("invalid stages")? {
            if stage["reviews"].is_object() && stage["reviews"].get("$forge_reviews").is_some() {
                stage["reviews"] =
                    crate::review_history::all(&dir, &refs[stage["id"].to_string()])?;
            }
        }
        Ok(plan)
    }
    fn validate_reviews(&self, plan: &Value) -> Result<(), String> {
        if let Some(refs) = plan["architecture"].get("review_history") {
            let dir = self.review_dir(plan)?;
            for (id, reference) in refs.as_object().ok_or("invalid review manifest")? {
                if !id.parse::<i64>().is_ok_and(|id| id > 0) {
                    return Err("invalid review stage".into());
                }
                crate::review_history::validate(&dir, reference)?;
            }
            for stage in plan["stages"].as_array().ok_or("invalid stages")? {
                if stage["reviews"].is_object()
                    && stage["reviews"].get("$forge_reviews").is_some()
                    && (refs[stage["id"].to_string()].is_null()
                        || stage["reviews"]["$forge_reviews"]
                            != refs[stage["id"].to_string()]["file"])
                {
                    return Err("review stage/reference mismatch".into());
                }
            }
        }
        Ok(())
    }
    pub(crate) fn state_plan(&self, mut plan: Value) -> Value {
        let refs = plan["architecture"]["review_history"].clone();
        for stage in plan["stages"].as_array_mut().into_iter().flatten() {
            if stage["reviews"].is_object() && stage["reviews"].get("$forge_reviews").is_some() {
                let reference = &refs[stage["id"].to_string()];
                stage["reviews"] = reference["recent"].clone();
                if reference["count"].as_u64().unwrap_or(0)
                    > crate::review_history::PREVIEW_COUNT as u64
                    || stage["reviews"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|r| r["truncated"] == true))
                {
                    stage["review_count"] = reference["count"].clone();
                    stage["reviews_truncated"] = json!(true);
                }
            }
        }
        crate::review_history::bounded(&mut plan);
        // References for removed stages stay on disk and remain accessible through pagination.
        if let Some(architecture) = plan.get_mut("architecture").and_then(Value::as_object_mut) {
            architecture.remove("review_history");
        }
        plan
    }
    pub(crate) fn checkpoint(&self, plan: &Value) -> Result<Value, String> {
        let id = plan["plan_id"].as_str().ok_or("missing plan identity")?;
        let token = plan["architecture"]["checkpoint"]
            .as_str()
            .filter(|s| safe_id(s))
            .ok_or("invalid checkpoint reference")?;
        if plan["contract_version"] != VERSION || plan["revision"].as_u64().unwrap_or(0) == 0 {
            return Err("unsupported plan contract".into());
        }
        let dir = self.directory(id)?;
        let bundle = read_json(&dir.join("checkpoints").join(format!("{token}.json")))?;
        if bundle["version"] != VERSION
            || (bundle["plan"] != *plan && self.hydrate(bundle["plan"].clone())? != *plan)
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
        let mut file = file;
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; (end - start) as usize];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.last() != Some(&b'\n') {
            return Err("unterminated committed event".into());
        }
        let event: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if event["checkpoint"] != token
            || event["plan_id"] != id
            || event["revision"] != plan["revision"]
            || event["version"] != VERSION
        {
            return Err("event/checkpoint mismatch".into());
        }
        self.validate_reviews(&bundle["plan"])?;
        validate_checkpoint(&bundle["checkpoint"], plan)?;
        Ok(bundle["checkpoint"].clone())
    }
    /// Persist an inactive contract record; callers supply a checkpoint-forked
    /// session reference separately. This never invokes a provider or changes gates.
    #[allow(dead_code)]
    pub(crate) fn record(&self, plan: &Value, kind: &str, record: Value) -> Result<Value, String> {
        let current = self.load()?.ok_or("no plan")?;
        if current != *plan {
            return Err("stale architecture record".into());
        }
        if record["version"] != VERSION
            || !record["id"].as_str().is_some_and(safe_id)
            || record["revision"] != plan["revision"]
        {
            return Err("invalid record identity/version/revision".into());
        }
        if let Some(id) = record.get("plan_id") {
            if *id != plan["plan_id"] {
                return Err("record belongs to another plan".into());
            }
        }
        if !record["stage_id"].is_null()
            && !plan["stages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["id"] == record["stage_id"])
        {
            return Err("record belongs to an unknown stage".into());
        }
        let mut cp = self.checkpoint(plan)?;
        match kind {
            "decision" => {
                let d: crate::contracts::Decision =
                    serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
                if d.summary.trim().is_empty()
                    || d.rationale.trim().is_empty()
                    || d.created_unix < 0
                    || d.updated_unix < d.created_unix
                    || d.supersedes
                        .as_ref()
                        .is_some_and(|id| !safe_id(id) || *id == d.id)
                {
                    return Err("invalid decision rationale/timestamps".into());
                }
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
            "model" => {
                let m = validate_model(&record)?;
                if matches!(m.kind, crate::contracts::SelectionKind::Agreement) {
                    if m.planner_reason.trim().is_empty() || m.architect_reason.trim().is_empty() {
                        return Err("agreement requires both participants' reasons".into());
                    }
                    let index = plan["stages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .position(|s| s["id"] == m.stage_id)
                        .ok_or("unknown stage")?;
                    if m.relevant_inputs != crate::plan::stage_inputs(plan, index) {
                        return Err("agreement inputs are stale".into());
                    }
                    let mut active = record.clone();
                    active["valid"] = json!(true);
                    cp["agreements"][m.stage_id.to_string()] = active;
                } else if matches!(m.kind, crate::contracts::SelectionKind::Invalidation) {
                    let active = &mut cp["agreements"][m.stage_id.to_string()];
                    if active.is_object() {
                        if m.agreement_id.as_ref().is_none_or(|id| active["id"] != *id) {
                            return Err("invalidation agreement mismatch".into());
                        }
                        active["valid"] = json!(false);
                        active["invalidation_trigger"] = json!(m.trigger);
                    }
                }
            }
            "guidance" => {
                let g: crate::contracts::Guidance =
                    serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
                let index = plan["stages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|s| s["id"] == g.stage_id)
                    .ok_or("unknown stage")?;
                if g.relevant_inputs != crate::plan::stage_inputs(plan, index) {
                    return Err("guidance inputs are stale".into());
                }
                cp["guidance"][g.stage_id.to_string()] = record.clone();
            }
            "review" => {
                let r: crate::contracts::ReviewRecord =
                    serde_json::from_value(record.clone()).map_err(|e| e.to_string())?;
                if r.attempt_id.is_empty()
                    || r.policy.version != VERSION
                    || r.policy.required_roles.is_empty()
                    || r.policy.rationale.trim().is_empty()
                    || r.policy.scope.trim().is_empty()
                    || r.round == 0
                    || !matches!(
                        r.role,
                        crate::contracts::Role::Architect | crate::contracts::Role::Reviewer
                    )
                {
                    return Err("invalid review role/attempt/round".into());
                }
            }
            _ => return Err("unknown architecture record kind".into()),
        }
        self.publish(plan.clone(), cp, json!({"kind": kind, "record": record}))
    }

    pub(crate) fn publish(&self, plan: Value, cp: Value, event: Value) -> Result<Value, String> {
        self.publish_at(plan, cp, event, "")
    }
    fn publish_at(
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
        validate_checkpoint(&cp, &plan)?;
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
        let event = json!({"version": VERSION, "id": identity(), "plan_id": id,
            "revision": revision, "unix": unix_timestamp(), "checkpoint": token, "payload": payload});
        let mut bytes = serde_json::to_vec(&event).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > EVENT_LIMIT {
            return Err("oversized architecture event".into());
        }
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
        sync_dir(&dir)?;
        if stop == "reviews" {
            return Err("injected interruption after review files".into());
        }
        let bundle = json!({"version": VERSION, "plan": plan, "checkpoint": cp});
        if stop == "partial_snapshot" {
            fs::write(
                dir.join("checkpoints").join(format!(".{token}.tmp")),
                b"{\"version\":",
            )
            .map_err(|e| e.to_string())?;
            return Err("injected partial snapshot".into());
        }
        atomic_json(
            &dir.join("checkpoints").join(format!("{token}.json")),
            &bundle,
        )?;
        // This validates the persisted snapshot and its event before publication.
        self.checkpoint(&plan)?;
        if stop == "checkpoint" {
            return Err("injected interruption after checkpoint".into());
        }
        let logical_plan = self.hydrate(plan.clone())?;
        atomic_json_checked(
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
        atomic_json(&dir.join("archived.json"), &bundle["plan"])
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
    pub(crate) fn summary(&self, plan: Option<&Value>) -> Result<Value, String> {
        let Some(plan) = plan else {
            return Ok(Value::Null);
        };
        if plan.get("architecture").is_none() {
            return Ok(
                json!({"context_status": "legacy", "revision": null, "recent_decisions": []}),
            );
        }
        let cp = self.checkpoint(plan)?;
        let pending_path = self.directory(plan["plan_id"].as_str().unwrap())?.join("architect-pending.json");
        let recovery_needed = match fs::read(&pending_path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map(|p| p["turn"] != cp["last_turn"]).unwrap_or(true),
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        };
        Ok(
            json!({"version": VERSION, "plan_id": plan["plan_id"], "revision": plan["revision"],
            "checkpoint": plan["architecture"]["checkpoint"], "event_end": plan["architecture"]["event_end"],
            "context_status": if recovery_needed { json!("needs_recovery") } else { cp["context_status"].clone() },
            "session":cp["session"], "recovery":cp["recovery"], "effective_model":cp["effective_model"],
            "guidance":cp["guidance"], "unresolved_risks":cp["unresolved_risks"], "constraints":cp["constraints"],
            "execution_outcomes":cp["execution_outcomes"], "role_usage":cp["role_usage"], "summary": cp["summary"].as_str().unwrap().chars().take(2000).collect::<String>(),
            "recent_decisions": cp["recent_decisions"].as_array().unwrap().iter().map(|d| json!({
                "id": d["id"], "status": d["status"], "summary": d["summary"].as_str().unwrap_or("").chars().take(240).collect::<String>(),
                "rationale":d["rationale"],"alternatives":d["alternatives"],"supersedes":d["supersedes"]
            })).collect::<Vec<_>>() }),
        )
    }
    fn history_plan(&self, id: Option<&str>) -> Result<Value, String> {
        let current = self.load_raw()?;
        let plan = match id {
            Some(id) if !current.as_ref().is_some_and(|p| p["plan_id"] == id) => {
                read_json(&self.directory(id)?.join("archived.json"))?
            }
            _ => current.ok_or("no plan")?,
        };
        if id.is_some_and(|id| plan["plan_id"] != id) {
            return Err("archive identity mismatch".into());
        }
        if plan.get("architecture").is_some() {
            self.checkpoint(&plan)?;
        }
        Ok(plan)
    }
    pub(crate) fn reviews(
        &self,
        id: Option<&str>,
        stage_id: i64,
        checkpoint: Option<&str>,
        cursor: u64,
        limit: usize,
    ) -> Result<Value, String> {
        let mut plan = self.history_plan(id)?;
        if let Some(token) = checkpoint {
            if !safe_id(token) {
                return Err("invalid checkpoint reference".into());
            }
            let dir = self.directory(
                plan["plan_id"]
                    .as_str()
                    .ok_or("legacy plan has no checkpoints")?,
            )?;
            let prior = read_json(&dir.join("checkpoints").join(format!("{token}.json")))?;
            // Unpublished crash residue must never be exposed as plan history.
            if prior["plan"]["plan_id"] != plan["plan_id"]
                || prior["plan"]["architecture"]["event_end"]
                    .as_u64()
                    .ok_or("invalid event position")?
                    > plan["architecture"]["event_end"].as_u64().unwrap()
                || prior["plan"]["architecture"]["checkpoint"] != token
            {
                return Err("checkpoint is not in the published history".into());
            }
            self.checkpoint(&prior["plan"])?;
            plan = prior["plan"].clone();
        }
        let reference = &plan["architecture"]["review_history"][stage_id.to_string()];
        let mut page = if !reference.is_null() {
            crate::review_history::page(&self.review_dir(&plan)?, reference, cursor, limit)?
        } else {
            let stage = plan["stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == stage_id)
                .ok_or("unknown review stage")?;
            let records = stage["reviews"].as_array().cloned().unwrap_or_default();
            let start = usize::try_from(cursor).map_err(|_| "invalid review cursor")?;
            if start > records.len() {
                return Err("cursor past review history".into());
            }
            let mut end = start;
            let mut bytes = 0;
            while end < records.len() && end - start < limit.clamp(1, 100) {
                let size = records[end].to_string().len();
                if end > start && bytes + size > 256 * 1024 {
                    break;
                }
                bytes += size;
                end += 1;
            }
            json!({"items": records[start..end], "count": records.len(),
                "next_cursor": if end < records.len() { json!(end) } else { Value::Null }})
        };
        page["plan_id"] = plan["plan_id"].clone();
        page["stage_id"] = json!(stage_id);
        page["checkpoint"] = plan["architecture"]["checkpoint"].clone();
        Ok(page)
    }
    /// Byte cursor, max 100 records / 256 KiB. Never reads beyond the published prefix.
    pub(crate) fn history(
        &self,
        id: Option<&str>,
        cursor: u64,
        limit: usize,
    ) -> Result<Value, String> {
        let plan = self.history_plan(id)?;
        if plan.get("architecture").is_none() {
            return Ok(json!({"items": [], "next_cursor": null}));
        }
        let end = plan["architecture"]["event_end"].as_u64().unwrap();
        if cursor > end {
            return Err("cursor past committed history".into());
        }
        let mut f = File::open(
            self.directory(plan["plan_id"].as_str().unwrap())?
                .join("events.jsonl"),
        )
        .map_err(|e| e.to_string())?;
        if cursor > 0 {
            f.seek(SeekFrom::Start(cursor - 1))
                .map_err(|e| e.to_string())?;
            let mut b = [0];
            f.read_exact(&mut b).map_err(|e| e.to_string())?;
            if b[0] != b'\n' {
                return Err("cursor must be an event boundary".into());
            }
        }
        f.seek(SeekFrom::Start(cursor)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        f.take((end - cursor).min(256 * 1024))
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        let mut next = cursor;
        for line in bytes
            .split_inclusive(|b| *b == b'\n')
            .take(limit.clamp(1, 100))
        {
            if line.last() != Some(&b'\n') {
                break;
            }
            if line.len() > EVENT_LIMIT {
                return Err("oversized event".into());
            }
            let item: Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
            if item["version"] != VERSION
                || item["plan_id"] != plan["plan_id"]
                || !item["id"].as_str().is_some_and(safe_id)
                || !item["checkpoint"].as_str().is_some_and(safe_id)
                || !item["revision"]
                    .as_u64()
                    .is_some_and(|r| r > 0 && r <= plan["revision"].as_u64().unwrap())
                || !item["payload"]["kind"].is_string()
            {
                return Err("mismatched history event".into());
            }
            next += line.len() as u64;
            items.push(item);
        }
        if items.is_empty() && cursor < end {
            return Err("unterminated or oversized history event".into());
        }
        Ok(
            json!({"items": items, "next_cursor": if next < end { json!(next) } else { Value::Null }, "event_end": end}),
        )
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
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("forge-architecture-{}", identity()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store::new(self.0.clone())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn plan() -> Value {
        json!({"goal": "goal", "custom": {"preserved": true}, "usage": {"codex": {"total_tokens": 77}},
            "stages": [{"id": 1, "title": "one", "instructions": "work", "acceptance": "test", "commit": "feat: one"}]})
    }
    fn publish(store: &Store) -> Value {
        store
            .publish(plan(), checkpoint_default(), json!({"kind": "created"}))
            .unwrap()
    }
    fn decision(plan: &Value, id: usize) -> Value {
        json!({"version": 1, "id": format!("decision-{id}"), "plan_id": plan["plan_id"],
            "stage_id": 1, "revision": plan["revision"], "summary": "a".repeat(1000),
            "rationale": "Keep the existing behavior", "alternatives": [{"description": "replace", "tradeoffs": "more risk"}],
            "status": "accepted", "supersedes": null, "created_unix": 10, "updated_unix": 10})
    }
    #[test]
    fn nested_checkpoint_records_reject_corruption_before_publication_and_on_load() {
        let temp = Temp::new();
        let store = temp.store();
        let p = publish(&store);
        let mut cp = checkpoint_default();
        cp["agreements"]["1"] = agreement_fixture(&p, 0);
        cp["guidance"]["1"] = json!({"version": 1, "id": "guidance-1", "stage_id": 1,
            "revision": 1, "relevant_inputs": crate::plan::stage_inputs(&p, 0),
            "text": "Preserve the boundary", "valid": true, "unix": 10});
        let p = store
            .publish(p, cp.clone(), json!({"kind": "context"}))
            .unwrap();
        let path = temp
            .0
            .join("architecture")
            .join(p["plan_id"].as_str().unwrap())
            .join("checkpoints")
            .join(format!(
                "{}.json",
                p["architecture"]["checkpoint"].as_str().unwrap()
            ));
        let bytes = fs::read(&path).unwrap();
        let original_plan = fs::read(temp.0.join("plan.json")).unwrap();
        let mut faults = Vec::new();
        for key in ["agreements", "guidance"] {
            for (field, value) in [
                ("version", json!(2)),
                ("revision", json!(999)),
                ("stage_id", json!(42)),
                ("id", json!("../bad")),
                ("valid", json!(null)),
                ("unix", json!(-1)),
                ("relevant_inputs", json!({})),
                ("plan_id", json!("OTHER-PLAN")),
            ] {
                let mut fault = cp.clone();
                fault[key]["1"][field] = value;
                faults.push(fault);
            }
            let mut fault = cp.clone();
            fault[key]["1"]["relevant_inputs"]["goal"] = json!("stale");
            faults.push(fault);
            let mut fault = cp.clone();
            fault[key]["42"] = fault[key]["1"].take();
            faults.push(fault);
        }
        for (field, value) in [
            ("effective", json!({"provider": "provider"})),
            ("planner_reason", json!("")),
            ("architect_reason", json!("")),
            ("provenance", json!({})),
            ("availability", json!("maybe")),
            ("kind", json!("proposal")),
            ("agreement_id", json!("another")),
            ("trigger", json!("")),
        ] {
            let mut fault = cp.clone();
            fault["agreements"]["1"][field] = value;
            faults.push(fault);
        }
        let mut fault = cp.clone();
        fault["guidance"]["1"]["text"] = json!("");
        faults.push(fault);
        let mut fault = cp.clone();
        fault["review_policy"]["required_roles"] = json!([]);
        faults.push(fault);
        for fault in faults {
            assert!(
                store
                    .publish(p.clone(), fault.clone(), json!({"kind": "bad"}))
                    .is_err(),
                "{fault}"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read(temp.0.join("plan.json")).unwrap(), original_plan);
            assert_eq!(store.load().unwrap().unwrap(), p);
            let mut bundle: Value = serde_json::from_slice(&bytes).unwrap();
            bundle["checkpoint"] = fault;
            atomic_json(&path, &bundle).unwrap();
            assert!(store.load_raw().is_err());
            fs::write(&path, &bytes).unwrap();
        }
        // Retained guidance can precede the current revision; removed records must be explicitly invalid.
        let mut next = p.clone();
        next["revision"] = json!(2);
        let next = store
            .publish(next, cp.clone(), json!({"kind": "unrelated_revision"}))
            .unwrap();
        let mut removed = next.clone();
        removed["revision"] = json!(3);
        removed["stages"] = json!([]);
        assert!(
            store
                .publish(removed.clone(), cp.clone(), json!({"kind": "bad_removal"}))
                .is_err()
        );
        for key in ["guidance", "agreements"] {
            cp[key]["1"]["valid"] = json!(false);
            cp[key]["1"]["invalidation_trigger"] = json!("stage_removed");
        }
        let removed = store
            .publish(removed, cp, json!({"kind": "removed"}))
            .unwrap();
        assert_eq!(store.load().unwrap().unwrap(), removed);
    }

    #[test]
    fn indexed_reviews_keep_polling_small_and_preserve_removed_and_archived_history() {
        let temp = Temp::new();
        let store = temp.store();
        let mut p = plan();
        let reviews: Vec<_> = (0..10_000)
            .map(|i| json!({"round": i, "summary": "legacy feedback", "custom": i}))
            .collect();
        p["stages"][0]["reviews"] = json!(reviews);
        p["stages"][0]["status"] = json!("committed");
        let p = store
            .publish(p, checkpoint_default(), json!({"kind": "legacy_import"}))
            .unwrap();
        assert_eq!(p["stages"][0]["reviews"], json!(reviews));
        let raw = store.load_raw().unwrap().unwrap();
        assert!(raw.to_string().len() < 5000);
        assert!(fs::metadata(temp.0.join("plan.json")).unwrap().len() < 8000);
        let state = store.state_plan(raw.clone());
        assert_eq!(state["stages"][0]["reviews"].as_array().unwrap().len(), 8);
        assert_eq!(state["stages"][0]["review_count"], 10_000);
        assert!(state.to_string().len() < 5000);
        // Direct seek near the end proves pages do not need to parse preceding records.
        let dir = store.review_dir(&p).unwrap();
        let data_path = dir.join(format!(
            "{}.jsonl",
            raw["architecture"]["review_history"]["1"]["file"]
                .as_str()
                .unwrap()
        ));
        let original = fs::read(&data_path).unwrap();
        let mut corrupt = original.clone();
        corrupt[0] = b'!';
        fs::write(&data_path, &corrupt).unwrap();
        assert!(store.load_raw().is_ok()); // polling checks the manifest, not unbounded history
        assert_eq!(
            store.reviews(None, 1, None, 9990, 10).unwrap()["items"],
            json!(reviews[9990..])
        );
        assert!(store.reviews(None, 1, None, 0, 10).is_err());
        fs::write(&data_path, original).unwrap();
        let mut collected = Vec::new();
        let mut cursor = 0;
        loop {
            let page = store.reviews(None, 1, None, cursor, 100).unwrap();
            collected.extend(page["items"].as_array().unwrap().clone());
            match page["next_cursor"].as_u64() {
                Some(next) => cursor = next,
                None => break,
            }
        }
        assert_eq!(collected, reviews);
        let mut removed = p.clone();
        removed["revision"] = json!(2);
        removed["stages"] = json!([]);
        store
            .publish(removed, checkpoint_default(), json!({"kind": "removed"}))
            .unwrap();
        assert_eq!(
            store.reviews(None, 1, None, 9999, 10).unwrap()["items"][0],
            reviews[9999]
        );
        store.reset().unwrap();
        publish(&store);
        assert_eq!(
            store.reviews(p["plan_id"].as_str(), 1, None, 0, 1).unwrap()["items"][0],
            reviews[0]
        );
    }

    #[test]
    fn legacy_round_trip_is_lazy_and_keeps_unknown_metadata() {
        let temp = Temp::new();
        let store = temp.store();
        let legacy = plan();
        atomic_json(&temp.0.join("plan.json"), &legacy).unwrap();
        assert_eq!(store.load().unwrap(), Some(legacy.clone()));
        assert!(!temp.0.join("architecture").exists());
        assert_eq!(
            store.summary(Some(&legacy)).unwrap()["context_status"],
            "legacy"
        );
        let imported = store
            .publish(
                legacy.clone(),
                checkpoint_default(),
                json!({"kind": "legacy_import"}),
            )
            .unwrap();
        let reloaded = temp.store().load().unwrap().unwrap();
        assert_eq!(reloaded, imported);
        for (k, v) in legacy.as_object().unwrap() {
            assert_eq!(&reloaded[k], v);
        }
        assert_eq!(reloaded["revision"], 1);
    }
    #[test]
    fn interrupted_writes_recover_at_the_single_publication_boundary() {
        for point in [
            "partial_event",
            "events",
            "reviews",
            "partial_snapshot",
            "checkpoint",
            "publication_error",
            "publication",
        ] {
            let temp = Temp::new();
            let store = temp.store();
            let mut initial = plan();
            initial["stages"][0]["reviews"] = json!([{"summary": "old review"}]);
            let old = store
                .publish(initial, checkpoint_default(), json!({"kind": "created"}))
                .unwrap();
            let old_cp = store.checkpoint(&old).unwrap();
            let old_bundle = fs::read(
                temp.0
                    .join("architecture")
                    .join(old["plan_id"].as_str().unwrap())
                    .join("checkpoints")
                    .join(format!(
                        "{}.json",
                        old["architecture"]["checkpoint"].as_str().unwrap()
                    )),
            )
            .unwrap();
            let mut revised = old.clone();
            revised["revision"] = json!(2);
            revised["goal"] = json!("new goal");
            revised["stages"][0]["reviews"]
                .as_array_mut()
                .unwrap()
                .push(json!({"summary": "new review"}));
            let mut cp = old_cp.clone();
            cp["summary"] = json!("new context");
            assert!(
                store
                    .publish_at(revised, cp, json!({"kind": "revision"}), point)
                    .is_err()
            );
            let recovered = temp.store().load().unwrap().unwrap();
            if point == "publication" {
                assert_eq!(recovered["goal"], "new goal");
                assert_eq!(store.reviews(None, 1, None, 0, 10).unwrap()["count"], 2);
                assert_eq!(
                    store.checkpoint(&recovered).unwrap()["summary"],
                    "new context"
                );
            } else {
                assert_eq!(recovered, old);
                assert_eq!(store.checkpoint(&old).unwrap(), old_cp);
                assert_eq!(
                    store.history(None, 0, 100).unwrap()["items"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
                let saved = store
                    .publish(old.clone(), old_cp, json!({"kind": "retry"}))
                    .unwrap();
                assert_eq!(
                    store.history(None, 0, 100).unwrap()["items"]
                        .as_array()
                        .unwrap()
                        .len(),
                    2
                );
                assert_eq!(saved["goal"], "goal");
            }
            assert_eq!(
                fs::read(
                    temp.0
                        .join("architecture")
                        .join(old["plan_id"].as_str().unwrap())
                        .join("checkpoints")
                        .join(format!(
                            "{}.json",
                            old["architecture"]["checkpoint"].as_str().unwrap()
                        ))
                )
                .unwrap(),
                old_bundle
            );
        }
    }
    #[test]
    fn failed_review_files_leave_the_last_plan_checkpoint_and_history_authoritative() {
        let temp = Temp::new();
        let store = temp.store();
        let old = publish(&store);
        let cp = store.checkpoint(&old).unwrap();
        let mut next = old.clone();
        next["revision"] = json!(2);
        next["stages"][0]["reviews"] = json!([{"summary": "new review"}]);
        let dir = store.review_dir(&old).unwrap();
        fs::write(&dir, b"blocked").unwrap();
        assert!(
            store
                .publish(next.clone(), cp.clone(), json!({"kind": "revision"}))
                .is_err()
        );
        assert_eq!(store.load().unwrap().unwrap(), old);
        assert_eq!(store.checkpoint(&old).unwrap(), cp);
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        fs::remove_file(&dir).unwrap();
        // A prepared but unpublished review snapshot cannot be read through the history API.
        assert!(
            store
                .publish_at(next, cp.clone(), json!({"kind": "revision"}), "checkpoint")
                .is_err()
        );
        let files = fs::read_dir(dir.parent().unwrap().join("checkpoints")).unwrap();
        for file in files {
            let path = file.unwrap().path();
            let token = path.file_stem().unwrap().to_str().unwrap();
            if token != old["architecture"]["checkpoint"].as_str().unwrap() {
                assert!(store.reviews(None, 1, Some(token), 0, 10).is_err());
            }
        }
        assert_eq!(store.load().unwrap().unwrap(), old);
        assert_eq!(store.checkpoint(&old).unwrap(), cp);
    }

    #[test]
    fn failed_snapshot_and_unsafe_session_never_replace_checkpoint() {
        let temp = Temp::new();
        let store = temp.store();
        let old = publish(&store);
        for cp in [
            json!({"version": 9}),
            {
                let mut cp = checkpoint_default();
                cp["summary"] = json!("x".repeat(CHECKPOINT_LIMIT));
                cp
            },
            {
                let mut cp = checkpoint_default();
                cp["session"] = json!({"provider": "codex", "reference": "session", "checkpoint_reference": "turn-2", "resume_policy": "latest"});
                cp
            },
        ] {
            assert!(
                store
                    .publish(old.clone(), cp, json!({"kind": "revision"}))
                    .is_err()
            );
            assert_eq!(store.load().unwrap(), Some(old.clone()));
        }
        // Real filesystem failure at snapshot preparation, after the event append.
        let checkpoints = temp
            .0
            .join("architecture")
            .join(old["plan_id"].as_str().unwrap())
            .join("checkpoints");
        let backup = checkpoints.with_extension("backup");
        fs::rename(&checkpoints, &backup).unwrap();
        fs::write(&checkpoints, b"blocked").unwrap();
        assert!(
            store
                .publish(
                    old.clone(),
                    checkpoint_default(),
                    json!({"kind": "revision"})
                )
                .is_err()
        );
        fs::remove_file(&checkpoints).unwrap();
        fs::rename(&backup, &checkpoints).unwrap();
        assert_eq!(store.load().unwrap(), Some(old));
    }
    #[test]
    fn malformed_mismatched_and_truncated_records_are_rejected() {
        for fault in [
            "plan",
            "checkpoint",
            "event",
            "truncated",
            "version",
            "path",
        ] {
            let temp = Temp::new();
            let store = temp.store();
            let old = publish(&store);
            let dir = temp
                .0
                .join("architecture")
                .join(old["plan_id"].as_str().unwrap());
            match fault {
                "plan" => {
                    let mut p = old.clone();
                    p["goal"] = json!("tampered");
                    atomic_json(&temp.0.join("plan.json"), &p).unwrap();
                }
                "checkpoint" => {
                    fs::write(
                        dir.join("checkpoints").join(format!(
                            "{}.json",
                            old["architecture"]["checkpoint"].as_str().unwrap()
                        )),
                        b"{",
                    )
                    .unwrap();
                }
                "event" => {
                    let path = dir.join("events.jsonl");
                    let text = fs::read_to_string(&path).unwrap();
                    fs::write(path, text.replace("created", "creat\"d")).unwrap();
                }
                "truncated" => {
                    fs::write(dir.join("events.jsonl"), b"").unwrap();
                }
                "version" => {
                    let mut p = old.clone();
                    p["contract_version"] = json!(99);
                    atomic_json(&temp.0.join("plan.json"), &p).unwrap();
                }
                _ => {
                    let mut p = old.clone();
                    p["plan_id"] = json!("../escape");
                    atomic_json(&temp.0.join("plan.json"), &p).unwrap();
                }
            }
            assert!(store.load().is_err(), "accepted {fault}");
            assert!(
                store
                    .publish(old, checkpoint_default(), json!({"kind": "retry"}))
                    .is_err()
            );
        }
    }
    #[test]
    fn decisions_are_bounded_and_history_is_paginated_with_archive_isolation() {
        let temp = Temp::new();
        let store = temp.store();
        let mut p = publish(&store);
        for id in 0..12 {
            let d = decision(&p, id);
            p = store.record(&p, "decision", d).unwrap();
        }
        let summary = store.summary(Some(&p)).unwrap();
        assert_eq!(summary["recent_decisions"].as_array().unwrap().len(), 8);
        assert!(summary.to_string().len() < 4000);
        let mut cursor = 0;
        let mut count = 0;
        loop {
            let page = store.history(None, cursor, 3).unwrap();
            assert!(page["items"].as_array().unwrap().len() <= 3);
            count += page["items"].as_array().unwrap().len();
            match page["next_cursor"].as_u64() {
                Some(n) => {
                    assert!(n > cursor);
                    cursor = n;
                }
                None => break,
            }
        }
        assert_eq!(count, 13);
        assert!(store.history(None, 1, 20).is_err());
        assert!(store.history(Some("../escape"), 0, 20).is_err());
        let old_id = p["plan_id"].as_str().unwrap().to_owned();
        store.reset().unwrap();
        assert!(store.load().unwrap().is_none());
        let new = publish(&store);
        assert_ne!(new["plan_id"], old_id);
        assert_eq!(
            store.history(Some(&old_id), 0, 1000).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            13
        );
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .publish(p, checkpoint_default(), json!({"kind": "reuse"}))
                .is_err()
        );
    }
    #[test]
    fn records_require_matching_identity_revision_and_complete_contracts() {
        let temp = Temp::new();
        let store = temp.store();
        let p = publish(&store);
        for (key, value) in [
            ("plan_id", json!("another")),
            ("revision", json!(2)),
            ("version", json!(2)),
            ("stage_id", json!(55)),
            ("rationale", json!("")),
        ] {
            let mut d = decision(&p, 1);
            d[key] = value;
            assert!(store.record(&p, "decision", d).is_err());
            assert_eq!(store.load().unwrap().unwrap(), p);
        }
        let d = decision(&p, 1);
        let updated = store.record(&p, "decision", d.clone()).unwrap();
        assert!(store.record(&p, "decision", d).is_err());
        assert_eq!(store.load().unwrap().unwrap(), updated);
    }
    #[test]
    fn model_contract_keeps_native_effort_reasons_and_provenance_without_routing() {
        let temp = Temp::new();
        let store = temp.store();
        let p = publish(&store);
        let mut record = json!({"version": 1, "id": "agreement-1", "kind": "agreement",
            "proposal_ids": ["planner-1", "architect-1"], "agreement_id": "agreement-1",
            "plan_id": p["plan_id"], "revision": p["revision"], "stage_id": 1,
            "relevant_inputs": crate::plan::stage_inputs(&p, 0),
            "effective": {"provider": "provider", "model": "model", "native_effort": "provider-native-effort"},
            "planner_reason": "bounded task", "architect_reason": "low design risk",
            "provenance": {"capability_policy_version": "policy-1", "catalogue_revision": "catalogue-2",
                "official_sources": [], "checked_unix": 10},
            "availability": "unverified", "trigger": "initial_assignment", "superseded_agreement": null, "unix": 10});
        let mut invalid = record.clone();
        invalid["architect_reason"] = json!("");
        assert!(store.record(&p, "model", invalid).is_err());
        let mut invalid = record.clone();
        invalid["relevant_inputs"]["goal"] = json!("old goal");
        assert!(store.record(&p, "model", invalid).is_err());
        let agreed = store.record(&p, "model", record.clone()).unwrap();
        let cp = store.checkpoint(&agreed).unwrap();
        assert_eq!(cp["agreements"]["1"]["effective"], record["effective"]);
        assert_eq!(cp["agreements"]["1"]["provenance"], record["provenance"]);
        assert_eq!(cp["agreements"]["1"]["valid"], true);
        assert_eq!(cp["context_status"], "inactive");
        record["id"] = json!("invalidation-1");
        record["kind"] = json!("invalidation");
        record["trigger"] = json!("dependency_changed");
        let invalidated = store.record(&agreed, "model", record).unwrap();
        assert_eq!(
            store.checkpoint(&invalidated).unwrap()["agreements"]["1"]["valid"],
            false
        );
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn decision_supersession_and_role_tagged_reviews_are_append_only() {
        let temp = Temp::new();
        let store = temp.store();
        let p = publish(&store);
        let first = decision(&p, 1);
        let p = store.record(&p, "decision", first.clone()).unwrap();
        let mut second = decision(&p, 2);
        second["supersedes"] = first["id"].clone();
        let p = store.record(&p, "decision", second).unwrap();
        assert_eq!(
            store.checkpoint(&p).unwrap()["recent_decisions"][0]["status"],
            "superseded"
        );
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"][1]["payload"]["record"],
            first
        );
        let review = json!({"version": 1, "id": "review-1", "role": "architect", "stage_id": 1,
            "revision": 1, "attempt_id": "attempt-1", "round": 1, "verdict": {"approved": true},
            "policy": {"version": 1, "required_roles": ["reviewer"], "scope": "all", "rationale": "existing gate"}, "unix": 10});
        let p = store.record(&p, "review", review.clone()).unwrap();
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"][3]["payload"]["record"],
            review
        );
        assert_eq!(
            store.checkpoint(&p).unwrap()["review_policy"]["required_roles"],
            json!(["reviewer"])
        );
    }
}
