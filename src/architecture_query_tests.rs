use super::*;
use super::test_support::*;

    #[test]
    fn compact_history_pages_bound_expansion_and_preserve_byte_cursors() {
        let temp = Temp::new();
        let store = temp.store();
        let payload = json!({"kind":"context","data":vec!["x".repeat(4000); 550]});
        let first = store.publish(plan(), checkpoint_default(), payload.clone()).unwrap();
        let second = store.publish(first.clone(), checkpoint_default(), payload.clone()).unwrap();
        let page = store.history(None, 0, 100).unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["payload"], payload);
        assert_eq!(page["next_cursor"], first["architecture"]["event_end"]);
        let page = store.history(None, page["next_cursor"].as_u64().unwrap(), 100).unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["payload"], payload);
        assert!(page["next_cursor"].is_null());
        assert_eq!(page["event_end"], second["architecture"]["event_end"]);
    }

    #[test]
    fn decisions_are_bounded_and_history_is_paginated_with_archive_isolation() {
        let temp = Temp::new();
        let store = temp.store();
        let mut p = publish(&store);
        for id in 0..12 {
            let d = decision(&p, id);
            p = store.record(&p, "decision", d).unwrap();
        }
        let summary = store.summary(Some(&p)).unwrap();
        assert_eq!(summary["recent_decisions"].as_array().unwrap().len(), 8);
        assert!(summary.to_string().len() < 4000);
        let mut cursor = 0;
        let mut count = 0;
        loop {
            let page = store.history(None, cursor, 3).unwrap();
            assert!(page["items"].as_array().unwrap().len() <= 3);
            count += page["items"].as_array().unwrap().len();
            match page["next_cursor"].as_u64() {
                Some(n) => {
                    assert!(n > cursor);
                    cursor = n;
                }
                None => break,
            }
        }
        assert_eq!(count, 13);
        assert!(store.history(None, 1, 20).is_err());
        assert!(store.history(Some("../escape"), 0, 20).is_err());
        let old_id = p["plan_id"].as_str().unwrap().to_owned();
        store.reset().unwrap();
        assert!(store.load().unwrap().is_none());
        let new = publish(&store);
        assert_ne!(new["plan_id"], old_id);
        assert_eq!(
            store.history(Some(&old_id), 0, 1000).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            13
        );
        assert_eq!(
            store.history(None, 0, 100).unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .publish(p, checkpoint_default(), json!({"kind": "reuse"}))
                .is_err()
        );
    }

