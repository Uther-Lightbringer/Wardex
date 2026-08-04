// WarDex Tauri backend entry point.
// Phase 1d wiring: Tauri commands (invoke) + event forwarding (emit) on top
// of chat::ChatManager and store::StoreRegistry, plus the logging / panic
// hook infrastructure (logs/ with phased startup marks, crashes/crash-*.txt).
//
// Frontend contract (architecture.md §3):
// - Commands below are invoked via `invoke('<name>', args)`; errors come back
//   as Err(String) with user-facing Chinese text where the old code had one.
// - Events pushed via `listen('<name>', cb)`; every chat payload carries
//   `sessionId`:
//     acp://chunk      {sessionId, kind: "text"|"thinking", text}  (50ms 合并)
//     acp://tool       {sessionId, tool}                           (upsert 归一化)
//     acp://turn       {sessionId, status, stopReason}
//     acp://permission {sessionId, requestId, params, questions}
//     acp://permissionCleared {sessionId}
//     acp://subagent   {sessionId, subagents[]}
//     chat://messageAppended {sessionId, row}   (user 行 + assistant 占位行)
//     chat://bubbleSet {sessionId, row}         (整段替换: 中断/错误/重试提示)
//     chat://status    {sessionId, statusText, busy, queueLength, retry*, lastError, acpReady, imageSupported}
//     chat://retry     {sessionId, active, countdown, attempt, maxAttempts}
//     chat://reminders {sessionId, reminders[]}  (push 待办列表变更: add/cancel/触发)
//     chat://unread    {sessionId}
//     store://sessions / store://prefs           (粗粒度变更通知，前端重拉)
// Phase 3b additions (thin shells over existing store/probe helpers):
//     project_exists / folder_drives / folder_list / folder_create
//     install_help / set_user_avatar_from_file / clear_user_avatar

pub mod acp;
pub mod chat;
pub mod cmd;
pub mod codegraph;
pub mod db;
pub mod inspect;
pub mod mcp_reminder;
pub mod models;
pub mod probe;
pub mod provider;
pub mod store;
pub mod usage_backfill;

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use chat::{ChatManager, EventSink};
use store::{AgentPatch, PanelLayoutEntry, Paths, StoreRegistry};

// ---------------------------------------------------------------------------
// Observability (performance.md §6: logs + crash dumps)
// ---------------------------------------------------------------------------

/// fern logger: file in <data root>/logs plus stderr, with phased startup
/// marks (the old AppLog/main.cpp timing points).
fn init_logging(paths: &Paths) {
    let log_path = paths.logs_dir().join(format!(
        "wardex-{}.log",
        chrono::Local::now().format("%Y%m%d")
    ));
    let file = fern::log_file(&log_path);
    let mut dispatch = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {:5} [{}] {}",
                chrono::Local::now().format("%H:%M:%S%.3f"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stderr());
    if let Ok(f) = file {
        dispatch = dispatch.chain(f);
    }
    if dispatch.apply().is_err() {
        eprintln!("WarDex: logger already initialized");
    }
}

/// Panic hook: write crashes/crash-<timestamp>.txt (CrashHandler.cpp 等价).
fn install_panic_hook(paths: &Paths) {
    let dir = paths.crashes_dir();
    std::panic::set_hook(Box::new(move |info| {
        let name = format!("crash-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S%.3f"));
        let body = format!(
            "WarDex panic\n============\n{info}\n\nbacktrace:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::fs::write(dir.join(name), body);
        eprintln!("WarDex panic: {info}");
    }));
}

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

struct AppState {
    chat: Arc<ChatManager>,
    stores: Arc<Mutex<StoreRegistry>>,
    probe: tokio::sync::Mutex<probe::CliProbe>,
    tester: probe::AgentTester,
    runs: cmd::CommandRunner,
    codegraph: codegraph::CodegraphRunner,
    db: db::DbManager,
}

struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(e) = self.0.emit(event, payload) {
            log::warn!("emit {event} failed: {e}");
        }
    }

    /// Desktop notification, only while the main window is unfocused — when
    /// the user is watching the app the SubagentPanel already shows it.
    fn notify(&self, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        let focused = self
            .0
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(true);
        if focused {
            return;
        }
        if let Err(e) = self.0.notification().builder().title(title).body(body).show() {
            log::warn!("notify failed: {e}");
        }
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    chat::runtime::lock_ok(m)
}

// ---------------------------------------------------------------------------
// Commands: chat runtime
// ---------------------------------------------------------------------------

#[tauri::command]
async fn create_session(
    state: State<'_, AppState>,
    project_dir: String,
    group_id: Option<String>,
    agent_id: Option<String>,
    perm_mode: Option<String>,
) -> Result<String, String> {
    state
        .chat
        .create_session_in_group(
            &project_dir,
            group_id.as_deref().unwrap_or(""),
            agent_id.as_deref(),
            perm_mode.as_deref(),
        )
        .await
        .map_err(err)
}

#[tauri::command]
async fn open_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.open_session(&session_id).await.map_err(err)
}

/// Monitor page mini-chat: create/warm the runtime for a session that has
/// meta but no live runtime, WITHOUT switching the active session.
#[tauri::command]
async fn ensure_runtime(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.ensure_runtime(&session_id).await.map_err(err)
}

#[tauri::command]
fn close_session(state: State<'_, AppState>, session_id: String) {
    state.chat.close_session(&session_id);
}

#[tauri::command]
async fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<bool, String> {
    state.chat.delete_session(&session_id).await.map_err(err)
}

#[tauri::command]
fn set_active_session(_state: State<'_, AppState>, session_id: String) {
    // Frontend-only active switching without model/runtime changes: unread
    // clears on open (open_session already does the full path).
    let _ = session_id;
}

#[tauri::command]
fn rename_session(state: State<'_, AppState>, session_id: String, title: String) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    stores.sessions.rename_session(&session_id, &title).map_err(err)
}

