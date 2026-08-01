// Transport layer: NDJSON framing and the stdio subprocess, separated from
// the protocol state machine (client.rs only knows the `Transport` trait).
// Ported from the transport half of AcpClient.cpp (writeJson/onReadyRead,
// start/stop, the Windows .cmd/.bat wrapping and env override semantics).
//
// Windows specifics preserved verbatim (acp-protocol.md §2):
//   - .cmd/.bat shims are wrapped in `cmd.exe /c` (CreateProcess cannot exec
//     npm shims directly)
//   - env overrides: a None value DELETES the variable from the inherited
//     environment (provider clearEnvs anti-nesting mechanism)
//   - spawn failure maps to the old waitForStarted(8000) failure path

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, Notify};

use crate::probe;
use crate::provider::EnvOverrides;
use crate::store::paths::is_absolute_windows;

/// Errors from the transport layer and the client driving it.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json serialize error: {0}")]
    Json(#[from] serde_json::Error),
    /// Spawn-level failure: message is the user-facing Chinese string the old
    /// startFailed carried (AcpClient.cpp:55-58, 110-117).
    #[error("spawn failed: {0}")]
    Spawn(String),
}

/// Byte-stream -> NDJSON lines (AcpClient.cpp:290-300): accumulate, split on
/// '\n', trim ASCII whitespace (kills the '\r' of CRLF too), drop empties.
/// Pure and synchronous so framing is unit-testable without any process.
#[derive(Default)]
pub struct LineFramer {
    buf: Vec<u8>,
}

impl LineFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns every complete line it now holds.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            if let Some(s) = trim_line(&line[..line.len() - 1]) {
                out.push(s);
            }
        }
        out
    }

    /// EOF flush: the leftover bytes without a trailing newline still count
    /// as a final line when non-empty.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        trim_line(&rest)
    }
}

