use crate::util::unix_timestamp;
use serde_json::{Value, json};

pub(crate) fn default_settings() -> Value {
    json!({
        "projects_root": "",
        "planner": "claude",
        "implementer": "codex",
        "reviewer": "claude",
        "planner_model": "",
        "implementer_model": "",
        "reviewer_model": "",
        "max_fix_rounds": 3,
        "auto_push": true,
        "queue_auto_approve": false,
    })
}

/// Build a replacement only after validation, leaving the saved plan untouched on errors.
pub(crate) fn edit_plan(plan: &Value, body: &Value) -> Result<Value, &'static str> {
    let old_stages = plan["stages"].as_array().ok_or("no plan")?;
    let incoming = body.get("plan").filter(|value| value.is_object())
        .ok_or("plan must be an object")?;
    let stages = incoming["stages"].as_array().filter(|stages| !stages.is_empty())
        .ok_or("stages must be a non-empty array")?;
    let mut submitted_ids = Vec::new();
    for stage in stages {
        for field in ["title", "instructions", "acceptance", "commit"] {
            let text = stage[field].as_str().ok_or("stage content fields must be strings")?;
            if matches!(field, "title" | "instructions") && text.trim().is_empty() {
                return Err("stage title and instructions must not be blank");
            }
        }
        if let Some(id) = stage.get("id") {
            if submitted_ids.contains(&id) {
                return Err("duplicate stage ids");
            }
            submitted_ids.push(id);
        }
    }

    let committed: Vec<&Value> = old_stages.iter()
        .filter(|stage| stage["status"] == "committed").collect();
    for stage in &committed {
        if !submitted_ids.contains(&&stage["id"]) {
            return Err("cannot remove a committed stage");
        }
    }
    // Completed work remains a prefix in its original order; editable stages may move freely.
    for (index, stage) in committed.iter().enumerate() {
        if stages[index].get("id") != stage.get("id") {
            return Err("cannot reorder committed stages");
        }
    }

    let valid_id = |stage: &Value| stage["id"].as_i64().filter(|id| *id > 0);
    let mut next_id = old_stages.iter().filter_map(valid_id).max().unwrap_or(0)
        .max(plan["stage_id_high_water"].as_i64().unwrap_or(0));
    let historic_max = next_id;
    let mut used_ids: Vec<i64> = stages.iter().filter_map(valid_id).collect();
    let mut edited_stages = Vec::with_capacity(stages.len());
    for (index, stage) in stages.iter().enumerate() {
        if index < committed.len() {
            edited_stages.push(committed[index].clone());
            continue;
        }
        let id = match valid_id(stage) {
            Some(id) if old_stages.iter().any(|s| s["id"] == id) || id > historic_max => id,
            _ => loop {
                next_id = next_id.checked_add(1).ok_or("stage id limit reached")?;
                if !used_ids.contains(&next_id) {
                    used_ids.push(next_id);
                    break next_id;
                }
            },
        };
        let mut updated = old_stages.iter().find(|old| old["id"] == id)
            .cloned().unwrap_or_else(|| json!({"id": id, "status": "pending", "rounds": 0}));
        for key in ["title", "instructions", "acceptance", "commit"] {
            updated[key] = stage[key].clone();
        }
        if let Some(deps) = stage.get("depends_on") { updated["depends_on"] = deps.clone(); }
        edited_stages.push(updated);
    }
    let mut edited = plan.clone();
    if let Some(goal) = incoming["goal"].as_str().filter(|goal| !goal.trim().is_empty()) {
        edited["goal"] = json!(goal);
    }
    edited["stages"] = json!(edited_stages);
    edited["status"] = json!("draft");
    edited["stage_id_high_water"] = json!(used_ids.into_iter().max().unwrap_or(0).max(next_id));
    reconcile(plan, &mut edited)?;
    if let Some(revision) = plan["revision"].as_u64() {
        edited["revision"] = json!(revision.checked_add(1).ok_or("revision limit reached")?);
    }
    Ok(edited)
}

