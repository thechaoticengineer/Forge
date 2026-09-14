use super::*;

    pub(super) struct Temp(pub(super) PathBuf);
    impl Temp {
        pub(super) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("forge-architecture-{}", identity()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        pub(super) fn store(&self) -> Store {
            Store::new(self.0.clone())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    pub(super) fn plan() -> Value {
        json!({"goal": "goal", "custom": {"preserved": true}, "usage": {"codex": {"total_tokens": 77}},
            "stages": [{"id": 1, "title": "one", "instructions": "work", "acceptance": "test", "commit": "feat: one"}]})
    }
    pub(super) fn publish(store: &Store) -> Value {
        store
            .publish(plan(), checkpoint_default(), json!({"kind": "created"}))
            .unwrap()
    }
    pub(super) fn decision(plan: &Value, id: usize) -> Value {
        json!({"version": 1, "id": format!("decision-{id}"), "plan_id": plan["plan_id"],
            "stage_id": 1, "revision": plan["revision"], "summary": "a".repeat(1000),
            "rationale": "Keep the existing behavior", "alternatives": [{"description": "replace", "tradeoffs": "more risk"}],
            "status": "accepted", "supersedes": null, "created_unix": 10, "updated_unix": 10})
    }