fn trim_line(raw: &[u8]) -> Option<String> {
    let mut start = 0;
    let mut end = raw.len();
    while start < end && raw[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && raw[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if start == end {
        return None;
    }
    Some(String::from_utf8_lossy(&raw[start..end]).into_owned())
}

/// One NDJSON line in / one out. `&self` + interior mutability so a future
/// actor loop can hold the client across awaits.
pub trait Transport: Send {
    /// Write one compact JSON line; the transport appends the '\n'
    /// (AcpClient.cpp:246-247).
    fn send_line(&self, line: &str) -> impl std::future::Future<Output = Result<(), AcpError>> + Send;
    /// Next inbound line, trimmed, empty lines already skipped.
    /// Ok(None) = EOF (stdout closed: the process exited).
    fn recv_line(&self) -> impl std::future::Future<Output = Result<Option<String>, AcpError>> + Send;
    /// Exit code once EOF was observed; None when unknown/unavailable.
    fn exit_code(&self) -> impl std::future::Future<Output = Option<i32>> + Send {
        async { None }
    }
    /// Tail of the child's stderr (last K_STDERR_TAIL_CHARS chars), for
    /// surfacing real diagnostics in failure bubbles. Transports without a
    /// captured stderr return an empty string.
    fn stderr_tail(&self) -> String {
        String::new()
    }
}

/// Everything needed to spawn the agent CLI subprocess
/// (AcpClient::start's transport half).
pub struct SpawnConfig {
    pub cli_path: String,
    pub args: Vec<String>,
    /// None value = delete the variable (provider clearEnvs).
    pub env: EnvOverrides,
    /// Child working directory; empty = inherit.
    pub cwd: String,
}

/// stderr tail kept for failure diagnostics (stall/kill bubbles): the last
/// this many chars of everything the child wrote to stderr.
pub const K_STDERR_TAIL_CHARS: usize = 4000;

/// tokio Child stdin/stdout transport. stderr is drained on a background
/// task and only logged (truncated to 500 chars, AcpClient.cpp:79-83); it
/// never participates in the protocol. The last K_STDERR_TAIL_CHARS chars
/// are kept in a tail buffer (stderr_tail()) so failure bubbles can carry
/// the CLI's own dying words instead of a guess.
///
/// Lifecycle: kill_on_drop replaces the old stop()'s kill+waitForFinished
/// (1500ms) — dropping this transport kills the child; tokio reaps it via
/// its orphan queue.
pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    read: Mutex<ReadState>,
    /// Held so Drop kills the child (kill_on_drop); taken by exit_code()
    /// once stdout hit EOF so the real exit code can be awaited.
    child: Mutex<Option<Child>>,
    /// Ring tail of child stderr (chars, capped at K_STDERR_TAIL_CHARS).
    stderr_tail: Arc<std::sync::Mutex<String>>,
}

struct ReadState {
    reader: BufReader<ChildStdout>,
    framer: LineFramer,
    pending: VecDeque<String>,
}

impl StdioTransport {
    /// Resolve the command (PATH lookup + .cmd/.bat wrapping), apply env and
    /// cwd, spawn.
    ///
    /// The old code waited up to 8000ms in waitForStarted (AcpClient.cpp:110);
    /// tokio's spawn() performs CreateProcess synchronously and reports the
    /// same failure class immediately, so no separate wait is needed.
    pub async fn spawn(config: &SpawnConfig) -> Result<Self, AcpError> {
        let cli = config.cli_path.trim();
        if cli.is_empty() {
            return Err(AcpError::Spawn(
                "未配置 CLI 命令，请在配置页填写".to_string(),
            ));
        }

        // Windows shim rule (AcpClient.cpp:93-108): resolve bare names on
        // PATH, then wrap .cmd/.bat in cmd.exe /c. Real .exe passes through.
        let mut program = cli.to_string();
        let mut args = config.args.clone();
        let mut resolved = program.clone();
        if !is_absolute_windows(&resolved) {
            if let Some(found) = probe::find_executable(&program) {
                resolved = found;
            }
        }
        if probe::is_cmd_shim(&resolved) {
            let mut wrapped = vec!["/c".to_string(), resolved];
            wrapped.append(&mut args);
            args = wrapped;
            program = "cmd.exe".to_string();
        } else if !resolved.is_empty() {
            program = resolved;
        }

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !config.cwd.is_empty() {
            cmd.current_dir(&config.cwd);
        }
        // The child inherits this process's full environment by default —
        // the same base the old QProcessEnvironment::systemEnvironment()
        // provided; overrides then apply on top, None = remove
        // (AcpClient.cpp:69-76).
        for (key, value) in &config.env {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }

        let mut child = cmd.spawn().map_err(|e| {
            let msg = e.to_string();
            // Old fallback when errorString was empty (AcpClient.cpp:113-115).
            if msg.is_empty() {
                AcpError::Spawn(format!("无法启动 ACP 进程 «{program}»"))
            } else {
                AcpError::Spawn(msg)
            }
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AcpError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "child stdin unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AcpError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "child stdout unavailable",
            ))
        })?;
        let stderr_tail = Arc::new(std::sync::Mutex::new(String::new()));
        if let Some(mut stderr) = child.stderr.take() {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = String::from_utf8_lossy(&chunk[..n]);
                            {
                                let mut t = tail.lock().unwrap_or_else(|p| p.into_inner());
                                t.push_str(&text);
                                let count = t.chars().count();
                                if count > K_STDERR_TAIL_CHARS {
                                    *t = t.chars().skip(count - K_STDERR_TAIL_CHARS).collect();
                                }
                            }
                            let text: String = text.chars().take(500).collect();
                            let text = text.trim();
                            if !text.is_empty() {
                                log::info!("AcpClient stderr: {text}");
                            }
                        }
                    }
                }
            });
        }

        Ok(Self {
            stdin: Mutex::new(stdin),
            read: Mutex::new(ReadState {
                reader: BufReader::new(stdout),
                framer: LineFramer::new(),
                pending: VecDeque::new(),
            }),
            child: Mutex::new(Some(child)),
            stderr_tail,
        })
    }
}

