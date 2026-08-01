// CLI probing (--version validation with a 4s hard timeout) and the testAgent
// connectivity check (a one-shot ACP `initialize` handshake with a 15s
// watchdog). Ported from src/CliProbe.cpp and the testAgent part of
// src/AgentStore.cpp; authoritative spec: docs/providers-and-cli.md §4-§5.
//
// Timeout semantics that MUST be preserved:
//   - probe: a binary still alive after 4s is ACCEPTED with an empty version
//     ("exists and runs, just has no --version or waits on stdin") — never
//     report that as a failure.
//   - testAgent: success means the initialize handshake AND a real one-word
//     model call (session/new + session/prompt) both completed — handshake
//     alone proves nothing about Base URL / API Key / network reachability.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::provider::{self, ProviderSpec};
use crate::store::agents::{Agent, AgentStore};
use crate::store::paths::{clean_path_forward, is_absolute_windows};

/// Hard timeout per `--version` candidate (CliProbe.cpp:143-151).
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(4);
/// testAgent handshake watchdog (AgentStore.cpp:408).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// testAgent phase 2 watchdog: session/new + one-word prompt. Generous
/// because a cold model endpoint can be slow, but bounded so a dead
/// endpoint (TLS resets, silent drops) reports instead of hanging.
pub const MODEL_CALL_TIMEOUT: Duration = Duration::from_secs(90);

/// Config-page help dialog constants (CliProbe.cpp:22-38).
pub const INSTALL_HELP_URL: &str = "https://www.kimi.com/code";
pub const INSTALL_HELP_TEXT: &str = "WarDex 通过本机 Kimi CLI 与模型通信。\n\n1. 安装 Kimi Code CLI（官方文档 / 产品页）。\n\n2. 安装完成后常见路径：\n   %USERPROFILE%\\.kimi-code\\bin\\kimi.exe\n\n3. 回到本页点击「检测 CLI」，成功后会自动填入绝对路径。\n   也可点「浏览…」手动选择 kimi.exe。\n\n说明：仅写 kimi 时，图形界面 PATH 可能与 PowerShell 不一致，因此建议保存绝对路径。";

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Probe outcome; serialized with the same camelCase keys the old QVariantMap
/// had. `error` is "" | "not_found" | "unsupported".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub provider_id: String,
    pub found: bool,
    /// Native separators; empty when not found.
    pub path: String,
    /// First output line; may be empty when the 4s timeout was accepted.
    pub version: String,
    pub error: String,
    pub message: String,
}

/// CLI detector with a per-provider result cache. Cache registration
/// (performance.md §3): one entry per provider, explicit refresh via probe(),
/// invalidated on agent config change via invalidate()/invalidate_all().
#[derive(Default)]
pub struct CliProbe {
    results: HashMap<String, ProbeResult>,
}

impl CliProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached result (key: trim + lowercase provider id).
    pub fn result(&self, provider_id: &str) -> Option<&ProbeResult> {
        self.results.get(&provider_id.trim().to_lowercase())
    }

    /// Drop one provider's cached result (agent config changed).
    pub fn invalidate(&mut self, provider_id: &str) {
        self.results.remove(&provider_id.trim().to_lowercase());
    }

    pub fn invalidate_all(&mut self) {
        self.results.clear();
    }

    /// probe(): scan the full candidate queue (CliProbe.cpp:63-75) and run
    /// --version on each surviving file until one is accepted. The result is
    /// cached per provider.
    pub async fn probe(&mut self, provider_id: &str, preferred_path: &str) -> ProbeResult {
        let Some(spec) = provider::spec(provider_id) else {
            return self.publish(unsupported_result(provider_id));
        };
        if spec.default_command.is_empty() {
            // "custom" has no canonical CLI to look for.
            return self.publish(unsupported_result(spec.id));
        }

        let home = dirs::home_dir();
        let app_data = std::env::var("APPDATA").ok();
        let program_files = std::env::var("ProgramFiles").ok();
        let mut queue = candidate_paths(
            spec,
            preferred_path,
            home.as_deref(),
            app_data.as_deref(),
            program_files.as_deref(),
        );
        let dirs = expanded_search_path(
            spec.id,
            &std::env::var("PATH").unwrap_or_default(),
            user_path_from_registry().as_deref(),
            home.as_deref(),
        );
        if let Some(found) = which_on_path(&dirs, spec.default_command) {
            queue.push(found);
        }
        let queue = dedupe_keep_order(queue);

        let result = self.scan(spec, queue).await;
        self.publish(result)
    }

    /// probePath(): validate a single user-picked file (supports file: URLs).
    /// Result is cached under the provider too.
    pub async fn probe_path(&mut self, provider_id: &str, absolute_path: &str) -> ProbeResult {
        let pid = provider_id.trim().to_lowercase();
        let path = local_file_from_maybe_url(absolute_path.trim());
        let spec = provider::spec(&pid);
        let display = spec.map(|s| s.display_name).unwrap_or(pid.as_str());
        // scan() needs a ProviderSpec for the message; for unknown providers
        // fall back to a manual loop with the raw id as display name.
        match spec {
            Some(s) => {
                let result = self.scan(s, vec![PathBuf::from(&path)]).await;
                self.publish(result)
            }
            None => {
                let result = match check_version(Path::new(&path)).await {
                    Ok(Some(version)) => found_result(&pid, display, &path, &version),
                    _ => not_found_result(&pid, display),
                };
                self.publish(result)
            }
        }
    }

    /// Walk the queue: skip missing files, --version the rest, stop at the
    /// first accepted candidate (CliProbe.cpp:114-199).
    async fn scan(&self, spec: &ProviderSpec, queue: Vec<PathBuf>) -> ProbeResult {
        for path in queue {
            let is_file = std::fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false);
            if !is_file {
                continue;
            }
            let native = native_separators(&path.to_string_lossy());
            match check_version(&path).await {
                Ok(Some(version)) => {
                    return found_result(spec.id, spec.display_name, &native, &version);
                }
                // Rejected candidate or spawn/read failure: try the next one.
                Ok(None) | Err(_) => continue,
            }
        }
        not_found_result(spec.id, spec.display_name)
    }

    fn publish(&mut self, result: ProbeResult) -> ProbeResult {
        if !result.provider_id.is_empty() {
            self.results
                .insert(result.provider_id.clone(), result.clone());
        }
        result
    }
}

