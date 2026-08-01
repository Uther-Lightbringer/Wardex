// Per-session runtime: one actor task owning the ACP client, the turn state
// machine, the 50ms coalescing flush, the send queue, rate-limit retry,
// interrupted-turn resume and sub-agent tracking. Ported function-by-function
// from the Runtime half of ChatController.cpp (see docs/features/chat.md §7
// and docs/acp-protocol.md §9 for the behavior contract).
//
// Driving model (acp-protocol.md §9 / client.rs header): the actor select!s
// over its command channel, the ACP event channel and client.recv_once().
// StdioTransport's framing state survives a cancelled recv (the framer lives
// in the transport, not the future), so select! cancellation is safe.
//
// Delayed actions (flush tick, retry countdown, cancel/guide timeouts) are
// plain tokio sleeps that post back into the command channel, guarded by
// generation counters — the Rust equivalent of the old single-shot QTimers
// re-checking runtime state on fire.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::acp::{AcpError, AcpEvent};
use crate::acp::events::TurnUsage;
use crate::chat::driver::{ClientDriver, SessionLaunch, Spawner};
use crate::chat::wire::{ArchiveKind, ArchiveReader};
use crate::provider;
use crate::store::agents::Agent;
use crate::store::json::now_ms;
use crate::store::sessions::MessageRow;
use crate::store::usage::UsageRecord;
use crate::store::StoreRegistry;

// ---- budget constants (performance.md §3, ChatController.h:47-168) ----

pub const K_MAX_QUEUE_SIZE: usize = 10;
pub const K_MAX_PARALLEL_ACP: usize = 3;
/// Streaming buffer keeps only this tail after each flush (resume anchor +
/// emptiness checks); the full text lives in the session store.
pub const K_STREAM_BUFFER_KEEP: usize = 2000;
/// Tool payload string fields are truncated to this size before entering the
/// message model (performance.md §3: 工具 payload 内存 64KB/条).
pub const K_PAYLOAD_TRUNCATE: usize = 64 * 1024;
pub const K_MAX_CONTINUE_RETRIES: u32 = 2;
pub const K_MAX_RATE_LIMIT_RETRIES: u32 = 3;
pub const K_RATE_LIMIT_BASE_DELAY_SEC: u64 = 20;
pub const K_RATE_LIMIT_MAX_DELAY_SEC: u64 = 300;
pub const K_CANCEL_TIMEOUT_MS: u64 = 2500;
pub const K_GUIDE_TIMEOUT_MS: u64 = 800;
pub const K_FLUSH_MS: u64 = 50;
/// Backlog above 64KB stretches the coalescing interval to 250ms.
pub const K_FLUSH_LONG_MS: u64 = 250;
pub const K_FLUSH_LONG_THRESHOLD: usize = 64 * 1024;
const PLACEHOLDER: &str = "…";
const INTERRUPTED_MARK: &str = "（已中断）";

/// 会话首条用户消息发往 ACP 前注入的引导语：告知 agent 本会话挂载了内置
/// wardex-reminder MCP 工具（build_launch 注入）。只进 prompt 文本，不进
/// 聊天显示的用户行；判定用 store 里的用户消息计数（resume 的老会话不会
/// 重复注入）。
pub const REMINDER_GUIDE_PREFIX: &str = "[Wardex 提示] 本会话已挂载 wardex-reminder MCP 工具（set_reminder/cancel_reminder/list_reminders）。当你想稍后主动跟进、或用户说\"过会提醒我/晚点通知我\"时，调用 set_reminder(minutes, content) 设置提醒；到点后系统会自动把 content 作为新消息发给你。当你自己启动了后台任务/子 Agent、需要等它们完成后继续时，也应主动调用 set_reminder 设置短时提醒（如 1 分钟）来唤醒自己。";

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Rate-limit detection (ChatController.cpp:50-64): agent CLIs surface HTTP
/// 429 / quota exhaustion as JSON-RPC error text on the prompt response.
pub fn is_rate_limit_error(err: &str) -> bool {
    if err.is_empty() {
        return false;
    }
    let e = err.to_lowercase();
    e.contains("429")
        || e.contains("rate limit")
        || e.contains("ratelimit")
        || e.contains("too many requests")
        || e.contains("quota")
        || e.contains("resource exhausted")
}

/// Exponential backoff for attempt N (1-based): 20 -> 40 -> 80s, cap 300s.
pub fn retry_delay_secs(attempt: u32) -> u64 {
    let mut delay = K_RATE_LIMIT_BASE_DELAY_SEC;
    for _ in 1..attempt {
        delay = delay.saturating_mul(2);
    }
    delay.min(K_RATE_LIMIT_MAX_DELAY_SEC)
}

/// Synthetic continuation prompt (ChatController.cpp:1221-1225). NOT appended
/// to local history; the tail anchors the model even if session/load fell
/// back to a fresh agent-side session.
pub fn continuation_prompt(tail: &str) -> String {
    format!(
        "上一条回复因连接中断被截断。请紧接着已输出的内容继续，不要重复已输出的部分，不要重新开头，不要解释。已输出内容的结尾片段：\n…{tail}"
    )
}

/// Attachment split rule (ChatController.cpp:947-953).
pub fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or_default().to_lowercase();
    matches!(ext.as_ref(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
}

/// Sub-agent tool names, case-insensitive (ChatController.cpp:497-502).
pub fn is_subagent_tool_name(name: &str) -> bool {
    let n = name.to_lowercase();
    matches!(n.as_ref(), "agent" | "agentswarm" | "task" | "spawn_agent")
}

/// Input JSON streams as cumulative snapshots on kimi (last block wins) but
/// other adapters may stream deltas — try the last block, then the
/// concatenation (ChatController.cpp:507-528).
pub fn parse_tool_input(tool: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut last = String::new();
    let mut all = String::new();
    if let Some(Value::Array(content)) = tool.get("content") {
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("content") {
                continue;
            }
            let t = block
                .get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            all.push_str(t);
            if !t.is_empty() {
                last = t.to_string();
            }
        }
    }
    let parse = |s: &str| -> Option<Map<String, Value>> {
        match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(o)) if !o.is_empty() => Some(o),
            _ => None,
        }
    };
    parse(&last).or_else(|| parse(&all))
}

/// Per-subagent outcome summary from rawOutput (ChatController.cpp:593-623):
/// swarm results count outcome="…" occurrences; single agents show the
/// actual_subagent_type line.
pub fn subagent_summary(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("<agent_swarm_result>") {
        let mut total = 0usize;
        let mut ok = 0usize;
        let mut from = 0usize;
        while let Some(p) = raw[from..].find("outcome=\"") {
            let start = from + p + "outcome=\"".len();
            total += 1;
            if raw[start..].starts_with("completed") {
                ok += 1;
            }
            from = start;
        }
        if total > 0 {
            return Some(format!("完成 {ok}/{total}"));
        }
        return None;
    }
    const KEY: &str = "actual_subagent_type:";
    if let Some(p) = raw.find(KEY) {
        let rest = &raw[p + KEY.len()..];
        let line = rest.split('\n').next().unwrap_or_default().trim();
        if !line.is_empty() {
            return Some(line.to_string());
        }
    }
    None
}

