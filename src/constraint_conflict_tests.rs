use super::*;

fn plan() -> Value {
    json!({"plan_id":"p","revision":4,"goal":"Stop contradictory stage constraints","stages":[
        {"id":1,"title":"Fixtures","instructions":"Move tests onto fixtures","acceptance":"Tests use fixtures","commit":"test: fixtures","status":"committed","sha":"abc1234"},
        {"id":2,"title":"Docs","instructions":"Change only documentation","acceptance":"Status is implemented","commit":"docs: status","status":"pending",
         "attempt_id":"a1","rounds":2,"review_budget":3,
         "review_gate":{"status":"blocked","requests":[{"role":"reviewer","text":"Milestone test fails"},{"role":"architect","text":"Keep docs-only scope"}]},
         "outcome_history":[{"round":1,"attempt_id":"a1","status":"completed","evidence":["cargo test: 1 failed"]},
                            {"round":0,"attempt_id":"old","status":"completed","evidence":["stale attempt"]}],
         "reviews":[{"identity":{"role":"reviewer","round":1,"attempt_id":"a1"},"project_checks":[{"command":"cargo test","status":"failed","evidence":"milestone test failed"}]},
                    {"identity":{"role":"reviewer","round":1,"attempt_id":"old"},"project_checks":[{"command":"stale","status":"passed","evidence":"old"}]}]},
        {"id":3,"title":"Readme","instructions":"Describe it","acceptance":"README explains it","commit":"docs: readme","status":"pending"}]})
}

fn answer(decision: Value) -> String {
    json!({"analysis":"The docs-only limit collides with the milestone test.","decision":decision}).to_string()
}

#[test]
fn verdict_field_accepts_null_or_a_statement_only() {
    assert_eq!(verdict_field(&json!({})).unwrap(), None);
    assert_eq!(verdict_field(&json!({"constraint_conflict":null})).unwrap(), None);
    assert_eq!(verdict_field(&json!({"constraint_conflict":"A contradicts B"})).unwrap().as_deref(), Some("A contradicts B"));
    for bad in [json!(""), json!(" \n"), json!(1), json!(false), json!(["A"]), json!({"a":"b"})] {
        assert!(verdict_field(&json!({"constraint_conflict":bad})).is_err(), "{bad}");
    }
}

#[test]
fn the_same_conflict_has_a_stable_signature_and_different_requests_do_not() {
    let statements = vec!["Docs only  contradicts\nthe milestone test".to_string(), "Second".to_string()];
    let requests = vec![json!({"role":"reviewer","text":"Milestone test fails"}), json!({"role":"architect","text":"Keep scope"})];
    let base = signature(&statements, &requests);
    // Order, whitespace, case and role attribution do not change identity.
    let reordered = vec!["second".to_string(), "docs only contradicts the milestone test".to_string()];
    let retagged = vec![json!({"role":"architect","text":"keep   scope"}), json!({"role":"engine","text":"Milestone test fails"})];
    assert_eq!(signature(&reordered, &retagged), base);
    assert_eq!(signature(&statements, &requests), base);
    // A different outstanding request or statement is a different conflict.
    let other = vec![json!({"role":"reviewer","text":"Milestone test fails"}), json!({"role":"architect","text":"Another request"})];
    assert_ne!(signature(&statements, &other), base);
    assert_ne!(signature(&statements[..1], &requests), base);
    // Long texts are signed complete: a difference beyond the display bound counts.
    let long = "x".repeat(MAX_TEXT + 10);
    let a = vec![format!("{long}a")];
    let b = vec![format!("{long}b")];
    assert_ne!(signature(&a, &requests), signature(&b, &requests));
}

