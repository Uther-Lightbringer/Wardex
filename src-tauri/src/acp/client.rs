// ACP protocol state machine, transport-agnostic: every method works against
// the `Transport` trait, so unit tests drive the full machine with a scripted
// MockTransport. Ported function-by-function from AcpClient.cpp
// (handleMessage/handleSessionUpdate/start/setMode/prompt/cancelTurn and the
// answer* helpers); the authoritative spec is docs/acp-protocol.md.
//
// Driving model (for the phase 1d chat layer): the owner spawns an actor
// task that select!s over client.recv_once() and its own command channel.
// recv_once() reads one inbound line and dispatches it (updating state,
// answering reverse RPCs, emitting events); the public command methods below
// write requests. Events always leave through the mpsc channel given at
// construction.
//
// Provider agnosticism (design red line C3): this layer never sees provider
// ids. The mode passed to set_mode() must already be provider-mapped by the
// caller (provider::map_mode; ChatController.cpp:908-916 did the same).

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::events::{AcpEvent, TurnUsage};
use super::transport::{AcpError, SpawnConfig, StdioTransport, Transport};
use super::types;

/// Parameters for the handshake part of start() (AcpClient.cpp:49-52 —
/// cliPath/acpArgs/env already happened at transport spawn time).
#[derive(Debug, Clone, Default)]
pub struct StartParams {
    /// Workspace dir; the source of session/new|load's `cwd` param.
    pub cwd: String,
    /// Already provider-mapped mode; empty means "default".
    pub preferred_mode: String,
    /// Agent-side session to resume via session/load; empty = session/new.
    pub resume_session_id: String,
    /// Passed through verbatim to session/new|load (no structural checks).
    pub mcp_servers: Vec<Value>,
}

pub struct AcpClient<T: Transport> {
    transport: T,
    events: mpsc::Sender<AcpEvent>,

    next_id: i64,
    // The five in-flight request ids (AcpClient.h:84-88).
    init_id: Option<i64>,
    new_session_id: Option<i64>,
    load_session_id: Option<i64>,
    prompt_id: Option<i64>,
    set_mode_id: Option<i64>,
    /// In-flight session/set_config_option request id (non-mode pickers).
    set_option_id: Option<i64>,

    session_id: String,
    cwd: String,
    pending_mode: String,
    resume_session_id: String,
    initialized: bool,
    turn_busy: bool,
    can_load_session: bool,
    image_supported: bool,
    /// Raw configOptions[] from session/new|load (kimi: model/thinking/mode
    /// pickers), refreshed by set_config_option responses and
    /// config_option_update notifications.
    config_options: Vec<Value>,
    /// Slash commands from available_commands_update (state, not history:
    /// kept during replay, re-emitted by session_ready). Raw ACP objects
    /// {name, description, input?} passed through.
    available_commands: Vec<Value>,
    /// session/load replays the whole history as session/update
    /// notifications; WarDex keeps its own local history, so everything
    /// arriving while this is set is discarded (AcpClient.cpp:489-491)
    /// except state-type updates (available_commands_update).
    replaying: bool,
    mcp_servers: Vec<Value>,
    /// authMethods[] from the initialize result; used when a prompt is
    /// rejected with -32002 (auth required) — authenticate is tried once
    /// with the first method, then the prompt is resent (see last_prompt).
    auth_methods: Vec<Value>,
    /// In-flight authenticate request id.
    auth_id: Option<i64>,
    /// Only one automatic authenticate attempt per spawn.
    auth_retried: bool,
    /// The prompt being sent, kept so an auth-required retry can resend it
    /// verbatim after authenticate succeeds.
    last_prompt: Option<(String, Vec<String>)>,
}

/// JSON-RPC error object -> display text. The numeric code is kept alongside
/// the message so bubbles show the agent's real failure class (HTTP status,
/// RPC code) instead of free text alone; the rate-limit detector's
/// substring matching is unaffected by the "[code] " prefix.
fn rpc_error_text(err: &Value) -> String {
    let msg = err.get("message").and_then(Value::as_str).unwrap_or_default();
    match err.get("code").and_then(types::json_int) {
        Some(code) if !msg.is_empty() => format!("[{code}] {msg}"),
        _ => msg.to_string(),
    }
}

