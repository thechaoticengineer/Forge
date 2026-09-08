//! Pure accounting for measured invocations and accumulated planner revisions.
use crate::agent::AgentUsage;
use serde_json::{Value, json};

/// Add one measured invocation, normalizing containers only for nonempty usage.
pub(crate) fn accumulate_invocation_usage(parent: &mut Value, key: &str, tool: &str, usage: &AgentUsage) {
    if usage.is_empty() {
        return;
    }
    if !parent.is_object() {
        *parent = json!({});
    }
    let target = &mut parent[key];
    if !target.is_object() {
        *target = json!({});
    }
    let totals = &mut target[tool];
    if !totals.is_object() {
        *totals = json!({});
    }
    for (field, amount) in [
        ("input_tokens", usage.input_tokens),
        ("output_tokens", usage.output_tokens),
        ("total_tokens", usage.total_tokens),
        ("calls", 1),
    ] {
        totals[field] = json!(totals[field].as_i64().unwrap_or(0) + amount);
    }
    if !totals["models"].is_object() {
        totals["models"] = json!({});
    }
    if !usage.model.is_empty() {
        let model_total = &mut totals["models"][&usage.model];
        *model_total = json!(model_total.as_i64().unwrap_or(0) + usage.total_tokens);
    }
}

/// Add revision totals, including their existing calls, without counting a new invocation.
/// Unlike invocation accounting, this preserves all supplied integer fields and
/// leaves the plan untouched until a provider entry is visited.
pub(crate) fn merge_planner_revision_totals(plan: &mut Value, usage: &Value) {
    for (tool, value) in usage.as_object().into_iter().flatten() {
        let target = &mut plan["planner_usage"];
        if !target.is_object() { *target = json!({}); }
        if !target[tool].is_object() { target[tool] = json!({}); }
        for (key, count) in value.as_object().into_iter().flatten() {
            if let Some(n) = count.as_i64() { target[tool][key] = json!(target[tool][key].as_i64().unwrap_or(0) + n); }
            else if key == "models" {
                if !target[tool][key].is_object() { target[tool][key] = json!({}); }
                for (model, n) in count.as_object().into_iter().flatten() { target[tool][key][model] = json!(target[tool][key][model].as_i64().unwrap_or(0) + n.as_i64().unwrap_or(0)); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_accumulates_separate_tools_and_models_and_skips_empty_values() {
        let mut target = Value::Null;
        accumulate_invocation_usage(&mut target, "usage", "codex", &AgentUsage::default());
        assert!(target.is_null());
        target = json!({"goal": "Test goal"});
        accumulate_invocation_usage(&mut target, "usage", "codex", &AgentUsage::default());
        assert_eq!(target, json!({"goal": "Test goal"}));
        let mut usage = AgentUsage {
            input_tokens: 80, output_tokens: 20, total_tokens: 100, model: "model-a".into(),
        };
        accumulate_invocation_usage(&mut target, "usage", "codex", &usage);
        accumulate_invocation_usage(&mut target, "usage", "codex", &usage);
        usage.model = "model-b".into();
        accumulate_invocation_usage(&mut target, "usage", "codex", &usage);
        usage.model.clear();
        accumulate_invocation_usage(&mut target, "usage", "claude", &usage);
        accumulate_invocation_usage(&mut target, "usage", "unused", &AgentUsage::default());
        accumulate_invocation_usage(&mut target, "planner_usage", "codex", &AgentUsage::default());
        assert_eq!(target, json!({
            "goal": "Test goal",
            "usage": {
                "codex": {"input_tokens": 240, "output_tokens": 60, "total_tokens": 300,
                    "calls": 3, "models": {"model-a": 200, "model-b": 100}},
                "claude": {"input_tokens": 80, "output_tokens": 20, "total_tokens": 100,
                    "calls": 1, "models": {}},
            },
        }));
    }

    #[test]
    fn empty_invocations_leave_every_container_untouched() {
        // Emptiness is determined by total_tokens, even with other populated fields.
        let usage = AgentUsage {
            input_tokens: 8, output_tokens: 2, total_tokens: 0, model: "model".into(),
        };
        for original in [
            Value::Null, json!(false), json!(7), json!("legacy"), json!([]),
            json!({}), json!({"usage": []}), json!({"usage": {"codex": false}}),
            json!({"usage": {"codex": {"models": "legacy", "calls": 3}}}),
        ] {
            let mut target = original.clone();
            accumulate_invocation_usage(&mut target, "usage", "codex", &usage);
            assert_eq!(target, original);
        }
    }

    #[test]
    fn invocation_normalizes_non_object_containers_and_unnamed_models() {
        let usage = AgentUsage {
            input_tokens: 8, output_tokens: 2, total_tokens: 10, model: String::new(),
        };
        let expected = json!({"input_tokens": 8, "output_tokens": 2,
            "total_tokens": 10, "calls": 1, "models": {}});
        for malformed in [Value::Null, json!(false), json!(7), json!("legacy"), json!([])] {
            let mut parent = malformed.clone();
            accumulate_invocation_usage(&mut parent, "usage", "codex", &usage);
            assert_eq!(parent, json!({"usage": {"codex": expected}}));

            let mut parent = json!({"keep": true, "usage": malformed});
            accumulate_invocation_usage(&mut parent, "usage", "codex", &usage);
            assert_eq!(parent, json!({"keep": true, "usage": {"codex": expected}}));

            let mut parent = json!({"usage": {"other": "keep", "codex": malformed}});
            accumulate_invocation_usage(&mut parent, "usage", "codex", &usage);
            assert_eq!(parent, json!({"usage": {"other": "keep", "codex": expected}}));

            let mut parent = json!({"usage": {"codex": {"models": malformed, "note": "keep"}}});
            accumulate_invocation_usage(&mut parent, "usage", "codex", &usage);
            assert_eq!(parent, json!({"usage": {"codex": {
                "input_tokens": 8, "output_tokens": 2, "total_tokens": 10,
                "calls": 1, "models": {}, "note": "keep",
            }}}));
        }
    }

    #[test]
    fn invocation_uses_integer_fallback_and_preserves_unknown_fields() {
        let usage = AgentUsage {
            input_tokens: 8, output_tokens: 2, total_tokens: 10, model: "model".into(),
        };
        for old in [Value::Null, json!(false), json!("12"), json!(1.5), json!([]), json!({}), json!(u64::MAX)] {
            let mut target = json!({"usage": {"codex": {
                "input_tokens": old, "output_tokens": -1, "total_tokens": old,
                "calls": old, "models": {"model": old, "other": "keep", "": 9},
                "legacy_count": 42, "note": {"keep": true},
            }}});
            accumulate_invocation_usage(&mut target, "usage", "codex", &usage);
            assert_eq!(target, json!({"usage": {"codex": {
                "input_tokens": 8, "output_tokens": 1, "total_tokens": 10,
                "calls": 1, "models": {"model": 10, "other": "keep", "": 9},
                "legacy_count": 42, "note": {"keep": true},
            }}}));
        }
    }

    #[test]
    fn revision_merges_multiple_providers_and_models_into_legacy_totals() {
        let mut plan = json!({"goal": "Keep", "planner_usage": {
            "codex": {"total_tokens": 10, "calls": 2, "legacy_count": 7,
                "models": {"old": 10, "new": "unknown", "untouched": "keep"},
                "note": {"keep": true}},
            "claude": {"input_tokens": "unknown", "calls": 1.5, "models": []},
            "unused": {"total_tokens": 99},
        }, "role_usage": {"planner": "synchronized by caller"}});
        let revision = json!({
            "codex": {"input_tokens": 4, "output_tokens": 1, "total_tokens": 5,
                "calls": 2, "legacy_count": 3,
                "models": {"old": 2, "new": 3, "": 1}, "note": "ignored"},
            "claude": {"input_tokens": 6, "output_tokens": 2, "total_tokens": 8,
                "calls": 1, "models": {"same": 8}},
            "new-provider": {"total_tokens": 4, "models": {"same": 4}},
        });
        merge_planner_revision_totals(&mut plan, &revision);
        assert_eq!(plan, json!({"goal": "Keep", "planner_usage": {
            "codex": {"input_tokens": 4, "output_tokens": 1, "total_tokens": 15,
                "calls": 4, "legacy_count": 10,
                "models": {"old": 12, "new": 3, "": 1, "untouched": "keep"},
                "note": {"keep": true}},
            "claude": {"input_tokens": 6, "output_tokens": 2, "total_tokens": 8,
                "calls": 1, "models": {"same": 8}},
            "new-provider": {"total_tokens": 4, "models": {"same": 4}},
            "unused": {"total_tokens": 99},
        }, "role_usage": {"planner": "synchronized by caller"}}));
    }

    #[test]
    fn revision_initializes_missing_and_non_object_totals_without_extra_calls() {
        let revision = json!({"codex": {"total_tokens": 5, "calls": 3,
            "models": {"model": 5}}, "claude": {"total_tokens": 2}});
        for mut plan in [
            json!({"goal": "Keep"}), json!({"goal": "Keep", "planner_usage": null}),
            json!({"goal": "Keep", "planner_usage": 20}),
            json!({"goal": "Keep", "planner_usage": "legacy"}),
            json!({"goal": "Keep", "planner_usage": []}),
            json!({"goal": "Keep", "planner_usage": false}),
            json!({"goal": "Keep", "planner_usage": {"codex": [], "claude": 7}}),
        ] {
            merge_planner_revision_totals(&mut plan, &revision);
            assert_eq!(plan, json!({"goal": "Keep", "planner_usage": {
                "codex": {"total_tokens": 5, "calls": 3, "models": {"model": 5}},
                "claude": {"total_tokens": 2},
            }}));
        }
    }

    #[test]
    fn empty_or_non_object_revisions_do_not_normalize_or_insert_totals() {
        for revision in [Value::Null, json!({}), json!([]), json!(false), json!(5), json!("legacy")] {
            for original in [Value::Null, json!(false), json!([]), json!({}),
                json!({"planner_usage": "legacy"}), json!({"planner_usage": {"codex": 5}}),
            ] {
                let mut plan = original.clone();
                merge_planner_revision_totals(&mut plan, &revision);
                assert_eq!(plan, original);
            }
        }
    }

    #[test]
    fn revision_preserves_permissive_provider_and_model_container_handling() {
        for malformed in [Value::Null, json!(false), json!("legacy"), json!(1.5), json!([]), json!(u64::MAX)] {
            let mut plan = json!({"planner_usage": {
                "empty": malformed, "existing": {"note": "keep", "models": {"old": 4}},
                "models": {"models": malformed},
            }});
            merge_planner_revision_totals(&mut plan, &json!({
                "empty": malformed, "existing": {"models": malformed},
                "models": {"models": malformed}, "new": malformed,
            }));
            assert_eq!(plan, json!({"planner_usage": {
                "empty": {}, "existing": {"note": "keep", "models": {"old": 4}},
                "models": {"models": {}}, "new": {},
            }}));
        }
        // Even a non-object provider entry triggers destination normalization.
        let mut plan = Value::Null;
        merge_planner_revision_totals(&mut plan, &json!({"codex": 7}));
        assert_eq!(plan, json!({"planner_usage": {"codex": {}}}));
    }

    #[test]
    fn revision_integer_fields_take_precedence_over_models_and_use_numeric_fallback() {
        let mut plan = json!({"planner_usage": {
            "integer-models": {"models": 7, "calls": "unknown"},
            "object-models": {"models": {"old": 4}},
            "model-values": {"models": {"old": 4, "bad": "unknown"},
                "unknown": {"keep": true}, "large": 6, "float": 7},
        }});
        merge_planner_revision_totals(&mut plan, &json!({
            "integer-models": {"models": 3, "calls": 2},
            "object-models": {"models": 3},
            "model-values": {"models": {"old": "ignored", "bad": 3,
                "new": null, "large": u64::MAX, "float": 1.5},
                "unknown": {"ignored": true}, "large": u64::MAX, "float": 1.5,
                "new-text": "ignored", "new-integer": -2},
        }));
        assert_eq!(plan, json!({"planner_usage": {
            "integer-models": {"models": 10, "calls": 2},
            "object-models": {"models": 3},
            "model-values": {"models": {"old": 4, "bad": 3, "new": 0,
                "large": 0, "float": 0}, "unknown": {"keep": true},
                "large": 6, "float": 7, "new-integer": -2},
        }}));
    }
}
