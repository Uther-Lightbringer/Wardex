// Embedded Pi agent driver (provider "pi"): Pi does NOT speak ACP, so this
// driver speaks Pi's RPC protocol directly — JSONL commands on stdin, events
// on stdout (third_party/pi packages/coding-agent/docs/rpc.md). It reuses
// StdioTransport for the subprocess + framing and translates Pi RPC events
// into the same AcpEvent vocabulary the actor consumes, so the chat layer and
// the frontend treat pi exactly like any other agent.
//
// WarDex is the desktop shell; Pi is the agent runtime. MCP is not injected
// (Pi has no MCP — WarDex features become Pi extensions). extension_ui_request
// is bridged to WarDex permission / notify. Session RESUME: pi saves the
// conversation to a per-Wardex-session --session-dir and resumes it by
// --session-id on every respawn (pi's own session persistence).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::acp::events::TurnUsage;
use crate::acp::{AcpError, AcpEvent, SpawnConfig, StdioTransport, Transport};
use crate::chat::driver::{BoxFuture, ClientDriver, PiLaunch};

/// Spawn `node <pluginDir>/packages/coding-agent/dist/rpc-entry.js
/// --no-session` and take over the RPC loop. Mirrors AcpClient::spawn's
/// contract: StartFailed is emitted into the event channel before Err on
/// any pre-session failure.
pub struct PiDriver {
    transport: StdioTransport,
    tx: mpsc::Sender<AcpEvent>,
    /// contentIndex -> toolCallId (raw toolcall_* events carry no id except
    /// at start/end).
    idx_to_id: HashMap<usize, String>,
    /// toolCallId -> accumulated raw arguments JSON string (delta shape).
    args_buf: HashMap<String, String>,
    /// toolCallId -> tool name (deltas don't repeat it).
    names: HashMap<String, String>,
    /// Stop reason of the last turn (agent_settled closes with it).
    last_stop: String,
    /// Cumulative session token usage (pi only reports totals via
    /// `get_session_stats`). Persisted per driver instance; each turn's delta
    /// is emitted as that turn's usage.
    stats_cum: Option<TurnUsage>,
    /// Stop reason of a turn waiting on a `get_session_stats` response.
    /// Shared with a fallback timer so a stalled stats reply can't hang the
    /// turn (the timer and the response handler race to resolve it; only one
    /// emits TurnFinished).
    pending: Arc<tokio::sync::Mutex<Option<String>>>,
    /// Allowed thinking levels (models.rs EFFORT_LEVELS; empty = every level).
    /// Drives the thinking picker advertised to the chat page.
    effort_options: Vec<String>,
    /// Current thinking level ("low"/"medium"/…). Sent to pi via
    /// `set_thinking_level`; echoed to the chat page as a ConfigOptions picker.
    current_level: String,
    /// Models from `get_available_models` (provider + id + display name).
    models: Vec<PiModel>,
    /// Current model as `provider/id` (ACP configOptions `id:"model"` value).
    current_model_key: String,
    /// Thinking levels reported by Pi for the current model (may include "off").
    thinking_levels: Vec<String>,
    /// Pi `extension_ui_request` id (uuid string) keyed by the i64 request id
    /// we emit as AcpEvent::PermissionRequested.
    ui_pending: HashMap<i64, PendingUi>,
    ui_seq: i64,
}

#[derive(Debug, Clone)]
struct PiModel {
    provider: String,
    id: String,
    name: String,
}

impl PiModel {
    fn key(&self) -> String {
        if self.provider.is_empty() {
            self.id.clone()
        } else {
            format!("{}/{}", self.provider, self.id)
        }
    }
    fn display(&self) -> String {
        if self.name.is_empty() || self.name == self.id {
            self.key()
        } else if self.provider.is_empty() {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, self.provider)
        }
    }
}

/// In-flight Pi extension UI request waiting on the WarDex permission dialog.
struct PendingUi {
    pi_id: String,
    method: String,
    prefill: String,
}

/// Shape the existing permission dialog understands (toolCall.title + options).
fn pi_ui_permission_params(method: &str, obj: &Map<String, Value>) -> Value {
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(match method {
            "select" => "Pi 请选择",
            "input" | "editor" => "Pi 请输入",
            _ => "Pi 请求确认",
        });
    let message = obj.get("message").and_then(Value::as_str).unwrap_or("");
    let placeholder = obj.get("placeholder").and_then(Value::as_str).unwrap_or("");
    let prefill = obj.get("prefill").and_then(Value::as_str).unwrap_or("");
    let mut body = message.to_string();
    if body.is_empty() {
        body = placeholder.to_string();
    }
    if !prefill.is_empty() {
        if body.is_empty() {
            body = prefill.to_string();
        } else {
            body = format!("{body}\n\n{prefill}");
        }
    }
    let options = if method == "select" {
        obj.get("options")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| o.as_str())
                    .map(|s| json!({ "optionId": s, "name": s, "kind": "allow_once" }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        vec![
            json!({ "optionId": "allow", "name": "Allow", "kind": "allow_once" }),
            json!({ "optionId": "reject", "name": "Reject", "kind": "reject_once" }),
        ]
    };
    json!({
        "toolCall": {
            "title": title,
            "kind": method,
            "content": [{ "content": { "text": body } }],
        },
        "options": options,
    })
}

fn pi_ui_response(pending: &PendingUi, option_id: &str, cancelled: bool) -> Value {
    let reject = cancelled || option_id.is_empty() || option_id == "reject";
    match pending.method.as_str() {
        "confirm" => {
            if cancelled {
                json!({ "type": "extension_ui_response", "id": pending.pi_id, "cancelled": true })
            } else {
                json!({
                    "type": "extension_ui_response",
                    "id": pending.pi_id,
                    "confirmed": !reject,
                })
            }
        }
        "select" => {
            if reject {
                json!({ "type": "extension_ui_response", "id": pending.pi_id, "cancelled": true })
            } else {
                json!({
                    "type": "extension_ui_response",
                    "id": pending.pi_id,
                    "value": option_id,
                })
            }
        }
        "input" | "editor" => {
            if reject {
                json!({ "type": "extension_ui_response", "id": pending.pi_id, "cancelled": true })
            } else {
                let value = if pending.prefill.is_empty() {
                    option_id.to_string()
                } else {
                    pending.prefill.clone()
                };
                json!({
                    "type": "extension_ui_response",
                    "id": pending.pi_id,
                    "value": value,
                })
            }
        }
        _ => json!({ "type": "extension_ui_response", "id": pending.pi_id, "cancelled": true }),
    }
}

/// Compiled pi binary filename (bundle-pi.mjs output).
pub fn pi_binary_name() -> &'static str {
    if cfg!(windows) {
        "pi.exe"
    } else {
        "pi"
    }
}