#[test]
fn bounded_inputs_truncate_deterministically_and_fingerprint_complete_input() {
    let items: Vec<Value> = (0..MAX_ITEMS + 3).map(|i| json!({"text": format!("{i}-{}", "y".repeat(MAX_TEXT))})).collect();
    let bounded = bounded(&items);
    assert_eq!(bounded["items"].as_array().unwrap().len(), MAX_ITEMS);
    assert_eq!(bounded["total"], MAX_ITEMS + 3);
    assert_eq!(bounded["truncated"], true);
    assert_eq!(bounded["items"][0]["text"].as_str().unwrap().chars().count(), MAX_TEXT + 1);
    assert_eq!(super::bounded(&items), bounded);
    let mut changed = items.clone();
    changed[MAX_ITEMS + 2] = json!({"text":"different tail"});
    assert_ne!(super::bounded(&changed)["fingerprint"], bounded["fingerprint"]);
    let small = super::bounded(&[json!("a")]);
    assert_eq!(small["truncated"], false);
    assert_eq!(small["items"], json!(["a"]));
}

#[test]
fn stage_inputs_carry_requests_replies_checks_files_and_rounds_of_the_current_attempt() {
    let plan = plan();
    let inputs = stage_inputs(&plan, 1, &["Docs only vs test".into()], &["docs/a.md".into(), "src/b.rs".into()]);
    assert_eq!(inputs["stage"]["id"], 2);
    assert_eq!(inputs["stage"]["instructions"]["text"], "Change only documentation");
    assert_eq!(inputs["requests"]["items"][1], json!({"role":"architect","text":"Keep docs-only scope"}));
    assert_eq!(inputs["fixer_replies"]["total"], 1);
    assert_eq!(inputs["fixer_replies"]["items"][0]["evidence"], json!(["cargo test: 1 failed"]));
    assert_eq!(inputs["checks"]["items"], json!([{"role":"reviewer","round":1,"command":"cargo test","status":"failed","evidence":"milestone test failed"}]));
    assert_eq!(inputs["changed_files"]["items"], json!(["docs/a.md","src/b.rs"]));
    assert_eq!(inputs["rounds"], json!({"used":2,"budget":3}));
    assert_eq!(inputs["statements"]["items"], json!(["Docs only vs test"]));
    assert_eq!(inputs["plan"][0], json!({"id":1,"title":"Fixtures","status":"committed","sha":"abc1234"}));
    assert_eq!(inputs["plan_revision"], 4);
}

#[test]
fn escalation_records_are_self_describing_and_validate_source_and_outcome() {
    let plan = plan();
    let statements = vec!["Docs only vs test".to_string()];
    let requests = plan["stages"][1]["review_gate"]["requests"].as_array().unwrap().clone();
    let sig = signature(&statements, &requests);
    let inputs = stage_inputs(&plan, 1, &statements, &[]);
    assert!(new_record("planner", KIND, "why", &sig, inputs.clone(), 1).is_err());
    assert!(new_record("reviewer", KIND, " ", &sig, inputs.clone(), 1).is_err());
    let mut record = new_record("reviewer", KIND, "Docs only vs test", &sig, inputs, 10).unwrap();
    assert_eq!(record["version"], RECORD_VERSION);
    assert_eq!(record["trigger"], json!({"source":"reviewer","kind":KIND,"reason":"Docs only vs test"}));
    assert_eq!(record["signature"], sig);
    assert_eq!(record["outcome"], "pending");
    for key in ["analysis", "decision", "correction", "plan_revision"] {
        assert!(record[key].is_null(), "{key}");
    }
    let parsed = validate_answer(&answer(json!({"refused":"Move the test first"})), &plan, 1).unwrap();
    record_answer(&mut record, &parsed);
    assert_eq!(record["decision"], "refused");
    assert_eq!(record["correction"], json!({"refused":"Move the test first"}));
    assert!(set_outcome(&mut record, "done", None, None, 11).is_err());
    set_outcome(&mut record, "refused", Some("kept the stage"), Some(&json!(4)), 12).unwrap();
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["plan_revision"], 4);
    assert_eq!(record["updated_unix"], 12);
    assert_eq!(record["unix"], 10);
    // The record is plain JSON and survives a save/load round trip.
    let mut stage = json!({"id":2});
    assert_eq!(push_record(&mut stage, record.clone()), 0);
    assert_eq!(push_record(&mut stage, record.clone()), 1);
    let reloaded: Value = serde_json::from_str(&stage.to_string()).unwrap();
    assert_eq!(reloaded[RECORDS][0], record);
}

