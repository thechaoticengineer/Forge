use crate::app::{App, Ctx};
use crate::http::handle;
use crate::plan::{default_settings, mutate_queue};
use serde_json::{Value, json};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) fn seed_mock_plan(ctx: &Ctx) {
    ctx.ensure_forge_dir();
    ctx.mock_agent("planner").unwrap();
    let plan: Value = serde_json::from_slice(&fs::read(ctx.forge_path("plan-candidate.json")).unwrap()).unwrap();
    ctx.publish_plan(&plan, true).unwrap();
}

pub(crate) fn editable_stage(id: i64) -> Value {
    json!({"id": id, "title": "Repaired title", "instructions": "Repaired instructions",
        "acceptance": "", "commit": "feat: repair stage"})
}

pub(crate) struct QueueTest {
    pub(crate) app: Ctx,
    pub(crate) path: PathBuf,
}

impl QueueTest {
    pub(crate) fn new(auto_approve: bool) -> Self {
        Self::with_engine(auto_approve, None)
    }

    pub(crate) fn with_engine(auto_approve: bool, engine: Option<Arc<App>>) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "forge-queue-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst),
        ));
        fs::create_dir(&path).unwrap();
        let mut settings = default_settings();
        settings["review_cadence"] = json!({"architect":"per_stage","reviewer":"per_stage"});
        settings["planner"] = json!("mock");
        settings["implementer"] = json!("mock");
        settings["reviewer"] = json!("mock");
        settings["auto_push"] = json!(false);
        settings["queue_auto_approve"] = json!(auto_approve);
        let engine = engine.unwrap_or_else(|| Arc::new(App::new(&path.display().to_string(), settings)));
        let app = engine.context(&path.display().to_string());
        app.git(&["init", "-q"]).unwrap();
        for (key, value) in [
            ("user.name", "Forge Test"), ("user.email", "test@example.invalid"),
            ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null"),
        ] {
            app.git(&["config", key, value]).unwrap();
        }
        app.git(&["commit", "--allow-empty", "-qm", "initial"]).unwrap();
        let mut queue = json!({"items": []});
        for goal in ["first goal", "second goal"] {
            mutate_queue(&mut queue, "add", &json!({"goal": goal})).unwrap();
        }
        app.save_queue(&queue);
        Self { app, path }
    }

    pub(crate) fn start(&self) {
        self.app.acquire_busy().unwrap();
        self.app.session.queue_active.store(true, Ordering::SeqCst);
        self.app.queue_worker(None);
    }

    pub(crate) fn statuses(&self) -> Vec<String> {
        self.app.load_queue()["items"].as_array().unwrap().iter()
            .map(|item| item["status"].as_str().unwrap().to_string()).collect()
    }
}

impl Drop for QueueTest {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn api_request(engine: &Arc<App>, method: &str, path: &str, body: Value) -> (u16, Value) {
    use std::io::Read as _;
    let server = tiny_http::Server::http(("127.0.0.1", 0)).unwrap();
    let address = server.server_addr().to_ip().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| handle(engine, server.recv().unwrap()));
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.to_string();
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        // Chunk boundaries are byte boundaries and may split a UTF-8 character.
        // Decode transfer framing before interpreting the JSON as text.
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let boundary = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&response[..boundary]).unwrap();
        let body = &response[boundary + 4..];
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        let decoded = if headers.to_lowercase().contains("transfer-encoding: chunked") {
            let mut rest = body;
            let mut decoded = Vec::new();
            loop {
                let boundary = rest.windows(2).position(|w| w == b"\r\n").unwrap();
                let size = usize::from_str_radix(std::str::from_utf8(&rest[..boundary]).unwrap(), 16).unwrap();
                if size == 0 { break; }
                let tail = &rest[boundary + 2..];
                decoded.extend_from_slice(&tail[..size]);
                rest = &tail[size + 2..];
            }
            decoded
        } else { body.to_vec() };
        (status, serde_json::from_slice(&decoded).unwrap())
    })
}

pub(crate) fn wait_for_worker(ctx: &Ctx) {
    let started = Instant::now();
    while ctx.session.busy.load(Ordering::SeqCst) {
        assert!(started.elapsed() < Duration::from_secs(5), "worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}
