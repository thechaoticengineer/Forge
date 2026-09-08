//! Lossless storage for large architecture records. Runtime contracts stay plain
//! JSON; repeated string values are stored once, without hashes or truncation.
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) const EXPANDED_LIMIT: usize = 4 * 1024 * 1024;
const COMPACT_THRESHOLD: usize = 64 * 1024;

fn encode(value: &Value, strings: &mut Vec<String>, indexes: &mut HashMap<String, usize>) -> Value {
    match value {
        Value::String(text) => {
            let next = strings.len();
            let index = *indexes.entry(text.clone()).or_insert_with(|| {
                strings.push(text.clone());
                next
            });
            json!({"string":index})
        }
        // Every object is tagged, so user values resembling a reference cannot
        // collide with the encoding. Arrays and scalar values keep their types.
        Value::Object(fields) => json!({"object":fields.iter().map(|(key, value)|
            (key.clone(), encode(value, strings, indexes))).collect::<Map<_, _>>()}),
        Value::Array(values) => {
            Value::Array(values.iter().map(|v| encode(v, strings, indexes)).collect())
        }
        _ => value.clone(),
    }
}

pub(crate) fn pack(value: &Value, limit: usize, kind: &str) -> Result<Value, String> {
    let expanded = serde_json::to_vec(value).map_err(|e| e.to_string())?.len();
    if expanded > EXPANDED_LIMIT {
        return Err(format!(
            "{kind} expands to {expanded} bytes; limit is {EXPANDED_LIMIT} bytes"
        ));
    }
    // Keep existing on-disk representations when they already fit.
    if expanded <= limit.min(COMPACT_THRESHOLD) {
        return Ok(value.clone());
    }
    let mut strings = Vec::new();
    let encoded = encode(value, &mut strings, &mut HashMap::new());
    let packed = json!({"$forge_compact":1,"strings":strings,"value":encoded});
    let stored = serde_json::to_vec(&packed)
        .map_err(|e| e.to_string())?
        .len();
    // Compaction is an optimization, independent of the storage acceptance
    // budget. Keep it for repeated content without penalizing unique facts.
    if expanded <= limit && expanded <= stored {
        return Ok(value.clone());
    }
    if stored > limit {
        return Err(format!(
            "{kind} needs {stored} bytes after deduplication ({expanded} expanded); limit is {limit} bytes"
        ));
    }
    Ok(packed)
}

fn charge(budget: &mut usize, bytes: usize) -> Result<(), String> {
    *budget = budget
        .checked_sub(bytes)
        .ok_or("compact architecture record exceeds 4 MiB expanded limit")?;
    Ok(())
}