/// Candidate queue in the exact old order (CliProbe.cpp:249-287); the PATH
/// hit is appended by the caller afterwards. Split out as a pure function
/// (env values passed in) so tests don't mutate process env.
fn candidate_paths(
    spec: &ProviderSpec,
    preferred_path: &str,
    home: Option<&Path>,
    app_data: Option<&str>,
    program_files: Option<&str>,
) -> Vec<PathBuf> {
    let cmd = spec.default_command;
    let mut out: Vec<PathBuf> = Vec::new();

    // 1. Agent's cliPath override first — unless it is just the bare command
    //    name (the PATH lookup covers that). Without an .exe/.cmd suffix, add
    //    a `preferred + ".exe"` variant.
    let pref = preferred_path.trim();
    if !pref.is_empty()
        && !pref.eq_ignore_ascii_case(cmd)
        && !pref.eq_ignore_ascii_case(&format!("{cmd}.exe"))
        && !pref.eq_ignore_ascii_case(&format!("{cmd}.cmd"))
    {
        out.push(PathBuf::from(pref));
        let lower = pref.to_lowercase();
        if !lower.ends_with(".exe") && !lower.ends_with(".cmd") {
            out.push(PathBuf::from(format!("{pref}.exe")));
        }
    }

    // 2. kimi-only known install dirs, best first.
    if spec.id == "kimi" {
        if let Some(home) = home {
            out.push(home.join(".kimi-code/bin/kimi.exe"));
            out.push(home.join(".kimi-code/bin/kimi"));
            out.push(home.join("AppData/Local/kimi-code/bin/kimi.exe"));
            out.push(home.join("AppData/Local/Programs/kimi-code/kimi.exe"));
        }
    }

    // 3. npm global shim locations (claude-code-acp / codex-acp install via
    //    npm -g; the GUI process PATH often misses these directories).
    if !cmd.is_empty() {
        if let Some(app_data) = app_data {
            if !app_data.is_empty() {
                out.push(PathBuf::from(format!(r"{app_data}\npm\{cmd}.cmd")));
                out.push(PathBuf::from(format!(r"{app_data}\npm\{cmd}.exe")));
            }
        }
        if let Some(pf) = program_files {
            if !pf.is_empty() {
                out.push(PathBuf::from(format!(r"{pf}\nodejs\{cmd}.cmd")));
            }
        }
    }
    out
}

/// GUI processes often lag the user shell's PATH (CliProbe.cpp:289-308):
/// base = system PATH; then the registry HKCU\Environment "Path" entries are
/// PREPENDED one by one when missing (user PATH first; iterating and
/// prepending reverses their relative order — faithful to the old loop);
/// kimi additionally prepends ~/.kimi-code/bin.
fn expanded_search_path(
    provider_id: &str,
    system_path: &str,
    user_path: Option<&str>,
    home: Option<&Path>,
) -> Vec<String> {
    let mut parts: Vec<String> = system_path
        .split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(user_path) = user_path {
        for p in user_path.split(';').filter(|s| !s.is_empty()) {
            if !parts.iter().any(|x| x.eq_ignore_ascii_case(p)) {
                parts.insert(0, p.to_string());
            }
        }
    }
    if provider_id == "kimi" {
        if let Some(home) = home {
            let kimi_bin = home.join(".kimi-code").join("bin");
            let kimi_bin = kimi_bin.to_string_lossy().into_owned();
            if !parts.iter().any(|x| x.eq_ignore_ascii_case(&kimi_bin)) {
                parts.insert(0, kimi_bin);
            }
        }
    }
    parts
}

