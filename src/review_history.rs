//! Immutable indexed review arrays. A plan/checkpoint selects the exact files;
//! unselected files left by interrupted publication are never authoritative.
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const PREVIEW_COUNT: usize = 8;
const PAGE_BYTES: usize = 256 * 1024;

// Read-only snapshot identity for pre-checkpoint histories. Include every record,
// including unknown legacy fields, so publication changes cannot reuse positions.
pub(crate) fn legacy_records(stage: &Value) -> Vec<Value> {
    let records = stage["reviews"].as_array().cloned().unwrap_or_default();
    if records.is_empty() && stage["last_verdict"].is_object() {
        vec![stage["last_verdict"].clone()]
    } else {
        records
    }
}

pub(crate) fn legacy_snapshot(stage: &Value) -> Result<String, String> {
    let records = legacy_records(stage);
    if records.is_empty() { return Ok("empty".into()); }
    crate::util::digest(&serde_json::to_vec(&records).map_err(|e| e.to_string())?)
}

fn preview(record: &Value) -> Value {
    if record.to_string().len() <= 4096 {
        return record.clone();
    }
    plan_preview(record)
}

fn plan_preview(record: &Value) -> Value {
    let strings = |key: &str| {
        record[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(1)
            .map(|s| s.chars().take(100).collect::<String>())
            .collect::<Vec<_>>()
    };
    json!({"round": record["round"].as_u64(), "approved": record["approved"].as_bool(),
        "role": record["role"].as_str().unwrap_or("").chars().take(40).collect::<String>(),
        "unix": record["unix"].as_i64(), "truncated": true,
        "issues": strings("issues"), "notes": strings("notes"), "checks": strings("checks"),
        "summary": record["summary"].as_str().unwrap_or("").chars().take(240).collect::<String>()})
}

pub(crate) fn bounded(plan: &mut Value) {
    for stage in plan["stages"].as_array_mut().into_iter().flatten() {
        if let Some(reviews) = stage["reviews"].as_array() {
            let count = reviews.len();
            let total = if stage["reviews_truncated"] == true {
                stage["review_count"]
                    .as_u64()
                    .unwrap_or(count as u64)
                    .max(count as u64)
            } else {
                count as u64
            };
            let recent: Vec<_> = reviews
                .iter()
                .skip(count.saturating_sub(PREVIEW_COUNT))
                .map(preview)
                .collect();
            let truncated = count > PREVIEW_COUNT || recent.iter().any(|r| r["truncated"] == true);
            stage["reviews"] = json!(recent);
            if truncated {
                stage["review_count"] = json!(total);
                stage["reviews_truncated"] = json!(true);
            }
        }
    }
    if let Some(review) = plan.get_mut("plan_review").filter(|r| r.is_object()) {
        *review = bounded_plan_review(review);
    } else if let Some(plan) = plan.as_object_mut() {
        plan.remove("plan_review");
    }
}

// Bound every auxiliary field, including arbitrary model metadata and identity
// strings. The byte budget counts JSON escaping, keys and container punctuation.
fn limited(value: &Value, budget: &mut usize, depth: usize) -> Value {
    let result = match value {
        Value::String(s) => json!(s.chars().take(240.min(budget.saturating_sub(2) / 6)).collect::<String>()),
        Value::Array(values) if depth > 0 => {
            *budget = budget.saturating_sub(2);
            return Value::Array(values.iter().take(8).map_while(|v| {
                if *budget < 5 { return None; }
                *budget -= 1;
                Some(limited(v, budget, depth - 1))
            }).collect());
        }
        Value::Object(fields) if depth > 0 => {
            *budget = budget.saturating_sub(2);
            let mut result = serde_json::Map::new();
            for (key, value) in fields.iter().take(32) {
                let cost = serde_json::to_vec(key).unwrap().len() + 2;
                if cost + 4 > *budget { break; }
                *budget -= cost;
                result.insert(key.clone(), limited(value, budget, depth - 1));
            }
            return Value::Object(result);
        }
        Value::Array(_) | Value::Object(_) => Value::Null,
        _ => value.clone(),
    };
    let bytes = result.to_string().len();
    if bytes > *budget { *budget = budget.saturating_sub(4); Value::Null }
    else { *budget -= bytes; result }
}

fn bounded_field(value: &Value) -> Value {
    limited(value, &mut 4096, 6)
}

fn bounded_plan_review(review: &Value) -> Value {
    let mut result = json!({});
    for key in ["version", "attempt_id", "status", "base", "head", "rounds", "budget",
        "required_roles", "fix_sha", "next_action", "usage", "role_usage", "outstanding_requests"] {
        if let Some(value) = review.get(key) {
            result[key] = bounded_field(value);
            if result[key] != *value || review[format!("{key}_truncated")] == true {
                result[format!("{key}_truncated")] = json!(true);
            }
        }
    }
    if let Some(gate) = review.get("gate").filter(|g| g.is_object()) {
        result["gate"] = json!({});
        for key in ["status", "roles", "requests", "identity", "policy"] {
            if let Some(value) = gate.get(key) {
                result["gate"][key] = bounded_field(value);
                if result["gate"][key] != *value || gate[format!("{key}_truncated")] == true {
                    result["gate"][format!("{key}_truncated")] = json!(true);
                }
            }
        }
    }
    for (key, count_key, flag) in [("reviews", "review_count", "reviews_truncated"),
        ("model_invocations", "model_invocation_count", "model_invocations_truncated")] {
        if let Some(records) = review[key].as_array() {
            let total = (records.len() as u64).max(review[count_key].as_u64().unwrap_or(0));
            let recent: Vec<_> = records.iter().skip(records.len().saturating_sub(PREVIEW_COUNT))
                .map(|r| if key == "reviews" { plan_preview(r) } else { bounded_field(r) }).collect();
            let truncated = review[flag] == true || total > recent.len() as u64
                || records.iter().rev().zip(recent.iter().rev()).any(|(a,b)| a != b)
                || recent.iter().any(|r| r["truncated"] == true);
            result[key] = json!(recent);
            result[count_key] = json!(total);
            result[flag] = json!(truncated);
        }
    }
    result
}

pub(crate) fn write(dir: &Path, records: &[Value]) -> Result<Value, String> {
    write_with_preview(dir, records, preview)
}

pub(crate) fn write_plan(dir: &Path, records: &[Value]) -> Result<Value, String> {
    write_with_preview(dir, records, plan_preview)
}

fn write_with_preview(dir: &Path, records: &[Value], preview: fn(&Value) -> Value) -> Result<Value, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let id = crate::architecture::identity();
    let mut data = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{id}.jsonl")))
        .map_err(|e| e.to_string())?;
    let mut index = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{id}.idx")))
        .map_err(|e| e.to_string())?;
    let mut end = 0u64;
    index
        .write_all(&end.to_le_bytes())
        .map_err(|e| e.to_string())?;
    for record in records {
        let mut bytes = serde_json::to_vec(record).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        data.write_all(&bytes).map_err(|e| e.to_string())?;
        end = end
            .checked_add(bytes.len() as u64)
            .ok_or("review length overflow")?;
        index
            .write_all(&end.to_le_bytes())
            .map_err(|e| e.to_string())?;
    }
    data.sync_all().map_err(|e| e.to_string())?;
    index.sync_all().map_err(|e| e.to_string())?;
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    let reference = json!({"version": 1, "file": id, "count": records.len(), "bytes": end,
        "recent": records.iter().skip(records.len().saturating_sub(PREVIEW_COUNT)).map(preview).collect::<Vec<_>>()});
    if all(dir, &reference)? != json!(records) {
        return Err("review snapshot readback mismatch".into());
    }
    Ok(reference)
}