impl<T: Transport> AcpClient<T> {
    pub fn new(transport: T, events: mpsc::Sender<AcpEvent>) -> Self {
        Self {
            transport,
            events,
            next_id: 1,
            init_id: None,
            new_session_id: None,
            load_session_id: None,
            prompt_id: None,
            set_mode_id: None,
            set_option_id: None,
            session_id: String::new(),
            cwd: String::new(),
            pending_mode: String::new(),
            resume_session_id: String::new(),
            initialized: false,
            turn_busy: false,
            can_load_session: false,
            image_supported: false,
            config_options: Vec::new(),
            available_commands: Vec::new(),
            replaying: false,
            mcp_servers: Vec::new(),
            auth_methods: Vec::new(),
            auth_id: None,
            auth_retried: false,
            last_prompt: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn turn_busy(&self) -> bool {
        self.turn_busy
    }

    /// From initialize -> agentCapabilities.promptCapabilities.image.
    pub fn image_supported(&self) -> bool {
        self.image_supported
    }

    /// From initialize -> agentCapabilities.loadSession.
    pub fn can_load_session(&self) -> bool {
        self.can_load_session
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Tail of the child CLI's stderr (empty for transports without one);
    /// the chat layer folds it into failure bubbles for real diagnostics.
    pub fn stderr_tail(&self) -> String {
        self.transport.stderr_tail()
    }

    /// State reset half of the old stop() (AcpClient.cpp:28-47): session id,
    /// in-flight ids and all flags cleared. Killing the process is the
    /// transport's business (drop StdioTransport = kill_on_drop).
    pub fn reset(&mut self) {
        self.session_id.clear();
        self.initialized = false;
        self.turn_busy = false;
        self.can_load_session = false;
        self.image_supported = false;
        self.replaying = false;
        self.init_id = None;
        self.new_session_id = None;
        self.load_session_id = None;
        self.prompt_id = None;
        self.set_mode_id = None;
        self.set_option_id = None;
        self.config_options.clear();
        self.available_commands.clear();
        self.auth_methods.clear();
        self.auth_id = None;
        self.auth_retried = false;
        self.last_prompt = None;
    }

    /// Begin the handshake: sends initialize (AcpClient.cpp:119-135). The
    /// rest of the sequence (session/load | session/new -> mode -> started)
    /// is driven by responses arriving through recv_once().
    pub async fn start(&mut self, params: StartParams) -> Result<(), AcpError> {
        self.cwd = params.cwd;
        self.pending_mode = if params.preferred_mode.is_empty() {
            "default".to_string()
        } else {
            params.preferred_mode
        };
        self.resume_session_id = params.resume_session_id;
        self.mcp_servers = params.mcp_servers;

        let id = self.alloc_id();
        self.init_id = Some(id);
        self.send_request("initialize", types::initialize_params(), id)
            .await
    }

    /// session/set_config_option with configId fixed to "mode"
    /// (AcpClient.cpp:138-150). Without a session the mode is only cached
    /// and applied by session_ready() later. NOTE: old-code quirk kept
    /// verbatim — the response always reports `pending_mode`, which is only
    /// refreshed while session-less (modeChanged after a mid-session switch
    /// can carry the stale pre-session value; nothing consumed the signal).
    pub async fn set_mode(&mut self, mode_id: &str) -> Result<(), AcpError> {
        if self.session_id.is_empty() {
            self.pending_mode = mode_id.to_string();
            return Ok(());
        }
        let id = self.alloc_id();
        self.set_mode_id = Some(id);
        self.send_request(
            "session/set_config_option",
            json!({
                "sessionId": self.session_id,
                "configId": "mode",
                "value": mode_id,
            }),
            id,
        )
        .await
    }

    /// Generic session/set_config_option for NON-mode pickers (kimi exposes
    /// "model" and "thinking" alongside "mode"; ACP method coverage table).
    /// The response carries the refreshed configOptions[], which we store
    /// and forward as AcpEvent::ConfigOptions.
    pub async fn set_config_option(&mut self, config_id: &str, value: &str) -> Result<(), AcpError> {
        if self.session_id.is_empty() {
            return Ok(()); // session-less: nothing to apply it to
        }
        let id = self.alloc_id();
        self.set_option_id = Some(id);
        self.send_request(
            "session/set_config_option",
            json!({
                "sessionId": self.session_id,
                "configId": config_id,
                "value": value,
            }),
            id,
        )
        .await
    }

    /// One turn of session/prompt (AcpClient.cpp:152-197). Empty text adds
    /// no text block (pure-image prompts are legal); images are base64
    /// blocks silently skipped unless the agent advertised image support;
    /// unreadable files are silently skipped.
    pub async fn prompt(&mut self, text: &str, image_paths: &[String]) -> Result<(), AcpError> {
        if self.session_id.is_empty() || self.turn_busy {
            self.emit(AcpEvent::ProtocolError {
                error: "ACP 会话未就绪或仍在生成".to_string(),
            })
            .await;
            return Ok(());
        }
        self.turn_busy = true;
        let id = self.alloc_id();
        self.prompt_id = Some(id);
        // Kept for the auth-required (-32002) retry: authenticate, then
        // resend this prompt verbatim.
        self.last_prompt = Some((text.to_string(), image_paths.to_vec()));

        let mut blocks: Vec<Value> = Vec::new();
        if !text.is_empty() {
            blocks.push(json!({ "type": "text", "text": text }));
        }
        if self.image_supported {
            for path in image_paths {
                let Ok(data) = tokio::fs::read(path).await else {
                    continue;
                };
                blocks.push(json!({
                    "type": "image",
                    "mimeType": types::mime_for_image(path),
                    "data": base64::engine::general_purpose::STANDARD.encode(data),
                }));
            }
        }
        self.send_request(
            "session/prompt",
            json!({ "sessionId": self.session_id, "prompt": blocks }),
            id,
        )
        .await
    }

    /// session/cancel notification (AcpClient.cpp:199-206); the agent may
    /// ignore it — the chat layer has the 2500ms force-kill fallback.
    pub async fn cancel_turn(&mut self) -> Result<(), AcpError> {
        if self.session_id.is_empty() {
            return Ok(());
        }
        self.send_notification(
            "session/cancel",
            json!({ "sessionId": self.session_id }),
        )
        .await
    }

    /// Answer session/request_permission (AcpClient.cpp:208-220). The
    /// outcome is DOUBLE-NESTED: result.outcome.outcome — do not flatten.
    pub async fn answer_permission(
        &mut self,
        request_id: i64,
        option_id: &str,
        cancelled: bool,
    ) -> Result<(), AcpError> {
        let outcome = if cancelled {
            json!({ "outcome": "cancelled" })
        } else {
            json!({ "outcome": "selected", "optionId": option_id })
        };
        self.send_response(request_id, json!({ "outcome": outcome }))
            .await
    }

    /// Read and dispatch one inbound line. Ok(true) = a line was processed;
    /// Ok(false) = EOF, the process exited (ProcessExited emitted, turnBusy
    /// and initialized cleared — AcpClient.cpp:302-307). A transport error
    /// before initialize completed emits StartFailed, matching the old
    /// errorOccurred handler (AcpClient.cpp:86-89).
    pub async fn recv_once(&mut self) -> Result<bool, AcpError> {
        match self.transport.recv_line().await {
            Ok(Some(line)) => {
                self.handle_line(&line).await?;
                Ok(true)
            }
            Ok(None) => {
                self.turn_busy = false;
                self.initialized = false;
                let code = self.transport.exit_code().await.unwrap_or(-1);
                self.emit(AcpEvent::ProcessExited { code }).await;
                Ok(false)
            }
            Err(e) => {
                if !self.initialized {
                    self.emit(AcpEvent::StartFailed {
                        error: e.to_string(),
                    })
                    .await;
                }
                Err(e)
            }
        }
    }

    /// One trimmed NDJSON line (AcpClient.cpp:309-318): bad JSON and
    /// non-objects are logged and dropped, never fatal — agent CLIs print
    /// banner/log noise on stdout.
    pub async fn handle_line(&mut self, line: &str) -> Result<(), AcpError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let msg: Value = match serde_json::from_str::<Value>(trimmed) {
            Ok(v) if v.is_object() => v,
            _ => {
                let head: String = trimmed.chars().take(200).collect();
                log::warn!("AcpClient bad JSON: {head}");
                return Ok(());
            }
        };
        self.handle_message(&msg).await
    }

    // ---- dispatch (AcpClient.cpp:320-466) ----

    async fn handle_message(&mut self, msg: &Value) -> Result<(), AcpError> {
        // Response to our request: has id, no method.
        if msg.get("id").is_some() && msg.get("method").is_none() {
            let id = msg.get("id").and_then(types::json_int).unwrap_or(-1);
            if let Some(err) = msg.get("error") {
                let code = err.get("code").and_then(types::json_int);
                return self.handle_error_response(id, rpc_error_text(err), code).await;
            }
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            return self.handle_result_response(id, &result).await;
        }

        // Request or notification from the agent.
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let req_id = msg.get("id").and_then(types::json_int);

        match method {
            "session/update" => self.handle_session_update(&params).await,
            "session/request_permission" => {
                if let Some(id) = req_id {
                    self.emit(AcpEvent::PermissionRequested {
                        request_id: id,
                        params,
                    })
                    .await;
                }
                Ok(())
            }
            "fs/read_text_file" => match req_id {
                Some(id) => self.handle_fs_read(id, &params).await,
                None => Ok(()),
            },
            "fs/write_text_file" => match req_id {
                Some(id) => self.handle_fs_write(id, &params).await,
                None => Ok(()),
            },
            // Unknown reverse request: reject only when it expects a
            // response (AcpClient.cpp:463-465); id-less notifications are
            // ignored. Both paths are logged now — terminal/* and other
            // unsupported namespaces must be visible, not silent.
            _ => match req_id {
                Some(id) => {
                    log::info!("AcpClient unsupported reverse method '{method}' -> -32601");
                    self.send_error(
                        id,
                        types::RPC_ERROR_METHOD_NOT_FOUND,
                        &format!("Method not found: {method}"),
                    )
                    .await
                }
                None => {
                    log::info!("AcpClient unknown notification '{method}' ignored");
                    Ok(())
                }
            },
        }
    }

    async fn handle_error_response(&mut self, id: i64, em: String, code: Option<i64>) -> Result<(), AcpError> {
        // -32002 (auth required): try authenticate once with the first
        // advertised method, then resend the failed prompt. kimi/claude
        // authenticate via env so this path is for on-demand-auth agents.
        if code == Some(types::RPC_ERROR_AUTH_REQUIRED)
            && !self.auth_methods.is_empty()
            && !self.auth_retried
            && self.prompt_id == Some(id)
        {
            self.auth_retried = true;
            self.prompt_id = None; // turn stays busy; the resend re-arms it
            let method_id = self
                .auth_methods
                .first()
                .and_then(|m| m.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            log::info!("AcpClient auth required; trying authenticate('{method_id}')");
            let auth_id = self.alloc_id();
            self.auth_id = Some(auth_id);
            return self
                .send_request(
                    "authenticate",
                    json!({ "methodId": method_id }),
                    auth_id,
                )
                .await;
        }
        if self.auth_id == Some(id) {
            // authenticate itself failed: close the turn like a prompt error.
            self.auth_id = None;
            self.turn_busy = false;
            self.emit(AcpEvent::ProtocolError {
                error: format!("ACP 认证失败：{em}"),
            })
            .await;
            self.emit(AcpEvent::TurnFinished {
                stop_reason: "error".to_string(),
                usage: None,
            })
            .await;
            return Ok(());
        }
        if self.load_session_id == Some(id) {
            // Stored session expired/unknown on the agent side — fall back
            // to a fresh one instead of failing the whole start
            // (AcpClient.cpp:328-334). The chat layer surfaces the real
            // error in a bubble (context is silently lost otherwise).
            self.load_session_id = None;
            self.replaying = false;
            log::info!("AcpClient session/load failed, falling back to session/new: {em}");
            self.emit(AcpEvent::SessionLoadFallback { error: em }).await;
            self.request_new_session().await
        } else if self.init_id == Some(id) || self.new_session_id == Some(id) {
            self.emit(AcpEvent::StartFailed {
                error: if em.is_empty() {
                    "ACP 初始化失败".to_string()
                } else {
                    em
                },
            })
            .await;
            Ok(())
        } else if self.prompt_id == Some(id) {
            self.turn_busy = false;
            self.prompt_id = None;
            // STRICT ORDER (acp-protocol.md §7): protocolError first so the
            // chat layer's rate-limit detector has the raw text in hand when
            // turnFinished("error") arrives; then the error surfaced in the
            // bubble; then the turn closes.
            self.emit(AcpEvent::ProtocolError { error: em.clone() }).await;
            if !em.is_empty() {
                self.emit(AcpEvent::MessageChunk {
                    text: format!("回合失败：{em}"),
                })
                .await;
            }
            self.emit(AcpEvent::TurnFinished {
                stop_reason: "error".to_string(),
                usage: None,
            })
            .await;
            Ok(())
        } else {
            self.emit(AcpEvent::ProtocolError { error: em }).await;
            Ok(())
        }
    }

    async fn handle_result_response(&mut self, id: i64, result: &Value) -> Result<(), AcpError> {
        if self.init_id == Some(id) {
            self.initialized = true;
            self.init_id = None;
            let caps = result.get("agentCapabilities").cloned().unwrap_or(Value::Null);
            self.can_load_session = caps
                .get("loadSession")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.image_supported = caps
                .get("promptCapabilities")
                .and_then(|p| p.get("image"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.auth_methods = result
                .get("authMethods")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if self.can_load_session && !self.resume_session_id.is_empty() {
                let req_id = self.alloc_id();
                self.load_session_id = Some(req_id);
                self.replaying = true;
                self.send_request(
                    "session/load",
                    json!({
                        "sessionId": self.resume_session_id,
                        "cwd": self.effective_cwd(),
                        "mcpServers": self.mcp_servers,
                    }),
                    req_id,
                )
                .await
            } else {
                self.request_new_session().await
            }
        } else if self.auth_id == Some(id) {
            // authenticate succeeded: resend the prompt that got -32002.
            self.auth_id = None;
            self.turn_busy = false; // prompt() re-arms it and re-checks
            match self.last_prompt.clone() {
                Some((text, images)) => self.prompt(&text, &images).await,
                None => {
                    self.emit(AcpEvent::TurnFinished {
                        stop_reason: "error".to_string(),
                        usage: None,
                    })
                    .await;
                    Ok(())
                }
            }
        } else if self.load_session_id == Some(id) {
            self.load_session_id = None;
            self.replaying = false;
            // No new sessionId in the result — the requested one sticks
            // (AcpClient.cpp:380-386).
            self.session_id = self.resume_session_id.clone();
            if let Some(opts) = result.get("configOptions").and_then(Value::as_array) {
                self.config_options = opts.clone();
            }
            self.session_ready().await
        } else if self.new_session_id == Some(id) {
            self.session_id = result
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.new_session_id = None;
            if self.session_id.is_empty() {
                self.emit(AcpEvent::StartFailed {
                    error: "session/new 未返回 sessionId".to_string(),
                })
                .await;
                return Ok(());
            }
            if let Some(opts) = result.get("configOptions").and_then(Value::as_array) {
                self.config_options = opts.clone();
            }
            self.session_ready().await
        } else if self.set_option_id == Some(id) {
            self.set_option_id = None;
            if let Some(opts) = result.get("configOptions").and_then(Value::as_array) {
                self.config_options = opts.clone();
                self.emit(AcpEvent::ConfigOptions {
                    options: self.config_options.clone(),
                })
                .await;
            }
            Ok(())
        } else if self.set_mode_id == Some(id) {
            self.set_mode_id = None;
            // result may include configOptions; never read (AcpClient.cpp:398-402).
            self.emit(AcpEvent::ModeChanged {
                mode: self.pending_mode.clone(),
            })
            .await;
            Ok(())
        } else if self.prompt_id == Some(id) {
            self.turn_busy = false;
            self.prompt_id = None;
            let stop = result
                .get("stopReason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_string();
            // Token usage rides the prompt result (kimi: result.usage). A
            // missing/broken usage is None — it must never fail the turn.
            let usage = result.get("usage").and_then(TurnUsage::from_acp);
            self.emit(AcpEvent::TurnFinished { stop_reason: stop, usage }).await;
            Ok(())
        } else {
            // Response for an unknown id: still ignored (AcpClient.cpp:411),
            // but logged — a stray/duplicated agent response is diagnosable.
            log::warn!("AcpClient response with no matching in-flight id: {id}");
            Ok(())
        }
    }

    // ---- session/update notifications (AcpClient.cpp:487-517) ----

    async fn handle_session_update(&mut self, params: &Value) -> Result<(), AcpError> {
        let Some(update) = params.get("update").filter(|u| u.is_object()) else {
            return Ok(());
        };
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Replay drop (AcpClient.cpp:489-491) — EXCEPT available_commands:
        // it is state (the session's slash-command list), not history, so a
        // resumed session must not lose it. It is stored during replay and
        // re-emitted by session_ready(). plan stays dropped: it is the
        // replayed turn's history, like message chunks.
        if self.replaying && kind != "available_commands_update" {
            return Ok(());
        }
        match kind {
            "agent_thought_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.emit(AcpEvent::ThoughtChunk { text }).await;
            }
            "agent_message_chunk" => {
                let text = update
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.emit(AcpEvent::MessageChunk { text }).await;
            }
            "tool_call" => {
                self.emit(AcpEvent::ToolCall {
                    call: types::normalize_tool_call(update),
                })
                .await;
            }
            "tool_call_update" => {
                self.emit(AcpEvent::ToolCallUpdate {
                    update: types::normalize_tool_call(update),
                })
                .await;
            }
            "available_commands_update" => {
                // [{name, description, input?}] passed through verbatim;
                // latest list replaces the old one. During replay: store
                // only, session_ready() emits it (like ConfigOptions).
                if let Some(cmds) = update.get("availableCommands").and_then(Value::as_array) {
                    self.available_commands = cmds.clone();
                    if !self.replaying {
                        self.emit(AcpEvent::AvailableCommands {
                            commands: self.available_commands.clone(),
                        })
                        .await;
                    }
                }
            }
            "plan" => {
                let entries = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.emit(AcpEvent::Plan { entries }).await;
            }
            "usage_update" => {
                // Token usage outside the prompt result; a broken payload is
                // simply skipped (same tolerance rule as the prompt result).
                if let Some(usage) = update.get("usage").and_then(TurnUsage::from_acp) {
                    self.emit(AcpEvent::UsageUpdate { usage }).await;
                }
            }
            "session_info_update" => {
                let title = update
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.emit(AcpEvent::SessionInfo { title }).await;
            }
            "current_mode_update" => {
                // Schema field is currentModeId; tolerate modeId for adapters.
                let mode = update
                    .get("currentModeId")
                    .or_else(|| update.get("modeId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !mode.is_empty() {
                    self.pending_mode = mode.clone();
                    self.emit(AcpEvent::ModeChanged { mode }).await;
                }
            }
            "user_message_chunk" => {
                // Only meaningful inside a session/load replay (which is
                // dropped wholesale above); outside one it is an agent quirk.
                log::info!("AcpClient user_message_chunk outside replay (ignored)");
            }
            "config_option_update" => {
                // kimi sends the single changed option {configId, value};
                // patch the stored list and forward the whole thing.
                let cid = update.get("configId").and_then(Value::as_str).unwrap_or_default();
                let val = update.get("value").and_then(Value::as_str).unwrap_or_default();
                if !cid.is_empty() {
                    for o in self.config_options.iter_mut() {
                        if o.get("id").and_then(Value::as_str) == Some(cid) {
                            o["currentValue"] = Value::String(val.to_string());
                        }
                    }
                    self.emit(AcpEvent::ConfigOptions {
                        options: self.config_options.clone(),
                    })
                    .await;
                }
            }
            other => {
                // Unknown kinds are dropped by design, but logged so new
                // agent-side update types are visible instead of silent.
                let keys: Vec<&str> = update
                    .as_object()
                    .map(|m| m.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                log::info!("AcpClient unknown sessionUpdate kind '{other}' (keys: {keys:?})");
            }
        }
        Ok(())
    }

    // ---- fs reverse RPCs (AcpClient.cpp:428-460) ----

    async fn handle_fs_read(&mut self, req_id: i64, params: &Value) -> Result<(), AcpError> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(_) => {
                return self
                    .send_error(req_id, types::RPC_ERROR_FS, &format!("无法读取: {path}"))
                    .await;
            }
        };
        // Qt's QString::fromUtf8 replaces invalid sequences — lossy matches.
        let mut content = String::from_utf8_lossy(&bytes).into_owned();
        let line = params.get("line").and_then(types::json_int).unwrap_or(0);
        let limit = params.get("limit").and_then(types::json_int).unwrap_or(0);
        if line > 0 || limit > 0 {
            content = types::clip_lines(&content, line, limit);
        }
        self.send_response(req_id, json!({ "content": content }))
            .await
    }

    async fn handle_fs_write(&mut self, req_id: i64, params: &Value) -> Result<(), AcpError> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = params
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // QDir().mkpath(fi.absolutePath()) then a truncating UTF-8 write.
        let parent_ok = match std::path::Path::new(path).parent() {
            Some(p) if !p.as_os_str().is_empty() => {
                tokio::fs::create_dir_all(p).await.is_ok()
            }
            _ => true,
        };
        let ok = parent_ok && tokio::fs::write(path, content.as_bytes()).await.is_ok();
        if ok {
            self.send_response(req_id, json!({})).await
        } else {
            self.send_error(req_id, types::RPC_ERROR_FS, &format!("无法写入: {path}"))
                .await
        }
    }

    // ---- internal helpers ----

    /// session/new with cwd and mcpServers passthrough (AcpClient.cpp:468-475).
    async fn request_new_session(&mut self) -> Result<(), AcpError> {
        let id = self.alloc_id();
        self.new_session_id = Some(id);
        self.send_request(
            "session/new",
            json!({
                "cwd": self.effective_cwd(),
                "mcpServers": self.mcp_servers,
            }),
            id,
        )
        .await
    }

    /// Session established: apply the pending mode (non-default only), then
    /// Started (AcpClient.cpp:477-485).
    async fn session_ready(&mut self) -> Result<(), AcpError> {
        if self.pending_mode_applies() {
            let mode = self.pending_mode.clone();
            self.set_mode(&mode).await?;
        } else {
            // Nothing to force onto the session: report the agent's current
            // mode so the frontend picker/status stay in sync.
            self.emit(AcpEvent::ModeChanged {
                mode: self.current_advertised_mode(),
            })
            .await;
        }
        self.emit(AcpEvent::Started {
            session_id: self.session_id.clone(),
        })
        .await;
        self.emit(AcpEvent::ConfigOptions {
            options: self.config_options.clone(),
        })
        .await;
        // Re-emit state captured during session/load replay (the replay path
        // stores available_commands without emitting).
        if !self.available_commands.is_empty() {
            self.emit(AcpEvent::AvailableCommands {
                commands: self.available_commands.clone(),
            })
            .await;
        }
        Ok(())
    }

    /// Whether the caller's pending (global WarDex) mode may be forced onto
    /// the session. Agents that advertise their own mode picker (configOptions
    /// `id=="mode"` with options) own the mode list — opencode exposes its
    /// agents (build/plan/general…), so a WarDex-only id such as "yolo" is
    /// rejected with `mode not found: <id>`. The pending mode only applies
    /// when there is no advertised picker, or the id is one of the advertised
    /// values.
    fn pending_mode_applies(&self) -> bool {
        if self.pending_mode.is_empty() || self.pending_mode == "default" {
            return false;
        }
        let Some(mode_opt) = self
            .config_options
            .iter()
            .find(|o| o.get("id").and_then(Value::as_str) == Some("mode"))
        else {
            // No advertised picker → old behavior applies.
            return true;
        };
        let Some(opts) = mode_opt.get("options").and_then(Value::as_array) else {
            return false;
        };
        opts.iter()
            .any(|v| v.get("value").and_then(Value::as_str) == Some(self.pending_mode.as_str()))
    }

    /// currentValue of the advertised mode picker, if any ("default" when no
    /// picker exists, keeping the pre-mode-advertisement event shape).
    fn current_advertised_mode(&self) -> String {
        self.config_options
            .iter()
            .find(|o| o.get("id").and_then(Value::as_str) == Some("mode"))
            .and_then(|o| o.get("currentValue").and_then(Value::as_str))
            .unwrap_or("default")
            .to_string()
    }

    fn effective_cwd(&self) -> String {
        if !self.cwd.is_empty() {
            return self.cwd.clone();
        }
        // QDir::currentPath() fallback (AcpClient.cpp:370-372).
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn alloc_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn emit(&self, event: AcpEvent) {
        // A dropped receiver (chat layer gone) must never kill the protocol.
        if let Err(e) = self.events.send(event).await {
            log::warn!("AcpClient event channel closed: {e}");
        }
    }

    async fn write_value(&self, msg: Value) -> Result<(), AcpError> {
        // serde_json::to_string is compact — one JSON per line (AcpClient.cpp:246-247).
        let line = serde_json::to_string(&msg)?;
        self.transport.send_line(&line).await
    }

    async fn send_request(&self, method: &str, params: Value, id: i64) -> Result<(), AcpError> {
        self.write_value(types::request(id, method, params)).await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), AcpError> {
        self.write_value(types::notification(method, params)).await
    }

    async fn send_response(&self, id: i64, result: Value) -> Result<(), AcpError> {
        self.write_value(types::response(id, result)).await
    }

    async fn send_error(&self, id: i64, code: i64, message: &str) -> Result<(), AcpError> {
        self.write_value(types::error_response(id, code, message)).await
    }
}

impl AcpClient<StdioTransport> {
    /// Production entry point: spawn the CLI subprocess, then begin the
    /// handshake. Spawn-level failures emit StartFailed (the old
    /// AcpClient::start did the same for every pre-initialize failure) and
    /// return Err.
    pub async fn spawn(
        config: SpawnConfig,
        start: StartParams,
        events: mpsc::Sender<AcpEvent>,
    ) -> Result<Self, AcpError> {
        let transport = match StdioTransport::spawn(&config).await {
            Ok(t) => t,
            Err(e) => {
                let _ = events
                    .send(AcpEvent::StartFailed {
                        error: e.to_string(),
                    })
                    .await;
                return Err(e);
            }
        };
        let mut client = AcpClient::new(transport, events);
        client.start(start).await?;
        Ok(client)
    }
}
#[cfg(test)]
mod tests {
// Full state-machine tests, driven by scripted NDJSON through MockTransport:
// feed inbound frames, step recv_once(), assert outbound frames and the
// emitted event sequence.

use super::*;
use crate::acp::transport::MockTransport;

use serde_json::{json, Value};
use tokio::sync::mpsc;

struct Harness {
    client: AcpClient<MockTransport>,
    rx: mpsc::Receiver<AcpEvent>,
    transport: MockTransport, // separate handle to the same mock state
}

fn harness() -> Harness {
    let transport = MockTransport::new();
    let (tx, rx) = mpsc::channel(64);
    // MockTransport is Arc-backed but not Clone; feed via a second handle.
    let feeder = transport.clone();
    Harness {
        client: AcpClient::new(transport, tx),
        rx,
        transport: feeder,
    }
}

/// Default start params: cwd set, default mode, no resume.
fn start_params() -> StartParams {
    StartParams {
        cwd: r"C:\ws\proj".to_string(),
        ..Default::default()
    }
}

fn init_result(load_session: bool, image: bool) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": load_session,
                "promptCapabilities": { "image": image }
            }
        }
    })
}

fn update_frame(kind: &str, extra: Value) -> Value {
    let mut update = json!({ "sessionUpdate": kind });
    update
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    json!({ "jsonrpc": "2.0", "method": "session/update",
            "params": { "sessionId": "s", "update": update } })
}

fn sent_requests(h: &Harness) -> Vec<Value> {
    h.transport.sent_json()
}

fn events(h: &mut Harness) -> Vec<AcpEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = h.rx.try_recv() {
        out.push(ev);
    }
    out
}