/// pi's assertValidSessionId (session-manager.ts): alphanumeric + '.', '_',
/// '-', starting and ending alphanumeric. Wardex session uuids (hex+dashes)
/// always pass; the guard makes a weird id degrade to --no-session (current
/// ephemeral behavior) instead of a pi startup error.
fn is_valid_pi_session_id(id: &str) -> bool {
    let b = id.as_bytes();
    if b.is_empty()
        || !b[0].is_ascii_alphanumeric()
        || !b[b.len() - 1].is_ascii_alphanumeric()
    {
        return false;
    }
    b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'_' || *c == b'-')
}

/// Locate the pi plugin dist directory (the one containing the compiled
/// binary `pi.exe`/`pi`). Priority: configured agent pi_dir → WARDEX_PI_DIR →
/// bundled resources/pi → sibling `pi/` next to the repo root (e.g.
/// C:\workspace\pi) → old in-repo `third_party/pi` layout. Errors carry a
/// user-facing Chinese message.
pub fn locate_plugin_dir(pi_dir: &str) -> Result<PathBuf, String> {
    let bin = pi_binary_name();
    let cli = pi_dir.trim();
    // candidate dir -> its dist dir (contains the binary).
    let entry = |dir: &Path| dir.join("packages/coding-agent/dist").join(bin);
    if !cli.is_empty() {
        let dir = PathBuf::from(cli);
        if entry(&dir).is_file() {
            return Ok(dir.join("packages/coding-agent/dist"));
        }
        return Err(format!(
            "Pi 插件路径无效（找不到 packages/coding-agent/dist/{bin}）：{cli}"
        ));
    }
    if let Ok(dir) = std::env::var("WARDEX_PI_DIR") {
        if !dir.trim().is_empty() && entry(Path::new(dir.trim())).is_file() {
            return Ok(PathBuf::from(dir.trim()).join("packages/coding-agent/dist"));
        }
    }
    // Dev checkout search: walk up from the executable. A sibling `pi`
    // directory of the repo root (C:\workspace\pi) is the primary layout;
    // the old in-repo third_party/pi is kept as a fallback. Bundled installs
    // ship the plugin under `<exe_dir>/resources/pi` (tauri bundle.resources).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled = exe_dir.join("resources/pi");
            if entry(&bundled).is_file() {
                return Ok(bundled.join("packages/coding-agent/dist"));
            }
        }
        let mut dir = exe.parent();
        let mut hops = 0;
        while let Some(d) = dir {
            if hops > 6 {
                break;
            }
            if d.join("src-tauri").is_dir() {
                // d is the repo root — look for the sibling pi checkout.
                if let Some(sibling) = d.parent().map(|p| p.join("pi")) {
                    if entry(&sibling).is_file() {
                        return Ok(sibling.join("packages/coding-agent/dist"));
                    }
                }
            }
            let candidate = d.join("third_party/pi");
            if entry(&candidate).is_file() {
                return Ok(candidate.join("packages/coding-agent/dist"));
            }
            dir = d.parent();
            hops += 1;
        }
    }
    Err(
        "找不到 Pi 插件二进制。请在配置页把该 Agent 的「Pi 插件目录」指向含 packages/coding-agent/dist/pi（或 pi.exe）的目录（可先用 bundle-pi.mjs 生成），或设置环境变量 WARDEX_PI_DIR。"
            .to_string(),
    )
}

/// Directory of WarDex-shipped Pi extensions (`wardex-reminders.ts` etc.).
/// Priority: WARDEX_PI_EXTENSIONS_DIR → bundled `resources/pi-extensions`
/// → repo-root `pi-extensions/` next to `src-tauri`.
pub fn locate_extensions_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("WARDEX_PI_EXTENSIONS_DIR") {
        let p = PathBuf::from(dir.trim());
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled = exe_dir.join("resources").join("pi-extensions");
            if bundled.is_dir() {
                return Some(bundled);
            }
        }
        let mut dir = exe.parent();
        let mut hops = 0;
        while let Some(d) = dir {
            if hops > 6 {
                break;
            }
            if d.join("src-tauri").is_dir() {
                let candidate = d.join("pi-extensions");
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
            hops += 1;
        }
    }
    None
}

/// Legacy fixed extension list — superseded by crate::plugins::extension_files
/// (kept for tests/tools that want the builtins only).
#[allow(dead_code)]
pub fn wardex_extension_files(use_codegraph: bool) -> Vec<PathBuf> {
    let Some(dir) = locate_extensions_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let reminders = dir.join("wardex-reminders.ts");
    if reminders.is_file() {
        out.push(reminders);
    }
    if use_codegraph {
        let cg = dir.join("wardex-codegraph.ts");
        if cg.is_file() {
            out.push(cg);
        }
    }
    out
}