pub(crate) fn mutate_queue(queue: &mut Value, action: &str, body: &Value) -> Result<String, &'static str> {
    let items = queue["items"].as_array_mut().ok_or("invalid queue")?;
    match action {
        "add" => {
            let goal = body["goal"].as_str().unwrap_or("").trim();
            if goal.is_empty() {
                return Err("goal required");
            }
            let id = items.iter().filter_map(|item| item["id"].as_u64())
                .max().unwrap_or(0).checked_add(1).ok_or("queue id limit reached")?;
            items.push(json!({
                "id": id,
                "goal": goal,
                "status": "queued",
                "added_unix": unix_timestamp(),
            }));
            Ok(format!("added goal {id}"))
        }
        "remove" | "move" => {
            let id = body["id"].as_u64().ok_or("id required")?;
            let idx = items.iter().position(|item| item["id"].as_u64() == Some(id))
                .ok_or("queue item not found")?;
            if items[idx]["status"] != "queued"
                && !(action == "remove"
                    && matches!(items[idx]["status"].as_str(), Some("failed" | "blocked")))
            {
                return Err("item is not queued");
            }
            if action == "remove" {
                items.remove(idx);
                return Ok(format!("removed goal {id}"));
            }
            let dir = body["dir"].as_str().unwrap_or("");
            let neighbor = match dir {
                "up" => (0..idx).rev().find(|&i| items[i]["status"] == "queued"),
                "down" => (idx + 1..items.len()).find(|&i| items[i]["status"] == "queued"),
                _ => return Err("dir must be up or down"),
            };
            if let Some(neighbor) = neighbor {
                items.swap(idx, neighbor);
            }
            Ok(format!("moved goal {id} {dir}"))
        }
        "clear" => {
            let before = items.len();
            items.retain(|item| item["status"] != "queued");
            Ok(format!("cleared {} queued goals", before - items.len()))
        }
        _ => Err("unknown queue action"),
    }
}

/// Absent dependency lists conservatively mean all preceding stages. Explicit
/// lists permit unrelated edits and reordering without losing model agreements.
pub(crate) fn stage_inputs(plan: &Value, index: usize) -> Value {
    let stages = plan["stages"].as_array().unwrap();
    let mut inputs = direct_stage_inputs(plan, index);
    let mut dependencies = serde_json::Map::new();
    let mut pending = inputs["dependencies"].as_array().cloned().unwrap_or_default();
    while let Some(id) = pending.pop() {
        let key = id.to_string();
        if dependencies.contains_key(&key) { continue; }
        if let Some(i) = stages[..index].iter().position(|s| s["id"] == id) {
            let dependency = direct_stage_inputs(plan, i);
            pending.extend(dependency["dependencies"].as_array().into_iter().flatten().cloned());
            dependencies.insert(key, dependency);
        }
    }
    inputs["dependency_inputs"] = json!(dependencies);
    inputs
}

fn direct_stage_inputs(plan: &Value, index: usize) -> Value {
    let stages = plan["stages"].as_array().unwrap();
    let stage = &stages[index];
    let dependencies = stage.get("depends_on").cloned().unwrap_or_else(||
        json!(stages[..index].iter().map(|s| s["id"].clone()).collect::<Vec<_>>()));
    json!({"goal": plan["goal"], "title": stage["title"], "instructions": stage["instructions"],
        "acceptance": stage["acceptance"], "commit": stage["commit"], "dependencies": dependencies})
}

pub(crate) fn affected_stages(old: &Value, new: &Value) -> Result<Vec<Value>, &'static str> {
    let stages = new["stages"].as_array().ok_or("invalid stages")?;
    let ids: Vec<Value> = stages.iter().map(|s| s["id"].clone()).collect();
    for (i, stage) in stages.iter().enumerate() {
        if let Some(deps) = stage.get("depends_on") {
            let deps = deps.as_array().ok_or("depends_on must be an array")?;
            if deps.iter().any(|d| !ids[..i].contains(d)) { return Err("dependencies must refer to earlier stages"); }
        }
    }
    let old_stages = old["stages"].as_array().ok_or("invalid old stages")?;
    let mut affected: Vec<Value> = old_stages.iter().filter(|s| !ids.contains(&s["id"]))
        .map(|s| s["id"].clone()).collect();
    for (i, id) in ids.iter().enumerate() {
        let inputs = stage_inputs(new, i);
        let changed = old_stages.iter().position(|s| s["id"] == *id)
            .is_none_or(|j| stage_inputs(old, j) != inputs)
            || inputs["dependencies"].as_array().unwrap().iter().any(|d| affected.contains(d));
        if changed { affected.push(id.clone()); }
    }
    Ok(affected)
}

pub(crate) fn reconcile(old: &Value, new: &mut Value) -> Result<(), &'static str> {
    let affected = affected_stages(old, new)?;
    for stage in new["stages"].as_array_mut().unwrap() {
        if !affected.contains(&stage["id"]) || stage["status"] == "committed" { continue; }
        stage["status"] = json!("pending");
        stage["rounds"] = json!(0);
        stage["context_valid"] = json!(false);
        stage["last_verdict_valid"] = json!(false);
        for key in ["guidance", "model_agreement"] {
            if stage[key].is_object() { stage[key]["valid"] = json!(false); }
        }
        for key in ["started_unix", "finished_unix", "duration_secs", "sha"] {
            stage.as_object_mut().unwrap().remove(key);
        }
    }
    Ok(())
}