/// Drive a client to session-ready via session/new (no load support).
async fn drive_to_ready(h: &mut Harness) {
    h.client.start(start_params()).await.expect("start");
    h.transport.feed_json(init_result(false, true)).await;
    h.client.recv_once().await.expect("init response");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h.client.recv_once().await.expect("new response");
    assert_eq!(h.client.session_id(), "s-1");
}

#[tokio::test]
async fn handshake_sends_pinned_initialize_then_session_new() {
    let mut h = harness();
    h.client.start(start_params()).await.expect("start");
    let sent = sent_requests(&h);
    assert_eq!(sent, vec![crate::acp::types::initialize_request(1)]);

    h.transport.feed_json(init_result(false, true)).await;
    h.client.recv_once().await.expect("init");
    assert!(h.client.is_initialized());
    assert!(h.client.image_supported());
    assert!(!h.client.can_load_session());

    let sent = sent_requests(&h);
    assert_eq!(
        sent[1],
        json!({ "jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": { "cwd": r"C:\ws\proj", "mcpServers": [] } })
    );

    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h.client.recv_once().await.expect("new");
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ModeChanged { mode: "default".into() },
            AcpEvent::Started { session_id: "s-1".into() },
            AcpEvent::ConfigOptions { options: vec![] },
        ]
    );
}