/// Probe result for the frontend (probe_pi command). Binary availability only —
/// pi 的二进制自带运行时，不需要 Node。
pub fn probe_result(found: bool, plugin: Result<PathBuf, String>) -> Value {
    let (plugin_dir, plugin_ready) = match &plugin {
        Ok(p) => (p.to_string_lossy().into_owned(), true),
        Err(_) => (String::new(), false),
    };
    json!({
        "found": found,
        "pluginDir": plugin_dir,
        "pluginReady": plugin_ready,
        "message": if !plugin_ready {
            plugin.as_ref().err().cloned().unwrap_or_else(|| {
                "找不到 Pi 二进制。请先用 bundle-pi.mjs 生成，或在配置页设置「Pi 插件目录」。".to_string()
            })
        } else {
            String::new()
        },
    })
}

impl PiDriver {
    /// Spawn the compiled pi binary (`<binary> --mode rpc --no-session`),
    /// verify the process is up, then emit Started (the actor fires pending
    /// prompts on it). Failures emit StartFailed and return Err.
    pub async fn spawn(launch: PiLaunch, tx: mpsc::Sender<AcpEvent>) -> Result<Self, AcpError> {
        let fail = |msg: String| {
            // try_send, NOT blocking_send: spawn() runs on the actor's tokio
            // worker thread, where blocking_send panics ("Cannot block the
            // current thread from within a runtime") and kills the actor,
            // leaving a dead registry entry ("会话已关闭" on every send).
            let _ = tx.try_send(AcpEvent::StartFailed { error: msg.clone() });
            Err(AcpError::Spawn(msg))
        };
        // Binary presence: the user-facing precondition. Missing binary means
        // the pi plugin isn't built/bundled (bundle-pi.mjs produces it).
        let bin = pi_binary_name();
        if !launch.binary.is_file() {
            return fail(format!(
                "Pi 启动失败：缺少 pi 二进制 {}。请先用 bundle-pi.mjs 生成，或在配置页把「Pi 插件目录」指向含 packages/coding-agent/dist/{bin} 的目录。",
                launch.binary.display()
            ));
        }
        let mut args = vec!["--mode".to_string(), "rpc".to_string()];
        // Session persistence (context recovery across respawns): pi saves
        // the conversation tree to --session-dir and resumes it by
        // --session-id (create-if-missing / resume-if-exists, main.ts
        // createSessionManager). The dir is isolated per Wardex session; a
        // missing dir or invalid id falls back to --no-session (ephemeral).
        let persist = !launch.session_dir.is_empty() && is_valid_pi_session_id(&launch.session_id);
        if persist {
            args.push("--session-dir".to_string());
            args.push(launch.session_dir.clone());
            args.push("--session-id".to_string());
            args.push(launch.session_id.clone());
        } else {
            args.push("--no-session".to_string());
        }
        // Route to the Wardex-rendered custom provider / model when set.
        if !launch.provider_key.is_empty() {
            args.push("--provider".to_string());
            args.push(launch.provider_key.clone());
        }
        if !launch.model.is_empty() {
            args.push("--model".to_string());
            args.push(launch.model.clone());
        }
        for ext in &launch.extensions {
            if !ext.trim().is_empty() {
                args.push("--extension".to_string());
                args.push(ext.clone());
            }
        }
        let config = SpawnConfig {
            cli_path: launch.binary.to_string_lossy().into_owned(),
            args,
            env: launch.env,
            cwd: launch.cwd,
        };
        let transport = match StdioTransport::spawn(&config).await {
            Ok(t) => t,
            Err(e) => {
                return fail(format!("Pi 启动失败: {e}"));
            }
        };
        // No ACP handshake: the subprocess is ready as soon as it spawned.
        let _ = tx.send(AcpEvent::Started { session_id: String::new() }).await;
        log::info!(
            "pi spawn extensions: {}",
            if launch.extensions.is_empty() {
                "(none)".to_string()
            } else {
                launch.extensions.join(", ")
            }
        );
        let mut driver = Self {
            transport,
            tx,
            idx_to_id: HashMap::new(),
            args_buf: HashMap::new(),
            names: HashMap::new(),
            last_stop: String::new(),
            stats_cum: None,
            pending: Arc::new(tokio::sync::Mutex::new(None)),
            effort_options: launch.effort_options,
            current_level: String::new(),
            models: Vec::new(),
            current_model_key: launch.model.trim().to_string(),
            thinking_levels: Vec::new(),
            ui_pending: HashMap::new(),
            ui_seq: 1,
        };
        // Thinking ON by default: send the agent's default level and advertise
        // the picker so the chat page offers the strength dropdown for pi.
        driver.init_thinking(&launch.default_effort).await;
        // Pull Pi's live model list, current state, thinking levels, and
        // slash commands. Responses land in on_response and refresh the
        // configOptions / commands the chat page already knows how to render.
        for cmd in [
            "{\"type\":\"get_available_models\"}",
            "{\"type\":\"get_available_thinking_levels\"}",
            "{\"type\":\"get_state\"}",
            "{\"type\":\"get_commands\"}",
        ] {
            let _ = driver.transport.send_line(cmd).await;
        }
        // Usage baseline: pi's get_session_stats reports the FULL session
        // total (cumulative, and once sessions persist it survives respawns).
        // Seed stats_cum with the current total so the first turn after a
        // resume charges only its own delta, not the whole history. The
        // response lands in on_stats_response with no pending turn → it only
        // seeds the baseline, never emits TurnFinished.
        let _ = driver
            .transport
            .send_line("{\"type\":\"get_session_stats\"}")
            .await;
        Ok(driver)
    }

