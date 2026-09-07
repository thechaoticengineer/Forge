//! Bounded, cancellation-aware subprocess transport. No raw provider diagnostics escape.
use crate::catalogue::{Failure, FailureKind};
use std::io::{Read, Write};
use std::os::{fd::AsRawFd, unix::process::CommandExt};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct Budget {
    pub deadline: Instant,
    pub cancel: Arc<AtomicBool>,
}
impl Budget {
    pub fn check(&self) -> Result<(), Failure> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(Failure::new(FailureKind::Cancelled))
        } else if Instant::now() >= self.deadline {
            Err(Failure::new(FailureKind::Timeout))
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct CommandSpec {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
}
pub(crate) trait Channel: Send {
    fn send(&mut self, text: &str) -> Result<(), Failure>;
    fn line(&mut self) -> Result<Option<String>, Failure>;
    fn finish(&mut self) -> Result<(), Failure>;
}
pub(crate) trait Launcher: Send + Sync {
    fn spawn(&self, spec: &CommandSpec, budget: Budget) -> Result<Box<dyn Channel>, Failure>;
}
pub(crate) struct SystemLauncher;
impl Launcher for SystemLauncher {
    fn spawn(&self, spec: &CommandSpec, budget: Budget) -> Result<Box<dyn Channel>, Failure> {
        budget.check()?;
        let child = Command::new(&spec.executable)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| {
                Failure::new(if e.kind() == std::io::ErrorKind::NotFound {
                    FailureKind::MissingExecutable
                } else {
                    FailureKind::Io
                })
            })?;
        let mut channel = Process {
            child,
            input: None,
            output: None,
            error: None,
            pending: Vec::new(),
            stderr_tail: String::new(),
            budget,
            eof: false,
            stderr_eof: false,
            bytes: 0,
        };
        channel.input = channel.child.stdin.take();
        channel.output = channel.child.stdout.take();
        channel.error = channel.child.stderr.take();
        for fd in [
            channel.input.as_ref().unwrap().as_raw_fd(),
            channel.output.as_ref().unwrap().as_raw_fd(),
            channel.error.as_ref().unwrap().as_raw_fd(),
        ] {
            // Pipes belong exclusively to this transport; nonblocking IO permits one deadline
            // to cover writes, both reads, and shutdown without orphaned reader threads.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    return Err(Failure::new(FailureKind::Io));
                }
            }
        }
        Ok(Box::new(channel))
    }
}
struct Process {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    error: Option<ChildStderr>,
    pending: Vec<u8>,
    stderr_tail: String,
    budget: Budget,
    eof: bool,
    stderr_eof: bool,
    bytes: usize,
}
impl Process {
    fn pump(&mut self) -> Result<(), Failure> {
        self.budget.check()?;
        let mut buf = [0; 8192];
        // Limit each pump even for a child continuously flooding stderr.
        for _ in 0..16 {
            match self.error.as_mut().unwrap().read(&mut buf) {
                Ok(0) => {
                    self.stderr_eof = true;
                    break;
                }
                Ok(n) => {
                    self.stderr_tail
                        .push_str(&String::from_utf8_lossy(&buf[..n]).to_lowercase());
                    if super::catalogue::auth_error(&self.stderr_tail) {
                        return Err(Failure::new(FailureKind::Auth));
                    }
                    self.stderr_tail = self
                        .stderr_tail
                        .chars()
                        .rev()
                        .take(256)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(Failure::new(FailureKind::Io)),
            }
        }
        if !self.eof {
            match self.output.as_mut().unwrap().read(&mut buf) {
                Ok(0) => self.eof = true,
                Ok(n) => {
                    self.bytes += n;
                    if self.bytes > 4 * 1024 * 1024 {
                        return Err(Failure::new(FailureKind::Malformed));
                    }
                    self.pending.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(Failure::new(FailureKind::Io)),
            }
        }
        Ok(())
    }
}
impl Channel for Process {
    fn send(&mut self, text: &str) -> Result<(), Failure> {
        let mut remaining = text.as_bytes();
        while !remaining.is_empty() {
            self.pump()?;
            match self.input.as_mut().unwrap().write(remaining) {
                Ok(0) => return Err(Failure::new(FailureKind::Io)),
                Ok(n) => remaining = &remaining[n..],
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err(Failure::new(FailureKind::Io)),
            }
        }
        Ok(())
    }
    fn line(&mut self) -> Result<Option<String>, Failure> {
        loop {
            self.pump()?;
            if let Some(index) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.pending.drain(..=index).collect();
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|_| Failure::new(FailureKind::Malformed));
            }
            if self.pending.len() > 1024 * 1024 {
                return Err(Failure::new(FailureKind::Malformed));
            }
            if self.eof {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                return String::from_utf8(std::mem::take(&mut self.pending))
                    .map(Some)
                    .map_err(|_| Failure::new(FailureKind::Malformed));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn finish(&mut self) -> Result<(), Failure> {
        loop {
            self.pump()?;
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| Failure::new(FailureKind::Io))?
            {
                if self.eof && self.stderr_eof {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(Failure::new(FailureKind::Process))
                    };
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // Only our own newly-created process group, never other Forge/CLI sessions.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