#[tauri::command]
fn set_session_project(state: State<'_, AppState>, session_id: String, project_dir: String) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    stores.sessions.set_session_project(&session_id, &project_dir).map_err(err)
}

#[tauri::command]
fn set_session_pinned(
    state: State<'_, AppState>,
    session_id: String,
    pinned: bool,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    stores
        .sessions
        .set_session_pinned(&session_id, pinned)
        .map_err(err)
}

/// Monitor page: hide/show a session there (like set_session_pinned — the
/// frontend updates its local copy and re-pulls list_sessions).
#[tauri::command]
fn set_session_shelved(
    state: State<'_, AppState>,
    session_id: String,
    shelved: bool,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    stores
        .sessions
        .set_session_shelved(&session_id, shelved)
        .map_err(err)
}

/// Per-session permission-mode override (default|plan|auto|yolo; null 清除
/// 回全局 prefs 默认)。运行时发 prompt 前读 meta 里的这个值。
#[tauri::command]
fn set_session_perm_mode(
    state: State<'_, AppState>,
    session_id: String,
    mode: Option<String>,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    stores
        .sessions
        .set_session_perm_mode(&session_id, mode.as_deref())
        .map_err(err)
}

#[tauri::command]
async fn send_prompt(
    state: State<'_, AppState>,
    session_id: String,
    text: String,
    attachments: Vec<String>,
) -> Result<String, String> {
    use crate::chat::runtime::SendOutcome;
    match state.chat.send_prompt(&session_id, &text, &attachments).await {
        Ok(SendOutcome::Started) => Ok("sent".to_string()),
        Ok(SendOutcome::Enqueued) => Ok("enqueued".to_string()),
        Ok(SendOutcome::Rejected(reason)) => {
            Err(if reason.is_empty() { "发送被拒绝".to_string() } else { reason })
        }
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
async fn cancel(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.cancel(&session_id).await.map_err(err)
}

/// ACP config option picker (kimi "thinking"/"model"); refreshed options come
/// back via acp://configOptions.
#[tauri::command]
async fn set_config_option(
    state: State<'_, AppState>,
    session_id: String,
    config_id: String,
    value: String,
) -> Result<(), String> {
    state
        .chat
        .set_config_option(&session_id, &config_id, &value)
        .await
        .map_err(err)
}

/// Re-push cached configOptions after a session switch (the frontend cleared
/// its copy); the pickers arrive via acp://configOptions.
#[tauri::command]
async fn resend_config_options(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    state
        .chat
        .resend_config_options(&session_id)
        .await
        .map_err(err)
}

#[tauri::command]
async fn retry_cancel(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.retry_cancel(&session_id).await.map_err(err)
}

#[tauri::command]
async fn answer_permission(
    state: State<'_, AppState>,
    session_id: String,
    option_id: String,
    cancelled: bool,
) -> Result<(), String> {
    state
        .chat
        .answer_permission(&session_id, &option_id, cancelled)
        .await
        .map_err(err)
}

/// Pending permission payload for a session (null when none). Pulled by the
/// chat store after a session switch — the live acp://permission event only
/// reached whichever session was active when it fired.
#[tauri::command]
fn pending_permission(state: State<'_, AppState>, session_id: String) -> Value {
    state.chat.pending_permission(&session_id).unwrap_or(Value::Null)
}

/// Latest sub-agent/task list for a session (null when the runtime is gone).
/// Pulled by the chat store after a session switch — the live acp://subagent
/// event only reached whichever session was active when it fired.
#[tauri::command]
fn get_subagents(state: State<'_, AppState>, session_id: String) -> Value {
    state.chat.get_subagents(&session_id)
}

#[tauri::command]
async fn set_permission_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    state.chat.set_permission_mode(&mode).await.map_err(err)
}

#[tauri::command]
async fn switch_agent(
    state: State<'_, AppState>,
    session_id: String,
    agent_id: String,
) -> Result<(), String> {
    state.chat.switch_agent(&session_id, &agent_id).await.map_err(err)
}

#[tauri::command]
async fn guide_at(state: State<'_, AppState>, session_id: String, index: usize) -> Result<(), String> {
    state.chat.guide_at(&session_id, index).await.map_err(err)
}

#[tauri::command]
async fn remove_queue_at(
    state: State<'_, AppState>,
    session_id: String,
    index: usize,
) -> Result<(), String> {
    state.chat.remove_queue_at(&session_id, index).await.map_err(err)
}

#[tauri::command]
async fn clear_queue(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.clear_queue(&session_id).await.map_err(err)
}

#[tauri::command]
fn session_messages(state: State<'_, AppState>, session_id: String) -> Vec<Value> {
    let mut rows = state.chat.session_messages(&session_id);
    // Command rows persisted mid-run are stale after an app restart (the
    // runner is gone); never show a "streaming" command as running forever.
    if !state.runs.has_active_run(&session_id) {
        for r in &mut rows {
            if let Some(obj) = r.as_object_mut() {
                if obj.get("kind").and_then(Value::as_str) == Some("command")
                    && obj.get("status").and_then(Value::as_str) == Some("streaming")
                {
                    obj.insert("status".to_string(), Value::String("interrupted".to_string()));
                }
            }
        }
    }
    rows
}

/// Terminal command (Composer `!` prefix, cmd.rs): spawn cmd.exe in the
/// project dir, append a kind=="command" row and stream output via
/// term://output; returns the run id. `stores` is cloned first so no State
/// borrow survives the await.
#[tauri::command(async)]
async fn run_command(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    command: String,
    work_dir: String,
) -> Result<String, String> {
    let stores = state.stores.clone();
    let runs = state.runs.clone();
    runs.run(app, stores, &session_id, &command, &work_dir).await
}

#[tauri::command(async)]
async fn kill_command(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let runs = state.runs.clone();
    runs.kill(&run_id).await
}

#[tauri::command(async)]
fn runtime_states(state: State<'_, AppState>) -> Value {
    let t0 = std::time::Instant::now();
    let states = state.chat.runtime_states();
    let total = t0.elapsed();
    if total.as_millis() > 50 {
        log::info!("[perf] runtime_states total={total:?}");
    }
    json!(states
        .into_iter()
        .map(|(id, s)| {
            (id, json!({
                "busy": s.busy,
                "acpRunning": s.acp_running,
                "queueLength": s.queue_len,
                "queue": s.queue,
                "agentId": s.agent_id,
                "imageSupported": s.image_supported,
                "lastActivity": s.last_activity_ms,
                "permPending": s.perm_pending.is_some(),
            }))
        })
        .collect::<serde_json::Map<String, Value>>())
}

#[tauri::command(async)]
fn unread_sessions(state: State<'_, AppState>) -> Vec<String> {
    let t0 = std::time::Instant::now();
    let v = state.chat.unread_ids();
    let total = t0.elapsed();
    if total.as_millis() > 50 {
        log::info!("[perf] unread_sessions total={total:?}");
    }
    v
}

// ---------------------------------------------------------------------------
// Commands: sessions / search
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.sessions.list()).unwrap_or(Value::Null)
}