    async fn emit(&self, ev: AcpEvent) {
        if self.tx.send(ev).await.is_err() {
            log::warn!("pi: event channel closed");
        }
    }

    /// Advertise model + thinking pickers as ACP-shaped configOptions so the
    /// chat page dropdowns work without a Pi-specific UI path. Always emit
    /// both together — a thinking-only batch would wipe the model picker.
    async fn emit_config_options(&self) {
        let mut options: Vec<Value> = Vec::new();
        if !self.models.is_empty() {
            let choices: Vec<Value> = self
                .models
                .iter()
                .map(|m| json!({ "value": m.key(), "name": m.display() }))
                .collect();
            options.push(json!({
                "type": "select",
                "id": "model",
                "name": "Model",
                "currentValue": self.current_model_key,
                "options": choices,
            }));
        }
        let levels = self.effective_thinking_levels();
        if !levels.is_empty() {
            let choices: Vec<Value> = levels
                .iter()
                .map(|l| json!({ "value": l, "name": l }))
                .collect();
            options.push(json!({
                "type": "select",
                "id": "thinking",
                "name": "Thinking",
                "currentValue": self.current_level,
                "options": choices,
            }));
        }
        if !options.is_empty() {
            self.emit(AcpEvent::ConfigOptions { options }).await;
        }
    }

    /// RPC-reported levels, intersected with the agent's allow-list when set.
    /// Drops `off` when any other level exists (WarDex keeps thinking on).
    fn effective_thinking_levels(&self) -> Vec<String> {
        effective_thinking_levels(&self.thinking_levels, &self.effort_options)
    }

    /// Set the initial thinking level (thinking ON by default) and advertise
    /// the picker. Best-effort: if pi can't apply it (e.g. unsupported model)
    /// the picker still shows with our intended default.
    async fn init_thinking(&mut self, default_effort: &str) {
        let allowed = crate::models::effective_efforts(&self.effort_options);
        let default = crate::models::pick_default_effort(&allowed, default_effort);
        self.current_level = default.to_string();
        let line =
            serde_json::to_string(&json!({ "type": "set_thinking_level", "level": default }))
                .unwrap_or_default();
        let _ = self.transport.send_line(&line).await;
        self.emit_config_options().await;
    }

