use super::*;
use crate::app::App;
use std::sync::Arc;

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) ctx: Ctx,
}
impl Fixture {
    pub(super) fn new(intent: &str, budget: u64) -> Self {
        Self::with_ignored_runtime(intent, budget, false)
    }
    pub(super) fn with_ignored_runtime(intent: &str, budget: u64, ignored: bool) -> Self {
        let mut settings = crate::plan::default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        Self::with_settings(intent, budget, ignored, settings)
    }
    pub(super) fn with_settings(intent: &str, budget: u64, ignored: bool, mut settings: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "forge-gate-test-{}",
            crate::architecture::identity()
        ));
        fs::create_dir_all(&root).unwrap();
        for role in ["planner", "architect", "implementer", "reviewer"] {
            settings[role] = json!("mock");
        }
        settings["auto_push"] = json!(false);
        settings["max_fix_rounds"] = json!(budget);
        let app = Arc::new(App::new(root.to_str().unwrap(), settings));
        let ctx = app.context(root.to_str().unwrap());
        ctx.git(&["init", "-q"]).unwrap();
        ctx.git(&["config", "user.name", "Test"]).unwrap();
        ctx.git(&["config", "user.email", "test@example.invalid"])
            .unwrap();
        ctx.git(&["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(root.join("README.md"), "The program prints a greeting.\n").unwrap();
        ctx.git(&["add", "README.md"]).unwrap();
        if ignored {
            fs::write(root.join(".gitignore"), ".forge/\n").unwrap();
            ctx.git(&["add", ".gitignore"]).unwrap();
        }
        ctx.git(&["commit", "-qm", "initial"]).unwrap();
        ctx.ensure_forge_dir();
        ctx.publish_plan(&json!({"goal":"Improve the project","status":"ready","stages":[{"id":1,"title":intent,"instructions":intent,"acceptance":"The requested change works.","commit":"feat: stage","status":"pending","rounds":0}]}), true).unwrap();
        Self { root, ctx }
    }
    pub(super) fn setting(&self, key: &str, v: Value) {
        self.ctx.app.settings.lock().unwrap()[key] = v;
    }
    pub(super) fn plan(&self) -> Value {
        self.ctx.load_plan().unwrap()
    }
    pub(super) fn run(&self) -> Value {
        self.ctx.run_worker();
        self.plan()
    }
    pub(super) fn count(&self, role: &str) -> usize {
        self.ctx.app.settings.lock().unwrap()[format!("mock_{role}_prompts")]
            .as_array()
            .map_or(0, |prompts| prompts.iter().filter(|p| !p.as_str().unwrap_or("").contains("\"scope\":\"plan\"")).count())
    }
    pub(super) fn docs(&self) {
        self.setting(
            "mock_edits",
            json!([{"README.md":"The program prints a friendly greeting.\n"}]),
        );
    }
    pub(super) fn reviewed(&self) -> Value {
        let p = self.plan();
        let mut p = self
            .ctx
            .architect_publish(p.clone(), Some(&p), "test")
            .unwrap();
        p["stages"][0]["attempt_id"] = json!(crate::architecture::identity());
        p["stages"][0]["review_budget"] = json!(0);
        self.ctx.save_plan(&p).unwrap();
        assert_eq!(self.ctx.run_review_stage(&mut p, 0).unwrap(), "approved");
        p
    }
    pub(super) fn assert_no_commit(&self) {
        assert_eq!(self.ctx.git(&["rev-list", "--count", "HEAD"]).unwrap(), "1");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
pub(super) fn reject(text: &str) -> Value {
    json!({"approved":false,"issues":[text]})
}
pub(super) fn clean() -> Value {
    json!({"approved":true,"issues":[]})
}

impl Fixture {
    pub(super) fn deferred_gate(&self) -> Value {
        let p = self.plan();
        let mut p = self.ctx.architect_publish(p.clone(), Some(&p), "test").unwrap();
        p["stages"][0]["attempt_id"] = json!(crate::architecture::identity());
        p["stages"][0]["attempt_revision"] = p["revision"].clone();
        p["stages"][0]["review_budget"] = json!(0);
        p["stages"][0]["review_cadence"] = json!({"architect":"per_plan","reviewer":"per_plan"});
        self.ctx.save_plan(&p).unwrap();
        assert_eq!(self.ctx.run_review_stage(&mut p, 0).unwrap(), "deferred");
        p
    }

    pub(super) fn two_deferred_stages(&self) -> String {
        let mut p = self.plan();
        let mut second = p["stages"][0].clone();
        second["id"] = json!(2);
        second["title"] = json!("Integrate feature");
        second["instructions"] = json!("Integrate the second feature with the first.");
        second["acceptance"] = json!("Second feature works.\n\n Both features integrate. ");
        second["commit"] = json!("feat: second stage");
        p["stages"].as_array_mut().unwrap().push(second);
        self.ctx.save_plan(&p).unwrap();
        self.setting("review_cadence", json!({"architect":"per_plan","reviewer":"per_plan"}));
        self.setting("mock_edits", json!([{"first.rs":"fn first() {}\n"},{"second.rs":"fn second() {}\n"}]));
        self.ctx.git(&["rev-parse","HEAD"]).unwrap()
    }

    pub(super) fn plan_calls(&self) -> Vec<Value> {
        self.ctx.app.settings.lock().unwrap()["test_review_sessions"].as_array().into_iter().flatten()
            .filter(|r| r["prompt"].as_str().unwrap().contains("\"scope\":\"plan\"" )).cloned().collect()
    }

    pub(super) fn pending_plan_review(&self) -> Value {
        self.two_deferred_stages();
        self.setting("reviewer",json!("unavailable"));
        let mut p = self.run();
        assert!(p["stages"].as_array().unwrap().iter().all(|s| s["status"] == "committed"));
        p.as_object_mut().unwrap().remove("plan_review");
        self.ctx.save_plan(&p).unwrap();
        self.setting("reviewer",json!("mock"));
        self.plan()
    }
}