/// User-level PATH from HKCU\Environment ("Path" value). May be
/// REG_EXPAND_SZ with %VAR% references — used verbatim, like the old
/// QSettings NativeFormat read. Any registry failure degrades to None.
fn user_path_from_registry() -> Option<String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let env = hkcu.open_subkey("Environment").ok()?;
    env.get_value::<String, _>("Path").ok()
}

/// First existing file among <name>, <name>.exe, <name>.cmd, <name>.bat in
/// each dir, dirs in order (CliProbe.cpp:310-327). pub(crate): shared with
/// the ACP transport's spawn-time command resolution.
pub(crate) fn which_on_path(dirs: &[String], name: &str) -> Option<PathBuf> {
    for dir in dirs {
        for candidate in [
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ] {
            let path = Path::new(dir).join(candidate);
            if std::fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false) {
                return Some(path);
            }
        }
    }
    None
}

/// Order-preserving dedupe: cleanPath-normalized, case-insensitive
/// (CliProbe.cpp:68-75).
fn dedupe_keep_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for p in paths {
        let clean = clean_path_forward(&p.to_string_lossy());
        if clean.is_empty() {
            continue;
        }
        let key = clean.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            out.push(PathBuf::from(clean));
        }
    }
    out
}

/// Run `<path> --version` with the 4s hard timeout. Return:
///   Ok(Some(version)) — accepted; version may be EMPTY (timeout acceptance)
///   Ok(None)          — ran but rejected (crash / no output / nonzero exit)
///   Err(_)            — spawn or pipe failure (caller tries the next one)
///
/// The exe's own directory is prepended to the child PATH so the CLI finds
/// its bundled DLLs/node (CliProbe.cpp:136-141).
async fn check_version(path: &Path) -> Result<Option<String>, ProbeError> {
    let path_str = path.to_string_lossy().into_owned();
    let mut cmd = if is_cmd_shim(&path_str) {
        // .cmd/.bat shims cannot be CreateProcess'd directly — wrap them.
        // (The old probe spawned them raw and relied on the failure falling
        // through to the next candidate; wrapping makes npm shims actually
        // validate, matching what testAgent/ACP transports already do.)
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&path_str).arg("--version");
        c
    } else {
        let mut c = Command::new(&path_str);
        c.arg("--version");
        c
    };
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // kill_on_drop replaces the old kill+waitForFinished(300) cleanup:
        // a timed-out child is reaped when the future is dropped.
        .kill_on_drop(true);
    // GUI app: never pop a console window for the version probe.
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    if let Some(dir) = path.parent() {
        let system_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{};{}", native_separators(&dir.to_string_lossy()), system_path));
    }

    let child = cmd.spawn()?;
    match tokio::time::timeout(VERSION_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let mut version = if stdout.is_empty() { stderr } else { stdout };
            if let Some(nl) = version.find('\n') {
                version = version[..nl].trim().to_string();
            }
            let ok = !version.is_empty() || output.status.code() == Some(0);
            Ok(if ok { Some(version) } else { None })
        }
        Ok(Err(e)) => Err(ProbeError::Io(e)),
        Err(_) => {
            // Timeout: kill (via kill_on_drop) but ACCEPT — the binary exists
            // and ran for 4s; it just has no --version or waits on stdin.
            Ok(Some(String::new()))
        }
    }
}