    async fn handle_line(&mut self, v: Value) {
        let Some(obj) = v.as_object() else { return };
        let Some(ty) = obj.get("type").and_then(Value::as_str) else {
            return;
        };
        match ty {
            "response" => self.on_response(obj).await,
            "message_update" => self.on_message_update(obj).await,
            "tool_execution_start" => self.on_tool_exec(obj, "in_progress", None).await,
            "tool_execution_update" => {
                self.on_tool_exec(obj, "in_progress", obj.get("partialResult"))
                    .await
            }
            "tool_execution_end" => {
                let failed = obj
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.on_tool_exec(
                    obj,
                    if failed { "failed" } else { "completed" },
                    obj.get("result"),
                )
                .await;
            }
            "turn_end" => {
                self.last_stop = obj
                    .get("message")
                    .and_then(|m| m.get("stopReason"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            "agent_settled" => {
                let stop = match self.last_stop.as_str() {
                    "error" => "error",
                    "aborted" | "cancelled" => "cancelled",
                    _ => "end_turn",
                };
                self.last_stop.clear();
                // Turn is fully settled — query cumulative session stats so we
                // can attribute this turn's token usage (pi reports totals via
                // get_session_stats; the delta vs the previous cumulative is
                // this turn's usage). Resolved by the stats response handler,
                // with a fallback timer so a stalled reply can't hang the turn.
                self.finish_turn_pending_stats(stop.to_string()).await;
            }
            "extension_ui_request" => self.on_extension_ui_request(obj).await,
            "extension_error" => {
                let err = obj
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                log::warn!("pi extension error: {err}");
            }
            // agent_start/agent_end, message_start/end, queue_update,
            // compaction_*, auto_retry_*, bash_execution_update,
            // summarization_*: no Wardex equivalent needed.
            _ => {}
        }
    }

    /// RPC command responses. get_session_stats resolves a pending turn's
    /// usage (delta vs. the previous cumulative); only a failed prompt ack
    /// otherwise surfaces (the turn that never starts must not leave the
    /// actor busy forever).
    async fn on_response(&mut self, obj: &Map<String, Value>) {
        match obj.get("command").and_then(Value::as_str) {
            Some("get_session_stats") => self.on_stats_response(obj),
            Some("prompt") => {
                if obj.get("success").and_then(Value::as_bool) != Some(true) {
                    let err = obj
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("prompt 被拒绝")
                        .to_string();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(AcpEvent::ProtocolError { error: err.clone() }).await;
                        let _ = tx.send(AcpEvent::TurnFinished { stop_reason: "error".into(), usage: None }).await;
                    });
                }
            }
            Some("get_available_models") => self.on_models_response(obj).await,
            Some("get_available_thinking_levels") => self.on_thinking_levels_response(obj).await,
            Some("get_state") => self.on_state_response(obj).await,
            Some("set_model") => self.on_set_model_response(obj).await,
            Some("get_commands") => self.on_commands_response(obj).await,
            _ => {}
        }
    }

    async fn on_models_response(&mut self, obj: &Map<String, Value>) {
        if obj.get("success").and_then(Value::as_bool) != Some(true) {
            return;
        }
        self.models = parse_pi_models(obj.get("data"));
        if self.current_model_key.is_empty() {
            if let Some(first) = self.models.first() {
                self.current_model_key = first.key();
            }
        }
        self.emit_config_options().await;
    }

    async fn on_thinking_levels_response(&mut self, obj: &Map<String, Value>) {
        if obj.get("success").and_then(Value::as_bool) != Some(true) {
            return;
        }
        self.thinking_levels = parse_thinking_levels(obj.get("data"));
        let allowed = self.effective_thinking_levels();
        if !allowed.is_empty() && !allowed.iter().any(|l| l == &self.current_level) {
            let next = allowed[0].clone();
            self.current_level = next.clone();
            let line = serde_json::to_string(&json!({
                "type": "set_thinking_level",
                "level": next,
            }))
            .unwrap_or_default();
            let _ = self.transport.send_line(&line).await;
        }
        self.emit_config_options().await;
    }

    async fn on_state_response(&mut self, obj: &Map<String, Value>) {
        if obj.get("success").and_then(Value::as_bool) != Some(true) {
            return;
        }
        let Some(data) = obj.get("data") else { return };
        if let Some(key) = model_key_from_value(data.get("model")) {
            self.current_model_key = key;
        }
        if let Some(level) = data.get("thinkingLevel").and_then(Value::as_str) {
            if !level.is_empty() {
                self.current_level = level.to_string();
            }
        }
        self.emit_config_options().await;
    }

    async fn on_set_model_response(&mut self, obj: &Map<String, Value>) {
        if obj.get("success").and_then(Value::as_bool) != Some(true) {
            log::warn!(
                "pi set_model failed: {}",
                obj.get("error").and_then(Value::as_str).unwrap_or("unknown")
            );
            return;
        }
        if let Some(key) = model_key_from_value(obj.get("data")) {
            self.current_model_key = key;
        }
        let _ = self
            .transport
            .send_line("{\"type\":\"get_available_thinking_levels\"}")
            .await;
        self.emit_config_options().await;
    }

    async fn on_commands_response(&mut self, obj: &Map<String, Value>) {
        if obj.get("success").and_then(Value::as_bool) != Some(true) {
            return;
        }
        let commands = parse_pi_commands(obj.get("data"));
        if !commands.is_empty() {
            self.emit(AcpEvent::AvailableCommands { commands }).await;
        }
    }

    /// Query `get_session_stats` after a turn settles, arm a fallback timer
    /// (5s), and record the stop reason as "pending". The stats response
    /// handler (or the timer) resolves the turn exactly once.
    async fn finish_turn_pending_stats(&mut self, stop: String) {
        let pending = self.pending.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Some(st) = pending.lock().await.take() {
                let _ = tx
                    .send(AcpEvent::TurnFinished { stop_reason: st, usage: None })
                    .await;
            }
        });
        let sent = self
            .transport
            .send_line("{\"type\":\"get_session_stats\"}")
            .await;
        if sent.is_err() {
            // Request failed outright — close the turn with no usage (the
            // fallback timer would only find an empty pending slot).
            self.pending.lock().await.take();
            self.emit(AcpEvent::TurnFinished { stop_reason: stop, usage: None })
                .await;
        } else {
            *self.pending.lock().await = Some(stop);
        }
    }

    /// Resolve the pending turn from a `get_session_stats` response: compute
    /// this turn's usage as the cumulative delta vs. the previous snapshot.
    fn on_stats_response(&mut self, obj: &Map<String, Value>) {
        let mut usage = None;
        if obj.get("success").and_then(Value::as_bool) == Some(true) {
            if let Some(cum) = obj
                .get("data")
                .and_then(|d| d.get("tokens"))
                .and_then(TurnUsage::from_pi)
            {
                usage = Some(self.turn_delta(cum));
            }
        }
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        tokio::spawn(async move {
            if let Some(st) = pending.lock().await.take() {
                let _ = tx
                    .send(AcpEvent::TurnFinished { stop_reason: st, usage })
                    .await;
            }
        });
    }

    /// Cumulative snapshot -> this turn's usage = new - previous cumulative.
    fn turn_delta(&mut self, cum: TurnUsage) -> TurnUsage {
        let prev = self.stats_cum.replace(cum.clone());
        match prev {
            Some(p) => TurnUsage {
                input_tokens: cum.input_tokens.saturating_sub(p.input_tokens),
                output_tokens: cum.output_tokens.saturating_sub(p.output_tokens),
                total_tokens: cum.total_tokens.saturating_sub(p.total_tokens),
                cached_read_tokens: opt_sub(cum.cached_read_tokens, p.cached_read_tokens),
                cached_write_tokens: opt_sub(cum.cached_write_tokens, p.cached_write_tokens),
                thought_tokens: None,
            },
            None => cum,
        }
    }

    /// message_update -> assistantMessageEvent delta translation. Text and
    /// thinking deltas become chunks; toolcall_* become tool segments. The
    /// raw provider shape ({id, toolName, delta, toolCall}) and the
    /// normalized shape ({partial: AssistantMessage}) are both handled.
    async fn on_message_update(&mut self, obj: &Map<String, Value>) {
        let Some(ev) = obj.get("assistantMessageEvent") else { return };
        let Some(kind) = ev.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "text_delta" => {
                if let Some(t) = ev.get("delta").and_then(Value::as_str) {
                    self.emit(AcpEvent::MessageChunk { text: t.to_string() }).await;
                }
            }
            "thinking_delta" => {
                if let Some(t) = ev.get("delta").and_then(Value::as_str) {
                    self.emit(AcpEvent::ThoughtChunk { text: t.to_string() }).await;
                }
            }
            "toolcall_start" => self.on_toolcall_start(ev).await,
            "toolcall_delta" => self.on_toolcall_delta(ev).await,
            "toolcall_end" => self.on_toolcall_end(ev).await,
            _ => {}
        }
    }

    fn toolcall_id_and_name(ev: &Map<String, Value>, idx: usize) -> (String, String, Option<Value>) {
        if let Some(id) = ev.get("id").and_then(Value::as_str) {
            // Raw shape: {contentIndex, id, toolName}.
            return (
                id.to_string(),
                ev.get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                None,
            );
        }
        // Normalized shape: {contentIndex, partial} — the toolCall block
        // inside the partial assistant message.
        if let Some(partial) = ev.get("partial") {
            if let Some(blocks) = partial.get("content").and_then(Value::as_array) {
                let mut block = blocks.get(idx).or_else(|| blocks.last());
                if block.is_some_and(|b| b.get("type").and_then(Value::as_str) != Some("toolCall")) {
                    block = blocks
                        .iter()
                        .find(|b| b.get("type").and_then(Value::as_str) == Some("toolCall"));
                }
                if let Some(b) = block {
                    let id = b
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = b
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    return (id, name, b.get("arguments").cloned());
                }
            }
        }
        (String::new(), String::new(), None)
    }

    async fn emit_tool(&self, id: &str, name: &str, extra: Map<String, Value>) {
        if id.is_empty() {
            return;
        }
        let mut call = json!({
            "toolCallId": id,
            "name": name,
            "status": "in_progress",
        })
        .as_object()
        .expect("object")
        .clone();
        call.extend(extra);
        self.emit(AcpEvent::ToolCallUpdate { update: Value::Object(call) })
            .await;
    }

    async fn on_toolcall_start(&mut self, ev: &Value) {
        let Some(ev) = ev.as_object() else { return };
        let idx = ev.get("contentIndex").and_then(Value::as_u64).unwrap_or(0) as usize;
        let (id, name, args) = Self::toolcall_id_and_name(ev, idx);
        if id.is_empty() {
            return;
        }
        self.idx_to_id.insert(idx, id.clone());
        self.names.insert(id.clone(), name.clone());
        self.args_buf.insert(id.clone(), String::new());
        let mut extra = Map::new();
        if let Some(a) = args {
            if a.is_object() {
                extra.insert("rawInput".to_string(), a);
            }
        }
        self.emit_tool(&id, &name, extra).await;
    }

    async fn on_toolcall_delta(&mut self, ev: &Value) {
        let Some(ev) = ev.as_object() else { return };
        let idx = ev.get("contentIndex").and_then(Value::as_u64).unwrap_or(0) as usize;
        // Normalized shape carries the whole partial message — emit the
        // live arguments object directly.
        if let Some(partial) = ev.get("partial") {
            if let Some(blocks) = partial.get("content").and_then(Value::as_array) {
                let block = blocks.get(idx).or_else(|| blocks.last());
                if let Some(b) = block {
                    let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
                    let name = self.names.get(id).cloned().unwrap_or_default();
                    let mut extra = Map::new();
                    if let Some(a) = b.get("arguments") {
                        if a.is_object() && !a.as_object().is_some_and(|o| o.is_empty()) {
                            extra.insert("rawInput".to_string(), a.clone());
                        }
                    }
                    self.emit_tool(id, &name, extra).await;
                }
            }
            return;
        }
        // Raw shape: {contentIndex, delta} — accumulate the arguments JSON.
        let Some(id) = self.idx_to_id.get(&idx).cloned() else { return };
        if let Some(d) = ev.get("delta").and_then(Value::as_str) {
            let buf = self.args_buf.entry(id.clone()).or_default();
            buf.push_str(d);
            let name = self.names.get(&id).cloned().unwrap_or_default();
            let mut extra = Map::new();
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(buf) {
                if !o.is_empty() {
                    extra.insert("rawInput".to_string(), Value::Object(o));
                }
            }
            self.emit_tool(&id, &name, extra).await;
        }
    }

    async fn on_toolcall_end(&mut self, ev: &Value) {
        let Some(ev) = ev.as_object() else { return };
        let idx = ev.get("contentIndex").and_then(Value::as_u64).unwrap_or(0) as usize;
        let (id, name, args) = Self::toolcall_id_and_name(ev, idx);
        if id.is_empty() {
            return;
        }
        let name = if name.is_empty() {
            self.names.get(&id).cloned().unwrap_or_default()
        } else {
            self.names.insert(id.clone(), name.clone());
            name
        };
        let mut extra = Map::new();
        if let Some(a) = args {
            if a.is_object() {
                extra.insert("rawInput".to_string(), a);
            }
        }
        self.emit_tool(&id, &name, extra).await;
    }

    /// tool_execution_*: status + streamed output. `payload` is partialResult
    /// (accumulated) or result (final) — text from its content blocks.
    async fn on_tool_exec(&mut self, obj: &Map<String, Value>, status: &str, payload: Option<&Value>) {
        let id = obj
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return;
        }
        let name = obj
            .get("toolName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.names.insert(id.clone(), name.clone());
        let mut extra = Map::new();
        let mut text = String::new();
        if let Some(p) = payload {
            if let Some(blocks) = p.get("content").and_then(Value::as_array) {
                for b in blocks {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
            } else if let Some(t) = p.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
        }
        if !text.is_empty() {
            extra.insert("rawOutput".to_string(), Value::String(text));
        }
        extra.insert("status".to_string(), Value::String(status.to_string()));
        self.emit_tool(&id, &name, extra).await;
    }

    /// Bridge Pi extension UI to WarDex: confirm/select/input/editor become
    /// the existing permission dialog; notify is a desktop toast. TUI-only
    /// methods (setStatus/setWidget/…) are ignored.
    async fn on_extension_ui_request(&mut self, obj: &Map<String, Value>) {
        let method = obj.get("method").and_then(Value::as_str).unwrap_or_default();
        if method == "notify" {
            let body = obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let kind = obj
                .get("notifyType")
                .and_then(Value::as_str)
                .unwrap_or("info");
            let title = match kind {
                "warning" => "Pi 警告",
                "error" => "Pi 错误",
                _ => "Pi",
            };
            self.emit(AcpEvent::Notify {
                title: title.to_string(),
                body,
            })
            .await;
            return;
        }
        if !matches!(method, "select" | "confirm" | "input" | "editor") {
            return;
        }
        let Some(pi_id) = obj.get("id").and_then(Value::as_str) else {
            return;
        };
        let request_id = self.ui_seq;
        self.ui_seq = self.ui_seq.saturating_add(1);
        let prefill = obj
            .get("prefill")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.ui_pending.insert(
            request_id,
            PendingUi {
                pi_id: pi_id.to_string(),
                method: method.to_string(),
                prefill,
            },
        );
        self.emit(AcpEvent::PermissionRequested {
            request_id,
            params: pi_ui_permission_params(method, obj),
        })
        .await;
    }

    fn mime_for(path: &str) -> &'static str {
        let ext = path.rsplit('.').next().unwrap_or_default().to_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        }
    }

    /// Test-only: send a raw RPC command line (e.g. get_state for smoke
    /// tests without quota cost). Not part of ClientDriver.
    pub async fn send_command_for_test(&self, line: &str) -> Result<(), AcpError> {
        self.transport.send_line(line).await
    }
}