#[test]
fn planner_answers_accept_exactly_one_valid_decision() {
    let plan = plan();
    let revise = validate_answer(&answer(json!({"revise":{
        "stages":[{"id":2,"instructions":"Change documentation and the fixture test","acceptance":"Status is implemented and tests pass"},
                  {"id":3,"title":"Readme update","instructions":"Describe it","acceptance":"README explains it"}],
        "insert_before":[{"title":"Move milestone test","instructions":"Move it onto fixtures","acceptance":"Test uses fixtures","commit":"test: fixture"}]}})), &plan, 1).unwrap();
    assert_eq!(revise.decision.name(), "revise");
    let insert_only = validate_answer(&answer(json!({"revise":{"insert_before":[
        {"title":"Move test","instructions":"Move it","acceptance":"Uses fixtures","commit":"test: move"}]}})), &plan, 1).unwrap();
    assert!(matches!(insert_only.decision, Decision::Revise { ref stages, .. } if stages.is_empty()));
    let wrong = validate_answer(&answer(json!({"constraint_wrong":{"constraint":"Change only documentation",
        "instructions":"Change documentation; test edits allowed","acceptance":"Status is implemented",
        "justification":"The delivered fixture change was required to keep tests passing"}})), &plan, 1).unwrap();
    assert_eq!(wrong.decision.name(), "constraint_wrong");
    // Prose around the JSON is tolerated, as for other planner answers.
    let refused = validate_answer(&format!("I checked.\n{}", answer(json!({"refused":"Update the fixture copy instead"}))), &plan, 1).unwrap();
    assert_eq!(refused.decision, Decision::Refused("Update the fixture copy instead".into()));
}

#[test]
fn planner_answers_are_rejected_for_every_invalid_shape() {
    let plan = plan();
    let edit = |id: i64, instructions: &str, acceptance: &str| json!({"revise":{"stages":[{"id":id,"instructions":instructions,"acceptance":acceptance}]}});
    let cases: Vec<(String, &str)> = vec![
        ("{broken".into(), "invalid constraint conflict answer"),
        (json!({"decision":{"refused":"x"}}).to_string(), "analysis"),
        (json!({"analysis":"  ","decision":{"refused":"x"}}).to_string(), "analysis must be a non-empty"),
        (json!({"analysis":"a","decision":{"refused":"x"},"extra":1}).to_string(), "unknown field"),
        (answer(json!({})), "invalid constraint conflict answer"),
        (answer(json!({"refused":"x","constraint_wrong":{"constraint":"c","instructions":"i","acceptance":"a","justification":"j"}})), "invalid constraint conflict answer"),
        (answer(json!({"maybe":"x"})), "invalid constraint conflict answer"),
        (answer(json!({"refused":" "})), "refused must be a non-empty"),
        (answer(json!({"refused":7})), "invalid constraint conflict answer"),
        (answer(json!({"revise":{}})), "at least one stage"),
        (answer(edit(1, "Rewrite history", "New")), "committed history"),
        (answer(edit(9, "Unknown", "Stage")), "unknown stage 9"),
        (answer(edit(2, "Change only   documentation", "Status is implemented")), "unchanged"),
        (answer(edit(2, "", "Status is implemented")), "instructions must be a non-empty"),
        (answer(edit(2, "New", " ")), "acceptance must be a non-empty"),
        (answer(json!({"revise":{"stages":[{"id":3,"instructions":"a","acceptance":"b"},{"id":3,"instructions":"c","acceptance":"d"}]}})), "twice"),
        (answer(json!({"revise":{"stages":[{"id":3,"instructions":"a","acceptance":"b","status":"committed"}]}})), "unknown field"),
        (answer(json!({"revise":{"insert_before":[{"title":"t","instructions":"i","acceptance":"a"}]}})), "commit"),
        (answer(json!({"revise":{"insert_before":[{"title":"t","instructions":"i","acceptance":"a","commit":" "}]}})), "commit must be a non-empty"),
        (answer(json!({"constraint_wrong":{"constraint":"c","instructions":"Change only documentation","acceptance":"Status is implemented","justification":"j"}})), "unchanged"),
        (answer(json!({"constraint_wrong":{"constraint":"c","instructions":"i","acceptance":"a","justification":""}})), "justification must be a non-empty"),
        (answer(json!({"constraint_wrong":{"constraint":"c","instructions":"i","acceptance":"a"}})), "invalid constraint conflict answer"),
    ];
    for (text, expected) in cases {
        let error = validate_answer(&text, &plan, 1).unwrap_err();
        assert!(error.contains(expected), "{text}: {error}");
    }
    // A stage before the current one cannot change, even when still pending.
    let mut earlier = plan.clone();
    earlier["stages"][0]["status"] = json!("pending");
    let error = validate_answer(&answer(edit(1, "Other", "Other")), &earlier, 1).unwrap_err();
    assert!(error.contains("precedes the current stage"), "{error}");
    // A committed current stage cannot be corrected at all.
    let error = validate_answer(&answer(json!({"refused":"x"})), &plan, 0).unwrap_err();
    assert!(error.contains("committed history"), "{error}");
}