#[tokio::test]
async fn mcp_servers_passed_through_verbatim() {
    let mut h = harness();
    let params = StartParams {
        mcp_servers: vec![json!({ "name": "fs", "command": "mcp-fs", "args": ["--ro"] })],
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    let sent = sent_requests(&h);
    assert_eq!(
        sent[1]["params"]["mcpServers"],
        json!([{ "name": "fs", "command": "mcp-fs", "args": ["--ro"] }])
    );
}

#[tokio::test]
async fn initialize_error_is_start_failed() {
    let mut h = harness();
    h.client.start(start_params()).await.expect("start");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 1,
                           "error": { "code": -32000, "message": "bad key" } }))
        .await;
    h.client.recv_once().await.expect("err");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::StartFailed { error: "[-32000] bad key".into() }]
    );
}

#[tokio::test]
async fn session_new_empty_session_id_is_start_failed() {
    let mut h = harness();
    h.client.start(start_params()).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": {} }))
        .await;
    h.client.recv_once().await.expect("new");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::StartFailed {
            error: "session/new 未返回 sessionId".into()
        }]
    );
}

#[tokio::test]
async fn session_load_replay_is_discarded_then_resumed() {
    let mut h = harness();
    let params = StartParams {
        resume_session_id: "old-42".to_string(),
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(true, false)).await;
    h.client.recv_once().await.expect("init");
    assert!(h.client.can_load_session());
    // session/load goes out with the resume id; replay flag now on.
    let sent = sent_requests(&h);
    assert_eq!(
        sent[1],
        json!({ "jsonrpc": "2.0", "id": 2, "method": "session/load",
                "params": { "sessionId": "old-42", "cwd": r"C:\ws\proj", "mcpServers": [] } })
    );

    // Replay notifications during the in-flight load: all discarded.
    h.transport
        .feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "history" } })))
        .await;
    h.client.recv_once().await.expect("replay update");
    assert!(events(&mut h).is_empty(), "replay updates must be dropped");

    // Load succeeds: no new sessionId — the requested one sticks.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": {} }))
        .await;
    h.client.recv_once().await.expect("load ok");
    assert_eq!(h.client.session_id(), "old-42");
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ModeChanged { mode: "default".into() },
            AcpEvent::Started { session_id: "old-42".into() },
            AcpEvent::ConfigOptions { options: vec![] },
        ]
    );

    // Post-load updates flow normally again.
    h.transport
        .feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "live" } })))
        .await;
    h.client.recv_once().await.expect("live update");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::MessageChunk { text: "live".into() }]
    );
}