/// .cmd/.bat shims cannot be CreateProcess'd directly — shared with the ACP
/// transport's cmd.exe /c wrapping.
pub(crate) fn is_cmd_shim(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

fn native_separators(path: &str) -> String {
    path.replace('/', "\\")
}

fn found_result(provider_id: &str, display_name: &str, path: &str, version: &str) -> ProbeResult {
    let mut message = format!("已找到 {display_name}");
    if !version.is_empty() {
        message.push(' ');
        message.push_str(version);
    }
    message.push_str(" @ ");
    message.push_str(path);
    ProbeResult {
        provider_id: provider_id.to_string(),
        found: true,
        path: path.to_string(),
        version: version.to_string(),
        error: String::new(),
        message,
    }
}

fn not_found_result(provider_id: &str, display_name: &str) -> ProbeResult {
    ProbeResult {
        provider_id: provider_id.to_string(),
        found: false,
        path: String::new(),
        version: String::new(),
        error: "not_found".to_string(),
        message: format!("未检测到 {display_name}。可点击「浏览…」手动选择可执行文件。"),
    }
}

fn unsupported_result(provider_id: &str) -> ProbeResult {
    ProbeResult {
        provider_id: provider_id.trim().to_lowercase(),
        found: false,
        path: String::new(),
        version: String::new(),
        error: "unsupported".to_string(),
        message: String::new(),
    }
}

/// Accept a plain path or a file: URL (CliProbe.cpp:82-97).
fn local_file_from_maybe_url(input: &str) -> String {
    if !input.starts_with("file:") {
        return input.to_string();
    }
    let rest = input
        .strip_prefix("file:///")
        .or_else(|| input.strip_prefix("file://"))
        .or_else(|| input.strip_prefix("file:"))
        .unwrap_or(input);
    percent_decode(rest)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// testAgent connectivity check (AgentStore.cpp:236-411). Single-flight: a
/// request arriving while one runs is IGNORED (returns None), matching the
/// old `m_testing` early return. Otherwise returns the user-facing Chinese
/// result string.
///
/// Differences from session start, kept verbatim from the old code:
///   - env injection does NOT apply clearEnvs (only ensureAcp does);
///   - program resolution has an extra fallback to "kimi".
#[derive(Default)]
pub struct AgentTester {
    in_flight: AtomicBool,
}

impl AgentTester {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn testing(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// None = a test is already running and this request was ignored.
    pub async fn test_agent(&self, store: &AgentStore, agent_id: &str) -> Option<String> {
        let Some(agent) = store.get(agent_id) else {
            return Some("Agent 不存在".to_string());
        };
        if !provider::chat_capable(&agent.provider) {
            return Some("该 Provider 暂不支持测试".to_string());
        }
        if self.in_flight.swap(true, Ordering::SeqCst) {
            return None; // single flight: ignored
        }
        let _guard = FlightGuard(&self.in_flight);
        Some(self.run(agent).await)
    }

    async fn run(&self, agent: &Agent) -> String {
        let spec = provider::spec(&agent.provider);

        // Env: apiKey (+ bearer special case) and baseUrl only — clearEnvs is
        // intentionally NOT applied here (AgentStore.cpp:263-279).
        let env = match spec {
            Some(s) => provider::env_overrides(s, &agent.api_key, &agent.base_url, false),
            None => Vec::new(),
        };

        // program: cliPath -> defaultCommand -> "kimi" (testAgent-only legacy
        // fallback, AgentStore.cpp:281-285).
        let mut program = provider::resolve_command(spec, &agent.cli_path);
        if program.is_empty() {
            program = "kimi".to_string();
        }
        let mut args = provider::resolve_args(spec, &agent.extra_args);

        // Windows: npm CLIs are .cmd/.bat shims — CreateProcess can't exec
        // those directly, so resolve on PATH and wrap cmd.exe /c (same rule
        // as the ACP transport, AgentStore.cpp:292-305).
        let mut resolved = program.clone();
        if !is_absolute_windows(&resolved) {
            if let Some(found) = find_executable(&program) {
                resolved = found;
            }
        }
        let display = resolved.clone(); // shown in messages, pre-wrap
        if is_cmd_shim(&resolved) {
            let mut wrapped = vec!["/c".to_string(), resolved];
            wrapped.extend(args);
            args = wrapped;
            program = "cmd.exe".to_string();
        } else if !resolved.is_empty() {
            program = resolved;
        }

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // GUI app: never pop a console window for the test-connection run.
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        for (key, value) in env {
            if let Some(v) = value {
                cmd.env(key, v);
            }
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return format!("无法启动 «{display}»: {e}"),
        };

        // stdin is held for the WHOLE conversation: many agents exit on
        // stdin EOF, which would kill the phase-2 model call before it
        // starts.
        let mut stdin = child.stdin.take();
        if let Some(s) = stdin.as_mut() {
            let _ = send_frame(s, &initialize_request()).await;
        }

        let stderr_task = child.stderr.take().map(|mut err| {
            tokio::spawn(async move {
                let mut s = String::new();
                let _ = err.read_to_string(&mut s).await;
                s
            })
        });

        let Some(stdout) = child.stdout.take() else {
            return format!("失败 ({display}): 无法读取子进程输出");
        };
        let mut lines = BufReader::new(stdout).lines();

        // Phase 1: success requires a real ACP `initialize` response, not
        // "it spawned".
        let handshake = async {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Some(verdict) = parse_initialize_response(&line, &display) {
                            return verdict;
                        }
                        // Banner/log noise or messages not addressed to id 1.
                    }
                    Ok(None) => {
                        // stdout closed: the process exited before the
                        // handshake completed.
                        let code = child.wait().await.ok().and_then(|s| s.code());
                        let stderr_text = match stderr_task {
                            Some(t) => t.await.unwrap_or_default(),
                            None => String::new(),
                        };
                        let stderr_text = stderr_text.trim();
                        let code_str = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                        let mut msg =
                            format!("失败 ({display}): 进程在握手前退出 (code {code_str})");
                        if !stderr_text.is_empty() {
                            msg.push_str(" — ");
                            msg.push_str(&truncate_200(stderr_text));
                        }
                        return Err(msg);
                    }
                    Err(e) => {
                        return Err(format!("失败 ({display}): 读取输出失败 — {e}"));
                    }
                }
            }
        };

        let agent_info = match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
            Ok(Ok(info)) => info,
            Ok(Err(msg)) => return msg,
            Err(_) => {
                // kill_on_drop reaps the child when the cancelled future and
                // `child` drop here.
                return format!(
                    "失败 ({display}): ACP initialize 握手超时 — 请检查 cli 路径、参数与登录态"
                );
            }
        };

        // Phase 2: prove the configured model is actually callable.
        model_call(&mut child, &mut stdin, &mut lines, &display, &agent_info).await
    }
}