#[tauri::command(async)]
fn sessions_for_project(state: State<'_, AppState>, project_dir: String) -> Value {
    // Switch-lag instrumentation: the frontend sees ~1s stalls here; split
    // lock-wait from body time to find who holds the stores mutex.
    let t0 = std::time::Instant::now();
    let stores = lock(&state.stores);
    let t_lock = t0.elapsed();
    let v = serde_json::to_value(stores.sessions.sessions_for_project(&project_dir))
        .unwrap_or(Value::Null);
    let total = t0.elapsed();
    if total.as_millis() > 50 {
        log::info!("[perf] sessions_for_project lock={t_lock:?} body={:?}", total - t_lock);
    }
    v
}

#[tauri::command]
fn session_meta(state: State<'_, AppState>, session_id: String) -> Value {
    let mut stores = lock(&state.stores);
    stores
        .sessions
        .meta_for(&session_id)
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .unwrap_or(Value::Null)
}

/// Fork a session at a message: new session with messages [0..=that one],
/// same agent/provider/project; acpSessionId is NOT inherited. The fork is
/// recorded as a sub-session (parentId/sourceMessageId in meta). `title`
/// (optional, default "") overrides the derived title — the selection-ask
/// entry passes a summary of the selected text. Returns the new session's
/// meta.
#[tauri::command]
fn branch_session(
    state: State<'_, AppState>,
    session_id: String,
    up_to_message_id: String,
    title: Option<String>,
) -> Result<Value, String> {
    let mut stores = lock(&state.stores);
    stores
        .sessions
        .branch_session(&session_id, &up_to_message_id, title.as_deref())
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .map_err(err)
}

/// Full-text search (data-formats.md §10): runs on the blocking pool with
/// generation-based supersede; a superseded scan returns an empty list.
#[tauri::command]
async fn search_messages(state: State<'_, AppState>, query: String) -> Result<Value, String> {
    let (engine, targets) = {
        let stores = lock(&state.stores);
        (stores.search.clone(), stores.sessions.search_targets())
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || engine.search(&targets, &query, 50))
        .await
        .map_err(err)?;
    match outcome {
        store::SearchOutcome::Superseded => Ok(json!([])),
        store::SearchOutcome::Done { results, .. } => {
            Ok(serde_json::to_value(results).unwrap_or(json!([])))
        }
    }
}

// ---------------------------------------------------------------------------
// Commands: agents / providers / probe
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_agents(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    json!({
        "agents": stores.agents.agents(),
        "defaultAgentId": stores.agents.default_agent_id(),
    })
}

#[tauri::command]
fn create_agent(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.agents.create_agent(&paths, &name).map_err(err)
}

#[tauri::command]
fn save_agent(
    state: State<'_, AppState>,
    agent_id: String,
    patch: AgentPatch,
) -> Result<Option<String>, String> {
    if let Some(v) = &patch.default_effort {
        let v = v.trim().to_lowercase();
        if !v.is_empty() && !crate::models::EFFORT_LEVELS.contains(&v.as_str()) {
            return Err(format!("无效的思考强度: {v}"));
        }
    }
    if let Some(list) = &patch.effort_options {
        for s in list {
            if !crate::models::EFFORT_LEVELS.contains(&s.trim().to_lowercase().as_str()) {
                return Err(format!("无效的思考强度: {s}"));
            }
        }
    }
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.agents.update_agent(&paths, &agent_id, &patch).map_err(err)?;
    // Declare the agent's model in the kimi CLI config with support_efforts =
    // the selected levels so the ACP thinking picker offers exactly those
    // (empty selection = every level); a cleared model removes the section.
    // Sync failures never block the save — they come back as a warning.
    let Some(agent) = stores.agents.get(&agent_id).cloned() else {
        return Ok(None);
    };
    if agent.provider != "kimi" {
        return Ok(None);
    }
    let result = if agent.model.trim().is_empty() {
        Ok(())
    } else {
        crate::models::sync_kimi_effort_model(
            agent.model.trim(),
            &agent.base_url,
            &agent.api_key,
            &agent.effort_options,
            &agent.default_effort,
            agent.max_context_k,
        )
    };
    match result {
        Ok(()) => Ok(None),
        Err(e) => Ok(Some(format!("思考强度同步 kimi config.toml 失败: {e}"))),
    }
}

#[tauri::command]
fn delete_agent(state: State<'_, AppState>, agent_id: String) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.agents.remove_agent(&paths, &agent_id).map_err(err)
}

#[tauri::command]
fn set_default_agent(state: State<'_, AppState>, agent_id: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.agents.set_default(&paths, &agent_id).map_err(err)
}

#[tauri::command]
fn provider_specs() -> Value {
    json!(provider::ids()
        .filter_map(provider::spec_view)
        .collect::<Vec<_>>())
}