pub(crate) fn validate(dir: &Path, reference: &Value) -> Result<(), String> {
    let id = reference["file"]
        .as_str()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .ok_or("invalid review file")?;
    let count = reference["count"].as_u64().ok_or("invalid review count")?;
    let bytes = reference["bytes"].as_u64().ok_or("invalid review bytes")?;
    if reference["version"] != 1
        || !reference["recent"].as_array().is_some_and(|a| {
            a.len() == count.min(PREVIEW_COUNT as u64) as usize
                && a.iter().all(|r| r.to_string().len() <= 4096)
        })
        || fs::metadata(dir.join(format!("{id}.jsonl")))
            .map_err(|e| e.to_string())?
            .len()
            != bytes
        || fs::metadata(dir.join(format!("{id}.idx")))
            .map_err(|e| e.to_string())?
            .len()
            != count
                .checked_add(1)
                .and_then(|n| n.checked_mul(8))
                .ok_or("review count overflow")?
    {
        return Err("invalid indexed review history".into());
    }
    Ok(())
}

/// Record cursor: seeks directly to the requested record, independent of history length.
/// One indivisible legacy record may exceed the normal page byte budget.
pub(crate) fn page(
    dir: &Path,
    reference: &Value,
    cursor: u64,
    limit: usize,
) -> Result<Value, String> {
    validate(dir, reference)?;
    let count = reference["count"].as_u64().unwrap();
    if cursor > count {
        return Err("cursor past review history".into());
    }
    let id = reference["file"].as_str().unwrap();
    let mut index = File::open(dir.join(format!("{id}.idx"))).map_err(|e| e.to_string())?;
    let mut data = File::open(dir.join(format!("{id}.jsonl"))).map_err(|e| e.to_string())?;
    index
        .seek(SeekFrom::Start(cursor * 8))
        .map_err(|e| e.to_string())?;
    let mut offset = [0; 8];
    index.read_exact(&mut offset).map_err(|e| e.to_string())?;
    let mut start = u64::from_le_bytes(offset);
    if cursor == 0 && start != 0 {
        return Err("invalid review index start".into());
    }
    let mut next = cursor;
    let mut items = Vec::new();
    let mut used = 0;
    while next < count && items.len() < limit.clamp(1, 100) {
        index.read_exact(&mut offset).map_err(|e| e.to_string())?;
        let end = u64::from_le_bytes(offset);
        if end <= start || end > reference["bytes"].as_u64().unwrap() {
            return Err("invalid review index position".into());
        }
        let size = usize::try_from(end - start).map_err(|e| e.to_string())?;
        if !items.is_empty() && used + size > PAGE_BYTES {
            break;
        }
        data.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; size];
        data.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.last() != Some(&b'\n') {
            return Err("unterminated review".into());
        }
        items.push(serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?);
        used += size;
        next += 1;
        start = end;
    }
    if next == count && start != reference["bytes"].as_u64().unwrap() {
        return Err("invalid review index end".into());
    }
    Ok(json!({"items": items, "count": count,
        "next_cursor": if next < count { json!(next) } else { Value::Null }}))
}

