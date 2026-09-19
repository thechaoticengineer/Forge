use super::*;
use super::test_support::*;

    #[test]
    fn prompt_checkpoints_drop_input_fingerprints_and_keep_everything_else() {
        let inputs = json!({"instructions": "copied stage text"});
        let checkpoint = json!({"summary": "keep", "guidance": {"1": {"text": "g", "relevant_inputs": inputs}},
            "agreements": {"1": {"id": "a", "dialogue": ["drop"], "relevant_inputs": inputs}},
            "recent_decisions": [{"relevant_inputs": inputs}]});
        let stripped = crate::prompt_view::checkpoint_for(&checkpoint, &Value::Null);
        assert_eq!(stripped["guidance"]["1"], json!({"text": "g"}));
        assert!(stripped["agreements"]["1"].get("dialogue").is_none());
        assert!(stripped["agreements"]["1"].get("relevant_inputs").is_none());
        assert_eq!(stripped["summary"], "keep");
        // Records outside guidance, agreements and outcomes are not rewritten.
        assert_eq!(stripped["recent_decisions"], checkpoint["recent_decisions"]);
        // A checkpoint without those groups gains no keys.
        assert_eq!(crate::prompt_view::checkpoint_for(&json!({"summary": "only"}), &Value::Null), json!({"summary": "only"}));
        // Storage keeps every field.
        assert_eq!(checkpoint["agreements"]["1"]["dialogue"], json!(["drop"]));
    }

    /// The validator accepts 16 decisions of 1 KiB summary, 4 KiB rationale and
    /// 8 alternatives of 1 KiB each. Storage has to accept the same turn.
    #[test]
    fn a_maximal_legal_architect_turn_fits_in_one_event() {
        let temp = Temp::new();
        let store = temp.store();
        let published = publish(&store);
        let decisions: Vec<Value> = (0..16).map(|i| json!({
            "id": format!("decision-{i}"), "stage_id": 1, "status": "accepted",
            "summary": "s".repeat(1000), "rationale": "r".repeat(4000),
            "alternatives": (0..8).map(|_| json!({
                "description": "d".repeat(1000), "tradeoffs": "t".repeat(1000)})).collect::<Vec<_>>(),
        })).collect();
        let cp = store.checkpoint(&published).unwrap();
        let published = store.publish(published, cp,
            json!({"kind":"architect_turn","turn":"t","decisions":decisions})).unwrap();
        let page = store.history(published["plan_id"].as_str(), 0, 100).unwrap();
        let stored = page["items"].as_array().unwrap().iter()
            .find(|e| e["payload"]["kind"] == "architect_turn").expect("turn event");
        assert_eq!(stored["payload"]["decisions"].as_array().unwrap().len(), 16);
        assert_eq!(stored["payload"]["decisions"][15]["rationale"], "r".repeat(4000));
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
            publish_pretty(&path, &bundle).unwrap();
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
                    publish_pretty(&temp.0.join("plan.json"), &p).unwrap();
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
                    publish_pretty(&temp.0.join("plan.json"), &p).unwrap();
                }
                _ => {
                    let mut p = old.clone();
                    p["plan_id"] = json!("../escape");
                    publish_pretty(&temp.0.join("plan.json"), &p).unwrap();
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
            json!(["architect", "reviewer"])
        );
    }

    #[test]
    fn completed_stage_checkpoint_can_exceed_an_individual_event_budget() {
        let temp = Temp::new();
        let store = temp.store();
        let old = publish(&store);
        let mut cp = store.checkpoint(&old).unwrap();
        // Accumulated distinct plan facts cannot be reduced by string deduplication.
        cp["retained_facts"] = json!((0..180).map(|i| format!("fact-{i}:{}", "x".repeat(1000))).collect::<Vec<_>>());
        let saved = store.publish(old, cp.clone(), json!({"kind":"execution"})).unwrap();
        assert_eq!(store.checkpoint(&saved).unwrap(),cp);
        assert_eq!(store.load().unwrap().unwrap(),saved);
    }
