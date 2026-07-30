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
//     acp://permission {sessionId, requestId, params}
//     acp://permissionCleared {sessionId}
//     acp://subagent   {sessionId, subagents[]}
//     chat://messageAppended {sessionId, row}   (user 行 + assistant 占位行)
//     chat://bubbleSet {sessionId, row}         (整段替换: 中断/错误/重试提示)
//     chat://status    {sessionId, statusText, busy, queueLength, retry*, lastError, acpReady, imageSupported}
//     chat://retry     {sessionId, active, countdown, attempt, maxAttempts}
//     chat://unread    {sessionId}
//     store://sessions / store://prefs           (粗粒度变更通知，前端重拉)
// Phase 3b additions (thin shells over existing store/probe helpers):
//     project_exists / folder_drives / folder_list / folder_create
//     install_help / set_user_avatar_from_file / clear_user_avatar

pub mod acp;
pub mod chat;
pub mod inspect;
pub mod probe;
pub mod provider;
pub mod store;

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{Emitter, Manager, State};

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
}

struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(e) = self.0.emit(event, payload) {
            log::warn!("emit {event} failed: {e}");
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
async fn create_session(state: State<'_, AppState>, project_dir: String) -> Result<String, String> {
    state.chat.create_session(&project_dir).await.map_err(err)
}

#[tauri::command]
async fn open_session(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.open_session(&session_id).await.map_err(err)
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

#[tauri::command]
async fn send_prompt(
    state: State<'_, AppState>,
    session_id: String,
    text: String,
    attachments: Vec<String>,
) -> Result<bool, String> {
    state
        .chat
        .send_prompt(&session_id, &text, &attachments)
        .await
        .map_err(err)
}

#[tauri::command]
async fn cancel(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    state.chat.cancel(&session_id).await.map_err(err)
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
    state.chat.session_messages(&session_id)
}

#[tauri::command]
fn runtime_states(state: State<'_, AppState>) -> Value {
    let states = state.chat.runtime_states();
    json!(states
        .into_iter()
        .map(|(id, s)| {
            (id, json!({
                "busy": s.busy,
                "acpRunning": s.acp_running,
                "queueLength": s.queue_len,
                "agentId": s.agent_id,
                "imageSupported": s.image_supported,
                "lastActivity": s.last_activity_ms,
            }))
        })
        .collect::<serde_json::Map<String, Value>>())
}

#[tauri::command]
fn unread_sessions(state: State<'_, AppState>) -> Vec<String> {
    state.chat.unread_ids()
}

// ---------------------------------------------------------------------------
// Commands: sessions / search
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.sessions.list()).unwrap_or(Value::Null)
}

#[tauri::command]
fn sessions_for_project(state: State<'_, AppState>, project_dir: String) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.sessions.sessions_for_project(&project_dir)).unwrap_or(Value::Null)
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
) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.agents.update_agent(&paths, &agent_id, &patch).map_err(err)
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
// Commands: todos / prompts / prefs
// ---------------------------------------------------------------------------

#[tauri::command]
fn todos_list(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    serde_json::to_value(stores.todos.rows()).unwrap_or(Value::Null)
}

#[tauri::command]
fn todo_add(state: State<'_, AppState>, title: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.add(&paths, &title).map_err(err)
}

#[tauri::command]
fn todo_toggle(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.toggle(&paths, &id).map_err(err)
}

#[tauri::command]
fn todo_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.remove(&paths, &id).map_err(err)
}

#[tauri::command]
fn todos_clear_done(state: State<'_, AppState>) -> Result<(), String> {
    let mut stores = lock(&state.stores);
    let paths = stores.paths.clone();
    stores.todos.clear_done(&paths).map_err(err)
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
fn get_prefs(state: State<'_, AppState>) -> Value {
    let stores = lock(&state.stores);
    json!({
        "userName": stores.prefs.user_name(),
        "permissionMode": stores.prefs.permission_mode(),
        "fontScale": stores.prefs.font_scale(),
        "previewWidth": stores.prefs.preview_width(),
        "previewHeight": stores.prefs.preview_height(),
        "panelLayout": stores.prefs.panel_layout(),
        "userAvatarPath": stores.prefs.user_avatar_path(),
    })
}

/// Background config, resolved next to the exe (old main.cpp:96-122 rules):
/// default image; background.json overrides {type, source}; relative source
/// anchors at the exe dir; file:/absolute sources are returned as plain
/// filesystem paths (the webview converts them via the asset protocol);
/// qrc: passes through (frontend maps to bundled /assets); `video` was
/// removed and falls back to the default image.
#[tauri::command]
fn background_config() -> Value {
    const DEFAULT_SOURCE: &str = "qrc:/qt/qml/WarDex/assets/background/LodolonFall.jpg";
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut bg_type = "image".to_string();
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
    if bg_type == "video" {
        log::info!("bg type=video is removed; forcing image");
        bg_type = "image".to_string();
        bg_source = DEFAULT_SOURCE.to_string();
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
        .setup(move |app| {
            let t = std::time::Instant::now();
            let sink: Arc<dyn EventSink> = Arc::new(TauriSink(app.handle().clone()));
            let chat = Arc::new(ChatManager::new(stores.clone(), sink));
            app.manage(AppState {
                chat,
                stores,
                probe: tokio::sync::Mutex::new(probe::CliProbe::new()),
                tester: probe::AgentTester::new(),
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
            close_session,
            delete_session,
            set_active_session,
            rename_session,
            set_session_pinned,
            send_prompt,
            cancel,
            retry_cancel,
            answer_permission,
            set_permission_mode,
            switch_agent,
            guide_at,
            remove_queue_at,
            clear_queue,
            session_messages,
            runtime_states,
            unread_sessions,
            // sessions / search
            list_sessions,
            sessions_for_project,
            session_meta,
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
            folder_create,
            install_help,
            read_file_range,
            preview_file,
            save_preview,
            workspace_files,
            git_branch,
            git_log,
            list_workspace_dir,
            save_clipboard_image,
            // todos / prompts / prefs
            todos_list,
            todo_add,
            todo_toggle,
            todo_remove,
            todos_clear_done,
            prompts_list,
            prompt_add,
            prompt_remove,
            get_prefs,
            background_config,
            set_user_name,
            set_user_avatar_from_file,
            clear_user_avatar,
            set_font_scale,
            set_preview_size,
            set_panel_layout,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WarDex");
}