#[test]
fn the_planner_prompt_carries_every_input_and_rule() {
    let plan = plan();
    let statements = vec!["Docs only vs milestone test".to_string()];
    let requests = plan["stages"][1]["review_gate"]["requests"].as_array().unwrap().clone();
    let inputs = stage_inputs(&plan, 1, &statements, &["docs/features/x/milestones.md".into()]);
    let record = new_record("fixer", KIND, "Docs only vs milestone test", &signature(&statements, &requests), inputs, 1).unwrap();
    let prompt = prompt(&plan, 1, &record);
    for needle in [
        "Stop contradictory stage constraints", // goal
        "STAGE 1 [committed — committed history, cannot change]: Fixtures", // committed stage with status
        "STAGE 2 (CURRENT) [pending]: Docs", "STAGE 3 [pending]: Readme", // current and later stages
        "CURRENT STAGE 2 — Docs", "Change only documentation", "Status is implemented", // stage text
        "\"source\": \"fixer\"", "Docs only vs milestone test", // trigger and statements
        "\"role\": \"architect\"", "Keep docs-only scope", // role-tagged requests
        "cargo test: 1 failed", // fixer replies
        "milestone test failed", // check results
        "docs/features/x/milestones.md", // changed files
        "\"used\": 2", "\"budget\": 3", // round usage
        "All tests must pass after every stage", "Committed stages are fixed history",
        "A business test must never be weakened", crate::prompts::PLAN_CONSTRAINT_RULES,
        "\"revise\"", "\"constraint_wrong\"", "\"refused\"", "\"analysis\"",
    ] {
        assert!(prompt.contains(needle), "prompt lacks {needle}:\n{prompt}");
    }
    assert!(!prompt.contains("stale attempt"));
    assert!(!prompt.contains("{requests}"));
}