#[tauri::command]
async fn probe_cli(
    state: State<'_, AppState>,
    provider_id: String,
    preferred_path: String,
) -> Result<Value, String> {
    let result = state.probe.lock().await.probe(&provider_id, &preferred_path).await;
    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
}

#[tauri::command]
async fn test_agent(state: State<'_, AppState>, agent_id: String) -> Result<Option<String>, String> {
    // Clone the (small) store so no std MutexGuard crosses the await.
    let agents = lock(&state.stores).agents.clone();
    let result = state.tester.test_agent(&agents, &agent_id).await;
    Ok(result)
}

// ---------------------------------------------------------------------------
// Commands: projects / workspace / files
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    json!({
        "recent": stores.projects.recent(),
        "aliases": stores.projects.aliases(),
    })
}

#[tauri::command]
fn open_project(state: State<'_, AppState>, dir: String) -> Result<Value, String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.projects.touch_project(&paths, &dir).map_err(err)?;
    let display = stores.projects.display_name_for(&dir);
    Ok(json!({ "dir": store::canonical_dir(&dir), "displayName": display }))
}

#[tauri::command]
fn remove_project(state: State<'_, AppState>, dir: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.projects.remove_project(&paths, &dir).map_err(err)
}

#[tauri::command]
fn set_project_alias(state: State<'_, AppState>, dir: String, alias: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.projects.set_alias(&paths, &dir, &alias).map_err(err)
}

/// Directory existence check (projectStore.exists in the old app): the
/// enter-session / open-recent-project guards need it before touching
//  sessions. Pure filesystem probe, no store state.
#[tauri::command]
fn project_exists(dir: String) -> bool {
    !dir.trim().is_empty() && std::path::Path::new(&dir).is_dir()
}

// ---- folder browser (FolderBrowserModel equivalent, store/browse.rs) ----

#[tauri::command]
fn folder_drives() -> Vec<String> {
    store::browse::drives()
}

#[tauri::command]
async fn folder_list(dir: String) -> Result<Value, String> {
    let entries = tauri::async_runtime::spawn_blocking(move || store::browse::list_dirs(&dir))
        .await
        .map_err(err)?;
    Ok(serde_json::to_value(entries).unwrap_or(json!([])))
}

/// Address-bar validation for the folder browser: list_dirs silently returns
/// [] for missing dirs, so the editable path bar probes this first.
#[tauri::command]
fn folder_exists(dir: String) -> bool {
    std::path::Path::new(&dir).is_dir()
}

#[tauri::command]
async fn folder_create(dir: String, name: String) -> Result<Value, String> {
    let entry = tauri::async_runtime::spawn_blocking(move || store::browse::create_dir(&dir, &name))
        .await
        .map_err(err)?;
    Ok(serde_json::to_value(entry?).unwrap_or(Value::Null))
}

/// Kimi install-guide dialog content (probe.rs constants; the config page
/// must not hardcode provider text, red line C3).
#[tauri::command]
fn install_help() -> Value {
    json!({ "text": probe::INSTALL_HELP_TEXT, "url": probe::INSTALL_HELP_URL })
}

#[tauri::command]
async fn read_file_range(
    root: String,
    rel_path: String,
    from: i64,
    to: i64,
) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        store::workspace::read_file_range(&root, &rel_path, from, to)
    })
    .await
    .map_err(err)?;
    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
}

#[tauri::command]
async fn preview_file(path: String) -> Result<Value, String> {
    let result =
        tauri::async_runtime::spawn_blocking(move || store::workspace::preview_file(&path))
            .await
            .map_err(err)?;
    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
}

#[tauri::command]
async fn save_preview(path: String, content: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        store::workspace::save_preview_text(&path, &content)
    })
    .await
    .map_err(err)?;
    Ok(serde_json::to_value(result).unwrap_or(Value::Null))
}

#[tauri::command]
async fn workspace_files(root: String, filter: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        store::workspace::workspace_file_list(std::path::Path::new(&root), &filter, 50)
    })
    .await
    .map_err(err)
}

/// Project-wide code search (Ctrl+F overlay on the chat page). Runs on the
/// blocking pool; results are {file, hits[{line, text}]} — line 0 marks a
/// filename-only match. `mode` = content | filename | both; `exts` = file
/// extension whitelist (leading dot/wildcard stripped, case-insensitive,
/// empty = all text files); `regex` treats the query as a case-insensitive
/// regex (invalid patterns return an error).
///
/// NOTE: separate String/Vec args (not one struct) — Tauri v2 resolves each
/// parameter by its own key in the invoke payload; a single struct arg would
/// require a `params` key instead (see the InvalidArgs failure this caused).
#[tauri::command]
async fn search_code(
    root: String,
    query: String,
    mode: Option<String>,
    exts: Option<Vec<String>>,
    regex: Option<bool>,
) -> Result<Value, String> {
    let mode = match mode.as_deref().unwrap_or("content") {
        "filename" => store::workspace::SearchMode::Filename,
        "both" => store::workspace::SearchMode::Both,
        _ => store::workspace::SearchMode::Content,
    };
    let exts = exts.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let exts: Vec<String> = exts
            .iter()
            .map(|e| e.trim().trim_start_matches('*').trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let opts = store::workspace::CodeSearchOptions {
            mode,
            exts: &exts,
            regex: regex.unwrap_or(false),
        };
        store::workspace::search_code(std::path::Path::new(&root), &query, &opts)
    })
    .await
    .map_err(err)?;
    let v = result.map_err(err)?;
    Ok(serde_json::to_value(v).map_err(err)?)
}

// ---------------------------------------------------------------------------
// Commands: codegraph (Ctrl+\ interface lookup, codegraph.rs)
// ---------------------------------------------------------------------------