struct FlightGuard<'a>(&'a AtomicBool);

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// SYNC POINT with the acp module (phase 1c): the request comes from
/// acp::types::initialize_request, the single owner of the pinned shape —
/// protocolVersion 1, clientInfo WarDex/0.2, capabilities fs rw + terminal
/// false. Bump the clientInfo version in acp/types.rs and both call sites
/// move together (docs/providers-and-cli.md §4.3).
fn initialize_request() -> Value {
    crate::acp::types::initialize_request(1)
}

/// One stdout line -> Some(verdict) iff it is our initialize response
/// (JSON object with id == 1); noise and foreign messages return None.
/// Ok(agentInfo) = handshake passed, Err(msg) = user-facing failure.
fn parse_initialize_response(line: &str, display: &str) -> Option<Result<String, String>> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return None; // banner/log noise on stdout
    };
    if !msg.is_object() {
        return None;
    }
    if msg.get("id").and_then(Value::as_i64) != Some(1) {
        return None; // not our initialize response
    }
    if let Some(detail) = error_detail(&msg) {
        return Some(Err(format!("失败 ({display}): initialize 被拒绝 — {detail}")));
    }
    let info = msg
        .get("result")
        .and_then(|r| r.get("agentInfo"))
        .cloned()
        .unwrap_or(Value::Null);
    let name = info.get("name").and_then(Value::as_str).unwrap_or_default();
    let version = info
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut agent_info = name.to_string();
    if !version.is_empty() {
        if !agent_info.is_empty() {
            agent_info.push(' ');
        }
        agent_info.push_str(version);
    }
    Some(Ok(agent_info))
}

/// Write one JSON-RPC frame; false = the child's stdin is gone.
async fn send_frame(stdin: &mut tokio::process::ChildStdin, v: &Value) -> bool {
    let mut payload = serde_json::to_vec(v).unwrap_or_default();
    payload.push(b'\n');
    stdin.write_all(&payload).await.is_ok() && stdin.flush().await.is_ok()
}

/// Some(message) iff the frame carries a JSON-RPC `error` member.
fn error_detail(msg: &Value) -> Option<String> {
    let e = msg.get("error")?;
    let m = e.get("message").and_then(Value::as_str).unwrap_or_default();
    Some(if m.is_empty() {
        "未知错误".to_string()
    } else {
        truncate_200(m)
    })
}