/// A plan whose stages are all committed, under plan review.
fn reviewed_plan() -> Value {
    let mut plan = plan();
    for stage in plan["stages"].as_array_mut().unwrap() { stage["status"] = json!("committed"); }
    plan["plan_review"] = json!({"attempt_id":"r2","base":"base-sha","head":"head-sha","rounds":3,"budget":3,
        "acceptance":"stage 2 (Docs): Status is implemented",
        "outstanding_requests":["[reviewer] Milestone test fails","[architect] Keep docs-only scope","untagged request"],
        "fixes":[{"round":2,"sha":"f1x","message":"fix(review): apply round 2 plan review findings"}],
        "fixer_replies":[{"attempt_id":"r2","round":2,"reply":"cargo test: 1 failed","constraint_conflict":null},
                         {"attempt_id":"r1","round":2,"reply":"stale reply"}],
        "reviews":[{"identity":{"role":"architect","round":2,"attempt_id":"r2"},"project_checks":[{"command":"cargo test","status":"failed","evidence":"milestone test failed"}]},
                   {"identity":{"role":"reviewer","round":1,"attempt_id":"r1"},"project_checks":[{"command":"stale","status":"passed","evidence":"old"}]}]});
    plan
}

#[test]
fn plan_review_answers_only_correct_forward() {
    let plan = reviewed_plan();
    let valid = [
        json!({"revise":{"append":[{"title":"Move test","instructions":"Move it onto fixtures","acceptance":"Uses fixtures","commit":"test: move"}]}}),
        json!({"revise":{"criteria":[{"id":2,"acceptance":"Status is implemented and tests pass"}]}}),
        json!({"constraint_wrong":{"stage":2,"constraint":"Change only documentation","instructions":"Change documentation and the fixture test",
            "acceptance":"Status is implemented","justification":"The fixture change keeps tests passing"}}),
        json!({"refused":"Point the test at a fixture copy"}),
    ];
    for decision in valid {
        validate_answer_for(&answer(decision.clone()), &plan, Scope::Plan).unwrap_or_else(|e| panic!("{decision}: {e}"));
    }
    let cases: Vec<(Value, &str)> = vec![
        (json!({"revise":{"stages":[{"id":2,"instructions":"New","acceptance":"New"}]}}), "committed history and cannot change"),
        (json!({"revise":{"insert_before":[{"title":"t","instructions":"i","acceptance":"a","commit":"c"}]}}), "only append stages"),
        (json!({"revise":{}}), "append a stage or correct"),
        (json!({"revise":{"criteria":[{"id":9,"acceptance":"x"}]}}), "not a committed stage"),
        (json!({"revise":{"criteria":[{"id":2,"acceptance":"Status  is implemented"}]}}), "unchanged"),
        (json!({"revise":{"criteria":[{"id":2,"acceptance":"a"},{"id":2,"acceptance":"b"}]}}), "twice"),
        (json!({"revise":{"criteria":[{"id":2,"acceptance":" "}]}}), "acceptance must be a non-empty"),
        (json!({"revise":{"append":[{"title":"t","instructions":"i","acceptance":"a","commit":""}]}}), "append.commit must be a non-empty"),
        (json!({"constraint_wrong":{"constraint":"c","instructions":"i","acceptance":"a","justification":"j"}}), "must name the committed stage"),
        (json!({"constraint_wrong":{"stage":7,"constraint":"c","instructions":"i","acceptance":"a","justification":"j"}}), "not a committed stage"),
        (json!({"constraint_wrong":{"stage":2,"constraint":"c","instructions":"Change only documentation","acceptance":"Status is implemented","justification":"j"}}), "unchanged"),
    ];
    for (decision, expected) in cases {
        let error = validate_answer_for(&answer(decision.clone()), &plan, Scope::Plan).unwrap_err();
        assert!(error.contains(expected), "{decision}: {error}");
    }
    // A corrected criterion is the text a later correction is compared with.
    let mut corrected = plan.clone();
    corrected[CRITERIA] = json!({"2":{"acceptance":"Status is implemented and tests pass"}});
    assert_eq!(review_acceptance(&corrected, &corrected["stages"][1]), "Status is implemented and tests pass");
    let error = validate_answer_for(&answer(json!({"revise":{"criteria":[{"id":2,"acceptance":"Status is implemented and tests pass"}]}})),
        &corrected, Scope::Plan).unwrap_err();
    assert!(error.contains("unchanged"), "{error}");
    // Plan-review fields are refused in a stage review, and a stage review's
    // constraint_wrong may only name the current stage.
    let stage_plan = self::plan();
    for decision in [json!({"revise":{"append":[{"title":"t","instructions":"i","acceptance":"a","commit":"c"}]}}),
        json!({"revise":{"criteria":[{"id":2,"acceptance":"x"}]}})] {
        let error = validate_answer(&answer(decision), &stage_plan, 1).unwrap_err();
        assert!(error.contains("only to a plan review"), "{error}");
    }
    let error = validate_answer(&answer(json!({"constraint_wrong":{"stage":3,"constraint":"c","instructions":"i","acceptance":"a","justification":"j"}})),
        &stage_plan, 1).unwrap_err();
    assert!(error.contains("only correct the current stage"), "{error}");
    // Plan-review fields stay out of a stage record's saved correction.
    let parsed = validate_answer(&answer(json!({"revise":{"insert_before":[{"title":"t","instructions":"i","acceptance":"a","commit":"c"}]}})), &stage_plan, 1).unwrap();
    assert_eq!(json!(parsed.decision), json!({"revise":{"stages":[],"insert_before":[{"title":"t","instructions":"i","acceptance":"a","commit":"c"}]}}));
}