/// Full Ctrl+\ overlay status: `installed` comes from the prefs-cached probe
/// (probed once on first use; 重新检测 re-probes and updates the cache),
/// plus the CLI path, per-project build state and index existence.
#[tauri::command(async)]
async fn codegraph_status(
    state: State<'_, AppState>,
    project_dir: String,
) -> Result<Value, String> {
    let prefs_installed = {
        let stores = lock(&state.stores);
        stores.prefs.codegraph_installed()
    };
    let installed = match prefs_installed {
        Some(v) => v,
        None => {
            let found = state.codegraph.resolve().is_some();
            {
                let mut stores = lock(&state.stores);
                let paths = stores.paths.clone();
                let _ = stores.prefs.set_codegraph_installed(&paths, found);
            }
            found
        }
    };
    let mut v = state.codegraph.status(&project_dir);
    v["installed"] = json!(installed);
    Ok(v)
}

/// Re-probe for the codegraph CLI and refresh the prefs cache (the install
/// card's 重新检测 button — no app restart needed).
#[tauri::command(async)]
async fn codegraph_reprobe(state: State<'_, AppState>) -> Result<Value, String> {
    state.codegraph.invalidate();
    let found = state.codegraph.resolve().is_some();
    {
        let mut stores = lock(&state.stores);
        let paths = stores.paths.clone();
        let _ = stores.prefs.set_codegraph_installed(&paths, found);
    }
    Ok(json!({ "installed": found }))
}

/// Start `codegraph build <dir>` in the background; progress is read via
/// codegraph_status polling (or the codegraph://build event).
#[tauri::command(async)]
async fn codegraph_build(
    app: AppHandle,
    state: State<'_, AppState>,
    project_dir: String,
) -> Result<(), String> {
    state.codegraph.start_build(app, project_dir);
    Ok(())
}

/// Interface name search against the codegraph index (empty query = the CLI
/// rejects it, so it maps to the "empty" error the overlay turns into a hint).
#[tauri::command(async)]
async fn codegraph_query_interfaces(
    state: State<'_, AppState>,
    project_dir: String,
    query: String,
) -> Result<Value, String> {
    let hits = state.codegraph.query_interfaces(&project_dir, &query).await.map_err(err)?;
    Ok(serde_json::to_value(hits).map_err(err)?)
}

/// Open the interactive dependency graph in the default browser.
#[tauri::command(async)]
async fn codegraph_plot(state: State<'_, AppState>, project_dir: String) -> Result<(), String> {
    state.codegraph.plot(&project_dir)
}

/// Java `interface` declarations (Ctrl+\ overlay). Heuristic text scan,
/// results {file, line, name, text}; empty query lists every declaration.
#[tauri::command]
async fn search_java_interfaces(root: String, query: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        store::workspace::search_java_interfaces(std::path::Path::new(&root), &query)
    })
    .await
    .map_err(err)?;
    Ok(serde_json::to_value(result).map_err(err)?)
}

#[tauri::command]
fn git_branch(dir: String) -> String {
    store::workspace::git_branch_for(&dir)
}

/// Commit history for the version-control panel (inspect/git.rs). Runs on the
/// blocking pool; the spawn itself has a 4s ceiling inside git_log.
#[tauri::command]
async fn git_log(dir: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        inspect::git::git_log(&dir, inspect::git::GIT_LOG_MAX)
    })
    .await
    .map_err(err)?;
    let commits = result.map_err(err)?;
    Ok(serde_json::to_value(commits).unwrap_or(json!([])))
}

/// Working-tree change list for the panel's 更改 view (inspect/git.rs).
#[tauri::command]
async fn git_status(dir: String) -> Result<Value, String> {
    let result =
        tauri::async_runtime::spawn_blocking(move || inspect::git::git_status(&dir))
            .await
            .map_err(err)?;
    let entries = result.map_err(err)?;
    Ok(serde_json::to_value(entries).unwrap_or(json!([])))
}

/// Diff of one file (mode: worktree | staged | untracked), R4-capped.
#[tauri::command]
async fn git_diff_file(dir: String, path: String, mode: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        inspect::git::git_diff_file_mode(&dir, &path, &mode)
    })
    .await
    .map_err(err)?;
    let diff = result.map_err(err)?;
    Ok(serde_json::to_value(diff).unwrap_or(json!({})))
}

/// Diff of one commit vs its parent, R4-capped.
#[tauri::command]
async fn git_diff_commit(dir: String, hash: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        inspect::git::git_diff_commit(&dir, &hash)
    })
    .await
    .map_err(err)?;
    let diff = result.map_err(err)?;
    Ok(serde_json::to_value(diff).unwrap_or(json!({})))
}

/// Sub-agent process steps from the CLI's on-disk wire.jsonl
/// (inspect/subagent.rs; kimi-code only, degrades to an error string).
#[tauri::command]
async fn subagent_process(
    state: State<'_, AppState>,
    session_id: String,
    agent_id: String,
) -> Result<Value, String> {
    let acp_session_id = {
        let mut stores = lock(&state.stores);
        stores.sessions.acp_session_id_for(&session_id)
    };
    tauri::async_runtime::spawn_blocking(move || {
        inspect::subagent::read_subagent_process(&acp_session_id, &agent_id)
    })
    .await
    .map_err(err)?
}

/// One directory level of the workspace tree (inspect/files.rs; lazy expand).
#[tauri::command]
async fn list_workspace_dir(root: String, rel: String) -> Result<Value, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        inspect::files::list_workspace_dir(&root, &rel)
    })
    .await
    .map_err(err)?;
    let entries = result.map_err(err)?;
    Ok(serde_json::to_value(entries).unwrap_or(json!([])))
}