impl Transport for StdioTransport {
    async fn send_line(&self, line: &str) -> Result<(), AcpError> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn recv_line(&self) -> Result<Option<String>, AcpError> {
        let mut st = self.read.lock().await;
        loop {
            if let Some(line) = st.pending.pop_front() {
                return Ok(Some(line));
            }
            let mut chunk = [0u8; 8192];
            let n = st.reader.read(&mut chunk).await?;
            if n == 0 {
                // EOF: flush the tail, then report closed.
                return Ok(st.framer.finish());
            }
            let lines = st.framer.push(&chunk[..n]);
            st.pending.extend(lines);
        }
    }

    async fn exit_code(&self) -> Option<i32> {
        let child = self.child.lock().await.take();
        match child {
            Some(mut c) => Some(c.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1)),
            None => None,
        }
    }

    fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// In-memory scripted transport for unit tests (and phase 1d chat tests):
/// inbound lines are fed explicitly, everything sent is recorded verbatim.
/// `feed_eof` scripts stdout closing. Clone-cheap (Arc-backed): keep one
/// handle for the client and another for the test script.
#[derive(Clone, Default)]
pub struct MockTransport {
    inbound: Arc<Mutex<VecDeque<Option<String>>>>,
    outbound: Arc<std::sync::Mutex<Vec<String>>>,
    notify: Arc<Notify>,
    code: Arc<std::sync::Mutex<Option<i32>>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script one inbound line (a full JSON frame without the '\n').
    pub async fn feed_line(&self, line: impl Into<String>) {
        self.inbound.lock().await.push_back(Some(line.into()));
        self.notify.notify_one();
    }

    /// Convenience: serialize a JSON value and feed it as one line.
    pub async fn feed_json(&self, value: serde_json::Value) {
        self.feed_line(value.to_string()).await;
    }

    /// Script EOF (process exit) with the given exit code.
    pub async fn feed_eof(&self, code: i32) {
        *self.code.lock().expect("mock code poisoned") = Some(code);
        self.inbound.lock().await.push_back(None);
        self.notify.notify_one();
    }

    /// Every line the client wrote, without the trailing '\n'.
    pub fn sent(&self) -> Vec<String> {
        self.outbound.lock().expect("mock outbound poisoned").clone()
    }

    /// sent() parsed as JSON values.
    pub fn sent_json(&self) -> Vec<serde_json::Value> {
        self.sent()
            .iter()
            .map(|l| serde_json::from_str(l).expect("client must emit valid JSON"))
            .collect()
    }
}

impl Transport for MockTransport {
    async fn send_line(&self, line: &str) -> Result<(), AcpError> {
        self.outbound
            .lock()
            .expect("mock outbound poisoned")
            .push(line.to_string());
        Ok(())
    }

    async fn recv_line(&self) -> Result<Option<String>, AcpError> {
        loop {
            let item = self.inbound.lock().await.pop_front();
            match item {
                Some(Some(line)) => return Ok(Some(line)),
                Some(None) => return Ok(None),
                None => self.notify.notified().await,
            }
        }
    }