impl ClientDriver for PiDriver {
    fn recv_once(&mut self) -> BoxFuture<'_, Result<bool, AcpError>> {
        Box::pin(async move {
            let Some(line) = self.transport.recv_line().await? else {
                let code = self.transport.exit_code().await.unwrap_or(-1);
                self.emit(AcpEvent::ProcessExited { code }).await;
                return Ok(false);
            };
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => self.handle_line(v).await,
                Err(e) => log::warn!("pi: unparsable line: {e}"),
            }
            Ok(true)
        })
    }

    fn prompt<'a>(
        &'a mut self,
        text: &'a str,
        image_paths: &'a [String],
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async move {
            let mut images = Vec::new();
            for p in image_paths {
                let data = tokio::fs::read(p).await?;
                images.push(json!({
                    "type": "image",
                    "data": base64::engine::general_purpose::STANDARD.encode(data),
                    "mimeType": Self::mime_for(p),
                }));
            }
            let mut cmd = json!({ "type": "prompt", "message": text });
            if !images.is_empty() {
                cmd["images"] = json!(images);
            }
            // Re-assert the thinking level before each turn: pi only creates
            // its session lazily, so the spawn-time set_thinking_level may have
            // preceded it. Idempotent + clamped by pi.
            if !self.current_level.is_empty() {
                let think = serde_json::to_string(&json!({
                    "type": "set_thinking_level",
                    "level": self.current_level,
                }))?;
                let _ = self.transport.send_line(&think).await;
            }
            let line = serde_json::to_string(&cmd)?;
            self.transport.send_line(&line).await
        })
    }

    fn cancel_turn(&mut self) -> BoxFuture<'_, Result<(), AcpError>> {
        Box::pin(async move { self.transport.send_line("{\"type\":\"abort\"}").await })
    }

    fn answer_permission<'a>(
        &'a mut self,
        request_id: i64,
        option_id: &'a str,
        cancelled: bool,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async move {
            let Some(pending) = self.ui_pending.remove(&request_id) else {
                return Ok(());
            };
            let resp = pi_ui_response(&pending, option_id, cancelled);
            let line = serde_json::to_string(&resp).unwrap_or_default();
            self.transport.send_line(&line).await
        })
    }

    fn set_mode<'a>(&'a mut self, _mode_id: &'a str) -> BoxFuture<'a, Result<(), AcpError>> {
        // Pi has no permission-mode concept; pi's own config governs.
        Box::pin(async { Ok(()) })
    }

    fn set_config_option<'a>(
        &'a mut self,
        config_id: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async move {
            if config_id == "model" {
                let (mut provider, model_id) = split_model_key(value);
                if model_id.is_empty() {
                    return Ok(());
                }
                if provider.is_empty() {
                    if let Some(m) = self.models.iter().find(|m| m.id == model_id || m.key() == value)
                    {
                        provider = m.provider.clone();
                    }
                }
                self.current_model_key = value.to_string();
                let line = serde_json::to_string(&json!({
                    "type": "set_model",
                    "provider": provider,
                    "modelId": model_id,
                }))
                .unwrap_or_default();
                self.transport.send_line(&line).await?;
                self.emit_config_options().await;
                return Ok(());
            }
            if config_id != "thinking" && config_id != "effort" {
                return Ok(());
            }
            self.current_level = value.to_string();
            let line = serde_json::to_string(&json!({ "type": "set_thinking_level", "level": value }))
                .unwrap_or_default();
            self.transport.send_line(&line).await?;
            self.emit_config_options().await;
            Ok(())
        })
    }

    fn image_supported(&self) -> bool {
        true
    }

    fn stderr_tail(&self) -> String {
        self.transport.stderr_tail()
    }
}