/// Clipboard image persistence (store/media.rs): the webview hands over the
/// pasted blob's bytes; decode + the PNG→downscale→JPEG chain run here.
/// Returns the native-separator absolute path for the attachment list, or ""
/// on failure (old code never throws on this path).
#[tauri::command]
async fn save_clipboard_image(
    state: State<'_, AppState>,
    session_id: String,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let paths = lock(&state.stores).paths.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        Ok::<String, String>(store::media::save_clipboard_image(
            &paths,
            &session_id,
            img.to_rgba8(),
        ))
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------------------
// Commands: todos (unified todo/reminder model, store/todos.rs)
// ---------------------------------------------------------------------------

/// Grouped view: pending session rows for `session_id`, pending project rows
/// for `project_dir`, pending global rows, and every done row.
#[tauri::command]
fn todos_list(state: State<'_, AppState>, session_id: String, project_dir: String) -> Value {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.reload(&paths);
    let (session, project, global) = stores.todos.pending_grouped();
    let done = stores.todos.done();
    json!({
        "session": session.iter().filter(|r| r.session_id == session_id).cloned().collect::<Vec<_>>(),
        "project": project.iter().filter(|r| r.project_dir == project_dir).cloned().collect::<Vec<_>>(),
        "global": global,
        "done": done,
    })
}

#[tauri::command]
async fn todo_add(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    title: String,
    scope: String,
    session_id: String,
    project_dir: String,
    due_at_ms: i64,
    notify_mode: String,
) -> Result<Value, String> {
    use crate::store::todos::NOTIFY_POPUP;
    let mode = if notify_mode.is_empty() { NOTIFY_POPUP } else { &notify_mode };
    let row = {
        let mut stores = lock(&state.stores);
        let paths = stores.paths.clone();
        stores
            .todos
            .add(&paths, &title, &scope, &session_id, &project_dir, due_at_ms, mode)
            .map_err(err)?
    };
    let Some(row) = row else {
        return Err("内容不能为空，且会话级需会话、项目级需项目".to_string());
    };
    if !session_id.is_empty() {
        state.chat.reminders_reload(&session_id).await;
    }
    app.emit("todos://changed", json!({})).ok();
    serde_json::to_value(row).map_err(err)
}

#[tauri::command]
async fn todo_toggle(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    {
        let mut stores = lock(&state.stores);
        let paths = stores.paths.clone();
        stores.todos.toggle(&paths, &id).map_err(err)?;
    }
    state.chat.reminders_reload_all().await;
    app.emit("todos://changed", json!({})).ok();
    Ok(())
}

#[tauri::command]
async fn todo_remove(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    {
        let mut stores = lock(&state.stores);
        let paths = stores.paths.clone();
        stores.todos.remove(&paths, &id).map_err(err)?;
    }
    state.chat.reminders_reload_all().await;
    app.emit("todos://changed", json!({})).ok();
    Ok(())
}

#[tauri::command]
async fn todos_clear_done(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.clear_done(&paths).map_err(err)?;
    app.emit("todos://changed", json!({})).ok();
    Ok(())
}

/// Send a reminder-styled prompt into a session (project-due 三选:
/// 跳转并处理 / 后台处理). The session runtime marks it kind=="reminder".
#[tauri::command]
async fn send_reminder(
    state: State<'_, AppState>,
    session_id: String,
    text: String,
) -> Result<String, String> {
    use crate::chat::runtime::SendOutcome;
    match state
        .chat
        .send_prompt_kind(&session_id, &text, &[], "reminder")
        .await
    {
        Ok(SendOutcome::Started) => Ok("sent".to_string()),
        Ok(SendOutcome::Enqueued) => Ok("enqueued".to_string()),
        Ok(SendOutcome::Rejected(reason)) => Err(if reason.is_empty() {
            "发送被拒绝".to_string()
        } else {
            reason
        }),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
fn prompts_list(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.prompts.rows()).unwrap_or(Value::Null)
}

#[tauri::command]
fn prompt_add(state: State<'_, AppState>, name: String, text: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prompts.add(&paths, &name, &text).map_err(err)
}

#[tauri::command]
fn prompt_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prompts.remove(&paths, &id).map_err(err)
}

#[tauri::command]
fn usage_report(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.usage.report()).unwrap_or(Value::Null)
}

/// Per-session usage for the 会话信息 panel: one in-memory aggregation over
/// that session's records (cheap; no disk IO, UsageStore stays resident).
#[tauri::command]
fn session_usage(state: State<'_, AppState>, session_id: String) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.usage.for_session(&session_id)).unwrap_or(Value::Null)
}

#[tauri::command]
fn usage_backfill(state: State<'_, AppState>, app: AppHandle) -> Value {
    let mut stores = lock(&state.stores);
    let result = usage_backfill::backfill(&mut stores);
    if result.added > 0 {
        // 让当前打开的会话立即重挂历史用量（chat store 监听后重新拉取消息）。
        let _ = app.emit("usage://backfilled", json!({}));
    }
    serde_json::to_value(result).unwrap_or(Value::Null)
}

#[tauri::command]
fn get_prefs(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    json!({
        "userName": stores.prefs.user_name(),
        "permissionMode": stores.prefs.permission_mode(),
        "fontScale": stores.prefs.font_scale(),
        "previewWidth": stores.prefs.preview_width(),
        "previewHeight": stores.prefs.preview_height(),
        "railWidth": stores.prefs.rail_width(),
        "panelLayout": stores.prefs.panel_layout(),
        "monitorLayout": stores.prefs.monitor_layout(),
        "panelWidth": stores.prefs.panel_width(),
        "composerHeight": stores.prefs.composer_height(),
        "actionBayWidth": stores.prefs.action_bay_width(),
        "actionBayHeight": stores.prefs.action_bay_height(),
        "monitorChatWidth": stores.prefs.monitor_chat_width(),
        "monitorChatHeight": stores.prefs.monitor_chat_height(),
        "userAvatarPath": stores.prefs.user_avatar_path(),
    })
}