    async fn exit_code(&self) -> Option<i32> {
        *self.code.lock().expect("mock code poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framer_splits_across_chunk_boundaries() {
        let mut f = LineFramer::new();
        // A line split over three pushes only surfaces once complete.
        assert!(f.push(br#"{"i"#).is_empty());
        assert!(f.push(br#"d":"#).is_empty());
        assert_eq!(f.push(b"1}\n{\"a\""), vec![r#"{"id":1}"#]);
        // Buffer stitching: two complete lines in one push.
        assert_eq!(f.push(b":2}\n{\"b\":3}\n"), vec![r#"{"a":2}"#, r#"{"b":3}"#]);
    }

    #[test]
    fn framer_trims_crlf_and_skips_empty_lines() {
        let mut f = LineFramer::new();
        let lines = f.push(b"  padded  \r\n\r\n\n\t\nnext\n");
        assert_eq!(lines, vec!["padded", "next"]);
    }

    #[test]
    fn framer_finish_flushes_tail_without_newline() {
        let mut f = LineFramer::new();
        assert!(f.push(b"tail").is_empty());
        assert_eq!(f.finish(), Some("tail".to_string()));
        assert_eq!(f.finish(), None, "tail is consumed once");
    }

    #[tokio::test]
    async fn mock_transport_records_and_replays() {
        let t = MockTransport::new();
        t.feed_line("hello").await;
        t.feed_eof(0).await;
        assert_eq!(t.recv_line().await.expect("recv"), Some("hello".to_string()));
        assert_eq!(t.recv_line().await.expect("recv"), None);
        assert_eq!(t.exit_code().await, Some(0));
        t.send_line(r#"{"a":1}"#).await.expect("send");
        assert_eq!(t.sent(), vec![r#"{"a":1}"#]);
    }

    /// End-to-end through real stdio: a fake .cmd agent proves the
    /// PATH-absolute shim stays wrapped in cmd.exe /c and NDJSON flows both
    /// ways.
    #[tokio::test]
    async fn stdio_transport_wraps_cmd_shim_and_exchanges_lines() {
        let tmp = tempfile::tempdir().expect("tmp");
        let fake = tmp.path().join("fake-agent.cmd");
        // Reads one line, echoes a fixed frame back, then exits (code 3
        // proves exit code propagation).
        std::fs::write(
            &fake,
            "@echo off\nset /p req=\necho {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\nexit /b 3\n",
        )
        .expect("write fake");

        let t = StdioTransport::spawn(&SpawnConfig {
            cli_path: fake.to_string_lossy().into_owned(),
            args: vec![],
            env: vec![],
            cwd: String::new(),
        })
        .await
        .expect("spawn wrapped shim");
        t.send_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .expect("send");
        let line = t.recv_line().await.expect("recv").expect("a line");
        // `set /p` writes its prompt to stdout with no newline, so the frame
        // arrives glued to "req=" — exactly the banner-noise shape real CLIs
        // produce; the client layer discards such lines as bad JSON.
        assert!(
            line.ends_with(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            "{line}"
        );
        assert_eq!(t.recv_line().await.expect("recv"), None, "script exits");
        assert_eq!(t.exit_code().await, Some(3));
    }

    /// env override semantics (AcpClient.cpp:69-76): Some sets/overrides,
    /// None DELETES the variable from the inherited environment.
    #[tokio::test]
    async fn spawn_env_overrides_set_and_delete() {
        // Unique names so parallel tests never collide on process env.
        std::env::set_var("WARDEX_ACP_TEST_DELETE_ME", "leaked");
        let tmp = tempfile::tempdir().expect("tmp");
        let fake = tmp.path().join("env-agent.cmd");
        std::fs::write(
            &fake,
            "@echo off\necho [%WARDEX_ACP_TEST_SET%]\necho [%WARDEX_ACP_TEST_DELETE_ME%]\nexit /b 0\n",
        )
        .expect("write fake");

        let t = StdioTransport::spawn(&SpawnConfig {
            cli_path: fake.to_string_lossy().into_owned(),
            args: vec![],
            env: vec![
                (
                    "WARDEX_ACP_TEST_SET".to_string(),
                    Some("injected".to_string()),
                ),
                ("WARDEX_ACP_TEST_DELETE_ME".to_string(), None),
            ],
            cwd: String::new(),
        })
        .await
        .expect("spawn");
        std::env::remove_var("WARDEX_ACP_TEST_DELETE_ME");

        assert_eq!(
            t.recv_line().await.expect("recv"),
            Some("[injected]".to_string())
        );
        // Undefined in the child: batch-file expansion turns %VAR% into an
        // empty string (interactive cmd would keep the literal), so an empty
        // bracket pair is exactly what deletion looks like. Had the parent's
        // "leaked" value survived, this would read "[leaked]".
        assert_eq!(t.recv_line().await.expect("recv"), Some("[]".to_string()));
        assert_eq!(t.recv_line().await.expect("recv"), None);
    }

    #[tokio::test]
    async fn spawn_rejects_empty_cli_path() {
        let err = match StdioTransport::spawn(&SpawnConfig {
            cli_path: "   ".into(),
            args: vec![],
            env: vec![],
            cwd: String::new(),
        })
        .await
        {
            Err(e) => e,
            Ok(_) => panic!("empty cli path must fail"),
        };
        assert!(matches!(err, AcpError::Spawn(_)));
        assert_eq!(err.to_string(), "spawn failed: 未配置 CLI 命令，请在配置页填写");
    }
}