/// Saturating subtraction of two optional token counts (None = absent on the
/// newer snapshot, treat as 0).
fn opt_sub(new: Option<u64>, prev: Option<u64>) -> Option<u64> {
    let n = new.unwrap_or(0);
    let p = prev.unwrap_or(0);
    if n > p {
        Some(n - p)
    } else {
        None
    }
}

fn parse_pi_models(data: Option<&Value>) -> Vec<PiModel> {
    let arr = data
        .and_then(|d| d.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    arr.iter().filter_map(parse_one_model).collect()
}

fn parse_one_model(v: &Value) -> Option<PiModel> {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("").trim();
    if id.is_empty() {
        return None;
    }
    let provider = v
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    Some(PiModel {
        provider,
        id: id.to_string(),
        name,
    })
}

fn model_key_from_value(v: Option<&Value>) -> Option<String> {
    parse_one_model(v?).map(|m| m.key())
}

fn split_model_key(key: &str) -> (String, String) {
    let key = key.trim();
    match key.split_once('/') {
        Some((p, id)) if !p.is_empty() && !id.is_empty() => (p.to_string(), id.to_string()),
        _ => (String::new(), key.to_string()),
    }
}

fn parse_thinking_levels(data: Option<&Value>) -> Vec<String> {
    data.and_then(|d| d.get("levels"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn effective_thinking_levels(reported: &[String], agent_allow: &[String]) -> Vec<String> {
    let mut levels: Vec<String> = if reported.is_empty() {
        crate::models::effective_efforts(agent_allow)
            .into_iter()
            .map(String::from)
            .collect()
    } else if agent_allow.is_empty() {
        reported.to_vec()
    } else {
        reported
            .iter()
            .filter(|l| *l == "off" || *l == "minimal" || agent_allow.iter().any(|a| a == *l))
            .cloned()
            .collect()
    };
    if levels.iter().any(|l| l != "off") {
        levels.retain(|l| l != "off");
    }
    levels
}

fn parse_pi_commands(data: Option<&Value>) -> Vec<Value> {
    let arr = data
        .and_then(|d| d.get("commands"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    arr.iter()
        .filter_map(|c| {
            let name = c.get("name").and_then(Value::as_str).unwrap_or("").trim();
            if name.is_empty() {
                return None;
            }
            let desc = c.get("description").and_then(Value::as_str).unwrap_or("");
            Some(json!({ "name": name, "description": desc }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_params_and_response() {
        let mut obj = Map::new();
        obj.insert("title".into(), json!("Dangerous!"));
        obj.insert("message".into(), json!("Allow rm -rf?"));
        let params = pi_ui_permission_params("confirm", &obj);
        assert_eq!(params["toolCall"]["title"], "Dangerous!");
        assert!(params["options"].as_array().unwrap().len() >= 2);

        let pending = PendingUi {
            pi_id: "u1".into(),
            method: "confirm".into(),
            prefill: String::new(),
        };
        let yes = pi_ui_response(&pending, "allow", false);
        assert_eq!(yes["confirmed"], true);
        let no = pi_ui_response(&pending, "reject", false);
        assert_eq!(no["confirmed"], false);
        let cancel = pi_ui_response(&pending, "allow", true);
        assert_eq!(cancel["cancelled"], true);
    }

    #[test]
    fn select_params_use_option_text() {
        let mut obj = Map::new();
        obj.insert("title".into(), json!("Pick"));
        obj.insert("options".into(), json!(["alpha", "beta"]));
        let params = pi_ui_permission_params("select", &obj);
        let opts = params["options"].as_array().unwrap();
        assert_eq!(opts[0]["optionId"], "alpha");
        assert_eq!(opts[1]["name"], "beta");

        let pending = PendingUi {
            pi_id: "u2".into(),
            method: "select".into(),
            prefill: String::new(),
        };
        let picked = pi_ui_response(&pending, "beta", false);
        assert_eq!(picked["value"], "beta");
    }

    #[test]
    fn wardex_extension_sources_exist_in_repo() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().expect("repo root");
        let dir = root.join("pi-extensions");
        assert!(dir.join("wardex-reminders.ts").is_file(), "missing reminders extension");
        assert!(dir.join("wardex-codegraph.ts").is_file(), "missing codegraph extension");
        assert!(dir.join("wardex-plugins.ts").is_file(), "missing plugin-manager extension");
    }

    #[test]
    fn parse_models_and_split_key() {
        let data = json!({
            "models": [
                { "provider": "anthropic", "id": "claude-sonnet-4-20250514", "name": "Claude Sonnet 4" },
                { "provider": "openai", "id": "gpt-5.4", "name": "GPT-5.4" },
                { "id": "" },
            ]
        });
        let models = parse_pi_models(Some(&data));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].key(), "anthropic/claude-sonnet-4-20250514");
        assert!(models[0].display().contains("Claude Sonnet 4"));
        assert_eq!(split_model_key("anthropic/claude-sonnet-4-20250514"),
            ("anthropic".into(), "claude-sonnet-4-20250514".into()));
        assert_eq!(split_model_key("plain"), ("".into(), "plain".into()));
    }

    #[test]
    fn thinking_levels_drop_off_and_intersect_agent() {
        let reported = vec!["off".into(), "low".into(), "high".into(), "max".into()];
        let all = effective_thinking_levels(&reported, &[]);
        assert_eq!(all, vec!["low", "high", "max"]);
        let clipped = effective_thinking_levels(&reported, &["low".into(), "high".into()]);
        assert_eq!(clipped, vec!["low", "high"]);
        let fallback = effective_thinking_levels(&[], &[]);
        assert!(fallback.contains(&"low".to_string()));
        assert!(!fallback.contains(&"off".to_string()));
    }

    #[test]
    fn parse_commands_keeps_name_and_description() {
        let data = json!({
            "commands": [
                { "name": "skill:foo", "description": "Foo skill", "source": "skill" },
                { "name": "   ", "description": "empty" },
            ]
        });
        let cmds = parse_pi_commands(Some(&data));
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0]["name"], "skill:foo");
        assert_eq!(cmds[0]["description"], "Foo skill");
    }
}