#[tokio::test]
async fn session_load_error_falls_back_to_session_new() {
    let mut h = harness();
    let params = StartParams {
        resume_session_id: "expired".to_string(),
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(true, false)).await;
    h.client.recv_once().await.expect("init");

    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2,
                           "error": { "code": -32000, "message": "unknown session" } }))
        .await;
    h.client.recv_once().await.expect("load err");
    // Fallback: session/new sent, NO startFailed, whole start still alive;
    // the chat layer is told WHY the resume failed (lost context surfaced).
    let sent = sent_requests(&h);
    assert_eq!(sent[2]["method"], "session/new");
    assert_eq!(sent[2]["id"], 3);
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::SessionLoadFallback {
            error: "[-32000] unknown session".into()
        }]
    );

    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 3, "result": { "sessionId": "fresh-1" } }))
        .await;
    h.client.recv_once().await.expect("new ok");
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ModeChanged { mode: "default".into() },
            AcpEvent::Started { session_id: "fresh-1".into() },
            AcpEvent::ConfigOptions { options: vec![] },
        ]
    );
}

#[tokio::test]
async fn non_default_pending_mode_is_applied_on_session_ready() {
    let mut h = harness();
    let params = StartParams {
        preferred_mode: "acceptEdits".to_string(), // already provider-mapped by chat
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h.client.recv_once().await.expect("new");

    // set_config_option went out BEFORE the Started event.
    let sent = sent_requests(&h);
    assert_eq!(
        sent[2],
        json!({ "jsonrpc": "2.0", "id": 3, "method": "session/set_config_option",
                "params": { "sessionId": "s-1", "configId": "mode", "value": "acceptEdits" } })
    );
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::Started { session_id: "s-1".into() },
            AcpEvent::ConfigOptions { options: vec![] },
        ]
    );

    // Its response emits modeChanged with the pending mode; result ignored.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 3, "result": { "configOptions": [] } }))
        .await;
    h.client.recv_once().await.expect("mode resp");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::ModeChanged { mode: "acceptEdits".into() }]
    );
}

