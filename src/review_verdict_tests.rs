use super::*;
use super::test_support::*;

#[test]
fn combined_conflicting_requests_get_clarification_without_losing_authority() {
    let f = Fixture::new("Implement feature", 1);
    f.setting("mock_verdicts", json!([reject("Use format A"), clean()]));
    f.setting("mock_architect_verdicts", json!([{"approved":false,"issues":["Use format B"],"architecture_context_gap":"A and B conflict; establish compatibility"}, clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    let settings = f.ctx.app.settings.lock().unwrap();
    let prompt = settings["mock_fixer_prompts"][0].as_str().unwrap();
    assert!(prompt.contains("[architect] Use format B"));
    assert!(prompt.contains("[reviewer] Use format A"));
    let turns = settings["mock_architect_requests"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert!(
        turns[1]["prompt"]
            .as_str()
            .unwrap()
            .contains("A and B conflict")
    );
    assert!(
        settings["mock_architect_prompts"][0]
            .as_str()
            .unwrap()
            .contains("Use format A")
    );
}

#[test]
fn legacy_notes_are_requests_and_new_defects_on_rereview_block() {
    let f = Fixture::new("Implement feature", 2);
    f.setting("mock_verdicts", json!([{"approved":true,"issues":[],"notes":["Legacy edit"]},reject("New regression"),clean()]));
    let p = f.run();
    assert_eq!(p["stages"][0]["status"], "committed");
    assert_eq!(f.count("fixer"), 2);
    let settings = f.ctx.app.settings.lock().unwrap();
    assert!(
        settings["mock_fixer_prompts"][0]
            .as_str()
            .unwrap()
            .contains("Legacy edit")
    );
    assert!(
        settings["mock_fixer_prompts"][1]
            .as_str()
            .unwrap()
            .contains("New regression")
    );
    assert!(
        !settings["mock_fixer_prompts"][1]
            .as_str()
            .unwrap()
            .contains("Legacy edit")
    );
}

#[test]
fn stale_malformed_contradictory_or_unevidenced_verdicts_never_commit() {
    for bad in [
        json!("garbage"),
        json!({"identity":{},"approved":true,"issues":[]}),
        json!({"approved":true,"issues":["fix"]}),
        json!({"approved":true,"issues":[],"checks":[null]}),
        json!({"approved":true,"issues":[],"checks":[]}),
        json!({"approved":true,"issues":[],"project_checks":[]}),
        json!({"approved":true,"issues":[],"acceptance_evidence":{"verified":true}}),
        json!({"approved":true,"issues":[],"summary":""}),
    ] {
        let f = Fixture::new("Implement feature", 0);
        f.setting("mock_verdicts", json!(vec![bad; 4]));
        fs::write(f.ctx.forge_path("verdict.json"), clean().to_string()).unwrap();
        let p = f.run();
        f.assert_no_commit();
        assert_eq!(p["stages"][0]["review_gate"]["status"], "error");
        assert_eq!(p["stages"][0]["status"], "blocked");
        assert!(p["stages"][0]["finished_unix"].as_i64().is_some());
        assert!(p["stages"][0]["duration_secs"].as_i64().is_some());
        assert!(!f.ctx.forge_path("verdict.json").exists());
    }
}

#[test]
fn missing_current_role_or_changed_identity_cannot_commit() {
    for field in ["revision", "attempt_id", "round", "policy", "snapshot"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.reviewed();
        p["stages"][0]["reviews"][1]["identity"][field] = json!("wrong");
        assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err());
        f.assert_no_commit();
    }
}

#[test]
fn criterion_evidence_is_complete_and_prompts_preserve_literal_inputs() {
    let f = Fixture::new("Implement {goal} feature", 0);
    let mut p = f.plan();
    p["stages"][0]["acceptance"] = json!("First {verdict_path}\nSecond {review_context}");
    f.ctx.save_plan(&p).unwrap();
    f.setting("mock_verdicts",json!(vec![json!({"approved":true,"issues":[],"criteria":[{"criterion":"First {verdict_path}","status":"passed","evidence":"One check"}]}); 4]));
    f.run();
    f.assert_no_commit();
    let settings = f.ctx.app.settings.lock().unwrap();
    let prompt = settings["mock_reviewer_prompts"][0].as_str().unwrap();
    assert!(prompt.contains("Implement {goal} feature"));
    assert!(prompt.contains("First {verdict_path}"));
    assert!(prompt.contains("Second {review_context}"));
}

#[test]
fn duplicate_keys_and_tampered_evidence_cannot_authorize_commit() {
    let f = Fixture::new("Implement feature", 0);
    let mut p = f.reviewed();
    let record = &p["stages"][0]["reviews"][0];
    let duplicate = record.to_string().replacen(
        "\"approved\":true",
        "\"approved\":false,\"approved\":true",
        1,
    );
    assert!(
        normalize_review_verdict(
            &duplicate,
            &record["identity"],
            "The requested change works."
        )
        .is_err()
    );
    p["stages"][0]["reviews"][1]["criteria"] = json!([]);
    assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err());
    f.assert_no_commit();
}

#[test]
fn verdict_presentation_wrappers_preserve_validation() {
    let f = Fixture::new("Implement feature", 0);
    let p = f.reviewed();
    let record = &p["stages"][0]["reviews"][0];
    let raw = record.to_string();
    let preamble = "All checks pass. Shared fixtures use `crate::test_support` as intended.";
    let parse = |output: &str| normalize_review_verdict(output, &record["identity"], "The requested change works.");
    for wrapped in [
        format!("  {raw}\n"),
        format!("{preamble}\n\n{raw}"),
        format!("```json\n{raw}\n```"),
        format!("{preamble}\n\n```\n{raw}\n```"),
        format!("```json\r\n{raw}\r\n```"),
    ] {
        assert_eq!(parse(&wrapped).unwrap(), parse(&raw).unwrap());
    }
    let duplicate = raw.replacen("\"approved\":true", "\"approved\":false,\"approved\":true", 1);
    let nested_duplicate = raw.replacen("\"role\":\"reviewer\"", "\"role\":\"architect\",\"role\":\"reviewer\"", 1);
    for invalid in [
        format!("{preamble}\n{duplicate}"),
        format!("```json\n{nested_duplicate}\n```"),
        format!("{preamble}\n{{}}\n{raw}"),
        format!("{preamble}\n{raw}\n{raw}"),
        format!("{preamble}\n{{broken\n{raw}"),
        format!("{preamble}\n[{raw}]"),
        format!("{preamble}\n{raw}\nActually, changes are needed."),
        format!("```json\n{raw}"),
        format!("```json\n{raw}\n```\nMore text"),
        format!("```text\n{raw}\n```"),
        format!("{}\n{raw}", "x".repeat(128 * 1024)),
    ] {
        assert!(parse(&invalid).is_err(), "unexpectedly accepted: {invalid}");
    }
    for field in ["identity", "criteria", "project_checks", "acceptance_evidence"] {
        let mut invalid = record.clone();
        invalid[field] = Value::Null;
        assert!(parse(&format!("{preamble}\n```json\n{invalid}\n```")).is_err(), "{field}");
    }
}

#[test]
fn deferred_commit_rejects_forged_partitions_identity_outcomes_and_attempt_cadence() {
    for case in ["empty", "overlap", "missing", "unknown", "scope", "stage", "attempt", "round", "cadence", "persisted_cadence", "approved", "roles", "policy"] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.deferred_gate();
        let stage = &mut p["stages"][0];
        let mut policy = stage["review_policy"].clone();
        match case {
            "empty" => policy["deferred_roles"] = json!([]),
            "overlap" => policy["stage_required_roles"] = json!(["architect"]),
            "missing" => { policy.as_object_mut().unwrap().remove("stage_required_roles"); },
            "unknown" => policy["deferred_roles"] = json!(["architect","unknown"]),
            "scope" => policy["required_roles"] = json!(["reviewer"]),
            "stage" => stage["review_gate"]["identity"]["stage_id"] = json!(2),
            "attempt" => stage["review_gate"]["identity"]["attempt_id"] = json!("other"),
            "round" => stage["review_gate"]["identity"]["round"] = json!(2),
            "cadence" | "persisted_cadence" => stage["review_cadence"]["architect"] = json!("per_stage"),
            "approved" => stage["review_gate"]["status"] = json!("approved"),
            "roles" => stage["review_gate"]["roles"]["architect"] = json!("approved"),
            "policy" => stage["review_policy"]["rationale"] = json!("tampered"),
            _ => unreachable!(),
        }
        if ["empty", "overlap", "missing", "unknown", "scope"].contains(&case) {
            stage["review_policy"] = policy.clone();
            stage["review_gate"]["policy"] = policy.clone();
            stage["review_gate"]["identity"]["policy"] = policy;
        }
        f.ctx.save_plan(&p).unwrap();
        if case == "persisted_cadence" {
            p["stages"][0]["review_cadence"]["architect"] = json!("per_plan");
        }
        assert!(f.ctx.commit_reviewed(&p, 0, "feat: stage").is_err(), "accepted {case}");
        f.assert_no_commit();
    }
}

#[test]
fn legacy_policy_without_partition_still_requires_evidenced_role_approvals() {
    for invalid in [false, true] {
        let f = Fixture::new("Implement feature", 0);
        let mut p = f.reviewed();
        let stage = &mut p["stages"][0];
        let mut policy = stage["review_policy"].clone();
        policy.as_object_mut().unwrap().remove("stage_required_roles");
        policy.as_object_mut().unwrap().remove("deferred_roles");
        stage["review_policy"] = policy.clone();
        let mut base = stage["review_gate"]["identity"].clone();
        base["policy"] = policy.clone();
        let records = stage["reviews"].as_array_mut().unwrap();
        for record in records.iter_mut() {
            record["policy"] = policy.clone();
            record["identity"]["policy"] = policy.clone();
        }
        if invalid { records[0]["criteria"] = json!([]); }
        stage["review_gate"] = aggregate_review_gate(&base, records);
        f.ctx.save_plan(&p).unwrap();
        let result = f.ctx.commit_reviewed(&p, 0, "feat: stage");
        assert_eq!(result.is_err(), invalid, "{result:?}");
    }
}

#[test]
fn constraint_conflict_verdict_field_is_validated_and_rejects_with_its_statement() {
    let f = Fixture::new("Implement feature", 0);
    let p = f.reviewed();
    let record = &p["stages"][0]["reviews"][0];
    let parse = |conflict: Value| {
        let mut verdict = record.clone();
        verdict["constraint_conflict"] = conflict;
        normalize_review_verdict(&verdict.to_string(), &record["identity"], "The requested change works.")
    };
    // Absent or null leaves an evidenced approval intact.
    assert_eq!(parse(Value::Null).unwrap()["approved"], true);
    // A statement turns the verdict into an ordinary rejection that keeps it.
    let statement = "Change only documentation contradicts keeping the milestone test passing";
    let rejected = parse(json!(statement)).unwrap();
    assert_eq!(rejected["approved"], false);
    assert_eq!(rejected["constraint_conflict"], statement);
    let issues = rejected["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].as_str().unwrap().contains(statement));
    // Malformed values invalidate the whole verdict.
    for bad in [json!(""), json!("   "), json!(42), json!(true), json!([statement]), json!({"text":statement})] {
        let error = parse(bad.clone()).unwrap_err();
        assert!(error.contains("constraint_conflict"), "{bad}: {error}");
    }
}

#[test]
fn review_gate_keeps_role_tagged_conflict_statements_and_rejects_normally() {
    let f = Fixture::new("Implement feature", 0);
    let statement = "The stage may change only docs, yet a test reads the changed milestones.md";
    f.setting("mock_verdicts", json!([{"approved":false,"issues":[],"constraint_conflict":statement}]));
    f.setting("mock_architect_verdicts", json!([{"approved":false,"issues":["Keep tests passing"]}]));
    let p = f.run();
    f.assert_no_commit();
    let gate = &p["stages"][0]["review_gate"];
    assert_eq!(gate["status"], "blocked");
    assert_eq!(gate["constraint_conflicts"], json!([{"role":"reviewer","text":statement}]));
    assert!(gate["requests"].as_array().unwrap().iter()
        .any(|r| r["role"] == "reviewer" && r["text"].as_str().unwrap().contains(statement)));
    // No planner hand-back yet: the conflict acts as a normal rejection.
    assert_eq!(f.count("planner"), 0);
    assert!(p["stages"][0]["constraint_escalations"].is_null());
    // Gates without conflicts keep their previous shape.
    let clean_gate = aggregate_review_gate(&gate["identity"], &[]);
    assert!(clean_gate.get("constraint_conflicts").is_none());
}
