//! Durable engine preferences, including model policy, in one atomic snapshot.
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(crate) fn validate(settings: &Value) -> Result<(), String> {
    if !settings["automatic_routing"].is_boolean() || !(settings["routing_billing_basis"].is_null() || settings["routing_billing_basis"].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 64)) {
        return Err("automatic_routing must be boolean; routing_billing_basis must be null or an exact billing basis (max 64 bytes)".into());
    }
    if !matches!(settings["reviewer_provider_mode"].as_str(), Some("other_provider" | "configured")) {
        return Err("reviewer_provider_mode must be other_provider or configured".into());
    }
    let limits = &settings["reassessment_limits"];
    if limits.as_object().is_none_or(|o| o.len() != 4) || [("max_reassessments",0,8),("max_operational_retries",0,5),("repeat_threshold",2,10),("context_percent",50,95)].iter().any(|(k,min,max)| limits[*k].as_u64().is_none_or(|v| v < *min || v > *max)) {
        return Err("invalid reassessment_limits: reassessments 0..8, retries 0..5, repeat threshold 2..10, context percent 50..95".into());
    }
    let cadence = &settings["review_cadence"];
    if cadence.as_object().is_none_or(|o| o.len() != 2) || ["architect", "reviewer"].iter().any(|role| !matches!(cadence[*role].as_str(), Some("per_stage" | "per_plan"))) {
        return Err("invalid review_cadence: use exactly architect and reviewer, each set to \"per_stage\" or \"per_plan\"".into());
    }
    for role in ["planner", "architect", "implementer", "reviewer"] {
        if !matches!(settings[role].as_str(), Some("codex" | "claude" | "mock")) {
            return Err(format!("invalid {role} provider"));
        }
        let key = format!("{role}_model");
        if !settings[&key].as_str().is_some_and(|s| s.is_empty() || crate::catalogue::identifier(s)) {
            return Err(format!("invalid {role} model"));
        }
    }
    for key in ["auto_push", "queue_auto_approve"] {
        if !settings[key].is_boolean() { return Err(format!("{key} must be boolean")); }
    }
    if !settings["projects_root"].is_string() || settings["max_fix_rounds"].as_u64().is_none() {
        return Err("invalid projects_root or max_fix_rounds".into());
    }
    crate::catalogue::Policy::from_settings(settings)?;
    Ok(())
}

pub(crate) fn load(path: &Path, defaults: &Value) -> Result<Option<Value>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read engine settings {}: {e}", path.display())),
    };
    let saved: Value = serde_json::from_reader(file)
        .map_err(|e| format!("invalid engine settings {}: {e}", path.display()))?;
    if saved["version"] != 1 || !saved["settings"].is_object() {
        return Err(format!("invalid engine settings schema: {}", path.display()));
    }
    let mut loaded = defaults.clone();
    for (key, value) in saved["settings"].as_object().unwrap() {
        if loaded.get(key).is_none() { return Err(format!("unknown engine setting: {key}")); }
        loaded[key] = value.clone();
    }
    validate(&loaded)?;
    Ok(Some(loaded))
}

pub(crate) fn save(path: &Path, settings: &Value) -> Result<(), String> {
    validate(settings)?;
    let mut persisted = crate::plan::default_settings();
    for (key, value) in persisted.as_object_mut().unwrap() {
        *value = settings[key].clone();
    }
    crate::catalogue::write_atomic_json(path, &json!({"version":1,"settings":persisted}))
        .map_err(|e| format!("could not save engine settings {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_allow_legacy_migration_but_corruption_does_not_reset_roles() {
        let f = crate::test_support::QueueTest::new(false);
        let path = f.path.join("settings.json");
        let defaults = crate::plan::default_settings();
        assert!(load(&path, &defaults).unwrap().is_none());
        for invalid in ["not json".into(), json!({"version":2,"settings":{}}).to_string(),
            json!({"version":1,"settings":{"planner":null}}).to_string(),
            json!({"version":1,"settings":{"auto_push":"yes"}}).to_string(),
            json!({"version":1,"settings":{"reviewer_provider_mode":"unknown"}}).to_string()] {
            fs::write(&path,invalid).unwrap();
            assert!(load(&path, &defaults).is_err());
        }
        fs::write(&path,json!({"version":1,"settings":{"planner":"codex","auto_push":false}}).to_string()).unwrap();
        let loaded = load(&path, &defaults).unwrap().unwrap();
        assert_eq!(loaded["planner"],"codex");
        assert_eq!(loaded["auto_push"],false);
        assert_eq!(loaded["review_cadence"],defaults["review_cadence"]);
    }
}