#[tokio::test]
async fn pending_mode_skipped_when_not_in_advertised_modes() {
    let mut h = harness();
    // opencode advertises its agents as modes (build/plan/general…) — a
    // WarDex global id like "yolo" must never be forced onto such a session
    // (it would be rejected with "mode not found: yolo").
    let options = json!([{
        "id": "mode", "name": "Session Mode", "category": "mode",
        "type": "select", "currentValue": "build",
        "options": [
            { "value": "build", "name": "Build" },
            { "value": "plan", "name": "Plan" }
        ]
    }]);
    let params = StartParams {
        preferred_mode: "yolo".to_string(),
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(
            json!({ "jsonrpc": "2.0", "id": 2,
                    "result": { "sessionId": "s-1", "configOptions": options } }),
        )
        .await;
    h.client.recv_once().await.expect("new");

    // No session/set_config_option went out — the agent keeps its mode.
    let sent = sent_requests(&h);
    assert_eq!(sent.len(), 2, "only initialize + session/new went out");
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ModeChanged { mode: "build".into() },
            AcpEvent::Started { session_id: "s-1".into() },
            AcpEvent::ConfigOptions {
                options: options.as_array().cloned().unwrap(),
            },
        ]
    );
}

#[tokio::test]
async fn pending_mode_applied_when_in_advertised_modes() {
    let mut h = harness();
    let options = json!([{
        "id": "mode", "name": "Session Mode", "category": "mode",
        "type": "select", "currentValue": "build",
        "options": [
            { "value": "build", "name": "Build" },
            { "value": "plan", "name": "Plan" }
        ]
    }]);
    let params = StartParams {
        preferred_mode: "plan".to_string(), // also advertised → still applied
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(
            json!({ "jsonrpc": "2.0", "id": 2,
                    "result": { "sessionId": "s-1", "configOptions": options } }),
        )
        .await;
    h.client.recv_once().await.expect("new");

    let sent = sent_requests(&h);
    assert_eq!(
        sent[2],
        json!({ "jsonrpc": "2.0", "id": 3, "method": "session/set_config_option",
                "params": { "sessionId": "s-1", "configId": "mode", "value": "plan" } })
    );
}

#[tokio::test]
async fn set_mode_without_session_only_caches() {
    let mut h = harness();
    h.client.start(start_params()).await.expect("start");
    // Between start() and session-ready there is no session yet: the mode is
    // only cached, nothing sent. (start() itself resets pending_mode from
    // its params, so caching must happen after start — as in the old code.)
    h.client.set_mode("plan").await.expect("cache");
    let sent = sent_requests(&h);
    assert_eq!(sent.len(), 1, "only initialize has gone out");

    // The cached mode is applied by session_ready after the handshake.
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h.client.recv_once().await.expect("new");
    let sent = sent_requests(&h);
    assert_eq!(sent[2]["method"], "session/set_config_option");
    assert_eq!(sent[2]["params"]["value"], "plan");
}

#[tokio::test]
async fn prompt_assembles_text_and_image_blocks() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h); // drain started/modeChanged

    let tmp = tempfile::tempdir().expect("tmp");
    let png = tmp.path().join("shot.png");
    let jpg = tmp.path().join("photo.jpeg");
    std::fs::write(&png, [1u8, 2, 3]).expect("png");
    std::fs::write(&jpg, [9u8]).expect("jpg");
    let missing = tmp.path().join("gone.gif").to_string_lossy().into_owned();

    h.client
        .prompt(
            "看看这两张图",
            &[
                png.to_string_lossy().into_owned(),
                jpg.to_string_lossy().into_owned(),
                missing, // unreadable: silently skipped
            ],
        )
        .await
        .expect("prompt");
    assert!(h.client.turn_busy());

    let sent = sent_requests(&h);
    let req = sent.last().expect("prompt request");
    assert_eq!(req["method"], "session/prompt");
    assert_eq!(req["params"]["sessionId"], "s-1");
    assert_eq!(
        req["params"]["prompt"],
        json!([
            { "type": "text", "text": "看看这两张图" },
            { "type": "image", "mimeType": "image/png", "data": "AQID" },
            { "type": "image", "mimeType": "image/jpeg", "data": "CQ==" },
        ])
    );

    // Success: stopReason present and defaulted; usage parsed from result.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": req["id"], "result": {
            "usage": { "inputTokens": 1200, "outputTokens": 300, "totalTokens": 1500,
                       "cachedReadTokens": 800 }
        } }))
        .await;
    h.client.recv_once().await.expect("prompt resp");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::TurnFinished {
            stop_reason: "end_turn".into(),
            usage: Some(TurnUsage {
                input_tokens: 1200,
                output_tokens: 300,
                total_tokens: 1500,
                cached_read_tokens: Some(800),
                ..Default::default()
            }),
        }]
    );
    assert!(!h.client.turn_busy());
}

#[tokio::test]
async fn prompt_pure_image_has_no_text_block() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    let tmp = tempfile::tempdir().expect("tmp");
    let gif = tmp.path().join("anim.gif");
    std::fs::write(&gif, [71u8, 73, 70]).expect("gif");

    h.client
        .prompt("", &[gif.to_string_lossy().into_owned()])
        .await
        .expect("prompt");
    let sent = sent_requests(&h);
    let prompt = sent.last().expect("req")["params"]["prompt"].clone();
    assert_eq!(
        prompt,
        json!([{ "type": "image", "mimeType": "image/gif", "data": "R0lG" }]),
        "empty text adds no text block"
    );
}

#[tokio::test]
async fn prompt_skips_images_when_agent_lacks_capability() {
    let mut h = harness();
    // image capability FALSE this time.
    h.client.start(start_params()).await.expect("start");
    h.transport.feed_json(init_result(false, false)).await;
    h.client.recv_once().await.expect("init");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h.client.recv_once().await.expect("new");
    assert!(!h.client.image_supported());

    let tmp = tempfile::tempdir().expect("tmp");
    let png = tmp.path().join("a.png");
    std::fs::write(&png, [1u8]).expect("png");
    h.client
        .prompt("文字", &[png.to_string_lossy().into_owned()])
        .await
        .expect("prompt");
    let sent = sent_requests(&h);
    assert_eq!(
        sent.last().expect("req")["params"]["prompt"],
        json!([{ "type": "text", "text": "文字" }]),
        "images silently skipped without capability"
    );
}

