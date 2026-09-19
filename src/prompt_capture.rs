//! Test-only prompt recorder. Every provider invocation boundary that receives
//! a complete rendered prompt calls `record_prompt` once, so growth tests see
//! the same {kind, stage_id, bytes, prompt} record for every role. Production
//! builds compile neither this module nor its call sites.
use crate::app::Ctx;
use serde_json::{Value, json};

/// Settings key holding the recorded prompts, oldest first.
pub(crate) const LOG_KEY: &str = "mock_prompt_log";

pub(crate) const ARCHITECT_PUBLISH: &str = "architect_publish";
pub(crate) const STAGE_REVIEW: &str = "stage_review";
pub(crate) const PLAN_REVIEW: &str = "plan_review";
pub(crate) const PLAN_FIX: &str = "plan_fix";
pub(crate) const IMPLEMENTER: &str = "implementer";
pub(crate) const FIXER: &str = "fixer";
pub(crate) const PLANNER_CHAT: &str = "planner_chat";
pub(crate) const ROUTING_RECONCILIATION: &str = "routing_reconciliation";

impl Ctx {
    /// Records one provider prompt. `role` is the provider role that receives
    /// it (`architect`, `reviewer`, `implementer`, ...); `kind` names the
    /// operation. Recording only happens for fake providers.
    pub(crate) fn record_prompt(&self, role: &str, kind: &str, stage_id: Option<i64>, prompt: &str) {
        let mut settings = self.app.settings.lock().unwrap();
        if settings["test_fake_providers"] != true { return; }
        if !settings[LOG_KEY].is_array() { settings[LOG_KEY] = json!([]); }
        settings[LOG_KEY].as_array_mut().unwrap()
            .push(json!({"role":role,"kind":kind,"stage_id":stage_id,"bytes":prompt.len(),"prompt":prompt}));
    }

    /// Every recorded prompt, oldest first.
    pub(crate) fn all_recorded_prompts(&self) -> Vec<Value> {
        self.app.settings.lock().unwrap()[LOG_KEY].as_array().cloned().unwrap_or_default()
    }
}
