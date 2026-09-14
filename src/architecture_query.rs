use super::*;

impl Store {
    pub(crate) fn state_plan(&self, mut plan: Value) -> Value {
        let refs = plan["architecture"]["review_history"].clone();
        for stage in plan["stages"].as_array_mut().into_iter().flatten() {
            let reference = &refs[stage["id"].to_string()];
            // Add only to the polling projection, never to saved records.
            stage["review_snapshot"] = if reference["file"].is_string() {
                reference["file"].clone()
            } else {
                crate::review_history::legacy_snapshot(stage).map(Value::String).unwrap_or(Value::Null)
            };
            stage.as_object_mut().unwrap().remove("model_proposal_inputs");
            // Full selection dialogue and transitive inputs are archived events,
            // not polling data. Preserve the current choice and both rationales.
            if let Some(a) = stage.get_mut("model_agreement").and_then(Value::as_object_mut) {
                for key in ["dialogue", "relevant_inputs", "architectural_constraints"] {
                    a.remove(key);
                }
            }
            if let Some(calls) = stage["model_invocations"].as_array() {
                let count = calls.len().max(if stage["model_invocations_truncated"] == true {
                    stage["model_invocation_count"].as_u64().unwrap_or(0) as usize
                } else { 0 });
                let recent = calls.iter().skip(calls.len().saturating_sub(8)).cloned().collect::<Vec<_>>();
                stage["model_invocation_count"] = json!(count);
                stage["model_invocations_truncated"] = json!(count > 8);
                stage["model_invocations"] = json!(recent);
            }
            if let Some(history) = stage["reassessment"]["history"].as_array().cloned() {
                stage["reassessment"]["history_count"] = json!(history.len().max(
                    stage["reassessment"]["history_count"].as_u64().unwrap_or(0) as usize));
                stage["reassessment"]["history"] = json!(history.into_iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().map(|mut h| {
                    for key in ["old_agreement","new_agreement"] {
                        if h[key].is_object() { h[key] = json!({"id":h[key]["id"],"effective":h[key]["effective"]}); }
                    }
                    h
                }).collect::<Vec<_>>());
            }
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
        if plan["plan_review"].is_object() {
            let reference = plan["architecture"]["plan_review_history"].clone();
            if reference.is_object() {
                plan["plan_review"]["reviews"] = reference["recent"].clone();
                plan["plan_review"]["review_count"] = reference["count"].clone();
            }
        }
        crate::review_history::bounded(&mut plan);
        // References for removed stages stay on disk and remain accessible through pagination.
        if let Some(architecture) = plan.get_mut("architecture").and_then(Value::as_object_mut) {
            architecture.remove("review_history");
            architecture.remove("plan_review_history");
        }
        plan
    }
    pub(crate) fn summary(&self, plan: Option<&Value>) -> Result<Value, String> {
        self.summary_inner(plan, false)
    }
    pub(crate) fn state_summary(&self, plan: Option<&Value>) -> Result<Value, String> {
        self.summary_inner(plan, true)
    }
    fn summary_inner(&self, plan: Option<&Value>, polling: bool) -> Result<Value, String> {
        let Some(plan) = plan else {
            return Ok(Value::Null);
        };
        if plan.get("architecture").is_none() {
            return Ok(
                json!({"context_status": "legacy", "revision": null, "recent_decisions": []}),
            );
        }
        let cp = self.checkpoint_inner(plan, polling)?;
        let pending_path = self.directory(plan["plan_id"].as_str().unwrap())?.join("architect-pending.json");
        let recovery_needed = match fs::read(&pending_path) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map(|p| p["turn"] != cp["last_turn"]).unwrap_or(true),
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        };
        let mut guidance = cp["guidance"].clone();
        for g in guidance.as_object_mut().into_iter().flat_map(|g| g.values_mut()) {
            if let Some(g) = g.as_object_mut() { g.remove("relevant_inputs"); }
        }
        Ok(
            json!({"version": VERSION, "plan_id": plan["plan_id"], "revision": plan["revision"],
            "checkpoint": plan["architecture"]["checkpoint"], "event_end": plan["architecture"]["event_end"],
            "context_status": if recovery_needed { json!("needs_recovery") } else { cp["context_status"].clone() },
            "session":cp["session"], "recovery":cp["recovery"], "effective_model":cp["effective_model"],
            "guidance":guidance, "unresolved_risks":cp["unresolved_risks"], "constraints":cp["constraints"],
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
        let mut snapshot = reference["file"].clone();
        let mut page = if !reference.is_null() {
            crate::review_history::page(&self.review_dir(&plan)?, reference, cursor, limit)?
        } else {
            let stage = plan["stages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == stage_id)
                .ok_or("unknown review stage")?;
            snapshot = json!(crate::review_history::legacy_snapshot(stage)?);
            let records = crate::review_history::legacy_records(stage);
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
        page["snapshot"] = snapshot;
        page["revision"] = plan["revision"].clone();
        page["plan_id"] = plan["plan_id"].clone();
        page["stage_id"] = json!(stage_id);
        page["checkpoint"] = plan["architecture"]["checkpoint"].clone();
        Ok(page)
    }
    /// Byte cursor, max 100 records / 256 KiB stored / 4 MiB expanded.
    /// Never reads beyond the published prefix.
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
        f.take((end - cursor).min(HISTORY_PAGE_BYTES as u64))
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        let mut next = cursor;
        let mut expanded_bytes = 0;
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
            let item = storage::unpack(serde_json::from_slice(line).map_err(|e| e.to_string())?, EVENT_LIMIT - 1, "architecture event")?;
            let item_bytes = serde_json::to_vec(&item).map_err(|e| e.to_string())?.len();
            if !items.is_empty() && expanded_bytes + item_bytes > storage::EXPANDED_LIMIT {
                break;
            }
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
            expanded_bytes += item_bytes;
            items.push(item);
        }
        if items.is_empty() && cursor < end {
            return Err("unterminated or oversized history event".into());
        }
        Ok(
            json!({"plan_id":plan["plan_id"], "checkpoint":plan["architecture"]["checkpoint"],
                "items": items, "next_cursor": if next < end { json!(next) } else { Value::Null }, "event_end": end}),
        )
    }
}