#[tokio::test]
async fn prompt_guards_unready_session_and_reentry() {
    let mut h = harness();
    // No session at all.
    h.client.prompt("hi", &[]).await.expect("guard");
    assert!(sent_requests(&h).is_empty());
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::ProtocolError {
            error: "ACP 会话未就绪或仍在生成".into()
        }]
    );

    // Busy turn: second prompt rejected without a second request.
    drive_to_ready(&mut h).await;
    events(&mut h);
    h.client.prompt("one", &[]).await.expect("first");
    let before = sent_requests(&h).len();
    h.client.prompt("two", &[]).await.expect("reentry guard");
    assert_eq!(sent_requests(&h).len(), before);
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::ProtocolError {
            error: "ACP 会话未就绪或仍在生成".into()
        }]
    );
}

#[tokio::test]
async fn prompt_error_path_order_is_protocol_error_chunk_turn_finished() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);
    h.client.prompt("go", &[]).await.expect("prompt");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 3,
                           "error": { "code": 429, "message": "rate limit exceeded" } }))
        .await;
    h.client.recv_once().await.expect("err");
    // The chat layer's rate-limit detector keys off THIS exact order.
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ProtocolError { error: "[429] rate limit exceeded".into() },
            AcpEvent::MessageChunk { text: "回合失败：[429] rate limit exceeded".into() },
            AcpEvent::TurnFinished { stop_reason: "error".into(), usage: None },
        ]
    );
    assert!(!h.client.turn_busy());

    // Empty error message: no bubble line, order still holds.
    h.client.prompt("go", &[]).await.expect("prompt2");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 4, "error": { "code": -1 } }))
        .await;
    h.client.recv_once().await.expect("err2");
    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ProtocolError { error: String::new() },
            AcpEvent::TurnFinished { stop_reason: "error".into(), usage: None },
        ]
    );
}

#[tokio::test]
async fn cancel_turn_is_a_notification() {
    let mut h = harness();
    h.client.cancel_turn().await.expect("no session: no-op");
    assert!(sent_requests(&h).is_empty());
    drive_to_ready(&mut h).await;
    h.client.cancel_turn().await.expect("cancel");
    let sent = sent_requests(&h);
    assert_eq!(
        sent.last().expect("cancel frame"),
        &json!({ "jsonrpc": "2.0", "method": "session/cancel",
                 "params": { "sessionId": "s-1" } }),
        "notification: no id field"
    );
}

#[tokio::test]
async fn permission_request_emits_event_and_answers_double_nested_outcome() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);

    let params = json!({ "sessionId": "s-1", "toolCall": { "title": "Write file" },
                         "options": [{ "optionId": "allow", "name": "Allow" }] });
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 77,
                           "method": "session/request_permission", "params": params }))
        .await;
    h.client.recv_once().await.expect("perm");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::PermissionRequested { request_id: 77, params }]
    );

    h.client
        .answer_permission(77, "allow", false)
        .await
        .expect("answer selected");
    h.client
        .answer_permission(78, "", true)
        .await
        .expect("answer cancelled");
    let sent = sent_requests(&h);
    assert_eq!(
        sent[sent.len() - 2],
        json!({ "jsonrpc": "2.0", "id": 77,
                "result": { "outcome": { "outcome": "selected", "optionId": "allow" } } })
    );
    assert_eq!(
        sent[sent.len() - 1],
        json!({ "jsonrpc": "2.0", "id": 78,
                "result": { "outcome": { "outcome": "cancelled" } } })
    );
}

#[tokio::test]
async fn fs_read_full_clip_and_error() {
    let mut h = harness();
    let tmp = tempfile::tempdir().expect("tmp");
    let file = tmp.path().join("lines.txt");
    std::fs::write(&file, "l1\nl2\nl3\nl4").expect("file");
    let path = file.to_string_lossy().into_owned();

    // Full read.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 10, "method": "fs/read_text_file",
                           "params": { "sessionId": "s", "path": path } }))
        .await;
    h.client.recv_once().await.expect("read");
    // 1-based line clip: line=2, limit=2 -> l2,l3.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 11, "method": "fs/read_text_file",
                           "params": { "sessionId": "s", "path": path, "line": 2, "limit": 2 } }))
        .await;
    h.client.recv_once().await.expect("read clip");
    // Missing file -> -32000.
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 12, "method": "fs/read_text_file",
                           "params": { "sessionId": "s", "path": r"Z:\nope\missing.txt" } }))
        .await;
    h.client.recv_once().await.expect("read err");

    let sent = sent_requests(&h);
    assert_eq!(
        sent[0],
        json!({ "jsonrpc": "2.0", "id": 10, "result": { "content": "l1\nl2\nl3\nl4" } })
    );
    assert_eq!(
        sent[1],
        json!({ "jsonrpc": "2.0", "id": 11, "result": { "content": "l2\nl3" } })
    );
    assert_eq!(
        sent[2],
        json!({ "jsonrpc": "2.0", "id": 12,
                "error": { "code": -32000, "message": "无法读取: Z:\\nope\\missing.txt" } })
    );
}

#[tokio::test]
async fn fs_write_mkpath_truncate_and_error() {
    let mut h = harness();
    let tmp = tempfile::tempdir().expect("tmp");
    let nested = tmp.path().join("a/b/c/out.txt");
    let path = nested.to_string_lossy().into_owned();

    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 20, "method": "fs/write_text_file",
                           "params": { "sessionId": "s", "path": path, "content": "中文内容" } }))
        .await;
    h.client.recv_once().await.expect("write");
    assert_eq!(std::fs::read_to_string(&nested).expect("file"), "中文内容");

    // Unwritable target: a directory used as the file path.
    let dir = tmp.path().to_string_lossy().into_owned();
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 21, "method": "fs/write_text_file",
                           "params": { "sessionId": "s", "path": dir, "content": "x" } }))
        .await;
    h.client.recv_once().await.expect("write err");

    let sent = sent_requests(&h);
    assert_eq!(sent[0], json!({ "jsonrpc": "2.0", "id": 20, "result": {} }));
    assert_eq!(sent[1]["id"], 21);
    assert_eq!(sent[1]["error"]["code"], -32000);
    assert!(sent[1]["error"]["message"]
        .as_str()
        .expect("msg")
        .starts_with("无法写入: "));
}

#[tokio::test]
async fn unknown_method_with_id_gets_32601_notification_ignored() {
    let mut h = harness();
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 30, "method": "terminal/create",
                           "params": {} }))
        .await;
    h.client.recv_once().await.expect("unknown");
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "method": "weird/notify", "params": {} }))
        .await;
    h.client.recv_once().await.expect("notify");
    let sent = sent_requests(&h);
    assert_eq!(
        sent,
        vec![json!({ "jsonrpc": "2.0", "id": 30,
                     "error": { "code": -32601, "message": "Method not found: terminal/create" } })],
        "id-less unknown notifications send nothing"
    );
}