/// Background config, resolved next to the exe (old main.cpp:96-122 rules):
/// default muted-looping video (background-default.mp4); background.json
/// overrides {type, source}; relative source anchors at the exe dir;
/// file:/absolute sources are returned as plain filesystem paths (the
/// webview converts them via the asset protocol); qrc: passes through
/// (frontend maps to bundled /assets); `video` plays in a muted looping
/// <video> (WebView2 has native H.264, no FFmpeg).
#[tauri::command]
fn background_config() -> Value {
    const DEFAULT_SOURCE: &str = "qrc:/qt/qml/WarDex/assets/background/background-default.mp4";
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut bg_type = "video".to_string();
    let mut bg_source = DEFAULT_SOURCE.to_string();
    if let Some(dir) = exe_dir {
        if let Ok(text) = std::fs::read_to_string(dir.join("background.json")) {
            if let Ok(obj) = serde_json::from_str::<Value>(&text) {
                if let Some(t) = obj.get("type").and_then(Value::as_str) {
                    bg_type = t.to_string();
                }
                let src = obj.get("source").and_then(Value::as_str).unwrap_or("");
                if !src.is_empty() {
                    if src.starts_with("qrc:") {
                        bg_source = src.to_string();
                    } else if let Some(rest) = src.strip_prefix("file://") {
                        bg_source = rest.to_string();
                    } else {
                        let p = std::path::Path::new(src);
                        let abs = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                        bg_source = abs.to_string_lossy().to_string();
                    }
                }
            }
        }
    }
    json!({ "type": bg_type, "source": bg_source })
}

#[tauri::command]
fn set_user_name(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_user_name(&paths, &name).map_err(err)
}

/// userPrefs.setUserAvatarFromFile (§8.1): import an image into the data dir
/// (center-crop → 128×128 PNG) and point userAvatarPath at it. False when
/// the file is missing/undecodable — the UI shows 头像导入失败.
#[tauri::command]
fn set_user_avatar_from_file(
    state: State<'_, AppState>,
    local_path: String,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores
        .prefs
        .set_user_avatar_from_file(&paths, &local_path)
        .map_err(err)
}

/// clearUserAvatar: drop the custom file and fall back to the built-in
/// blonde portrait.
#[tauri::command]
fn clear_user_avatar(state: State<'_, AppState>) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.clear_user_avatar(&paths).map_err(err)
}

#[tauri::command]
fn set_font_scale(state: State<'_, AppState>, scale: f64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_font_scale(&paths, scale).map_err(err)
}

#[tauri::command]
fn set_preview_size(state: State<'_, AppState>, width: i64, height: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_preview_width(&paths, width).map_err(err)?;
    stores.prefs.set_preview_height(&paths, height).map_err(err)
}

/// Monitor page mini chat window size (corner-grip drag; 0/0 clears back to
/// the frontend default 560×74vh).
#[tauri::command]
fn set_monitor_chat_size(state: State<'_, AppState>, width: i64, height: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_monitor_chat_width(&paths, width).map_err(err)?;
    stores.prefs.set_monitor_chat_height(&paths, height).map_err(err)
}

#[tauri::command]
fn set_panel_layout(
    state: State<'_, AppState>,
    panel_id: String,
    entry: PanelLayoutEntry,
) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_panel_layout(&paths, &panel_id, &entry).map_err(err)
}

/// Monitor page sandbox layout: key=projectDir, value={x: 0..1, y: 0..1}
/// (relative coords, window-resize safe); entry=None razes the barracks.
/// Deploy (entry=Some) is an empty-barracks start AND raze (entry=None) also
/// clears the field: in both cases the project's un-shelved sessions are all
/// shelved in the same call (restorable via the building menu).
#[tauri::command]
fn set_monitor_layout(
    state: State<'_, AppState>,
    project_dir: String,
    entry: Option<Value>,
) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores
        .prefs
        .set_monitor_layout(&paths, &project_dir, entry)
        .map_err(err)?;
    stores.sessions.shelve_all_for_project(&project_dir);
    Ok(())
}

/// Shared right-dock drawer width (px) — one width for ALL dock tabs,
/// dragged live and persisted once on release (mirrors set_rail_width).
#[tauri::command]
fn set_panel_width(state: State<'_, AppState>, width: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_panel_width(&paths, width).map_err(err)
}

/// Chat-page left rail column width (px). The frontend drags it live and
/// persists once on release; clamp 180..340 keeps both panes usable.
#[tauri::command]
fn set_rail_width(state: State<'_, AppState>, width: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_rail_width(&paths, width).map_err(err)
}

/// 输入框高度 (px)：拖拽中只改本地 state，松手持久化一次；0 清回响应式默认。
#[tauri::command]
fn set_composer_height(state: State<'_, AppState>, height: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_composer_height(&paths, height).map_err(err)
}

/// 右下操作台宽度 (px)：拖拽中只改本地 state，松手持久化一次；0 清回默认 354。
#[tauri::command]
fn set_action_bay_width(state: State<'_, AppState>, width: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_action_bay_width(&paths, width).map_err(err)
}

/// 右下操作台高度 (px)：拖拽中只改本地 state，松手持久化一次；0 清回响应式默认。
#[tauri::command]
fn set_action_bay_height(state: State<'_, AppState>, height: i64) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.prefs.set_action_bay_height(&paths, height).map_err(err)
}

// ---------------------------------------------------------------------------
// Rail groups (groups.json): per-project session buckets. Every mutation
// emits store://sessions so the rail re-pulls both the list and the groups.
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_groups(state: State<'_, AppState>, project_dir: String) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.sessions.groups_for(&project_dir)).unwrap_or(Value::Null)
}

#[tauri::command]
fn create_group(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_dir: String,
    name: String,
) -> Result<Value, String> {
    let mut stores = lock(&state.stores);
    let g = stores
        .sessions
        .create_group(&project_dir, &name)
        .map_err(err)?;
    app.emit("store://sessions", json!({})).ok();
    serde_json::to_value(g).map_err(err)
}

#[tauri::command]
fn rename_group(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    name: String,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    let ok = stores.sessions.rename_group(&group_id, &name).map_err(err)?;
    if ok {
        app.emit("store://sessions", json!({})).ok();
    }
    Ok(ok)
}