pub(crate) fn all(dir: &Path, reference: &Value) -> Result<Value, String> {
    let mut cursor = 0;
    let mut records = Vec::new();
    loop {
        let page = page(dir, reference, cursor, 100)?;
        records.extend(page["items"].as_array().unwrap().iter().cloned());
        match page["next_cursor"].as_u64() {
            Some(next) => cursor = next,
            None => break,
        }
    }
    Ok(json!(records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_previews_keep_total_count_and_bound_large_unicode_feedback() {
        let record = json!({"round": 2, "approved": true, "summary": "\u{0001}".repeat(5000),
            "issues": ["界🙂".repeat(5000)], "notes": ["\u{0002}".repeat(5000)],
            "checks": ["\u{0003}".repeat(5000)], "role": "r".repeat(5000)});
        let mut plan = json!({"stages": [{"reviews": vec![record; 12]}]});
        bounded(&mut plan);
        let first = plan.clone();
        bounded(&mut plan);
        assert_eq!(first, plan);
        assert_eq!(plan["stages"][0]["review_count"], 12);
        let records = plan["stages"][0]["reviews"].as_array().unwrap();
        assert_eq!(records.len(), 8);
        assert!(records.iter().all(|r| r.to_string().len() <= 4096));
        assert!(!records[0]["issues"].as_array().unwrap().is_empty());
    }
}
