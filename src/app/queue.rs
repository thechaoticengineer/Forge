//! Goal queue storage and coordination under the caller-owned session busy claim.
use super::{Ctx, PlanMode, WorkerGuard};
use crate::util::unix_timestamp;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::Ordering;

impl Ctx {
    pub(crate) fn load_queue(&self) -> Value {
        fs::read_to_string(self.forge_path("queue.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|queue| queue.get("items").is_some_and(Value::is_array))
            .unwrap_or_else(|| json!({"items": []}))
    }

    pub(crate) fn save_queue(&self, queue: &Value) {
        self.ensure_forge_dir();
        let _ = fs::write(
            self.forge_path("queue.json"),
            serde_json::to_string_pretty(queue).unwrap(),
        );
    }

    /// Caller holds queue_lock so API edits cannot overwrite worker transitions.
    pub(crate) fn set_queue_status(&self, queue: &mut Value, id: u64, status: &str) {
        if let Some(item) = queue["items"].as_array_mut().unwrap().iter_mut()
            .find(|item| item["id"].as_u64() == Some(id))
        {
            item["status"] = json!(status);
            if matches!(status, "planning" | "awaiting_approval" | "running") {
                item["resume_phase"] = json!(status);
            }
            if matches!(status, "awaiting_approval" | "running") {
                if let Some(plan) = self.load_plan().filter(|plan| plan["goal"] == item["goal"]) {
                    item["plan_id"] = plan["plan_id"].clone();
                }
            }
            self.save_queue(queue);
            self.log_event("queue", &format!("goal {id}: {status}"));
        }
    }

    /// Reuse the saved plan when possible; only a failed planning attempt may
    /// regenerate a plan. Never reset execution/review budgets to unblock a goal.
    pub(crate) fn queue_retry_mode(&self, item: &Value, plan: Option<&Value>) -> Result<&'static str, String> {
        if let Some(plan) = plan.filter(|plan| plan["goal"] == item["goal"]
            && (item["plan_id"].is_null() || item["plan_id"] == plan["plan_id"])) {
            return match plan["status"].as_str() {
                Some("draft") => Ok("awaiting_approval"),
                Some("approved" | "done") => Ok("running"),
                _ => Err("The saved queue plan has an unsupported status; restore it before retrying.".into()),
            };
        }
        let phase = if let Some(phase) = item["resume_phase"].as_str() {
            Some(phase.to_owned())
        } else {
            // Older queues did not persist their resume phase. Require a matching
            // lifecycle record. Stream the durable log, not the UI's last 400
            // events: a preceding goal may have produced many events meanwhile.
            use std::io::BufRead as _;
            let prefix = format!("goal {}: ", item["id"]);
            let mut phase = None;
            match fs::File::open(self.forge_path("history.jsonl")) {
                Ok(file) => for line in std::io::BufReader::new(file).lines() {
                    let line = line.map_err(|e| format!("Cannot verify queue recovery history: {e}"))?;
                    if line.trim().is_empty() { continue; }
                    let event: Value = serde_json::from_str(&line)
                        .map_err(|e| format!("Cannot verify queue recovery history: {e}"))?;
                    if event["kind"] == "queue"
                        && event["unix"].as_i64().unwrap_or(0) >= item["added_unix"].as_i64().unwrap_or(0) {
                        if let Some(value) = event["text"].as_str().and_then(|text| text.strip_prefix(&prefix))
                            .filter(|value| matches!(*value, "planning" | "awaiting_approval" | "running")) {
                            phase = Some(value.to_owned());
                        }
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("Cannot verify queue recovery history: {e}")),
            }
            phase
        };
        if phase.as_deref() == Some("planning") && item["plan_id"].is_null() {
            Ok("queued")
        } else {
            Err("The saved plan for this queue goal is unavailable. Restore it before retrying; execution progress will not be reset.".into())
        }
    }

    /// Caller holds queue_lock and owns the busy claim.
    pub(crate) fn start_queue_run(&self, queue: &mut Value, id: u64, plan: &mut Value) -> Result<(), String> {
        let head = crate::plan::queue_head(queue).ok_or("no queued goals")?;
        if head["id"].as_u64() != Some(id) || head["goal"] != plan["goal"] {
            return Err(crate::plan::queue_order_error(head));
        }
        *plan = self.architect_publish(plan.clone(), Some(plan), "queue approval")?;
        plan["status"] = json!("approved");
        self.save_plan(plan)?;
        self.set_queue_status(queue, id, "running");
        let mut s = self.session.state.lock().unwrap();
        s.goal = plan["goal"].as_str().unwrap_or("").to_string();
        s.phase = "running".into();
        s.run_started_unix = unix_timestamp();
        Ok(())
    }

    fn run_queue_item(&self, id: u64) -> bool {
        {
            let _queue_guard = self.session.queue_lock.lock().unwrap();
            let queue = self.load_queue();
            let plan = self.load_plan();
            let valid = crate::plan::queue_head(&queue).is_some_and(|head|
                head["id"].as_u64() == Some(id)
                    && matches!(head["status"].as_str(), Some("running" | "blocked" | "failed"))
                    && plan.as_ref().is_some_and(|plan| head["goal"] == plan["goal"]));
            if !valid {
                self.session.queue_active.store(false, Ordering::SeqCst);
                self.set_phase("blocked");
                self.log_event("queue", "queue paused: saved plan does not match the first unfinished goal");
                return false;
            }
        }
        self.run_with_busy_claim();
        let _queue_guard = self.session.queue_lock.lock().unwrap();
        let phase = self.session.state.lock().unwrap().phase.clone();
        // A user-stopped run retains its plan for human intervention.
        let status = match phase.as_str() {
            "done" => "done",
            "failed" => "failed",
            _ => "blocked",
        };
        let mut queue = self.load_queue();
        if status == "done" {
            queue["items"].as_array_mut().unwrap()
                .retain(|item| item["id"].as_u64() != Some(id));
            self.save_queue(&queue);
            self.log_event("queue", &format!("goal {id}: done — removed from queue"));
        } else {
            self.set_queue_status(&mut queue, id, status);
            self.session.queue_active.store(false, Ordering::SeqCst);
        }
        status == "done" && self.session.queue_active.load(Ordering::SeqCst)
    }

    /// Start fresh, or finish an explicitly approved item before taking the next.
    pub(crate) fn queue_worker(&self, approved: Option<u64>) {
        let _worker = WorkerGuard(&self.session);
        if let Some(id) = approved
            && !self.run_queue_item(id)
        {
            return;
        }
        loop {
            let (id, goal) = {
                let _queue_guard = self.session.queue_lock.lock().unwrap();
                if !self.session.queue_active.load(Ordering::SeqCst) {
                    return;
                }
                let mut queue = self.load_queue();
                let Some(item) = crate::plan::queue_head(&queue)
                else {
                    self.session.queue_active.store(false, Ordering::SeqCst);
                    self.log_event("queue", "queue complete");
                    return;
                };
                if item["status"] != "queued" {
                    self.session.queue_active.store(false, Ordering::SeqCst);
                    self.set_phase("blocked");
                    self.log_event("queue", &crate::plan::queue_order_error(item));
                    return;
                }
                let id = item["id"].as_u64().unwrap();
                let goal = item["goal"].as_str().unwrap_or("").to_string();
                {
                    let mut s = self.session.state.lock().unwrap();
                    s.goal = goal.clone();
                    s.phase = "planning".into();
                }
                self.set_queue_status(&mut queue, id, "planning");
                (id, goal)
            };
            let planned = self.plan_with_busy_claim(&goal, &PlanMode::Standard);
            {
                let _queue_guard = self.session.queue_lock.lock().unwrap();
                let mut queue = self.load_queue();
                if !planned {
                    let blocked = self.session.state.lock().unwrap().phase == "blocked";
                    self.set_queue_status(&mut queue, id, if blocked { "blocked" } else { "failed" });
                    self.session.queue_active.store(false, Ordering::SeqCst);
                    return;
                }
                if !self.session.queue_active.load(Ordering::SeqCst) {
                    self.set_queue_status(&mut queue, id, "blocked");
                    return;
                }
                let auto_approve = self.app.settings.lock().unwrap()["queue_auto_approve"]
                    .as_bool().unwrap_or(false);
                if !auto_approve {
                    self.set_queue_status(&mut queue, id, "awaiting_approval");
                    return;
                }
                let mut plan = self.load_plan().unwrap();
                self.log_event("queue", &format!("goal {id}: automatically approved"));
                if let Err(error) = self.start_queue_run(&mut queue, id, &mut plan) {
                    self.set_phase("failed");
                    self.log_event("error", &error);
                    self.set_queue_status(&mut queue, id, "failed");
                    break;
                }
            }
            if !self.run_queue_item(id) {
                return;
            }
        }
    }
}