/// testAgent phase 2: prove the configured model is actually callable —
/// session/new, then a one-word prompt. Success = the agent streams any
/// message/thought chunk back or answers the prompt at all. Handshake-only
/// health said nothing about Base URL / API Key / network reachability;
/// those failures surface HERE.
async fn model_call(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    display: &str,
    agent_info: &str,
) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let suffix = if agent_info.is_empty() {
        String::new()
    } else {
        format!(" — {agent_info}")
    };

    let phase = async {
        let new_session = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] },
        });
        let Some(s) = stdin.as_mut() else {
            return format!("失败 ({display}): 无法写入子进程输入");
        };
        if !send_frame(s, &new_session).await {
            return format!("失败 ({display}): 无法写入子进程输入");
        }
        let mut prompted = false;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                        continue; // banner/log noise on stdout
                    };
                    if !msg.is_object() {
                        continue;
                    }
                    match msg.get("id").and_then(Value::as_i64) {
                        Some(2) => {
                            if let Some(detail) = error_detail(&msg) {
                                return format!("失败 ({display}): 创建会话被拒绝 — {detail}");
                            }
                            let sid = msg
                                .get("result")
                                .and_then(|r| r.get("sessionId"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let prompt = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": 3,
                                "method": "session/prompt",
                                "params": {
                                    "sessionId": sid,
                                    "prompt": [{ "type": "text", "text": "ping" }],
                                },
                            });
                            let Some(s) = stdin.as_mut() else {
                                return format!("失败 ({display}): 无法写入子进程输入");
                            };
                            if !send_frame(s, &prompt).await {
                                return format!("失败 ({display}): 无法写入子进程输入");
                            }
                            prompted = true;
                        }
                        Some(3) => {
                            if let Some(detail) = error_detail(&msg) {
                                return format!("失败 ({display}): 模型调用失败 — {detail}");
                            }
                            return format!("成功: ACP 握手 + 模型调用通过{suffix}");
                        }
                        _ => {
                            // Streaming activity after the prompt also proves
                            // the model is alive — no need to wait for the
                            // final prompt response.
                            if prompted
                                && msg.get("method").and_then(Value::as_str)
                                    == Some("session/update")
                            {
                                let kind = msg
                                    .get("params")
                                    .and_then(|p| p.get("update"))
                                    .and_then(|u| u.get("sessionUpdate"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                if matches!(kind, "agent_message_chunk" | "agent_thought_chunk") {
                                    return format!("成功: ACP 握手 + 模型调用通过{suffix}");
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    let code = child.wait().await.ok().and_then(|s| s.code());
                    let code_str = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                    return format!("失败 ({display}): 进程在模型调用完成前退出 (code {code_str})");
                }
                Err(e) => return format!("失败 ({display}): 读取输出失败 — {e}"),
            }
        }
    };

    match tokio::time::timeout(MODEL_CALL_TIMEOUT, phase).await {
        Ok(msg) => msg,
        Err(_) => format!(
            "失败 ({display}): 已连接但模型 {} 秒内无响应 — 请检查 Base URL / API Key / 网络代理",
            MODEL_CALL_TIMEOUT.as_secs()
        ),
    }
}

/// QStandardPaths::findExecutable equivalent: system PATH only, suffixes
/// bare/.exe/.cmd/.bat. Shared by testAgent and the ACP transport.
pub(crate) fn find_executable(name: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    let dirs: Vec<String> = path.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect();
    // Spawn resolution must only return files Windows can actually execute
    // (QStandardPaths::findExecutable semantics): an extensionless npm shim
    // script (e.g. `claude-code-acp` next to `claude-code-acp.cmd`) is NOT
    // executable — CreateProcess fails with os error 193. So unlike
    // which_on_path (existence probing for CliProbe), skip the bare name
    // unless it already carries an extension.
    let has_ext = Path::new(name).extension().is_some();
    let candidates: Vec<String> = if has_ext {
        vec![name.to_string()]
    } else {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ]
    };
    for dir in &dirs {
        for candidate in &candidates {
            let path = Path::new(dir).join(candidate);
            if std::fs::metadata(&path).map(|m| m.is_file()).unwrap_or(false) {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn truncate_200(s: &str) -> String {
    s.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::agents::AgentPatch;
    use crate::store::paths::Paths;
    use std::time::Instant;

    fn fake_cmd(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write fake cli");
        path
    }

    fn spec_of(id: &str) -> &'static ProviderSpec {
        provider::spec(id).expect("spec")
    }

    // ---- candidate queue ----

    #[test]
    fn candidate_queue_full_order_kimi() {
        let home = Path::new(r"C:\Users\u");
        let q = candidate_paths(
            spec_of("kimi"),
            r"D:\tools\kimi",
            Some(home),
            Some(r"C:\Users\u\AppData\Roaming"),
            Some(r"C:\Program Files"),
        );
        // Compare in cleanPath (forward-slash) form: join() keeps the literal
        // "/" inside the appended components, and dedupe_keep_order is what
        // normalizes separators before validation (like the old cleanPath).
        let got: Vec<String> = q
            .iter()
            .map(|p| clean_path_forward(&p.to_string_lossy()))
            .collect();
        assert_eq!(
            got,
            vec![
                "D:/tools/kimi",
                "D:/tools/kimi.exe",
                "C:/Users/u/.kimi-code/bin/kimi.exe",
                "C:/Users/u/.kimi-code/bin/kimi",
                "C:/Users/u/AppData/Local/kimi-code/bin/kimi.exe",
                "C:/Users/u/AppData/Local/Programs/kimi-code/kimi.exe",
                "C:/Users/u/AppData/Roaming/npm/kimi.cmd",
                "C:/Users/u/AppData/Roaming/npm/kimi.exe",
                "C:/Program Files/nodejs/kimi.cmd",
            ]
        );
    }

    #[test]
    fn candidate_queue_skips_bare_command_preferred() {
        for pref in ["kimi", "kimi.exe", "kimi.cmd", "KIMI.EXE"] {
            let q = candidate_paths(spec_of("kimi"), pref, None, None, None);
            assert!(q.is_empty(), "preferred {pref} must be skipped");
        }
        // A preferred path already ending in .exe gets no extra candidate.
        let q = candidate_paths(spec_of("kimi"), r"D:\x\kimi.exe", None, None, None);
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn candidate_queue_npm_paths_for_claude_and_codex() {
        let q = candidate_paths(
            spec_of("claude"),
            "",
            Some(Path::new(r"C:\Users\u")), // must NOT add kimi dirs
            Some(r"C:\Users\u\AppData\Roaming"),
            Some(r"C:\Program Files"),
        );
        let got: Vec<String> = q.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(
            got,
            vec![
                r"C:\Users\u\AppData\Roaming\npm\claude-code-acp.cmd",
                r"C:\Users\u\AppData\Roaming\npm\claude-code-acp.exe",
                r"C:\Program Files\nodejs\claude-code-acp.cmd",
            ]
        );
    }

    #[test]
    fn expanded_search_path_merges_registry_user_path_first() {
        // Old prepend loop: each missing user entry is prepended in turn.
        let dirs = expanded_search_path(
            "codex",
            r"A;B",
            Some(r"B;C;D"),
            Some(Path::new(r"C:\Users\u")),
        );
        assert_eq!(dirs, vec!["D", "C", "A", "B"]);
        // kimi additionally front-inserts ~/.kimi-code/bin (only once).
        let dirs = expanded_search_path(
            "kimi",
            r"A;C:\Users\u\.kimi-code\bin",
            None,
            Some(Path::new(r"C:\Users\u")),
        );
        assert_eq!(dirs, vec!["A", r"C:\Users\u\.kimi-code\bin"]);
    }

    #[test]
    fn which_on_path_suffix_and_dir_order() {
        let tmp = tempfile::tempdir().expect("tmp");
        let d1 = tmp.path().join("d1");
        let d2 = tmp.path().join("d2");
        std::fs::create_dir_all(&d1).expect("d1");
        std::fs::create_dir_all(&d2).expect("d2");
        fake_cmd(&d2, "foo.cmd", "@echo off\n");
        let dirs = vec![
            d1.to_string_lossy().into_owned(),
            d2.to_string_lossy().into_owned(),
        ];
        assert!(which_on_path(&dirs, "foo").expect("found").ends_with("foo.cmd"));
        // .exe beats .cmd within the same dir; earlier dir beats later.
        fake_cmd(&d2, "foo.exe", "x");
        assert!(which_on_path(&dirs, "foo").expect("found").ends_with("foo.exe"));
        fake_cmd(&d1, "foo.bat", "@echo off\n");
        assert!(which_on_path(&dirs, "foo").expect("found").ends_with("foo.bat"));
        assert!(which_on_path(&dirs, "missing").is_none());
    }

    #[test]
    fn dedupe_is_case_insensitive_and_keeps_order() {
        let out = dedupe_keep_order(vec![
            PathBuf::from(r"C:\A\kimi.exe"),
            PathBuf::from("C:/a/kimi.exe"),
            PathBuf::from(r"C:\B\kimi.exe"),
            PathBuf::from(""),
        ]);
        let got: Vec<String> = out.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(got, vec!["C:/A/kimi.exe", "C:/B/kimi.exe"]);
    }

    // ---- --version validation ----

    #[tokio::test]
    async fn version_check_reads_first_line() {
        let tmp = tempfile::tempdir().expect("tmp");
        let fake = fake_cmd(tmp.path(), "fake.cmd", "@echo fakecli 1.2.3\n@echo second line\n");
        let v = check_version(&fake).await.expect("spawn");
        assert_eq!(v, Some("fakecli 1.2.3".to_string()));
    }

    #[tokio::test]
    async fn version_check_timeout_is_accepted_with_empty_version() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Sleeps well past the 4s hard timeout (~7s via ping).
        let fake = fake_cmd(tmp.path(), "sleeper.cmd", "@echo off\nping -n 8 127.0.0.1 >nul\n");
        let start = Instant::now();
        let v = check_version(&fake).await.expect("spawn");
        let elapsed = start.elapsed();
        assert_eq!(v, Some(String::new()), "timeout must be ACCEPTED, empty version");
        assert!(elapsed >= Duration::from_millis(3500), "elapsed {elapsed:?}");
        assert!(elapsed < Duration::from_secs(7), "elapsed {elapsed:?}");
    }

    #[tokio::test]
    async fn version_check_rejects_crash_and_accepts_clean_silent_exit() {
        let tmp = tempfile::tempdir().expect("tmp");
        let crash = fake_cmd(tmp.path(), "crash.cmd", "@exit /b 1\n");
        assert_eq!(check_version(&crash).await.expect("spawn"), None);
        let silent = fake_cmd(tmp.path(), "silent.cmd", "@exit /b 0\n");
        assert_eq!(
            check_version(&silent).await.expect("spawn"),
            Some(String::new()),
            "exit code 0 with no output is still accepted"
        );
    }

    // ---- probe() / cache ----

    #[tokio::test]
    async fn probe_unsupported_for_custom_and_unknown() {
        let mut probe = CliProbe::new();
        let r = probe.probe("custom", "").await;
        assert!(!r.found);
        assert_eq!(r.error, "unsupported");
        let r = probe.probe("  NOPE ", "").await;
        assert_eq!(r.error, "unsupported");
        assert_eq!(r.provider_id, "nope");
    }

    #[tokio::test]
    async fn probe_finds_preferred_fake_and_caches() {
        let tmp = tempfile::tempdir().expect("tmp");
        let fake = fake_cmd(tmp.path(), "kimi-fake.cmd", "@echo kimi 0.29.1\n");
        let mut probe = CliProbe::new();
        let r = probe.probe("kimi", &fake.to_string_lossy()).await;
        assert!(r.found, "{}", r.message);
        assert_eq!(r.version, "kimi 0.29.1");
        assert!(r.error.is_empty());
        assert!(r.message.starts_with("已找到 Kimi CLI kimi 0.29.1 @ "));
        assert!(r.path.contains('\\'), "native separators: {}", r.path);
        // Cached under the provider; invalidation drops it.
        assert!(probe.result(" KIMI ").is_some());
        probe.invalidate("kimi");
        assert!(probe.result("kimi").is_none());
    }

    #[tokio::test]
    async fn probe_not_found_message() {
        let mut probe = CliProbe::new();
        // No preferred path; on a machine without claude-code-acp this is
        // not_found. Skip the assertion when the host actually has it.
        let r = probe.probe("claude", r"Z:\definitely\missing\claude-code-acp").await;
        if !r.found {
            assert_eq!(r.error, "not_found");
            assert_eq!(
                r.message,
                "未检测到 Claude Code。可点击「浏览…」手动选择可执行文件。"
            );
        }
    }

    #[tokio::test]
    async fn probe_path_accepts_file_url() {
        let tmp = tempfile::tempdir().expect("tmp");
        let fake = fake_cmd(tmp.path(), "pick.cmd", "@echo picked 2.0\n");
        let url = format!("file:///{}", fake.to_string_lossy().replace('\\', "/"));
        let mut probe = CliProbe::new();
        let r = probe.probe_path("kimi", &url).await;
        assert!(r.found, "{}", r.message);
        assert_eq!(r.version, "picked 2.0");
    }

    // ---- testAgent ----

    fn store_with_agent(tmp: &tempfile::TempDir, cli_body: &str) -> (AgentStore, String) {
        let paths = Paths::new(tmp.path().to_path_buf());
        let mut store = AgentStore::default();
        let id = store.create_agent(&paths, "t").expect("create");
        let fake = fake_cmd(tmp.path(), &format!("agent-{id}.cmd"), cli_body);
        store
            .update_agent(
                &paths,
                &id,
                &AgentPatch {
                    cli_path: Some(fake.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .expect("patch");
        (store, id)
    }

    #[tokio::test]
    async fn test_agent_success_skips_noise_and_banner_lines() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (store, id) = store_with_agent(
            &tmp,
            concat!(
                "@echo warming up\n",
                "@set /p req1=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"fake-agent\",\"version\":\"9.9\"},\"capabilities\":{}}}\n",
                "@set /p req2=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s1\"}}\n",
                "@set /p req3=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"s1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"pong\"}}}}\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"end_turn\"}}\n",
            ),
        );
        let tester = AgentTester::new();
        let msg = tester.test_agent(&store, &id).await.expect("not busy");
        assert_eq!(msg, "成功: ACP 握手 + 模型调用通过 — fake-agent 9.9");
        assert!(!tester.testing(), "flight guard released");
    }

    #[tokio::test]
    async fn test_agent_model_call_rejected() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (store, id) = store_with_agent(
            &tmp,
            concat!(
                "@set /p req1=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"fake-agent\",\"version\":\"9.9\"},\"capabilities\":{}}}\n",
                "@set /p req2=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"s1\"}}\n",
                "@set /p req3=\n",
                "@echo {\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-32000,\"message\":\"HTTP 401 invalid api key\"}}\n",
            ),
        );
        let tester = AgentTester::new();
        let msg = tester.test_agent(&store, &id).await.expect("not busy");
        assert!(
            msg.contains("模型调用失败 — HTTP 401 invalid api key"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn test_agent_initialize_rejected() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (store, id) = store_with_agent(
            &tmp,
            "@set /p req=\n@echo {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32000,\"message\":\"bad key\"}}\n",
        );
        let tester = AgentTester::new();
        let msg = tester.test_agent(&store, &id).await.expect("not busy");
        assert!(msg.contains("initialize 被拒绝 — bad key"), "{msg}");
    }

    #[tokio::test]
    async fn test_agent_early_exit_reports_code_and_stderr() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (store, id) = store_with_agent(&tmp, "@echo boom 1>&2\n@exit /b 3\n");
        let tester = AgentTester::new();
        let msg = tester.test_agent(&store, &id).await.expect("not busy");
        assert!(msg.contains("进程在握手前退出 (code 3)"), "{msg}");
        assert!(msg.contains("boom"), "{msg}");
    }

    #[tokio::test]
    async fn test_agent_preconditions() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (store, _id) = store_with_agent(&tmp, "@exit /b 0\n");
        let tester = AgentTester::new();
        assert_eq!(
            tester.test_agent(&store, "missing").await,
            Some("Agent 不存在".to_string())
        );
    }
}