fn decode(
    value: &Value,
    strings: &[String],
    sizes: &[usize],
    budget: &mut usize,
) -> Result<Value, String> {
    match value {
        Value::Object(tag) if tag.len() == 1 && tag.contains_key("string") => {
            let index = tag["string"]
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| *i < strings.len())
                .ok_or("invalid compact architecture string reference")?;
            charge(budget, sizes[index])?;
            Ok(json!(strings[index]))
        }
        Value::Object(tag) if tag.len() == 1 && tag.contains_key("object") => {
            let fields = tag["object"]
                .as_object()
                .ok_or("invalid compact architecture object")?;
            charge(budget, 2 + fields.len().saturating_sub(1))?;
            let mut result = Map::new();
            for (key, value) in fields {
                charge(
                    budget,
                    serde_json::to_vec(key).map_err(|e| e.to_string())?.len() + 1,
                )?;
                result.insert(key.clone(), decode(value, strings, sizes, budget)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(values) => {
            charge(budget, 2 + values.len().saturating_sub(1))?;
            values
                .iter()
                .map(|v| decode(v, strings, sizes, budget))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {
            charge(
                budget,
                serde_json::to_vec(value).map_err(|e| e.to_string())?.len(),
            )?;
            Ok(value.clone())
        }
        _ => Err("invalid compact architecture value".into()),
    }
}

pub(crate) fn unpack(value: Value, limit: usize, kind: &str) -> Result<Value, String> {
    let stored = serde_json::to_vec(&value).map_err(|e| e.to_string())?.len();
    if stored > limit {
        return Err(format!(
            "{kind} is {stored} bytes; stored limit is {limit} bytes"
        ));
    }
    // Native v1 records may contain arbitrary extra metadata. Their version
    // distinguishes them from the storage-only envelope, including collisions.
    if value.get("version").is_some() {
        return Ok(value);
    }
    if value["$forge_compact"] != 1 || value.as_object().is_none_or(|o| o.len() != 3) {
        return Err("unsupported compact architecture encoding".into());
    }
    let strings: Vec<String> =
        serde_json::from_value(value["strings"].clone()).map_err(|e| e.to_string())?;
    let sizes = strings
        .iter()
        .map(|s| {
            serde_json::to_vec(s)
                .map(|s| s.len())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut budget = EXPANDED_LIMIT;
    let decoded = decode(&value["value"], &strings, &sizes, &mut budget)?;
    if decoded["version"] != 1 {
        return Err("unsupported decoded architecture record".into());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_records_round_trip_json_types_and_reference_shaped_user_data() {
        let text = "Zażółć 🦀 \"quoted\"\n".repeat(100);
        let value = json!({"version":1,"repeated":vec![text; 40],
            "metadata":{"string":0,"object":{"$forge_compact":1},"strings":["a"],"value":null},
            "types":[null,true,false,0,-42,1.5,18446744073709551615u64,[],{},""],
            "escaped\nkey":"\\"});
        let packed = pack(&value, 16 * 1024, "test").unwrap();
        assert_eq!(packed["$forge_compact"], 1);
        assert_eq!(unpack(packed, 16 * 1024, "test").unwrap(), value);
        let old = json!({"version":1,"$forge_compact":999,"strings":"user metadata"});
        assert_eq!(pack(&old, 1024, "test").unwrap(), old);
        assert_eq!(unpack(old.clone(), 1024, "test").unwrap(), old);
    }

    #[test]
    fn corrupt_unsupported_or_amplified_records_are_rejected() {
        let valid = json!({"$forge_compact":1,"strings":["text"],"value":{"object":{
            "version":1,"summary":{"string":0}}}});
        for bad in [
            json!({"$forge_compact":9,"strings":[],"value":null}),
            json!({"$forge_compact":1,"strings":[42],"value":null}),
            json!({"$forge_compact":1,"strings":[],"value":{"string":0}}),
            json!({"$forge_compact":1,"strings":["text"],"value":{"string":-1}}),
            json!({"$forge_compact":1,"strings":[],"value":{"object":[]}}),
            json!({"$forge_compact":1,"strings":[],"value":{"object":{},"string":0}}),
            json!({"$forge_compact":1,"strings":[],"value":"untagged"}),
            json!({"$forge_compact":1,"strings":[],"value":{"object":{"version":2}}}),
            json!({"$forge_compact":1,"strings":["x".repeat(60_000)],
                "value":{"object":{"version":1,"data":vec![json!({"string":0});100]}}}),
        ] {
            assert!(unpack(bad, 64 * 1024, "test").is_err());
        }
        assert!(unpack(valid.clone(), 1, "test").is_err());
        assert!(unpack(valid, 1024, "test").is_ok());
        let unique = json!({"version":1,"summary":"x".repeat(64 * 1024)});
        let error = pack(&unique, 64 * 1024, "architectural checkpoint").unwrap_err();
        assert!(error.contains("after deduplication"));
        assert!(error.contains("65536"));
    }

    #[test]
    fn expanded_bound_counts_escaped_bytes_and_accepts_exact_limit() {
        let value = json!({"version":1,"s":"\n\"\\".repeat(100)});
        let mut strings = Vec::new();
        let encoded = encode(&value, &mut strings, &mut HashMap::new());
        let sizes = strings
            .iter()
            .map(|s| serde_json::to_vec(s).unwrap().len())
            .collect::<Vec<_>>();
        let size = serde_json::to_vec(&value).unwrap().len();
        let mut budget = size;
        assert_eq!(
            decode(&encoded, &strings, &sizes, &mut budget).unwrap(),
            value
        );
        assert_eq!(budget, 0);
        assert!(decode(&encoded, &strings, &sizes, &mut (size - 1)).is_err());
    }
}