#[tokio::test]
async fn session_update_chunks_and_tool_normalization() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);

    // thought + message chunks.
    h.transport
        .feed_json(update_frame("agent_thought_chunk", json!({ "content": { "text": "想…" } })))
        .await;
    h.client.recv_once().await.expect("thought");
    h.transport
        .feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "答" } })))
        .await;
    h.client.recv_once().await.expect("msg");

    // Level 1 unwrap: no top-level toolCallId, nested toolCall object;
    // level 2 fills name from title.
    h.transport
        .feed_json(update_frame(
            "tool_call",
            json!({ "toolCall": { "toolCallId": "t1", "title": "Agent", "status": "pending" } }),
        ))
        .await;
    h.client.recv_once().await.expect("tool_call");
    // Level 2 from kind on a flat tool_call_update.
    h.transport
        .feed_json(update_frame(
            "tool_call_update",
            json!({ "toolCallId": "t1", "kind": "edit", "status": "completed" }),
        ))
        .await;
    h.client.recv_once().await.expect("tool_call_update");

    // available_commands_update / plan are now wired: they emit events.
    h.transport
        .feed_json(update_frame("available_commands_update", json!({ "availableCommands": [] })))
        .await;
    h.client.recv_once().await.expect("commands");
    h.transport
        .feed_json(update_frame("plan", json!({ "entries": [] })))
        .await;
    h.client.recv_once().await.expect("plan");

    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::ThoughtChunk { text: "想…".into() },
            AcpEvent::MessageChunk { text: "答".into() },
            AcpEvent::ToolCall {
                call: json!({ "toolCallId": "t1", "title": "Agent",
                              "status": "pending", "name": "Agent" }),
            },
            AcpEvent::ToolCallUpdate {
                // The whole update map is forwarded (old toVariantMap), so
                // the sessionUpdate key rides along; name filled from kind.
                update: json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
                                "kind": "edit", "status": "completed", "name": "edit" }),
            },
            AcpEvent::AvailableCommands { commands: vec![] },
            AcpEvent::Plan { entries: vec![] },
        ]
    );
}

#[tokio::test]
async fn bad_json_lines_are_dropped_not_fatal() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);
    h.transport.feed_line("not json at all").await;
    h.client.recv_once().await.expect("noise tolerated");
    h.transport.feed_line("[1,2,3]").await;
    h.client.recv_once().await.expect("non-object tolerated");
    assert!(events(&mut h).is_empty());
    // Client still alive: a real chunk arrives normally afterwards.
    h.transport
        .feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "ok" } })))
        .await;
    h.client.recv_once().await.expect("alive");
    assert_eq!(
        events(&mut h),
        vec![AcpEvent::MessageChunk { text: "ok".into() }]
    );
}

#[tokio::test]
async fn unknown_response_ids_are_silently_ignored() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);
    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 999, "result": { "stopReason": "x" } }))
        .await;
    h.client.recv_once().await.expect("ignored");
    assert!(events(&mut h).is_empty());
}

#[tokio::test]
async fn eof_emits_process_exited_with_code() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);
    h.transport.feed_eof(7).await;
    let alive = h.client.recv_once().await.expect("eof");
    assert!(!alive);
    assert_eq!(events(&mut h), vec![AcpEvent::ProcessExited { code: 7 }]);
}

#[tokio::test]
async fn usage_session_info_and_mode_updates_are_forwarded() {
    let mut h = harness();
    drive_to_ready(&mut h).await;
    events(&mut h);

    h.transport
        .feed_json(update_frame("usage_update", json!({
            "usage": { "inputTokens": 100, "outputTokens": 20, "totalTokens": 120 }
        })))
        .await;
    h.client.recv_once().await.expect("usage");
    h.transport
        .feed_json(update_frame("session_info_update", json!({ "title": "修 bug" })))
        .await;
    h.client.recv_once().await.expect("info");
    h.transport
        .feed_json(update_frame("current_mode_update", json!({ "currentModeId": "yolo" })))
        .await;
    h.client.recv_once().await.expect("mode");
    // Broken usage payloads are skipped without an event.
    h.transport
        .feed_json(update_frame("usage_update", json!({ "usage": "oops" })))
        .await;
    h.client.recv_once().await.expect("bad usage");

    assert_eq!(
        events(&mut h),
        vec![
            AcpEvent::UsageUpdate {
                usage: TurnUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    total_tokens: 120,
                    ..Default::default()
                },
            },
            AcpEvent::SessionInfo {
                title: Some("修 bug".into()),
            },
            AcpEvent::ModeChanged { mode: "yolo".into() },
        ]
    );
}

#[tokio::test]
async fn available_commands_survive_load_replay() {
    let mut h = harness();
    let params = StartParams {
        resume_session_id: "old-9".to_string(),
        ..start_params()
    };
    h.client.start(params).await.expect("start");
    h.transport.feed_json(init_result(true, false)).await;
    h.client.recv_once().await.expect("init");

    // During replay: commands stored but NOT emitted; message chunks dropped.
    h.transport
        .feed_json(update_frame("available_commands_update", json!({
            "availableCommands": [{ "name": "tasks", "description": "后台任务" }]
        })))
        .await;
    h.client.recv_once().await.expect("replay commands");
    h.transport
        .feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "history" } })))
        .await;
    h.client.recv_once().await.expect("replay chunk");
    assert!(events(&mut h).is_empty(), "replay emits nothing");

    h.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": {} }))
        .await;
    h.client.recv_once().await.expect("load ok");
    let evs = events(&mut h);
    assert!(
        evs.contains(&AcpEvent::AvailableCommands {
            commands: vec![json!({ "name": "tasks", "description": "后台任务" })],
        }),
        "session_ready re-emits replayed commands: {evs:?}"
    );
}

#[tokio::test]
async fn auth_required_triggers_authenticate_and_resend() {
    let mut h2 = harness();
    h2.client.start(start_params()).await.expect("start");
    let mut init = init_result(false, true);
    init["result"]["authMethods"] = json!([{ "id": "oauth", "name": "OAuth" }]);
    h2.transport.feed_json(init).await;
    h2.client.recv_once().await.expect("init");
    h2.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    h2.client.recv_once().await.expect("new");
    events(&mut h2);

    h2.client.prompt("hello", &[]).await.expect("prompt");
    let sent = sent_requests(&h2);
    let prompt_id = sent.last().unwrap()["id"].as_i64().unwrap();

    // Prompt rejected with -32002: authenticate goes out, no error events.
    h2.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": prompt_id,
                           "error": { "code": -32002, "message": "auth required" } }))
        .await;
    h2.client.recv_once().await.expect("auth error");
    assert!(events(&mut h2).is_empty(), "no turn error while authenticating");
    let sent = sent_requests(&h2);
    assert_eq!(
        sent.last().unwrap(),
        &json!({ "jsonrpc": "2.0", "id": prompt_id + 1, "method": "authenticate",
                 "params": { "methodId": "oauth" } })
    );

    // authenticate succeeds: the same prompt is resent verbatim.
    h2.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": prompt_id + 1, "result": {} }))
        .await;
    h2.client.recv_once().await.expect("auth ok");
    let sent = sent_requests(&h2);
    let resent = sent.last().unwrap();
    assert_eq!(resent["method"], json!("session/prompt"));
    assert_eq!(resent["params"]["prompt"], json!([{ "type": "text", "text": "hello" }]));

    // Turn completes normally.
    h2.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": resent["id"].as_i64().unwrap(),
                           "result": { "stopReason": "end_turn" } }))
        .await;
    h2.client.recv_once().await.expect("turn");
    assert_eq!(
        events(&mut h2),
        vec![AcpEvent::TurnFinished {
            stop_reason: "end_turn".into(),
            usage: None,
        }]
    );

    // A second -32002 is NOT retried (one attempt per spawn).
    h2.client.prompt("again", &[]).await.expect("prompt2");
    let pid2 = sent_requests(&h2).last().unwrap()["id"].as_i64().unwrap();
    h2.transport
        .feed_json(json!({ "jsonrpc": "2.0", "id": pid2,
                           "error": { "code": -32002, "message": "auth required" } }))
        .await;
    h2.client.recv_once().await.expect("second auth error");
    let evs = events(&mut h2);
    assert!(
        evs.iter().any(|e| matches!(e, AcpEvent::TurnFinished { stop_reason, .. } if stop_reason == "error")),
        "second auth failure surfaces as a turn error: {evs:?}"
    );
}
}
