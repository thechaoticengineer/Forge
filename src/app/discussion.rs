//! Read-only pre-planning discussion about the repository and possible solutions.
use super::{Ctx, WorkerGuard};
use crate::prompts::DISCUSS_PROMPT;
use crate::util::{fill_template, json_payload, unix_timestamp};
use serde_json::{Value, json};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};

impl Ctx {
    /// Answers one discussion message. Only a successful reply extends the
    /// transcript; the plan, goal, phase and Plan Q&A are never touched.
    pub(crate) fn discuss_worker(&self, message: &str, request_id: i64) {
        let _worker = WorkerGuard(&self.session);
        self.set_step(None, "discussing before planning");
        let result = (|| -> Result<(), String> {
            self.ensure_forge_dir();
            match fs::remove_file(self.forge_path("answer.json")) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("could not remove previous answer: {e}")),
            }
            let history = serde_json::to_string_pretty(&self.read_discussion()).unwrap();
            let prompt = fill_template(DISCUSS_PROMPT, &[
                ("{history}", history.as_str()), ("{message}", message),
                ("{answer_path}", ".forge/answer.json"),
            ]);
            let readonly_prompt = format!("{prompt}\nOUTPUT CONTRACT OVERRIDE: read-only discussion in a fresh conversation. Do not write files or create a plan. Return ONLY {{\"answer\":\"your reply\"}}.");
            let reply = self.readonly_response("chat", &readonly_prompt, Some("answer.json"), |text| {
                let output: Value = crate::response::parse_json(json_payload(text))
                    .map_err(|e| format!("invalid answer JSON: {e}"))?;
                crate::response::object_fields(&output, &["answer"])?;
                output["answer"].as_str().filter(|answer| !answer.trim().is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| "agent did not produce a non-empty answer string".to_string())
            })?;
            let user = json!({"role": "user", "text": message, "unix": unix_timestamp()});
            let assistant = json!({"role": "assistant", "text": reply.value, "unix": unix_timestamp()});
            let mut file = fs::OpenOptions::new().create(true).read(true).append(true)
                .open(self.forge_path("discussion.jsonl")).map_err(|e| e.to_string())?;
            // An interrupted earlier append can leave a partial final line; start
            // on a fresh line so the reader skips only that fragment.
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            let mut last = [b'\n'];
            if size > 0 {
                file.seek(SeekFrom::Start(size - 1)).and_then(|_| file.read_exact(&mut last))
                    .map_err(|e| e.to_string())?;
            }
            let separator = if last[0] == b'\n' { "" } else { "\n" };
            writeln!(file, "{separator}{user}\n{assistant}").map_err(|e| e.to_string())
        })();
        let (activity, kind, text) = match result {
            Ok(()) => (json!({"status":"ready","request_id":request_id,"unix":unix_timestamp()}),
                "discussion", "discussion reply ready".to_string()),
            Err(error) => (json!({"status":"failed","request_id":request_id,"message":message,
                "error":error,"unix":unix_timestamp()}), "error", format!("discussion failed: {error}")),
        };
        let current = {
            let mut state = self.session.state.lock().unwrap();
            let current = state.discussion_serial == request_id;
            if current { state.discussion_activity = activity; }
            current
        };
        if current { self.log_event(kind, &text); }
    }
}