/// Delete a group + cascade-delete every session in it (confirm dialog is
/// the frontend's job). Returns the number of removed sessions.
#[tauri::command]
fn delete_group(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    group_id: String,
) -> Result<usize, String> {
    let removed = {
        let mut stores = lock(&state.stores);
        stores.sessions.delete_group(&group_id).map_err(err)?
    };
    state.chat.drop_deleted_runtimes(&removed);
    app.emit("store://sessions", json!({})).ok();
    Ok(removed.len())
}

#[tauri::command]
fn move_session_group(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    group_id: String,
) -> Result<bool, String> {
    let mut stores = lock(&state.stores);
    let ok = stores
        .sessions
        .move_session_group(&session_id, &group_id)
        .map_err(err)?;
    if ok {
        app.emit("store://sessions", json!({})).ok();
    }
    Ok(ok)
}

// ---------------------------------------------------------------------------
// Model list probing (models.rs)
// ---------------------------------------------------------------------------

/// GET {baseUrl}/models (OpenAI-compatible); the /chat/completions suffix is
/// stripped before appending /models. Returns sorted model ids.
/// `command(async)`: blocking ureq on the main thread stalled every other
/// sync command behind it (~1s per switch when the endpoint is slow).
#[tauri::command(async)]
fn fetch_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    crate::models::fetch_models(&base_url, &api_key)
}

/// Model aliases from the [models] table of ~/.kimi-code/config.toml.
#[tauri::command]
fn kimi_model_aliases() -> Vec<String> {
    crate::models::kimi_model_aliases()
}

/// Thinking effort levels for the config-page strength multi-select.
#[tauri::command]
fn effort_options() -> Vec<String> {
    crate::models::EFFORT_LEVELS.map(str::to_string).into()
}

// ---------------------------------------------------------------------------
// App entry
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let boot = std::time::Instant::now();
    let paths = Paths::production();
    paths.ensure_layout();
    init_logging(&paths);
    install_panic_hook(&paths);
    log::info!("startup: paths + logging ready ({:?})", boot.elapsed());

    let t = std::time::Instant::now();
    let stores = Arc::new(Mutex::new(StoreRegistry::init(paths.clone())));
    log::info!("startup: stores loaded ({:?})", t.elapsed());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let t = std::time::Instant::now();
            let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app.handle().clone()));
            let chat = Arc::new(ChatManager::new(stores.clone(), sink));
            // App-level due tick for todos (popup rows → notification; project
            // rows → auto new session). 30s cadence; push rows are the
            // per-session runtimes' own timers.
            {
                let chat = chat.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        chat.tick_due_todos().await;
                    }
                });
            }
            app.manage(AppState {
                chat,
                stores,
                probe: tokio::sync::Mutex::new(probe::CliProbe::new()),
                tester: probe::AgentTester::new(),
                runs: cmd::CommandRunner::new(),
                codegraph: codegraph::CodegraphRunner::new(),
                db: db::DbManager::new(),
            });
            log::info!(
                "startup: chat manager + state ready ({:?}, total {:?})",
                t.elapsed(),
                boot.elapsed()
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // chat runtime
            create_session,
            open_session,
            ensure_runtime,
            close_session,
            delete_session,
            set_active_session,
            rename_session,
            set_session_project,
            set_session_pinned,
            set_session_shelved,
            set_session_perm_mode,
            send_prompt,
            cancel,
            set_config_option,
            resend_config_options,
            retry_cancel,
            answer_permission,
            pending_permission,
            get_subagents,
            set_permission_mode,
            switch_agent,
            guide_at,
            remove_queue_at,
            clear_queue,
            session_messages,
            runtime_states,
            unread_sessions,
            // terminal commands (`!` prefix)
            run_command,
            kill_command,
            // sessions / search
            list_sessions,
            sessions_for_project,
            session_meta,
            branch_session,
            search_messages,
            // agents / providers / probe
            list_agents,
            create_agent,
            save_agent,
            delete_agent,
            set_default_agent,
            provider_specs,
            probe_cli,
            test_agent,
            // projects / workspace / files
            list_projects,
            open_project,
            remove_project,
            set_project_alias,
            project_exists,
            folder_drives,
            folder_list,
            folder_exists,
            folder_create,
            install_help,
            read_file_range,
            preview_file,
            save_preview,
            workspace_files,
            search_code,
            search_java_interfaces,
            // codegraph (Ctrl+\ interface lookup)
            codegraph_status,
            codegraph_reprobe,
            codegraph_build,
            codegraph_query_interfaces,
            codegraph_plot,
            git_branch,
            git_log,
            git_status,
            git_diff_file,
            git_diff_commit,
            subagent_process,
            list_workspace_dir,
            save_clipboard_image,
            // todos / prompts / prefs
            todos_list,
            todo_add,
            todo_toggle,
            todo_remove,
            todos_clear_done,
            send_reminder,
            prompts_list,
            prompt_add,
            prompt_remove,
            usage_report,
            usage_backfill,
            session_usage,
            get_prefs,
            background_config,
            set_user_name,
            set_user_avatar_from_file,
            clear_user_avatar,
            set_font_scale,
            set_preview_size,
            set_monitor_chat_size,
            set_panel_layout,
            set_monitor_layout,
            set_panel_width,
            set_rail_width,
            set_composer_height,
            set_action_bay_width,
            set_action_bay_height,
            list_groups,
            create_group,
            rename_group,
            delete_group,
            move_session_group,
            // model list probing
            fetch_models,
            kimi_model_aliases,
            effort_options,
            // database (db/commands.rs)
            db::commands::db_conns,
            db::commands::db_save_conns,
            db::commands::db_set_alias,
            db::commands::db_open,
            db::commands::db_close,
            db::commands::db_close_all,
            db::commands::db_tables,
            db::commands::db_columns,
            db::commands::db_execute,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WarDex");
}