/// rawOutput normalization: kimi sends a plain string, claude-code-acp sends
/// an array of text blocks ([{"type":"text","text":"…"}]). Concatenate block
/// texts so downstream parsers see one string either way.
pub fn tool_raw_text(tool: &Map<String, Value>) -> String {
    match tool.get("rawOutput") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Provider-dispatched outcome summary. kimi uses subagent_summary
/// (swarm outcome counts / actual_subagent_type line). claude-code-acp Task
/// output is "<report>\nagentId: xxx (for resuming…)\n<usage>…": the report
/// body (elided) when non-empty, otherwise the usage duration as a bare
/// fallback (a subagent that produced no text still gets a useful summary).
pub fn subagent_summary_for(provider: &str, raw: &str) -> Option<String> {
    if provider == "claude" {
        let report = raw.split("agentId:").next().unwrap_or_default().trim();
        if !report.is_empty() {
            return Some(elide(report, 80));
        }
        const KEY: &str = "duration_ms:";
        if let Some(p) = raw.find(KEY) {
            let num: String = raw[p + KEY.len()..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(ms) = num.parse::<u64>() {
                return Some(format!("耗时 {:.1}s", ms as f64 / 1000.0));
            }
        }
        return None;
    }
    subagent_summary(raw)
}

/// Agent id(s) per provider: kimi `agent_id: agent-0` lines, claude
/// `agentId: a807b73` (hex token, trailing remark dropped).
pub fn subagent_agent_ids_for(provider: &str, raw: &str) -> Vec<String> {
    if provider == "claude" {
        const KEY: &str = "agentId:";
        let mut ids = Vec::new();
        for line in raw.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix(KEY) {
                let id: String = rest
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if !id.is_empty() && ids.iter().all(|x: &String| x != &id) {
                    ids.push(id);
                }
            }
        }
        return ids;
    }
    subagent_agent_ids(raw)
}

/// CLI-side agent id(s) from the Agent tool's rawOutput — the `agent_id:`
/// line(s) (e.g. "agent-0"). This is the directory name of the sub-agent's
/// on-disk wire transcript (~/.kimi-code/.../agents/<agentId>/wire.jsonl),
/// used by the dialog's 执行过程 section. Swarm results may carry several.
pub fn subagent_agent_ids(raw: &str) -> Vec<String> {
    const KEY: &str = "agent_id:";
    let mut ids = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        // ids only appear in the small header block; stop scanning early
        if i > 40 {
            break;
        }
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(KEY) {
            let id = rest.trim();
            if !id.is_empty() && ids.iter().all(|x| x != &id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

/// Truncate oversized string payload fields before they enter the in-memory
/// model (performance.md §3). NOTE: with segments as the single source of
/// truth the rewritten JSONL carries the same truncated form — this diverges
/// from "全文只落盘" (would need a dual representation in the store; a
/// documented follow-up, matching the phase-1d scope).
pub fn truncate_tool_payloads(tool: &mut Map<String, Value>) {
    for key in ["rawInput", "rawOutput", "content", "output", "arguments"] {
        let Some(Value::String(s)) = tool.get(key) else {
            continue;
        };
        if s.chars().count() <= K_PAYLOAD_TRUNCATE {
            continue;
        }
        let head: String = s.chars().take(K_PAYLOAD_TRUNCATE).collect();
        tool.insert(key.to_string(), Value::String(format!("{head}\n…（已截断）")));
    }
}

/// Char-based right(n) (QString::right equivalent, UTF-8 safe).
pub fn right_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    s.chars().skip(count - n).collect()
}

/// toolFromUpdate (ChatController.cpp:466-483): unwrap a nested toolCall
/// object and backfill `name` from title/kind.
pub fn tool_from_update(u: &Value) -> Map<String, Value> {
    let mut out = match u {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    if !out.contains_key("toolCallId") {
        if let Some(Value::Object(nested)) = out.get("toolCall") {
            out = nested.clone();
        }
    }
    if !out.contains_key("name") {
        let title = out.get("title").and_then(Value::as_str).unwrap_or_default();
        if !title.is_empty() {
            out.insert("name".to_string(), Value::String(title.to_string()));
        } else if let Some(kind) = out.get("kind").cloned() {
            out.insert("name".to_string(), kind);
        }
    }
    out
}

/// Middle-elide for titles (ChatController.cpp:530-533 elide, left(n)+"…").
fn elide(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Event sink: Tauri emit in production, recorder in tests.
// ---------------------------------------------------------------------------

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
    /// Desktop notification for the human (sub-agent completion etc.).
    /// Default no-op: tests and non-desktop sinks ignore it.
    fn notify(&self, _title: &str, _body: &str) {}
}

// ---------------------------------------------------------------------------
// Sub-agent tracking entry (features/chat.md §4.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentEntry {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub children: usize,
    pub child_names: Vec<String>,
    pub summary: String,
    /// Full task brief (the prompt / swarm template+items as pretty JSON),
    /// shown in the detail dialog. Capped at 32KB.
    pub input: String,
    /// Final report (rawOutput), filled on completion. Capped at 64KB.
    pub output: String,
    /// CLI-side agent id(s) from rawOutput (`agent_id:` lines) — names of the
    /// on-disk wire transcript dirs, for the dialog's 执行过程 section.
    pub agent_ids: Vec<String>,
    pub started_at: i64,
    pub finished_at: i64,
    /// Last time any tool_call(_update) touched this entry — stuck detection.
    pub last_update: i64,
}

// ---------------------------------------------------------------------------
// Cross-runtime shared state
// ---------------------------------------------------------------------------

/// What enforceProcessCap / the manager need to know about a runtime without
/// talking to its actor (ChatController Runtime fields busy/lastActivity).
#[derive(Debug, Clone, Default)]
pub struct RuntimeSnap {
    pub busy: bool,
    pub acp_running: bool,
    pub queue_len: usize,
    /// Queue contents — the frontend mirror rebuilds from this after a
    /// session switch (its local mirror does not survive switching away).
    pub queue: Vec<String>,
    pub agent_id: String,
    pub image_supported: bool,
    pub last_activity_ms: i64,
    /// Full acp://permission payload while a request awaits an answer — the
    /// dialog survives a session switch by re-pulling this (the live event
    /// only reaches whichever session was active when it fired).
    pub perm_pending: Option<Value>,
}

pub struct RuntimeEntry {
    pub tx: mpsc::Sender<RuntimeCmd>,
    pub snap: Arc<Mutex<RuntimeSnap>>,
}

pub type SharedRegistry = Arc<Mutex<HashMap<String, RuntimeEntry>>>;

/// Manager-level shared flags the actors consult (active session, unread).
#[derive(Debug, Default)]
pub struct ManagerShared {
    pub active_id: String,
    pub unread: std::collections::HashSet<String>,
}

pub(crate) fn lock_ok<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Poisoned mutex: recover the guard rather than panicking (stability red
    // line — a poisoned lock must not take the app down).
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// ChatController.cpp:293-317 — before starting a new ACP process: at cap,
/// stop the least-recently-active IDLE process; all busy → allow overshoot.
pub fn enforce_process_cap(registry: &SharedRegistry, exempt: &str) {
    let mut victims: Vec<mpsc::Sender<RuntimeCmd>> = Vec::new();
    {
        let reg = lock_ok(registry);
        let mut running = reg.values().filter(|e| lock_ok(&e.snap).acp_running).count();
        while running >= K_MAX_PARALLEL_ACP {
            let victim = reg
                .iter()
                .filter(|(id, e)| {
                    let s = lock_ok(&e.snap);
                    id.as_str() != exempt && !s.busy && s.acp_running
                })
                .min_by_key(|(_, e)| lock_ok(&e.snap).last_activity_ms)
                .map(|(_, e)| (e.tx.clone(), e.snap.clone()));
            let Some((tx, snap)) = victim else {
                break; // everyone busy — temporary overshoot
            };
            lock_ok(&snap).acp_running = false; // optimistic, avoids double-pick
            victims.push(tx);
            running -= 1;
        }
    }
    for tx in victims {
        let _ = tx.try_send(RuntimeCmd::StopProcess);
    }
}

// ---------------------------------------------------------------------------
// Runtime commands (manager / Tauri -> actor, plus self-posted timer ticks)
// ---------------------------------------------------------------------------

/// Result of a user-initiated send, reported back through the oneshot ack so
/// the caller (and therefore the composer) knows whether the message started
/// a turn, landed in the queue, or was rejected.
#[derive(Debug)]
pub enum SendOutcome {
    Started,
    Enqueued,
    Rejected(String),
}

/// A queued user message. Attachments ride along as already-persisted media
/// paths (the composer saves them before send), so waiting in the queue
/// cannot stale or lose them.
#[derive(Clone, Debug, Default)]
struct QueuedItem {
    text: String,
    images: Vec<String>,
    display: Vec<String>,
    /// Row marker ("reminder"); empty for user-typed messages.
    kind: String,
}

#[derive(Debug)]
pub enum RuntimeCmd {
    /// Already attachment-split: text to send (inline "[附件]" lines folded
    /// in by the manager), image block paths, display-only attachment list.
    /// `ack` is Some only for user-initiated sends (internal drain/guide
    /// re-sends pass None).
    SendPrompt {
        text: String,
        images: Vec<String>,
        display: Vec<String>,
        /// Row marker for non-interactive sends ("reminder"; empty = normal).
        kind: String,
        ack: Option<tokio::sync::oneshot::Sender<SendOutcome>>,
    },
    Cancel,
    RetryCancel,
    GuideAt(usize),
    RemoveQueueAt(usize),
    ClearQueue,
    AnswerPermission { option_id: String, cancelled: bool },
    /// Raw WarDex mode id; the actor maps it through the provider registry.
    SetMode(String),
    /// ACP config option picker (kimi: "thinking" / "model"); passthrough.
    SetConfigOption { config_id: String, value: String },
    /// Chat-page pick of a model the CLI picker does not advertise (per-agent
    /// endpoint model): persist to the agent record and respawn so
    /// build_launch injects it via KIMI_MODEL_* env.
    SetModel(String),
    /// New agent snapshot; the actor persists meta and handles the
    /// same/cross-provider acpSessionId rule itself (it owns the old agent).
    SwitchAgent(Box<Agent>),
    /// Warm-up / reconnect (spawn + handshake; pendingPrompt fires on ready).
    EnsureAcp,
    /// Parallel-cap eviction: drop the process, keep everything else.
    StopProcess,
    /// Runtime destroyed (session closed/deleted): settle a busy turn, exit.
    Shutdown,
    FlushTick,
    RetryTick(u64),
    /// Re-read reminders.json (MCP 子进程/命令层可能刚写过) 并重排定时器。
    RemindersReload,
    /// 提醒到点定时器回投（gen 校验作废旧定时器，仿 RetryTick）。
    ReminderTick(u64),
    CancelTimeout(u64),
    GuideTimeout(u64),
}

// ---------------------------------------------------------------------------
// The actor
// ---------------------------------------------------------------------------

pub(crate) struct Actor {
    session_id: String,
    stores: Arc<Mutex<StoreRegistry>>,
    sink: Arc<dyn EventSink>,
    registry: SharedRegistry,
    shared: Arc<Mutex<ManagerShared>>,
    spawner: Spawner,
    client: Option<Box<dyn ClientDriver>>,
    ev_tx: mpsc::Sender<AcpEvent>,
    ev_rx: mpsc::Receiver<AcpEvent>,
    cmd_rx: mpsc::Receiver<RuntimeCmd>,
    self_tx: mpsc::Sender<RuntimeCmd>,
    snap: Arc<Mutex<RuntimeSnap>>,
    agent: Agent,
    /// Model the current turn is running on: the agent's configured default,
    /// refreshed from the CLI's model picker (configOptions currentValue).
    current_model: String,
    /// Last configOptions batch forwarded to the frontend; current_mode_update
    /// patches the "mode" picker's currentValue and re-emits it.
    last_config_options: Vec<Value>,
    /// usage_update notifications seen during the current turn; used when the
    /// prompt result carries no usage (before the archive backfill).
    turn_usage_live: Option<TurnUsage>,
    /// 档案用量补源（kimi/claude/codex，见 chat/wire.rs），Started 时按
    /// provider 定位；其他 provider 为 None。
    archive_usage: Option<ArchiveReader>,

    busy: bool,
    user_stop: bool,
    acp_ready: bool,
    pending_prompt: Option<(String, Vec<String>)>,
    pending_guide: Option<QueuedItem>,
    queue: VecDeque<QueuedItem>,
    /// The in-flight spawn started a brand-new agent-side session (no
    /// session/load resume id). The agent's default model is only applied to
    /// fresh sessions; resumed ones keep their remembered model.
    fresh_launch: bool,
    /// Default-model application happens once per spawn, on the first
    /// ConfigOptions batch after Started.
    model_applied: bool,
    assistant_buf: String,
    thinking_buf: String,
    pending_content: String,
    pending_thinking: String,
    flush_pending: bool,
    retry_prompt: Option<String>,
    retry_attempt: u32,
    retry_countdown: u64,
    retry_active: bool,
    retry_gen: u64,
    reminder_gen: u64,
    continue_retries: u32,
    last_turn_error: String,
    perm_request_id: Option<i64>,
    subagents: Vec<SubagentEntry>,
    /// Ids of sub-agents still pending/in_progress at a normal turn end —
    /// possibly background tasks whose completion events arrive later. When
    /// the last of them reports done outside a turn, the agent is woken with
    /// a follow-up prompt (see maybe_wake_for_subagent_batch).
    bg_pending: HashSet<String>,
    /// Batch wake-up already posted for the current bg_pending set (armed
    /// again when a new turn starts).
    bg_wake_sent: bool,
    turn_gen: u64,
    guide_gen: u64,
    last_error: String,
}

/// Spawn the per-session actor task; returns the command sender.
#[allow(clippy::too_many_arguments)] // actor wiring is inherently 8-tuple; a builder would add noise
pub(crate) fn spawn_actor(
    session_id: &str,
    agent: Agent,
    stores: Arc<Mutex<StoreRegistry>>,
    sink: Arc<dyn EventSink>,
    registry: SharedRegistry,
    shared: Arc<Mutex<ManagerShared>>,
    spawner: Spawner,
    snap: Arc<Mutex<RuntimeSnap>>,
) -> mpsc::Sender<RuntimeCmd> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<RuntimeCmd>(64);
    let (ev_tx, ev_rx) = mpsc::channel::<AcpEvent>(256);
    let self_tx = cmd_tx.clone();
    let current_model = agent.model.clone();
    let mut actor = Actor {
        session_id: session_id.to_string(),
        agent,
        current_model,
        last_config_options: Vec::new(),
        turn_usage_live: None,
        archive_usage: None,
        stores,
        sink,
        registry,
        shared,
        spawner,
        client: None,
        ev_tx,
        ev_rx,
        cmd_rx,
        self_tx,
        snap,
        busy: false,
        user_stop: false,
        acp_ready: false,
        fresh_launch: false,
        model_applied: false,
        pending_prompt: None,
        pending_guide: None,
        queue: VecDeque::new(),
        assistant_buf: String::new(),
        thinking_buf: String::new(),
        pending_content: String::new(),
        pending_thinking: String::new(),
        flush_pending: false,
        retry_prompt: None,
        retry_attempt: 0,
        retry_countdown: 0,
        retry_active: false,
        retry_gen: 0,
        reminder_gen: 0,
        continue_retries: 0,
        last_turn_error: String::new(),
        perm_request_id: None,
        subagents: Vec::new(),
        bg_pending: HashSet::new(),
        bg_wake_sent: false,
        turn_gen: 0,
        guide_gen: 0,
        last_error: String::new(),
    };
    tokio::spawn(async move { actor.run().await });
    // 启动即 reload 一次提醒：重启恢复时过期的提醒会立即触发。
    let _ = cmd_tx.try_send(RuntimeCmd::RemindersReload);
    cmd_tx
}

/// recv future for the select!: pending forever when no client is alive.
async fn recv_step(client: &mut Option<Box<dyn ClientDriver>>) -> Result<bool, AcpError> {
    match client.as_mut() {
        Some(c) => c.recv_once().await,
        None => std::future::pending::<Result<bool, AcpError>>().await,
    }
}

impl Actor {
    async fn run(&mut self) {
        loop {
            tokio::select! {
                biased;
                cmd = self.cmd_rx.recv() => {
                    let Some(cmd) = cmd else { return };
                    if matches!(cmd, RuntimeCmd::Shutdown) {
                        self.handle_shutdown();
                        return;
                    }
                    self.handle_cmd(cmd).await;
                }
                ev = self.ev_rx.recv() => {
                    if let Some(ev) = ev {
                        self.handle_event(ev).await;
                    }
                }
                r = recv_step(&mut self.client) => {
                    self.handle_recv(r);
                }
            }
        }
    }

    // ---- recv lifecycle ----

    fn handle_recv(&mut self, r: Result<bool, AcpError>) {
        match r {
            Ok(true) => {}
            Ok(false) => {
                // EOF: process exited; ProcessExited was already emitted into
                // the event channel by recv_once and is handled next turn.
                self.client = None;
                self.acp_ready = false;
                self.snap().acp_running = false;
            }
            Err(e) => {
                log::warn!("chat[{}] transport error: {e}", self.session_id);
                self.client = None;
                self.acp_ready = false;
                self.snap().acp_running = false;
            }
        }
    }

    fn handle_shutdown(&mut self) {
        if self.busy {
            self.mark_interrupted();
        }
        self.client = None;
        self.snap().acp_running = false;
        lock_ok(&self.stores).sessions.release_session(&self.session_id);
    }

    // ---- commands ----

    async fn handle_cmd(&mut self, cmd: RuntimeCmd) {
        match cmd {
            RuntimeCmd::SendPrompt {
                text,
                images,
                display,
                kind,
                ack,
            } => self.on_send_prompt(text, images, display, kind, ack).await,
            RuntimeCmd::Cancel => self.on_cancel().await,
            RuntimeCmd::RetryCancel => self.cancel_retry(true),
            RuntimeCmd::GuideAt(index) => self.on_guide_at(index).await,
            RuntimeCmd::RemoveQueueAt(index) => {
                if index < self.queue.len() {
                    self.queue.remove(index);
                    self.sync_snap();
                    self.emit_status(None);
                }
            }
            RuntimeCmd::ClearQueue => {
                if !self.queue.is_empty() {
                    self.queue.clear();
                    self.sync_snap();
                    self.emit_status(None);
                }
            }
            RuntimeCmd::AnswerPermission {
                option_id,
                cancelled,
            } => self.respond_permission(&option_id, cancelled).await,
            RuntimeCmd::SetMode(mode) => {
                let mapped = self.mapped_mode(&mode);
                if let Some(c) = self.client.as_mut() {
                    if let Err(e) = c.set_mode(&mapped).await {
                        log::warn!("chat[{}] set_mode failed: {e}", self.session_id);
                    }
                }
            }
            RuntimeCmd::SetConfigOption { config_id, value } => {
                if let Some(c) = self.client.as_mut() {
                    if let Err(e) = c.set_config_option(&config_id, &value).await {
                        log::warn!("chat[{}] set_config_option failed: {e}", self.session_id);
                    }
                }
            }
            RuntimeCmd::SwitchAgent(agent) => self.on_switch_agent(*agent).await,
            RuntimeCmd::SetModel(model) => self.on_set_model(model).await,
            RuntimeCmd::EnsureAcp => self.ensure_acp().await,
            RuntimeCmd::StopProcess => {
                self.client = None;
                self.acp_ready = false;
                self.snap().acp_running = false;
                self.emit_status(None);
            }
            RuntimeCmd::Shutdown => { /* intercepted in run() before dispatch */ }
            RuntimeCmd::FlushTick => {
                if self.flush_pending {
                    self.flush_stream_buffers();
                }
            }
            RuntimeCmd::RetryTick(gen) => self.retry_tick(gen).await,
            RuntimeCmd::RemindersReload => self.reminders_reload(),
            RuntimeCmd::ReminderTick(gen) => self.reminder_tick(gen).await,
            RuntimeCmd::CancelTimeout(gen) => {
                // 2500ms force-kill fallback (ChatController.cpp:1088-1101).
                if gen == self.turn_gen && self.busy && self.user_stop {
                    self.client = None;
                    self.acp_ready = false;
                    self.snap().acp_running = false;
                    self.mark_interrupted();
                    self.emit_turn("interrupted", "cancelled", None);
                    self.finish_reply();
                    self.user_stop = false;
                }
            }
            RuntimeCmd::GuideTimeout(gen) => self.guide_timeout(gen).await,
        }
    }

    /// sendUserMessage / sendUserMessageWithAttachments post-split
    /// (ChatController.cpp:920-1004).
    async fn on_send_prompt(
        &mut self,
        text: String,
        images: Vec<String>,
        display: Vec<String>,
        kind: String,
        ack: Option<tokio::sync::oneshot::Sender<SendOutcome>>,
    ) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() && display.is_empty() {
            if let Some(ack) = ack {
                let _ = ack.send(SendOutcome::Rejected(String::new()));
            }
            return;
        }
        // A new user message supersedes a pending rate-limit retry (Phase 4).
        if self.retry_active {
            self.cancel_retry(true);
        }
        if self.busy {
            if self.queue.len() >= K_MAX_QUEUE_SIZE {
                self.set_error(format!("队列已满（最多 {K_MAX_QUEUE_SIZE} 条）"));
                self.emit_status(None);
                if let Some(ack) = ack {
                    let _ = ack.send(SendOutcome::Rejected(format!(
                        "队列已满（最多 {K_MAX_QUEUE_SIZE} 条）"
                    )));
                }
                return;
            }
            self.queue.push_back(QueuedItem { text: trimmed, images, display, kind });
            self.set_error(String::new());
            self.sync_snap();
            self.emit_status(None);
            if let Some(ack) = ack {
                let _ = ack.send(SendOutcome::Enqueued);
            }
            return;
        }
        self.set_error(String::new());
        self.start_send(trimmed, images, display, &kind).await;
        if let Some(ack) = ack {
            let _ = ack.send(SendOutcome::Started);
        }
    }

    /// cancel() (ChatController.cpp:1070-1107).
    async fn on_cancel(&mut self) {
        // Manual cancel first aborts a pending rate-limit retry: the turn
        // already ended, so session/cancel must NOT be sent here.
        if self.retry_active {
            self.cancel_retry(true);
            return;
        }
        self.pending_guide = None;
        self.pending_prompt = None;
        if self.perm_request_id.is_some() {
            self.respond_permission("", true).await;
        }
        if !self.busy {
            self.emit_status(None);
            return;
        }
        self.user_stop = true;
        if let Some(c) = self.client.as_mut() {
            if let Err(e) = c.cancel_turn().await {
                log::warn!("chat[{}] cancel_turn failed: {e}", self.session_id);
            }
        }
        let gen = self.turn_gen;
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(K_CANCEL_TIMEOUT_MS)).await;
            let _ = tx.send(RuntimeCmd::CancelTimeout(gen)).await;
        });
    }

    /// guideAt() (ChatController.cpp:1129-1180): queue jump — cancel the
    /// current turn, then send this entry; 800ms kill fallback.
    async fn on_guide_at(&mut self, index: usize) {
        if index >= self.queue.len() {
            return;
        }
        let Some(item) = self.queue.remove(index) else {
            return;
        };
        self.sync_snap();
        self.emit_status(None);
        self.set_error(String::new());
        // A pending permission request blocks the agent's turn — settle it
        // as cancelled first, or session/cancel never lands and the stale
        // dialog wedges on top of the guide's new turn.
        if self.perm_request_id.is_some() {
            self.respond_permission("", true).await;
        }
        // A queued guide supersedes a pending rate-limit retry (Phase 4).
        if self.retry_active {
            self.cancel_retry(true);
        }
        if !self.busy {
            self.start_send(item.text, item.images, item.display, &item.kind).await;
            return;
        }
        self.pending_guide = Some(item);
        self.user_stop = true;
        if let Some(c) = self.client.as_mut() {
            if let Err(e) = c.cancel_turn().await {
                log::warn!("chat[{}] guide cancel_turn failed: {e}", self.session_id);
            }
        }
        self.guide_gen += 1;
        let gen = self.guide_gen;
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(K_GUIDE_TIMEOUT_MS)).await;
            let _ = tx.send(RuntimeCmd::GuideTimeout(gen)).await;
        });
    }

    async fn guide_timeout(&mut self, gen: u64) {
        if gen != self.guide_gen {
            return;
        }
        let Some(guide) = self.pending_guide.clone() else {
            return; // turn finished in time; finish_reply already sent it
        };
        if !self.busy {
            self.pending_guide = None;
            self.start_send(guide.text, guide.images, guide.display, &guide.kind).await;
        } else {
            // Old turn still alive inside AcpClient — kill it, or its
            // remaining chunks would stream into the guide's new bubble and
            // prompt() would reject the guide while turnBusy is set.
            self.client = None;
            self.acp_ready = false;
            self.snap().acp_running = false;
            self.mark_interrupted();
            self.set_busy(false);
            self.pending_guide = None;
            self.user_stop = false;
            self.start_send(guide.text, guide.images, guide.display, &guide.kind).await;
        }
    }

    /// switchAgent (ChatController.cpp:748-808). The manager validated the
    /// target agent; meta persistence happens here where the old agent (and
    /// therefore the provider comparison) lives.
    async fn on_switch_agent(&mut self, agent: Agent) {
        let old_provider = self.agent.provider.trim().to_lowercase();
        let new_provider = agent.provider.trim().to_lowercase();
        if self.agent.id == agent.id {
            return; // no-op
        }
        // Assign before finish_reply so a drained queue starts on the NEW
        // agent (old code achieved this via a deferred drainQueue).
        self.agent = agent;
        self.current_model = self.agent.model.clone();

        if self.busy {
            self.cancel_retry(false);
            self.user_stop = true;
            // A prompt waiting for the handshake must not fire into the NEW
            // agent's process when it comes up.
            self.pending_prompt = None;
            self.client = None;
            self.acp_ready = false;
            self.snap().acp_running = false;
            self.mark_interrupted();
            self.emit_turn("interrupted", "cancelled", None);
            self.finish_reply();
            self.user_stop = false;
        } else if self.client.is_some() {
            self.client = None;
            self.acp_ready = false;
            self.snap().acp_running = false;
        }
        // A pending permission request belongs to the dead process.
        self.clear_permission();

        let (agent_id, agent_name) = (self.agent.id.clone(), self.agent.name.clone());
        {
            let mut stores = lock_ok(&self.stores);
            if let Err(e) = stores.sessions.set_session_agent_id(
                &self.session_id,
                &agent_id,
                &agent_name,
                &new_provider,
            ) {
                log::warn!("chat[{}] set_session_agent_id failed: {e}", self.session_id);
            }
            // The stored ACP session id belongs to the old CLI; across
            // providers a session/load with it is doomed, so drop it and
            // start session/new. Same-provider switches keep it and resume
            // agent-side history. Local history is untouched either way.
            if old_provider != new_provider {
                if let Err(e) = stores.sessions.set_acp_session_id(&self.session_id, "") {
                    log::warn!("chat[{}] set_acp_session_id failed: {e}", self.session_id);
                }
            }
        }
        self.sync_snap();
        // Warm the new connection in the background (session/load when kept).
        self.ensure_acp().await;
        self.emit_status(Some(format!("已切换 Agent · {agent_name}")));
        self.emit("store://sessions", json!({}));
    }

    /// Chat-page model pick for a model the CLI's own picker does not
    /// advertise (endpoint model from the agent's baseUrl /models list):
    /// persist it onto the agent record and restart the CLI process so
    /// build_launch injects it via the KIMI_MODEL_* env family. Picker
    /// (alias) models go through SetConfigOption instead — no respawn.
    /// Teardown mirrors on_switch_agent; the stored ACP session id is dropped
    /// because a resumed session would keep the CLI-remembered model and the
    /// env injection only applies on session/new.
    async fn on_set_model(&mut self, model: String) {
        let model = model.trim().to_string();
        if model.is_empty() || model == self.agent.model.trim() {
            return;
        }
        self.agent.model = model.clone();
        self.current_model = model.clone();

        if self.busy {
            self.cancel_retry(false);
            self.user_stop = true;
            // A prompt waiting for the handshake must not fire into the NEW
            // model's process when it comes up.
            self.pending_prompt = None;
            self.client = None;
            self.acp_ready = false;
            self.snap().acp_running = false;
            self.mark_interrupted();
            self.emit_turn("interrupted", "cancelled", None);
            self.finish_reply();
            self.user_stop = false;
        } else if self.client.is_some() {
            self.client = None;
            self.acp_ready = false;
            self.snap().acp_running = false;
        }
        // A pending permission request belongs to the dead process.
        self.clear_permission();

        {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            // update_agent (not save_agent) so the in-memory record other
            // sessions spawn from picks up the new model too.
            let patch = crate::store::agents::AgentPatch {
                model: Some(model.clone()),
                ..Default::default()
            };
            if let Err(e) = stores.agents.update_agent(&paths, &self.agent.id, &patch) {
                log::warn!("chat[{}] update_agent (model) failed: {e}", self.session_id);
            }
            if let Err(e) = stores.sessions.set_acp_session_id(&self.session_id, "") {
                log::warn!("chat[{}] set_acp_session_id failed: {e}", self.session_id);
            }
        }
        self.sync_snap();
        // Warm the new connection (session/new with the env-injected model).
        self.ensure_acp().await;
        self.emit_status(Some(format!("已切换模型 · {model}")));
        self.emit("store://sessions", json!({}));
    }

    // ---- ACP events ----

    async fn handle_event(&mut self, ev: AcpEvent) {
        match ev {
            AcpEvent::Started { session_id } => {
                self.acp_ready = true;
                self.snap().acp_running = true;
                // ACP 适配器不报 usage 的 provider（kimi/claude/codex）：
                // 定位各自的本地会话档案做增量补源（见 chat/wire.rs）。
                self.archive_usage = ArchiveKind::for_provider(&self.agent.provider).and_then(
                    |kind| {
                        let home = match kind {
                            ArchiveKind::Kimi => crate::chat::wire::kimi_home(),
                            ArchiveKind::Claude => crate::chat::wire::claude_home(),
                            ArchiveKind::Codex => crate::chat::wire::codex_home(),
                        }?;
                        let work_dir = {
                            let mut stores = lock_ok(&self.stores);
                            stores
                                .sessions
                                .meta_for(&self.session_id)
                                .map(|m| m.work_dir)
                                .unwrap_or_default()
                        };
                        Some(ArchiveReader::locate(kind, home, &session_id, &work_dir))
                    },
                );
                {
                    let mut stores = lock_ok(&self.stores);
                    if let Err(e) =
                        stores.sessions.set_acp_session_id(&self.session_id, &session_id)
                    {
                        log::warn!("chat[{}] set_acp_session_id failed: {e}", self.session_id);
                    }
                }
                self.set_error(String::new());
                self.sync_snap(); // image_supported now known (attachment split)
                self.emit_status(None);
                // fires now (user prompt, rate-limit resend or continuation).
                if let Some((text, images)) = self.pending_prompt.take() {
                    if let Some(c) = self.client.as_mut() {
                        if let Err(e) = c.prompt(&text, &images).await {
                            log::warn!("chat[{}] pending prompt failed: {e}", self.session_id);
                        }
                    }
                }
            }
            AcpEvent::StartFailed { error } => {
                self.acp_ready = false;
                self.snap().acp_running = false;
                self.set_error(error.clone());
                if self.busy {
                    if !self.assistant_buf.trim().is_empty() {
                        // Restart-for-continue failed: keep the partial reply
                        // instead of overwriting it with the error text.
                        self.mark_interrupted();
                        self.emit_turn("interrupted", "error", None);
                    } else {
                        self.update_last_assistant(&format!("ACP 启动失败: {error}"), "error");
                        self.emit_turn("error", "error", None);
                    }
                    self.finish_reply();
                }
                self.emit_status(None);
            }
            AcpEvent::ModeChanged { mode } => {
                // current_mode_update / set_mode ack: patch the mode picker's
                // currentValue in the last options batch and re-forward so the
                // status bar reflects agent-side mode switches.
                let mut changed = false;
                for o in self.last_config_options.iter_mut() {
                    if o["id"].as_str() == Some("mode") {
                        o["currentValue"] = json!(mode);
                        changed = true;
                    }
                }
                if changed {
                    self.emit(
                        "acp://configOptions",
                        json!({ "sessionId": self.session_id, "options": self.last_config_options }),
                    );
                }
            }
            AcpEvent::AvailableCommands { commands } => {
                self.emit(
                    "acp://commands",
                    json!({ "sessionId": self.session_id, "commands": commands }),
                );
            }
            AcpEvent::Plan { entries } => {
                // Render the plan as a tool-style segment on the last
                // assistant row; repeated plan updates replace it by id.
                let tool = json!({
                    "toolCallId": "plan",
                    "kind": "plan",
                    "title": "计划",
                    "status": "completed",
                    "entries": entries,
                });
                self.on_tool(tool).await;
            }
            AcpEvent::UsageUpdate { usage } => {
                self.turn_usage_live = Some(usage);
            }
            AcpEvent::SessionInfo { title } => {
                if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
                    let renamed = {
                        let mut stores = lock_ok(&self.stores);
                        stores.sessions.rename_session(&self.session_id, &t).unwrap_or(false)
                    };
                    if renamed {
                        self.emit("store://sessions", json!({}));
                    }
                }
            }
            AcpEvent::ConfigOptions { options } => {
                // Track the running model for usage records: the CLI's model
                // picker currentValue is authoritative once seen.
                if let Some(cur) = options
                    .iter()
                    .find(|o| o["id"].as_str() == Some("model"))
                    .and_then(|p| p["currentValue"].as_str())
                {
                    self.current_model = cur.to_string();
                }
                // Apply the agent's default model once per fresh session:
                // only when it appears in the CLI's own model picker (aliases
                // come through here; non-alias models were injected via
                // KIMI_MODEL_* at spawn). Resumed sessions keep whatever the
                // CLI remembers.
                if self.fresh_launch && !self.model_applied {
                    self.model_applied = true;
                    let want = self.agent.model.trim();
                    if !want.is_empty() {
                        let picker = options.iter().find(|o| o["id"].as_str() == Some("model"));
                        let applicable = picker.is_some_and(|p| {
                            p["currentValue"].as_str() != Some(want)
                                && p["options"].as_array().is_some_and(|list| {
                                    list.iter().any(|o| o["value"].as_str() == Some(want))
                                })
                        });
                        if applicable {
                            if let Some(c) = self.client.as_mut() {
                                if let Err(e) = c.set_config_option("model", want).await {
                                    log::warn!(
                                        "chat[{}] default model '{want}' apply failed: {e}",
                                        self.session_id
                                    );
                                }
                            }
                        }
                    }
                }
                self.last_config_options = options.clone();
                self.emit(
                    "acp://configOptions",
                    json!({ "sessionId": self.session_id, "options": options }),
                );
            }
            AcpEvent::ThoughtChunk { text } => {
                self.thinking_buf.push_str(&text);
                self.pending_thinking.push_str(&text);
                self.schedule_flush();
            }
            AcpEvent::MessageChunk { text } => {
                self.assistant_buf.push_str(&text);
                self.pending_content.push_str(&text);
                self.schedule_flush();
            }
            AcpEvent::ToolCall { call } => self.on_tool(call).await,
            AcpEvent::ToolCallUpdate { update } => self.on_tool(update).await,
            AcpEvent::PermissionRequested { request_id, params } => {
                self.perm_request_id = Some(request_id);
                // AskUserQuestion requests carry q{n}_*-namespaced options;
                // the parsed question groups ride alongside the verbatim
                // params so the dialog can render EVERY question (nothing
                // dropped client-side; see acp/types.rs).
                let questions = crate::acp::types::parse_question_request(&params);
                let payload = json!({
                    "sessionId": self.session_id,
                    "requestId": request_id,
                    "params": params,
                    "questions": questions,
                });
                self.snap().perm_pending = Some(payload.clone());
                self.emit("acp://permission", payload);
                self.emit_status(None);
            }
            AcpEvent::TurnFinished { stop_reason, usage } => {
                self.on_turn_finished(&stop_reason, usage).await
            }
            AcpEvent::ProtocolError { error } => {
                // Emitted BEFORE turnFinished("error") so the retry detector
                // has the raw text in hand (acp-protocol.md §7).
                self.last_turn_error = error.clone();
                self.set_error(error);
            }
            AcpEvent::SessionLoadFallback { error } => {
                // session/load failed and the client silently opened a fresh
                // session: the resume LOOKS normal but the agent-side history
                // is gone. Surface the real load error in the chat, or the
                // user never learns the context was lost.
                let notice = format!(
                    "⚠ 原会话恢复失败（{error}），已自动开启新会话：历史上下文未恢复，agent 只能看到本会话之后的内容。"
                );
                self.set_error(notice.clone());
                if self.busy {
                    // Fold into the in-flight bubble: a standalone row would
                    // divert the streamed reply (flushes always target the
                    // LAST assistant row).
                    self.assistant_buf.push_str(&notice);
                    self.pending_content.push_str(&notice);
                    self.schedule_flush();
                } else {
                    // Warm-up spawn outside any turn: a standalone error row.
                    let provider = self.agent.provider.trim().to_lowercase();
                    let row = {
                        let mut stores = lock_ok(&self.stores);
                        if let Err(e) = stores.sessions.append_message(
                            &self.session_id,
                            "assistant",
                            &notice,
                            &provider,
                            "error",
                            &[],
                            "",
                        ) {
                            log::warn!(
                                "chat[{}] load-fallback notice append failed: {e}",
                                self.session_id
                            );
                        }
                        last_row(&stores, &self.session_id)
                    };
                    if let Some(row) = row {
                        self.emit("chat://messageAppended", row_json(&self.session_id, &row));
                    }
                    self.emit("store://sessions", json!({}));
                }
                self.emit_status(None);
            }
            AcpEvent::ProcessExited { code } => {
                log::info!("chat[{}] ACP process exited (code {code})", self.session_id);
                self.acp_ready = false;
                if self.busy {
                    // A dying process takes any pending rate-limit retry with
                    // it; the interrupted/continue logic decides the rest.
                    self.cancel_retry(false);
                    self.flush_stream_buffers();
                    if !self.user_stop
                        && !self.assistant_buf.trim().is_empty()
                        && self.continue_retries < K_MAX_CONTINUE_RETRIES
                    {
                        self.continue_retries += 1;
                        self.resume_interrupted_turn().await;
                    } else {
                        self.mark_interrupted();
                        self.emit_turn("interrupted", "processExited", None);
                        self.finish_reply();
                    }
                }
                self.emit_status(None);
            }
        }
    }

    /// tool_call / tool_call_update (ChatController.cpp:373-384).
    async fn on_tool(&mut self, update: Value) {
        // Flush text first: text/tool arrival order in the segment stream.
        self.flush_stream_buffers();
        let mut tool = tool_from_update(&update);
        truncate_tool_payloads(&mut tool);
        {
            let mut stores = lock_ok(&self.stores);
            stores.sessions.upsert_last_assistant_tool(&self.session_id, &tool);
        }
        self.emit(
            "acp://tool",
            json!({ "sessionId": self.session_id, "tool": Value::Object(tool.clone()) }),
        );
        self.track_subagent(&tool);
    }

    /// turnFinished (ChatController.cpp:404-437).
    async fn on_turn_finished(&mut self, stop: &str, usage: Option<TurnUsage>) {
        self.flush_stream_buffers();
        self.clear_permission();
        // Usage priority: prompt result > usage_update notifications seen
        // during the turn > local archive backfill (chat/wire.rs) for
        // providers whose ACP adapter reports nothing.
        let usage = usage
            .or_else(|| self.turn_usage_live.take())
            .or_else(|| self.read_archive_usage());
        // Phase 4: a rate-limited turn is not final — enter backoff and
        // resend the same prompt instead of closing with an error.
        if stop == "error"
            && !self.user_stop
            && is_rate_limit_error(&self.last_turn_error)
            && self.retry_prompt.is_some()
            && self.retry_attempt < K_MAX_RATE_LIMIT_RETRIES
        {
            self.last_turn_error.clear();
            self.schedule_retry();
            return;
        }
        self.last_turn_error.clear();
        self.retry_attempt = 0;
        let status = if self.user_stop || stop == "cancelled" || stop == "canceled" {
            "interrupted"
        } else if stop == "error" {
            "error"
        } else {
            "done"
        };
        if self.assistant_buf.trim().is_empty() && status == "interrupted" {
            self.update_last_assistant(INTERRUPTED_MARK, "interrupted");
        } else {
            let mut stores = lock_ok(&self.stores);
            if let Err(e) = stores
                .sessions
                .flush_last_assistant(&self.session_id, Some(status), usage.clone())
            {
                log::warn!("chat[{}] flush_last_assistant failed: {e}", self.session_id);
            }
        }
        // Persist the turn's token usage (when the agent reported it).
        if let Some(u) = &usage {
            let rec = UsageRecord::new(
                &self.session_id,
                &self.agent.id,
                &self.agent.name,
                &self.current_model,
                u,
            );
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            if let Err(e) = stores.usage.append(&paths, rec) {
                log::warn!("chat[{}] usage append failed: {e}", self.session_id);
            }
        }
        self.user_stop = false;
        self.assistant_buf.clear();
        self.thinking_buf.clear();
        self.continue_retries = 0;
        self.finish_subagents(status == "interrupted");
        self.emit_turn(status, stop, usage.as_ref());
        self.finish_reply();
        // 回合结束 reload 一次：AI 只可能在回合内调 set_reminder，这里能
        // 捕获 MCP 子进程刚写入 reminders.json 的新提醒。
        self.reminders_reload();
    }

    // ---- turn lifecycle ----

    /// startSend (ChatController.cpp:1006-1068) — order is a hard contract.
    async fn start_send(&mut self, text: String, images: Vec<String>, display: Vec<String>, kind: &str) {
        let provider = self.agent.provider.trim().to_lowercase();
        self.turn_usage_live = None; // stale usage must not leak into a new turn
        // 首条用户消息：发给 agent 的文本前注入提醒工具引导语（只进
        // prompt，不进显示的用户行；pending_prompt 路径复用同一文本）。
        let prompt_text = {
            let mut stores = lock_ok(&self.stores);
            stores.sessions.ensure_open(&self.session_id);
            let first_user = stores
                .sessions
                .messages(&self.session_id)
                .is_none_or(|ms| !ms.iter().any(|m| m.role == "user"));
            if first_user {
                format!("{REMINDER_GUIDE_PREFIX}\n\n{text}")
            } else {
                text.clone()
            }
        };
        let (user_row, asst_row) = {
            let mut stores = lock_ok(&self.stores);
            if let Err(e) = stores.sessions.append_message(
                &self.session_id,
                "user",
                &text,
                &provider,
                "done",
                &display,
                kind,
            ) {
                log::warn!("chat[{}] append user row failed: {e}", self.session_id);
            }
            let user_row = last_row(&stores, &self.session_id);
            if let Err(e) = stores.sessions.append_message(
                &self.session_id,
                "assistant",
                PLACEHOLDER,
                &provider,
                "pending",
                &[],
                "",
            ) {
                log::warn!("chat[{}] append assistant row failed: {e}", self.session_id);
            }
            let asst_row = last_row(&stores, &self.session_id);
            (user_row, asst_row)
        };
        if let Some(row) = user_row {
            self.emit("chat://messageAppended", row_json(&self.session_id, &row));
        }
        if let Some(row) = asst_row {
            self.emit("chat://messageAppended", row_json(&self.session_id, &row));
        }
        self.emit("store://sessions", json!({}));

        self.assistant_buf.clear();
        self.thinking_buf.clear();
        self.drop_pending_stream(); // stale chunks must not land in the fresh row
        self.continue_retries = 0;
        self.user_stop = false;
        // Phase 4 retry bookkeeping: only text-only prompts are resendable.
        // Image payloads are not kept and re-reading files could pick up
        // changed content; non-image attachments are already inlined text.
        self.retry_prompt = if images.is_empty() { Some(prompt_text.clone()) } else { None };
        self.retry_attempt = 0;
        self.last_turn_error.clear();
        // New turn — reset the sub-agent panel and the batch wake-up state.
        if !self.subagents.is_empty() {
            self.subagents.clear();
            self.emit_subagents();
        }
        self.bg_pending.clear();
        self.bg_wake_sent = false;
        self.turn_gen += 1;
        self.set_busy(true);
        self.set_error(String::new());
        self.emit_status(None);

        if !provider::chat_capable(&provider) {
            self.update_last_assistant(
                &format!(
                    "Provider «{provider}» 未注册，请在配置页选择 kimi / claude / codex / custom。"
                ),
                "error",
            );
            self.emit_turn("error", "error", None);
            self.finish_reply();
            return;
        }

        if self.client.is_none() || !self.acp_ready {
            self.pending_prompt = Some((prompt_text, images));
            self.ensure_acp().await;
            return;
        }

        // Keep mode in sync, then send.
        let mode = self.current_mapped_mode();
        if let Some(c) = self.client.as_mut() {
            if let Err(e) = c.set_mode(&mode).await {
                log::warn!("chat[{}] set_mode failed: {e}", self.session_id);
            }
            if let Err(e) = c.prompt(&prompt_text, &images).await {
                log::warn!("chat[{}] prompt failed: {e}", self.session_id);
            }
        }
    }

    fn finish_reply(&mut self) {
        self.clear_permission();
        self.set_busy(false);
        // Background turn completion marks the session unread (runtime flag,
        // not persisted); opening the session clears it.
        let is_active = lock_ok(&self.shared).active_id == self.session_id;
        if !is_active {
            lock_ok(&self.shared)
                .unread
                .insert(self.session_id.clone());
            self.emit("chat://unread", json!({ "sessionId": self.session_id }));
        }
        self.emit("store://sessions", json!({}));
        // guide插队优先于队列 drain（ChatController.cpp:1368-1384）。
        if let Some(guide) = self.pending_guide.take() {
            let tx = self.self_tx.clone();
            // Post back so start_send runs outside this call stack (mirrors
            // the old singleShot(0) deferral and keeps ordering simple).
            tokio::spawn(async move {
                let _ = tx
                    .send(RuntimeCmd::SendPrompt {
                        text: guide.text,
                        images: guide.images,
                        display: guide.display,
                        kind: guide.kind,
                        ack: None,
                    })
                    .await;
            });
            return;
        }
        if !self.busy {
            if let Some(next) = self.queue.pop_front() {
                self.sync_snap();
                self.emit_status(None);
                let tx = self.self_tx.clone();
                tokio::spawn(async move {
                    let _ = tx
                        .send(RuntimeCmd::SendPrompt {
                            text: next.text,
                            images: next.images,
                            display: next.display,
                            kind: next.kind,
                            ack: None,
                        })
                        .await;
                });
            } else {
                self.emit_status(None);
            }
        }
    }

    /// ensureAcp (ChatController.cpp:836-896): compute launch params from the
    /// agent snapshot + store, enforce the process cap, spawn, handshake.
    async fn ensure_acp(&mut self) {
        if self.client.is_some() && self.acp_ready {
            return;
        }
        self.client = None;
        self.acp_ready = false;
        self.snap().acp_running = false;

        let launch = self.build_launch();
        self.fresh_launch = launch.start.resume_session_id.is_empty();
        self.model_applied = false;
        enforce_process_cap(&self.registry, &self.session_id);
        self.emit_status(None); // 连接 ACP…

        let tx = self.ev_tx.clone();
        match (self.spawner)(launch, tx).await {
            Ok(client) => {
                self.client = Some(client);
                self.snap().acp_running = true;
            }
            Err(e) => {
                // AcpClient::spawn already emitted StartFailed (spawner
                // contract); just log here.
                log::warn!("chat[{}] ACP spawn failed: {e}", self.session_id);
                self.snap().acp_running = false;
            }
        }
    }

    fn build_launch(&self) -> SessionLaunch {
        let provider = self.agent.provider.trim().to_lowercase();
        let spec = provider::spec(&provider);
        let mut env = match spec {
            Some(s) => provider::env_overrides(s, &self.agent.api_key, &self.agent.base_url, true),
            None => Vec::new(),
        };
        // Per-agent model for kimi: a model that is NOT one of the CLI's own
        // config.toml aliases cannot be picked via ACP configOptions, so we
        // synthesize it through the KIMI_MODEL_* env family (kimi CLI docs,
        // env-vars). Aliases are applied at runtime via set_config_option
        // once the session's configOptions arrive.
        let model = self.agent.model.trim();
        if provider == "kimi" && !model.is_empty() && !crate::models::kimi_model_aliases().iter().any(|a| a == model) {
            env.push(("KIMI_MODEL_NAME".to_string(), Some(model.to_string())));
            if !self.agent.api_key.trim().is_empty() {
                env.push(("KIMI_MODEL_API_KEY".to_string(), Some(self.agent.api_key.trim().to_string())));
            }
            if !self.agent.base_url.trim().is_empty() {
                env.push(("KIMI_MODEL_BASE_URL".to_string(), Some(self.agent.base_url.trim().to_string())));
                // A custom baseUrl is an OpenAI-compatible endpoint.
                env.push(("KIMI_MODEL_PROVIDER_TYPE".to_string(), Some("openai".to_string())));
            }
        }
        let cli = provider::resolve_command(spec, &self.agent.cli_path);
        let args = provider::resolve_args(spec, &self.agent.extra_args);
        // Per-agent MCP servers: the config page stores raw JSON-array text;
        // a typo degrades to "no MCP servers" with a log line
        // (ChatController.cpp:876-889).
        let mcp_text = self.agent.mcp_servers.trim();
        let mut mcp_servers = if mcp_text.is_empty() {
            Vec::new()
        } else {
            match serde_json::from_str::<Value>(mcp_text) {
                Ok(Value::Array(a)) => a,
                _ => {
                    log::warn!(
                        "chat[{}] ignoring invalid mcpServers JSON for agent {}",
                        self.session_id,
                        self.agent.name
                    );
                    Vec::new()
                }
            }
        };
        // Built-in reminder MCP server (mcp_reminder.rs): one `--mcp-reminder`
        // subprocess per session, context passed via env; it reads/writes the
        // shared reminders.json directly (no IPC).
        if let Ok(exe) = std::env::current_exe() {
            let reminders_path = lock_ok(&self.stores).paths.reminders_path();
            mcp_servers.push(json!({
                "name": "wardex-reminder",
                "command": exe.to_string_lossy(),
                "args": ["--mcp-reminder"],
                "env": [
                    { "name": "WARDEX_SESSION_ID", "value": self.session_id },
                    { "name": "WARDEX_REMINDERS_PATH", "value": reminders_path.to_string_lossy() },
                ],
            }));
        }
        let (cwd, resume) = {
            let mut stores = lock_ok(&self.stores);
            let mut cwd = stores.sessions.workspace_path_for(&self.session_id);
            if cwd.is_empty() {
                cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            let resume = stores.sessions.acp_session_id_for(&self.session_id);
            (cwd, resume)
        };
        let mode = self.current_mapped_mode();
        SessionLaunch {
            spawn: crate::acp::SpawnConfig {
                cli_path: cli,
                args,
                env,
                cwd: cwd.clone(),
            },
            start: crate::acp::StartParams {
                cwd,
                preferred_mode: mode,
                resume_session_id: resume,
                mcp_servers,
            },
        }
    }

    /// resumeInterruptedTurn (ChatController.cpp:1210-1231): synthetic
    /// continuation prompt — NOT appended to local history, reply streams
    /// into the same bubble. The tail anchors the model even when
    /// session/load fell back to a fresh agent-side session.
    async fn resume_interrupted_turn(&mut self) {
        let tail = right_chars(&self.assistant_buf, 500);
        self.pending_prompt = Some((continuation_prompt(&tail), Vec::new()));
        self.emit_status(Some("连接中断，自动续写…".to_string()));
        self.ensure_acp().await;
    }

    /// markAssistantInterrupted (ChatController.cpp:1335-1353).
    fn mark_interrupted(&mut self) {
        self.flush_stream_buffers();
        let out = self.assistant_buf.trim().to_string();
        if out.is_empty() || out == PLACEHOLDER {
            self.update_last_assistant(INTERRUPTED_MARK, "interrupted");
        } else if !out.contains("已中断") {
            // Buffer holds only the tail; the full text lives in the store —
            // append the mark in place instead of replacing the whole body.
            let mark = "\n\n（已中断）";
            {
                let mut stores = lock_ok(&self.stores);
                stores
                    .sessions
                    .append_last_assistant_content(&self.session_id, mark);
            }
            self.emit(
                "acp://chunk",
                json!({ "sessionId": self.session_id, "kind": "text", "text": mark }),
            );
        }
        {
            let mut stores = lock_ok(&self.stores);
            if let Err(e) = stores
                .sessions
                .flush_last_assistant(&self.session_id, Some("interrupted"), None)
            {
                log::warn!("chat[{}] flush interrupted failed: {e}", self.session_id);
            }
        }
        self.finish_subagents(true);
    }

    // ---- streaming flush (ChatController.cpp:356-372, 1184-1208) ----

    fn schedule_flush(&mut self) {
        if self.flush_pending {
            return;
        }
        self.flush_pending = true;
        // Big backlogs stretch the interval (50 -> 250ms) to relieve the UI.
        let backlog = self.pending_content.len() + self.pending_thinking.len();
        let ms = if backlog > K_FLUSH_LONG_THRESHOLD {
            K_FLUSH_LONG_MS
        } else {
            K_FLUSH_MS
        };
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            let _ = tx.send(RuntimeCmd::FlushTick).await;
        });
    }

    fn flush_stream_buffers(&mut self) {
        self.flush_pending = false;
        if !self.pending_thinking.is_empty() {
            let chunk = std::mem::take(&mut self.pending_thinking);
            {
                let mut stores = lock_ok(&self.stores);
                stores
                    .sessions
                    .append_last_assistant_thinking(&self.session_id, &chunk);
            }
            self.emit(
                "acp://chunk",
                json!({ "sessionId": self.session_id, "kind": "thinking", "text": chunk }),
            );
        }
        if !self.pending_content.is_empty() {
            let chunk = std::mem::take(&mut self.pending_content);
            {
                let mut stores = lock_ok(&self.stores);
                stores
                    .sessions
                    .append_last_assistant_content(&self.session_id, &chunk);
            }
            self.emit(
                "acp://chunk",
                json!({ "sessionId": self.session_id, "kind": "text", "text": chunk }),
            );
        }
        // The store holds the full text per segment; buffers keep only the
        // tail (resume right(500) anchor + emptiness checks) so a long reply
        // does not live twice in memory.
        if self.thinking_buf.chars().count() > K_STREAM_BUFFER_KEEP {
            self.thinking_buf = right_chars(&self.thinking_buf, K_STREAM_BUFFER_KEEP);
        }
        if self.assistant_buf.chars().count() > K_STREAM_BUFFER_KEEP {
            self.assistant_buf = right_chars(&self.assistant_buf, K_STREAM_BUFFER_KEEP);
        }
    }

    fn drop_pending_stream(&mut self) {
        self.flush_pending = false;
        self.pending_content.clear();
        self.pending_thinking.clear();
    }

    // ---- rate-limit retry (ChatController.cpp:1233-1333) ----

    fn schedule_retry(&mut self) {
        self.retry_attempt += 1;
        self.retry_countdown = retry_delay_secs(self.retry_attempt);
        // Replace the "回合失败：…" chunk (already flushed into the bubble)
        // with a retry notice; buffers stay clear so a stale tail cannot
        // leak into the retried reply.
        let notice = format!(
            "请求被限流，{} 秒后自动重试（第 {}/{K_MAX_RATE_LIMIT_RETRIES} 次）…",
            self.retry_countdown, self.retry_attempt
        );
        self.update_last_assistant(&notice, "pending");
        self.assistant_buf.clear();
        self.thinking_buf.clear();
        self.drop_pending_stream();
        self.retry_active = true;
        self.retry_gen += 1;
        let gen = self.retry_gen;
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = tx.send(RuntimeCmd::RetryTick(gen)).await;
        });
        self.emit_retry();
        self.emit_status(None);
    }

    async fn retry_tick(&mut self, gen: u64) {
        if !self.retry_active || gen != self.retry_gen {
            return; // superseded / cancelled
        }
        self.retry_countdown = self.retry_countdown.saturating_sub(1);
        if self.retry_countdown == 0 {
            self.fire_retry().await;
            return;
        }
        self.emit_retry();
        self.emit_status(None);
        // Self-perpetuating chain: the next tick is only scheduled while the
        // retry is still active, so no orphan timer tasks leak.
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = tx.send(RuntimeCmd::RetryTick(gen)).await;
        });
    }

    /// Countdown expired: reset the bubble to the placeholder and resend the
    /// SAME prompt — no new history rows (ChatController.cpp:1284-1309).
    async fn fire_retry(&mut self) {
        let Some(prompt) = self.retry_prompt.clone() else {
            return;
        };
        self.retry_active = false;
        self.assistant_buf.clear();
        self.thinking_buf.clear();
        self.drop_pending_stream();
        self.update_last_assistant(PLACEHOLDER, "pending");
        self.emit_retry();
        self.emit_status(None);
        if self.client.is_none() || !self.acp_ready {
            // Reuse the pendingPrompt handshake path (no image blocks —
            // image turns never reach retry).
            self.pending_prompt = Some((prompt, Vec::new()));
            self.ensure_acp().await;
            return;
        }
        let mode = self.current_mapped_mode();
        if let Some(c) = self.client.as_mut() {
            if let Err(e) = c.set_mode(&mode).await {
                log::warn!("chat[{}] retry set_mode failed: {e}", self.session_id);
            }
            if let Err(e) = c.prompt(&prompt, &[]).await {
                log::warn!("chat[{}] retry prompt failed: {e}", self.session_id);
            }
        }
    }

    /// cancelRateLimitRetry (ChatController.cpp:1311-1333). finalize=true
    /// settles the bubble as a plain failure and closes the turn; false just
    /// stops the clock (process death, agent switch).
    fn cancel_retry(&mut self, finalize: bool) {
        if !self.retry_active {
            return;
        }
        self.retry_active = false;
        self.retry_gen += 1; // invalidate in-flight ticks
        self.retry_countdown = 0;
        self.retry_attempt = 0;
        self.emit_retry();
        if finalize {
            self.update_last_assistant("回合失败：请求被限流，已取消自动重试", "error");
            self.assistant_buf.clear();
            self.thinking_buf.clear();
            self.emit_turn("error", "error", None);
            self.finish_reply();
        }
        self.emit_status(None);
    }

    // ---- reminders (mcp_reminder.rs / reminder_* 命令共用存储) ----

    /// Re-read reminders.json and re-arm the next-fire timer. Called on actor
    /// start, turn end, ReminderTick, and the manual add/cancel commands.
    fn reminders_reload(&mut self) {
        {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            stores.reminders.reload(&paths);
        }
        self.schedule_next_reminder();
    }

    /// Arm a one-shot sleep for the earliest pending reminder; with none
    /// pending only the gen bump invalidates any in-flight timer.
    fn schedule_next_reminder(&mut self) {
        self.reminder_gen += 1;
        let gen = self.reminder_gen;
        let next_due = {
            let stores = lock_ok(&self.stores);
            stores
                .reminders
                .list(&self.session_id)
                .into_iter()
                .map(|r| r.due_at_ms)
                .min()
        };
        let Some(due) = next_due else { return };
        let wait_ms = due.saturating_sub(now_ms()).max(0) as u64;
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            let _ = tx.send(RuntimeCmd::ReminderTick(gen)).await;
        });
    }

    /// Timer fired: mark every due reminder done (persisted BEFORE the prompt
    /// goes out so a crash cannot double-fire), then post each as a normal
    /// SendPrompt — busy turns queue it like any user message.
    async fn reminder_tick(&mut self, gen: u64) {
        if gen != self.reminder_gen {
            return; // superseded by a newer reload/schedule
        }
        let due = {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            stores.reminders.reload(&paths);
            let now = now_ms();
            let due: Vec<_> = stores
                .reminders
                .list(&self.session_id)
                .into_iter()
                .filter(|r| r.due_at_ms <= now)
                .collect();
            for r in &due {
                if let Err(e) = stores.reminders.mark_done(&paths, &r.id) {
                    log::warn!("chat[{}] reminder mark_done failed: {e}", self.session_id);
                }
            }
            due
        };
        let fired = !due.is_empty();
        for r in due {
            let tx = self.self_tx.clone();
            tokio::spawn(async move {
                let _ = tx
                    .send(RuntimeCmd::SendPrompt {
                        text: format!("⏰ 提醒时间到：{}", r.content),
                        images: Vec::new(),
                        display: Vec::new(),
                        kind: "reminder".to_string(),
                        ack: None,
                    })
                    .await;
            });
        }
        if fired {
            self.emit_reminders();
        }
        self.schedule_next_reminder();
    }

    /// chat://reminders {sessionId, reminders[]} — after add/cancel/fire.
    fn emit_reminders(&self) {
        let reminders = lock_ok(&self.stores).reminders.list(&self.session_id);
        self.emit(
            "chat://reminders",
            json!({ "sessionId": self.session_id, "reminders": reminders }),
        );
    }

    // ---- sub-agent tracking (ChatController.cpp:485-652) ----

    fn track_subagent(&mut self, tool: &Map<String, Value>) {
        let id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return;
        }
        // tool_call_update carries no name — new entries need a sub-agent
        // tool name from the initial tool_call; updates match by id only.
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let idx = self.subagents.iter().position(|e| e.id == id);
        if idx.is_none() && !is_subagent_tool_name(&name) {
            return;
        }
        let mut entry = match idx {
            Some(i) => self.subagents[i].clone(),
            None => SubagentEntry {
                id: id.clone(),
                kind: name.clone(),
                title: "子 Agent".to_string(),
                status: "pending".to_string(),
                started_at: now_ms(),
                ..Default::default()
            },
        };
        if let Some(status) = tool.get("status").and_then(Value::as_str) {
            if !status.is_empty() {
                entry.status = status.to_string();
            }
        }
        entry.last_update = now_ms();
        // Input args stream in while the call runs — fill title/children as
        // soon as the JSON becomes parseable.
        if let Some(input) = parse_tool_input(tool) {
            let desc = input.get("description").and_then(Value::as_str).unwrap_or_default();
            let prompt = input.get("prompt").and_then(Value::as_str).unwrap_or_default();
            if !desc.is_empty() {
                entry.title = desc.to_string();
            } else if !prompt.is_empty() {
                entry.title = elide(prompt, 48);
            }
            // Task brief for the detail dialog: the plain prompt when there
            // is one, otherwise the full args JSON (swarm template+items).
            let brief = if !prompt.is_empty() {
                prompt.to_string()
            } else {
                serde_json::to_string_pretty(&input).unwrap_or_default()
            };
            if !brief.is_empty() {
                entry.input = if brief.len() > 32 * 1024 {
                    let cut: String = brief.chars().take(32 * 1024).collect();
                    format!("{cut}\n…（已截断）")
                } else {
                    brief
                };
            }
            if let Some(Value::Array(items)) = input.get("items") {
                if !items.is_empty() {
                    let names: Vec<String> = items
                        .iter()
                        .filter_map(|it| it.as_str().map(str::to_string))
                        .collect();
                    entry.children = names.len();
                    entry.child_names = names;
                }
            }
        }
        let done = entry.status == "completed" || entry.status == "failed";
        if done && entry.finished_at == 0 {
            entry.finished_at = now_ms();
            let raw = tool_raw_text(tool);
            if !raw.is_empty() {
                let provider = self.agent.provider.trim().to_lowercase();
                if let Some(summary) = subagent_summary_for(&provider, &raw) {
                    entry.summary = summary;
                }
                entry.agent_ids = subagent_agent_ids_for(&provider, &raw);
                // Final report for the detail dialog (already ≤64KB upstream).
                entry.output = if raw.len() > 64 * 1024 {
                    let cut: String = raw.chars().take(64 * 1024).collect();
                    format!("{cut}\n…（已截断）")
                } else {
                    raw
                };
            }
        }
        if done {
            self.notify_subagent_done(&entry);
        }
        match idx {
            Some(i) => self.subagents[i] = entry,
            None => self.subagents.push(entry),
        }
        self.emit_subagents();
        if done {
            self.settle_bg_subagent(&id);
        }
    }

    /// A sub-agent reached a terminal status. If it was one of the batch left
    /// running at the last turn's end, settle it; the whole batch finishing
    /// OUTSIDE a turn wakes the agent with a follow-up prompt (the CLI ended
    /// its turn with "等它们完成再继续" and nothing else would re-prompt it).
    /// Inside a turn completions are the norm and never wake.
    fn settle_bg_subagent(&mut self, id: &str) {
        if !self.bg_pending.remove(id) {
            return;
        }
        if self.bg_pending.is_empty() && !self.busy && !self.bg_wake_sent {
            self.bg_wake_sent = true;
            let _ = self.self_tx.try_send(RuntimeCmd::SendPrompt {
                text: "你的子 Agent 已全部完成，请读取结果并继续。".to_string(),
                images: Vec::new(),
                display: Vec::new(),
                kind: "reminder".to_string(),
                ack: None,
            });
        }
    }

    /// Desktop notification for the human when a sub-agent settles. The
    /// agent-side channel is unaffected — this is purely UI-side. The only
    /// false positive filtered here is a background-mode launch ack: the CLI
    /// returns a `task_id:` immediately (start, not completion), so those
    /// entries don't notify. Summary formats differ per provider (kimi has
    /// `actual_subagent_type:`/swarm `outcome=`, Claude has neither), so an
    /// empty summary still notifies with the bare status.
    fn notify_subagent_done(&self, entry: &SubagentEntry) {
        let status = match entry.status.as_str() {
            "completed" => "完成",
            "failed" => "失败",
            "interrupted" => "被中断",
            _ => return,
        };
        if entry.output.contains("task_id:") {
            return;
        }
        let body = if entry.summary.is_empty() {
            status.to_string()
        } else {
            format!("{status} · {}", entry.summary)
        };
        self.sink
            .notify(&format!("子 Agent「{}」{status}", entry.title), &body);
    }

    /// finishSubagents (ChatController.cpp:630-646): at turn end, anything
    /// still pending/in_progress settles completed (or interrupted).
    /// On a NORMAL turn end the unfinished ids are also kept in bg_pending:
    /// a background sub-agent's real completion may only arrive after the
    /// turn — its later tool_call_update then settles it (and the last one
    /// wakes the agent). Interrupted turns settle for real, no wake-up.
    fn finish_subagents(&mut self, interrupted: bool) {
        let mut changed = false;
        for e in self.subagents.iter_mut() {
            if e.status != "pending" && e.status != "in_progress" {
                continue;
            }
            if !interrupted {
                self.bg_pending.insert(e.id.clone());
            }
            e.status = if interrupted { "interrupted" } else { "completed" }.to_string();
            e.finished_at = now_ms();
            changed = true;
        }
        if changed {
            self.emit_subagents();
        }
    }

    // ---- permission ----

    async fn respond_permission(&mut self, option_id: &str, cancelled: bool) {
        let Some(id) = self.perm_request_id.take() else {
            return;
        };
        self.snap().perm_pending = None;
        self.emit(
            "acp://permissionCleared",
            json!({ "sessionId": self.session_id }),
        );
        if let Some(c) = self.client.as_mut() {
            if let Err(e) = c.answer_permission(id, option_id, cancelled).await {
                log::warn!("chat[{}] answer_permission failed: {e}", self.session_id);
            }
        }
        self.emit_status(None);
    }

    /// Turn end / process switch: pending requests auto-clear (rejected by
    /// the dying turn on the agent side; ChatController.cpp:654-662).
    fn clear_permission(&mut self) {
        if self.perm_request_id.take().is_some() {
            self.snap().perm_pending = None;
            self.emit(
                "acp://permissionCleared",
                json!({ "sessionId": self.session_id }),
            );
        }
    }

    // ---- small helpers ----

    fn update_last_assistant(&mut self, content: &str, status: &str) {
        let row = {
            let mut stores = lock_ok(&self.stores);
            if let Err(e) =
                stores
                    .sessions
                    .update_last_assistant(&self.session_id, content, status)
            {
                log::warn!("chat[{}] update_last_assistant failed: {e}", self.session_id);
            }
            last_row(&stores, &self.session_id)
        };
        if let Some(row) = row {
            self.emit("chat://bubbleSet", row_json(&self.session_id, &row));
        }
    }

    fn current_mapped_mode(&self) -> String {
        let mode = {
            let stores = lock_ok(&self.stores);
            stores.prefs.permission_mode().to_string()
        };
        self.mapped_mode(&mode)
    }

    fn mapped_mode(&self, mode: &str) -> String {
        provider::map_mode(&self.agent.provider, mode).to_string()
    }

    fn set_busy(&mut self, busy: bool) {
        if self.busy == busy {
            return;
        }
        self.busy = busy;
        self.touch_activity();
        self.sync_snap();
    }

    fn set_error(&mut self, e: String) {
        if self.last_error == e {
            return;
        }
        self.last_error = e;
        self.emit_status(None);
    }

    fn touch_activity(&mut self) {
        self.snap().last_activity_ms = now_ms();
    }

    fn sync_snap(&mut self) {
        let mut s = self.snap();
        s.busy = self.busy;
        s.queue_len = self.queue.len();
        s.queue = self
            .queue
            .iter()
            .map(|i| {
                if i.display.is_empty() {
                    i.text.clone()
                } else {
                    format!("{} 📎{}", i.text, i.display.len())
                }
            })
            .collect();
        s.agent_id = self.agent.id.clone();
        if let Some(c) = self.client.as_ref() {
            s.image_supported = c.image_supported();
        }
    }

    fn snap(&self) -> MutexGuard<'_, RuntimeSnap> {
        lock_ok(&self.snap)
    }

    /// refreshStatusLine (ChatController.cpp:1398-1427). `override_text` is a
    /// one-shot custom line ("已切换 Agent · name", "连接中断，自动续写…").
    fn status_text(&self) -> String {
        let mode = {
            let stores = lock_ok(&self.stores);
            stores.prefs.permission_mode().to_string()
        };
        let mut s = if self.retry_active {
            format!(
                "限流，{} 秒后自动重试（第 {}/{K_MAX_RATE_LIMIT_RETRIES} 次）…",
                self.retry_countdown, self.retry_attempt
            )
        } else if self.perm_request_id.is_some() {
            "等待批准…".to_string()
        } else if !self.acp_ready && self.client.is_some() {
            "连接 ACP…".to_string()
        } else if self.busy {
            "生成中…".to_string()
        } else {
            "就绪".to_string()
        };
        s += match mode.as_str() {
            "default" => " · 需批准",
            "plan" => " · 计划",
            "auto" => " · 自动",
            "yolo" => " · YOLO",
            _ => "",
        };
        if !self.queue.is_empty() {
            s += &format!(" · 队列 {}/{K_MAX_QUEUE_SIZE}", self.queue.len());
        }
        s
    }

    // ---- event emission (frontend contract, architecture.md §3) ----

    fn emit(&self, event: &str, payload: Value) {
        self.sink.emit(event, payload);
    }

    fn emit_status(&self, override_text: Option<String>) {
        let status_text = override_text.unwrap_or_else(|| self.status_text());
        self.emit(
            "chat://status",
            json!({
                "sessionId": self.session_id,
                "statusText": status_text,
                "busy": self.busy,
                "queueLength": self.queue.len(),
                "retryActive": self.retry_active,
                "retryCountdown": self.retry_countdown,
                "retryAttempt": self.retry_attempt,
                "retryMax": K_MAX_RATE_LIMIT_RETRIES,
                "lastError": self.last_error,
                "acpReady": self.acp_ready,
                "imageSupported": self.client.as_ref().map(|c| c.image_supported()).unwrap_or(false),
            }),
        );
    }

    /// 档案用量补读（chat/wire.rs）：增量求和新记录；reader 未定位
    /// （不支持的 provider / 未 Started）或无新增返回 None。
    fn read_archive_usage(&mut self) -> Option<TurnUsage> {
        self.archive_usage.as_mut()?.read_new()
    }

    fn emit_turn(&self, status: &str, stop_reason: &str, usage: Option<&TurnUsage>) {
        let mut payload =
            json!({ "sessionId": self.session_id, "status": status, "stopReason": stop_reason });
        if let Some(u) = usage {
            payload["usage"] = json!(u);
        }
        self.emit("acp://turn", payload);
    }

    fn emit_retry(&self) {
        self.emit(
            "chat://retry",
            json!({
                "sessionId": self.session_id,
                "active": self.retry_active,
                "countdown": self.retry_countdown,
                "attempt": self.retry_attempt,
                "maxAttempts": K_MAX_RATE_LIMIT_RETRIES,
            }),
        );
    }

    fn emit_subagents(&self) {
        self.emit(
            "acp://subagent",
            json!({ "sessionId": self.session_id, "subagents": self.subagents }),
        );
    }
}

