use super::*;
use super::test_support::*;

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
        publish_pretty(&temp.0.join("plan.json"), &legacy).unwrap();
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
    fn compact_records_keep_transaction_recovery_and_archive_history() {
        for point in ["partial_event", "events", "partial_snapshot", "checkpoint", "publication_error", "publication"] {
            let temp = Temp::new();
            let store = temp.store();
            let old = publish(&store);
            let mut cp = checkpoint_default();
            cp["retained_metadata"] = json!(vec!["long retained context ".repeat(200); 40]);
            let payload = json!({"kind":"revision","retained_metadata":cp["retained_metadata"]});
            let mut next = old.clone();
            next["revision"] = json!(2);
            assert!(store.publish_at(next.clone(), cp.clone(), payload.clone(), point).is_err());
            let current = store.load().unwrap().unwrap();
            if point == "publication" {
                assert_eq!(store.checkpoint(&current).unwrap(), cp);
            } else {
                assert_eq!(current, old);
                let committed = store.publish(next, cp.clone(), payload.clone()).unwrap();
                assert_eq!(store.checkpoint(&committed).unwrap(), cp);
            }
            let current = store.load().unwrap().unwrap();
            let history = store.history(None, 0, 10).unwrap();
            assert_eq!(history["items"].as_array().unwrap().len(), 2);
            assert_eq!(history["items"][1]["payload"], payload);
            store.reset().unwrap();
            assert_eq!(store.history(current["plan_id"].as_str(), 0, 10).unwrap()["items"], history["items"]);
            assert_eq!(store.checkpoint(&current).unwrap(), cp);
        }
    }

    #[test]
    fn plan_review_events_and_references_publish_atomically_with_bounded_previews() {
        for point in ["events", "reviews", "checkpoint", "publication_error", "publication"] {
            let temp = Temp::new();
            let store = temp.store();
            let old = publish(&store);
            let cp = store.checkpoint(&old).unwrap();
            let verdict = json!({"scope":"plan","stage_id":null,"role":"reviewer","approved":true,
                "identity":{"scope":"plan"},"criteria":[{"evidence":"full evidence"}],"summary":"small verdict"});
            let mut next = old.clone();
            next["plan_review"] = json!({"version":1,"reviews":[verdict.clone()]});
            let event = json!({"kind":"plan_review","reviews":[verdict.clone()]});
            assert!(store.publish_at(next.clone(),cp.clone(),event.clone(),point).is_err());
            let current = store.load().unwrap().unwrap();
            if point != "publication" {
                assert_eq!(current,old);
                store.publish(next,cp,event.clone()).unwrap();
            }
            let raw = store.load_raw().unwrap().unwrap();
            assert!(raw["plan_review"]["reviews"]["$forge_reviews"].is_string());
            assert_eq!(store.load().unwrap().unwrap()["plan_review"]["reviews"],json!([verdict]));
            assert_eq!(store.history(None,0,10).unwrap()["items"].as_array().unwrap().last().unwrap()["payload"],event);
            let state = store.state_plan(raw.clone());
            assert!(state["plan_review"]["reviews"][0]["criteria"].is_null());
            assert!(state["plan_review"]["reviews"][0]["identity"].is_null());
            assert_eq!(state["plan_review"]["review_count"],1);
            // Polling reads the projection even if full data is not JSON-readable.
            let reference = &raw["architecture"]["plan_review_history"];
            let path = store.review_dir(&raw).unwrap().join(format!("{}.jsonl",reference["file"].as_str().unwrap()));
            fs::write(path,vec![b'x';reference["bytes"].as_u64().unwrap() as usize]).unwrap();
            assert!(store.load().is_err());
            assert_eq!(store.state_plan(store.load_raw().unwrap().unwrap()),state);
        }
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