#[test]
fn the_plan_review_planner_prompt_carries_every_input() {
    let plan = reviewed_plan();
    let statements = vec![plan_fallback_statement()];
    let requests = plan_review_requests(&plan);
    assert_eq!(requests, vec![json!({"role":"reviewer","text":"Milestone test fails"}),
        json!({"role":"architect","text":"Keep docs-only scope"}), json!({"role":"reviewer","text":"untagged request"})]);
    let inputs = plan_review_inputs(&plan, &statements, &["docs/features/x/milestones.md".into()]);
    assert_eq!(inputs["rounds"], json!({"used":3,"budget":3}));
    assert_eq!(inputs["commit_range"], json!({"base":"base-sha","head":"head-sha"}));
    assert_eq!(inputs["fixer_replies"]["total"], 1);
    assert_eq!(inputs["checks"]["total"], 1);
    let sig = signature(&statements, &requests);
    let record = new_record("engine", PLAN_STALLED_KIND, "two fix rounds changed nothing", &sig, inputs, 1).unwrap();
    let prompt = plan_prompt(&plan, &record);
    for needle in [
        "Stop contradictory stage constraints", "STAGE 2 [committed — committed history, cannot change]: Docs",
        "COMMIT RANGE UNDER REVIEW: base-sha..head-sha", "stage 2 (Docs): Status is implemented",
        "\"source\": \"engine\"", "plan_review_stalled", "could not converge",
        "\"role\": \"architect\"", "Keep docs-only scope", "fix(review): apply round 2", "cargo test: 1 failed",
        "milestone test failed", "docs/features/x/milestones.md", "\"used\": 3", "\"budget\": 3",
        crate::prompts::PLAN_CONSTRAINT_RULES, "A business test must never be weakened", "\"append\"", "\"criteria\"",
    ] {
        assert!(prompt.contains(needle), "prompt lacks {needle}:\n{prompt}");
    }
    assert!(!prompt.contains("(CURRENT)"));
    assert!(!prompt.contains("stale reply"));
    assert!(!prompt.contains("{fixes}"));
    // Stall and exhaustion with the same requests are the same conflict.
    assert_eq!(signature(&[plan_fallback_statement()], &requests), sig);
}

#[test]
fn a_plan_fixer_reports_a_conflict_with_a_trailing_json_object() {
    assert_eq!(fixer_reported("Ran cargo test.\n{\"constraint_conflict\": \"Docs only vs the milestone test\"}").as_deref(),
        Some("Docs only vs the milestone test"));
    for output in ["Mock implementation completed.", "{\"constraint_conflict\": null}", "{\"constraint_conflict\": \" \"}",
        "{\"constraint_conflict\": 3}", "{\"other\": \"x\"}"] {
        assert_eq!(fixer_reported(output), None, "{output}");
    }
}