fn last_row(stores: &StoreRegistry, session_id: &str) -> Option<MessageRow> {
    stores
        .sessions
        .messages(session_id)
        .and_then(|m| m.last())
        .cloned()
}

/// Frontend-facing JSON view of a message row (camelCase keys matching the
/// disk format, data-formats.md §4.1).
fn row_json(session_id: &str, row: &MessageRow) -> Value {
    json!({
        "sessionId": session_id,
        "row": {
            "id": row.id,
            "role": row.role,
            "content": row.content,
            "createdAt": row.created_at,
            "provider": row.provider,
            "status": row.status,
            "thinking": row.thinking,
            "toolCalls": row.tool_calls,
            "segments": row.segments,
            "attachments": row.attachments,
            "kind": row.kind,
        }
    })
}

// ---------------------------------------------------------------------------
// Tests: pure helpers only here; actor-level integration tests (MockTransport)
// live in chat/tests.rs.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rate_limit_detection_signatures() {
        assert!(is_rate_limit_error("HTTP 429 Too Many Requests"));
        assert!(is_rate_limit_error("Rate limit reached"));
        assert!(is_rate_limit_error("RATELIMIT exceeded"));
        assert!(is_rate_limit_error("quota exhausted for today"));
        assert!(is_rate_limit_error("RESOURCE EXHAUSTED: retry later"));
        assert!(!is_rate_limit_error("permission denied"));
        assert!(!is_rate_limit_error(""));
    }

    #[test]
    fn backoff_progression_and_cap() {
        assert_eq!(retry_delay_secs(1), 20);
        assert_eq!(retry_delay_secs(2), 40);
        assert_eq!(retry_delay_secs(3), 80);
        assert_eq!(retry_delay_secs(4), 160);
        assert_eq!(retry_delay_secs(5), 300); // 320 capped
        assert_eq!(retry_delay_secs(10), 300);
    }

    #[test]
    fn continuation_prompt_carries_tail() {
        let p = continuation_prompt("TAIL-500");
        assert!(p.starts_with("上一条回复因连接中断被截断。"));
        assert!(p.ends_with("\n…TAIL-500"));
    }

    #[test]
    fn image_path_extensions() {
        assert!(is_image_path("a/b.PNG"));
        assert!(is_image_path("x.jpeg"));
        assert!(!is_image_path("x.png.txt"));
        assert!(!is_image_path("noext"));
    }

    #[test]
    fn subagent_tool_names() {
        for n in ["Agent", "AGENTSWARM", "task", "spawn_agent"] {
            assert!(is_subagent_tool_name(n), "{n}");
        }
        assert!(!is_subagent_tool_name("Read"));
        assert!(!is_subagent_tool_name(""));
    }

    #[test]
    fn parse_tool_input_last_snapshot_wins_then_concat() {
        // Cumulative snapshot: last block parses.
        let tool = json!({ "content": [
            { "type": "content", "content": { "text": "{\"description\":\"part" } },
            { "type": "content", "content": { "text": "{\"description\":\"done\"}" } },
        ]});
        let map = tool.as_object().expect("object").clone();
        let input = parse_tool_input(&map).expect("parsed");
        assert_eq!(input.get("description").and_then(Value::as_str), Some("done"));

        // Delta streaming: no single block parses, the concatenation does.
        let tool = json!({ "content": [
            { "type": "content", "content": { "text": "{\"prom" } },
            { "type": "content", "content": { "text": "pt\":\"hi\"}" } },
        ]});
        let map = tool.as_object().expect("object").clone();
        let input = parse_tool_input(&map).expect("parsed");
        assert_eq!(input.get("prompt").and_then(Value::as_str), Some("hi"));

        // Non-content blocks are ignored; unparseable → None.
        let tool = json!({ "content": [
            { "type": "diff", "content": { "text": "x" } },
        ]});
        let map = tool.as_object().expect("object").clone();
        assert!(parse_tool_input(&map).is_none());
    }

    #[test]
    fn swarm_and_single_summaries() {
        let raw = "<agent_swarm_result>\n<subagent outcome=\"completed\"/>\n<subagent outcome=\"failed\"/>\n<subagent outcome=\"completed\"/>";
        assert_eq!(subagent_summary(raw).as_deref(), Some("完成 2/3"));
        let single = "agent_id: x\nactual_subagent_type: explore\nstatus: ok";
        assert_eq!(subagent_summary(single).as_deref(), Some("explore"));
        assert!(subagent_summary("").is_none());
        assert!(subagent_summary("nothing here").is_none());
    }

    #[test]
    fn tool_raw_text_normalizes_string_and_block_array() {
        let s = json!({ "rawOutput": "plain" });
        assert_eq!(tool_raw_text(s.as_object().unwrap()), "plain");
        // claude-code-acp shape: array of text blocks.
        let a = json!({ "rawOutput": [
            { "type": "text", "text": "报告" },
            { "type": "text", "text": "agentId: a1 (for resuming)" },
        ]});
        assert_eq!(tool_raw_text(a.as_object().unwrap()), "报告\nagentId: a1 (for resuming)");
        let none = json!({});
        assert_eq!(tool_raw_text(none.as_object().unwrap()), "");
    }

    #[test]
    fn claude_summary_report_then_duration_fallback() {
        let with_report = "架构要点如下……\nagentId: a807b73 (for resuming)\n<usage>duration_ms: 1640</usage>";
        assert_eq!(
            subagent_summary_for("claude", with_report).as_deref(),
            Some("架构要点如下……")
        );
        // Empty report (subagent produced nothing) → duration fallback.
        let empty = "\n\n\nagentId: afbc298 (for resuming)\n<usage>total_tokens: 13236\nduration_ms: 1882</usage>";
        assert_eq!(
            subagent_summary_for("claude", empty).as_deref(),
            Some("耗时 1.9s")
        );
        assert!(subagent_summary_for("claude", "").is_none());
        // kimi path unchanged.
        let kimi = "agent_id: x\nactual_subagent_type: explore";
        assert_eq!(subagent_summary_for("kimi", kimi).as_deref(), Some("explore"));
        assert!(subagent_summary_for("kimi", empty).is_none());
    }

    #[test]
    fn claude_agent_ids_hex_token() {
        let raw = "报告\nagentId: a807b73 (for resuming to continue this agent's work if needed)";
        assert_eq!(subagent_agent_ids_for("claude", raw), vec!["a807b73"]);
        let kimi = "agent_id: agent-0\nagent_id: agent-1";
        assert_eq!(
            subagent_agent_ids_for("kimi", kimi),
            vec!["agent-0", "agent-1"]
        );
    }

    #[test]
    fn truncation_only_over_64kb() {
        let mut tool = json!({ "rawOutput": "x".repeat(K_PAYLOAD_TRUNCATE + 10), "other": "y" })
            .as_object()
            .expect("object")
            .clone();
        truncate_tool_payloads(&mut tool);
        let s = tool.get("rawOutput").and_then(Value::as_str).expect("str");
        assert!(s.ends_with("…（已截断）"));
        assert!(s.chars().count() <= K_PAYLOAD_TRUNCATE + 8);
        assert_eq!(tool.get("other").and_then(Value::as_str), Some("y"));
    }

    #[test]
    fn right_chars_utf8_safe() {
        assert_eq!(right_chars("你好世界", 2), "世界");
        assert_eq!(right_chars("abc", 10), "abc");
        assert_eq!(right_chars("", 5), "");
    }

    #[test]
    fn tool_from_update_unwraps_and_names() {
        let nested = json!({ "toolCall": { "toolCallId": "t1", "title": "Read" } });
        let out = tool_from_update(&nested);
        assert_eq!(out.get("name").and_then(Value::as_str), Some("Read"));

        let kind_only = json!({ "toolCallId": "t2", "kind": "execute" });
        let out = tool_from_update(&kind_only);
        assert_eq!(out.get("name").and_then(Value::as_str), Some("execute"));

        let named = json!({ "toolCallId": "t3", "name": "Agent", "title": "ignored" });
        let out = tool_from_update(&named);
        assert_eq!(out.get("name").and_then(Value::as_str), Some("Agent"));
    }
}
