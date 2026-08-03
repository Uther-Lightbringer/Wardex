// ChatManager: owns the runtime map (HashMap<sessionId, Runtime>) and every
// session-lifecycle entry point the Tauri command layer calls. Ported from
// the manager half of ChatController.cpp (startNewSession/openSession/
// closeRuntime/switchAgent/discardIfEmpty/sendUserMessage[WithAttachments]).
//
// Concurrency: the manager is shared as Arc<ChatManager> behind Tauri State;
// all mutable state lives in the store registry mutex, the runtime registry
// mutex and per-runtime actors (message passing — no shared turn state).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::chat::driver::{stdio_spawner, Spawner};
use crate::chat::runtime::{
    enforce_process_cap, is_image_path, lock_ok, spawn_actor, EventSink, ManagerShared,
    RuntimeCmd, RuntimeEntry, RuntimeSnap, SendOutcome, SharedRegistry,
};
use crate::provider;
use crate::store::agents::Agent;
use crate::store::sessions::AgentSnapshot;
use crate::store::StoreRegistry;

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("{0}")]
    Message(String),
    #[error("没有打开的会话")]
    NoSession,
    #[error("会话不存在")]
    SessionMissing,
    #[error("store error: {0}")]
    Store(#[from] crate::store::SessionsError),
}

/// Factory producing a fresh Spawner per ensureAcp of one session. Production
/// spawns real CLI subprocesses; tests substitute MockTransport clients.
pub type SpawnerFactory = Arc<dyn Fn(&str) -> Spawner + Send + Sync>;

pub struct ChatManager {
    stores: Arc<Mutex<StoreRegistry>>,
    sink: Arc<dyn EventSink>,
    registry: SharedRegistry,
    shared: Arc<Mutex<ManagerShared>>,
    spawner_factory: SpawnerFactory,
}

/// canUseForChat (old AgentStore): enabled + provider registry chat_capable.
pub fn can_use_for_chat(agent: &Agent) -> bool {
    agent.enabled && provider::chat_capable(&agent.provider)
}

fn snapshot_of(agent: &Agent) -> AgentSnapshot {
    AgentSnapshot {
        id: agent.id.clone(),
        name: agent.name.clone(),
        provider: agent.provider.clone(),
        model: agent.model.clone(),
        base_url: agent.base_url.clone(),
        cli_path: agent.cli_path.clone(),
    }
}

impl ChatManager {
    pub fn new(stores: Arc<Mutex<StoreRegistry>>, sink: Arc<dyn EventSink>) -> Self {
        Self::with_factory(stores, sink, Arc::new(|_| stdio_spawner()))
    }

    pub fn with_factory(
        stores: Arc<Mutex<StoreRegistry>>,
        sink: Arc<dyn EventSink>,
        spawner_factory: SpawnerFactory,
    ) -> Self {
        Self {
            stores,
            sink,
            registry: Arc::new(Mutex::new(HashMap::new())),
            shared: Arc::new(Mutex::new(ManagerShared::default())),
            spawner_factory,
        }
    }

    pub fn stores(&self) -> &Arc<Mutex<StoreRegistry>> {
        &self.stores
    }

    pub fn registry(&self) -> &SharedRegistry {
        &self.registry
    }

    pub fn active_id(&self) -> String {
        lock_ok(&self.shared).active_id.clone()
    }

    /// Unread session ids (runtime flag, not persisted; chat.md §7.7).
    pub fn unread_ids(&self) -> Vec<String> {
        lock_ok(&self.shared).unread.iter().cloned().collect()
    }

    /// Per-session runtime snapshots for the rail state dots.
    pub fn runtime_states(&self) -> HashMap<String, RuntimeSnap> {
        lock_ok(&self.registry)
            .iter()
            .map(|(id, e)| (id.clone(), lock_ok(&e.snap).clone()))
            .collect()
    }

    fn entry_tx(&self, session_id: &str) -> Option<tokio::sync::mpsc::Sender<RuntimeCmd>> {
        lock_ok(&self.registry).get(session_id).map(|e| e.tx.clone())
    }

    async fn send(&self, session_id: &str, cmd: RuntimeCmd) -> Result<(), ChatError> {
        let Some(tx) = self.entry_tx(session_id) else {
            return Err(ChatError::NoSession);
        };
        if tx.send(cmd).await.is_err() {
            return Err(ChatError::NoSession);
        }
        Ok(())
    }

    /// resolveAgentFor (ChatController.cpp:676-686): session meta's agent,
    /// else the default agent, else a pseudo-agent built from the meta.
    fn resolve_agent_for(&self, session_id: &str) -> Agent {
        let mut stores = lock_ok(&self.stores);
        let Some(meta) = stores.sessions.meta_for(session_id) else {
            return Agent::default();
        };
        if let Some(a) = stores.agents.get(&meta.agent_id) {
            return a.clone();
        }
        if let Some(a) = stores.agents.default_agent() {
            return a.clone();
        }
        Agent {
            id: meta.agent_id,
            name: meta.agent_name,
            provider: meta.provider,
            cli_path: meta.cli_path,
            base_url: meta.base_url,
            model: meta.model,
            ..Default::default()
        }
    }

    fn create_runtime(&self, session_id: &str, agent: Agent) {
        let snap = Arc::new(Mutex::new(RuntimeSnap {
            agent_id: agent.id.clone(),
            ..Default::default()
        }));
        let tx = spawn_actor(
            session_id,
            agent,
            self.stores.clone(),
            self.sink.clone(),
            self.registry.clone(),
            self.shared.clone(),
            (self.spawner_factory)(session_id),
            snap.clone(),
        );
        lock_ok(&self.registry).insert(session_id.to_string(), RuntimeEntry { tx, snap });
    }

    /// destroyRuntime (ChatController.cpp:238-256): stop the actor (which
    /// settles a busy turn and releases the resident model), drop the entry.
    pub fn destroy_runtime(&self, session_id: &str) {
        let entry = lock_ok(&self.registry).remove(session_id);
        if let Some(entry) = entry {
            let _ = entry.tx.try_send(RuntimeCmd::Shutdown);
        }
    }

    /// Teardown runtimes for sessions the store just removed wholesale
    /// (delete_group cascade). The active pointer is cleared if it died;
    /// the frontend switches to a remaining session itself.
    pub fn drop_deleted_runtimes(&self, ids: &[String]) {
        let active = self.active_id();
        if !active.is_empty() && ids.iter().any(|id| *id == active) {
            lock_ok(&self.shared).active_id.clear();
        }
        for id in ids {
            self.destroy_runtime(id);
        }
    }

    /// startNewSession (ChatController.cpp:688-721): default agent required,
    /// session created + warmed, previous empty session discarded.
    pub async fn create_session(&self, project_dir: &str) -> Result<String, ChatError> {
        self.create_session_in_group(project_dir, "").await
    }

    /// createSession landing directly in a rail group ("" = default group).
    pub async fn create_session_in_group(
        &self,
        project_dir: &str,
        group_id: &str,
    ) -> Result<String, ChatError> {
        self.create_session_with(project_dir, "", true, group_id).await
    }

    /// Internal: `title` empty → default title. `active` false keeps the
    /// current active session untouched (used by the project-due flow — the
    /// new session runs in the background until the user acts on it).
    async fn create_session_with(
        &self,
        project_dir: &str,
        title: &str,
        active: bool,
        group_id: &str,
    ) -> Result<String, ChatError> {
        let agent = {
            let stores = lock_ok(&self.stores);
            stores.agents.default_agent().cloned()
        };
        let Some(agent) = agent.filter(can_use_for_chat) else {
            return Err(ChatError::Message(
                "请先在配置中创建 Kimi Agent 并设为默认".to_string(),
            ));
        };
        let id = {
            let mut stores = lock_ok(&self.stores);
            let id = stores
                .sessions
                .create_session_with_group(&snapshot_of(&agent), project_dir, Some(group_id))?;
            if !title.trim().is_empty() {
                let _ = stores.sessions.rename_session(&id, title.trim());
            }
            id
        };
        self.create_runtime(&id, agent);
        if active {
            let prev = self.active_id();
            lock_ok(&self.shared).active_id = id.clone();
            self.sink.emit("store://sessions", json!({}));
            if !prev.is_empty() && prev != id {
                self.discard_if_empty(&prev).await;
            }
        }
        self.send(&id, RuntimeCmd::EnsureAcp).await?; // warm in background
        self.sink.emit("store://sessions", json!({}));
        Ok(id)
    }

    /// openSession (ChatController.cpp:810-834).
    pub async fn open_session(&self, session_id: &str) -> Result<(), ChatError> {
        {
            let mut stores = lock_ok(&self.stores);
            if !stores.sessions.ensure_open(session_id) {
                return Err(ChatError::SessionMissing);
            }
        }
        let prev = self.active_id();
        lock_ok(&self.shared).active_id = session_id.to_string();
        lock_ok(&self.shared).unread.remove(session_id);
        // 切走一个从未发言的空会话 → 直接丢弃
        if !prev.is_empty() && prev != session_id {
            self.discard_if_empty(&prev).await;
        }
        if self.entry_tx(session_id).is_none() {
            let agent = self.resolve_agent_for(session_id);
            self.create_runtime(session_id, agent);
            self.send(session_id, RuntimeCmd::EnsureAcp).await?;
        }
        Ok(())
    }

    /// closeRuntime (ChatController.cpp:258-267).
    pub fn close_session(&self, session_id: &str) {
        self.destroy_runtime(session_id);
        let mut shared = lock_ok(&self.shared);
        if shared.active_id == session_id {
            shared.active_id.clear();
        }
        shared.unread.remove(session_id);
    }

    /// Rail 删除会话: closeRuntime first, then delete from disk.
    pub async fn delete_session(&self, session_id: &str) -> Result<bool, ChatError> {
        self.close_session(session_id);
        let deleted = {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            if let Err(e) = stores.todos.remove_session(&paths, session_id) {
                log::warn!("todos remove_session failed: {e}");
            }
            stores.sessions.delete_session(session_id)?
        };
        self.sink.emit("store://sessions", json!({}));
        Ok(deleted)
    }

    /// discardIfEmpty (ChatController.cpp:274-286): never-spoken sessions are
    /// deleted outright when switching away / creating a new one.
    pub async fn discard_if_empty(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        if let Some(entry) = lock_ok(&self.registry).get(session_id) {
            let s = lock_ok(&entry.snap);
            if s.busy || s.queue_len > 0 {
                return;
            }
        }
        let count = {
            let mut stores = lock_ok(&self.stores);
            stores
                .sessions
                .meta_for(session_id)
                .map(|m| m.message_count)
                .unwrap_or(0)
        };
        if count > 0 {
            return;
        }
        self.destroy_runtime(session_id);
        {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            if let Err(e) = stores.todos.remove_session(&paths, session_id) {
                log::warn!("todos remove_session failed: {e}");
            }
            if let Err(e) = stores.sessions.delete_session(session_id) {
                log::warn!("discard_if_empty delete failed: {e}");
            }
        }
        self.sink.emit("store://sessions", json!({}));
    }

    /// sendUserMessage / sendUserMessageWithAttachments
    /// (ChatController.cpp:920-1004). The ack oneshot reports the runtime's
    /// decision: Started / Enqueued / Rejected(reason) — the composer keeps
    /// its draft unless the send was accepted.
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: &str,
        attachments: &[String],
    ) -> Result<SendOutcome, ChatError> {
        self.send_prompt_kind(session_id, text, attachments, "").await
    }

    /// send_prompt with a row kind marker ("reminder" for the project-due
    /// flow — the frontend sends the todo text in with kind="reminder").
    pub async fn send_prompt_kind(
        &self,
        session_id: &str,
        text: &str,
        attachments: &[String],
        kind: &str,
    ) -> Result<SendOutcome, ChatError> {
        let Some(tx) = self.entry_tx(session_id) else {
            return Err(ChatError::NoSession);
        };

        // Attachment split (ChatController.cpp:961-1004): image + agent
        // support -> image block; otherwise inline a "[附件] path" line.
        let image_supported = lock_ok(&self.registry)
            .get(session_id)
            .map(|e| lock_ok(&e.snap).image_supported)
            .unwrap_or(false);
        let mut send_text = text.trim().to_string();
        let mut images: Vec<String> = Vec::new();
        let mut display: Vec<String> = Vec::new();
        for f in attachments {
            if f.is_empty() || !std::path::Path::new(f).exists() {
                continue;
            }
            display.push(f.clone());
            if is_image_path(f) && image_supported {
                images.push(f.clone());
            } else {
                // Non-image (or agent lacks the cap): reference by path — the
                // agent runs inside the project cwd and reads it with tools.
                send_text += &format!("\n[附件] {}", f.replace('/', "\\"));
            }
        }
        if send_text.is_empty() && display.is_empty() {
            return Ok(SendOutcome::Rejected(String::new()));
        }
        if send_text.is_empty() {
            send_text = "（图片）".to_string();
        }

        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        tx.send(RuntimeCmd::SendPrompt {
            text: send_text,
            images,
            display,
            kind: kind.to_string(),
            ack: Some(ack_tx),
        })
        .await
        .map_err(|_| ChatError::Message("会话已关闭".to_string()))?;
        match tokio::time::timeout(std::time::Duration::from_secs(10), ack_rx).await {
            Ok(Ok(outcome)) => Ok(outcome),
            _ => Err(ChatError::Message("发送超时，请重试".to_string())),
        }
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), ChatError> {
        self.send(session_id, RuntimeCmd::Cancel).await
    }

    pub async fn retry_cancel(&self, session_id: &str) -> Result<(), ChatError> {
        self.send(session_id, RuntimeCmd::RetryCancel).await
    }

    /// 提醒变更后通知对应 runtime reload（手动 add/cancel；runtime 不存在
    /// 时静默跳过——下次打开会话 actor 启动会自行 reload）。
    pub async fn reminders_reload(&self, session_id: &str) {
        if let Some(tx) = self.entry_tx(session_id) {
            let _ = tx.send(RuntimeCmd::RemindersReload).await;
        }
    }

    /// Tell every live runtime to re-arm its push-due timer (toggle/remove
    /// may affect push rows of any session).
    pub async fn reminders_reload_all(&self) {
        let tx_list: Vec<_> = lock_ok(&self.registry)
            .values()
            .map(|e| e.tx.clone())
            .collect();
        for tx in tx_list {
            let _ = tx.send(RuntimeCmd::RemindersReload).await;
        }
    }

    /// App-level due scan (30s tick, lib.rs setup). Handles the rows that are
    /// NOT the session runtimes' business:
    ///   - popup rows (session + global scope): desktop notification + emit
    ///     todos://due (dedup via notifiedAtMs)
    ///   - project rows: auto-create a session in the project (named after
    ///     the todo, active untouched), settle the row, then emit
    ///     todos://projectDue so the frontend can ask the user how to proceed
    /// push rows are left to the owning session runtime.
    pub async fn tick_due_todos(&self) {
        use crate::store::todos::{now_ms, TodoRow, SCOPE_PROJECT, SCOPE_SESSION};
        let now = now_ms();
        let due: Vec<TodoRow> = {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            stores.todos.reload(&paths);
            stores.todos.due_not_notified(now)
        };
        if due.is_empty() {
            return;
        }
        for r in due {
            match r.scope.as_str() {
                SCOPE_PROJECT => self.fire_project_due(r).await,
                SCOPE_SESSION => self.fire_popup_due(r, now),
                _ => self.fire_popup_due(r, now),
            }
        }
    }

    /// Popup due: mark notified FIRST (persisted dedup guard), then notify.
    fn fire_popup_due(&self, r: crate::store::todos::TodoRow, now: i64) {
        let notified = {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            stores
                .todos
                .mark_notified(&paths, &r.id, now)
                .unwrap_or(false)
        };
        if !notified {
            return; // another tick already surfaced it
        }
        self.sink.notify("待办到期", &r.title);
        self.sink.emit("todos://due", json!({ "row": r }));
    }

    /// Project due: new detached session in the project, settle the row,
    /// then let the frontend pop the three-way choice.
    async fn fire_project_due(&self, r: crate::store::todos::TodoRow) {
        // Name = the todo title (40 chars cap).
        let title: String = r.title.trim().chars().take(40).collect();
        let session_id = match self.create_session_with(&r.project_dir, &title, false, "").await {
            Ok(id) => id,
            Err(e) => {
                log::warn!("todos: project due create_session failed: {e}");
                return;
            }
        };
        {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            if let Err(e) = stores.todos.settle(&paths, &r.id) {
                log::warn!("todos: project due settle failed: {e}");
            }
        }
        self.sink.notify("项目待办到期", &title);
        self.sink.emit(
            "todos://projectDue",
            json!({ "row": r, "sessionId": session_id }),
        );
    }

    pub async fn answer_permission(
        &self,
        session_id: &str,
        option_id: &str,
        cancelled: bool,
    ) -> Result<(), ChatError> {
        self.send(
            session_id,
            RuntimeCmd::AnswerPermission {
                option_id: option_id.to_string(),
                cancelled,
            },
        )
        .await
    }

    /// Stored acp://permission payload while a request awaits an answer
    /// (None when idle or the runtime is gone). Read from the snap — no
    /// round-trip into the actor needed.
    pub fn pending_permission(&self, session_id: &str) -> Option<Value> {
        lock_ok(&self.registry)
            .get(session_id)
            .and_then(|e| lock_ok(&e.snap).perm_pending.clone())
    }

    /// Latest sub-agent/task list for a session (Null when the runtime is
    /// gone). Read from the snap — no round-trip into the actor needed; the
    /// chat store re-pulls it after a session switch.
    pub fn get_subagents(&self, session_id: &str) -> Value {
        lock_ok(&self.registry)
            .get(session_id)
            .map(|e| json!(lock_ok(&e.snap).subagents))
            .unwrap_or(Value::Null)
    }

    pub async fn guide_at(&self, session_id: &str, index: usize) -> Result<(), ChatError> {
        self.send(session_id, RuntimeCmd::GuideAt(index)).await
    }

    pub async fn remove_queue_at(&self, session_id: &str, index: usize) -> Result<(), ChatError> {
        self.send(session_id, RuntimeCmd::RemoveQueueAt(index)).await
    }

    pub async fn clear_queue(&self, session_id: &str) -> Result<(), ChatError> {
        self.send(session_id, RuntimeCmd::ClearQueue).await
    }

    /// switchAgent (ChatController.cpp:741-808): validation + no-op check
    /// here; the provider-comparison and acpSessionId rule run in the actor
    /// (it owns the old agent snapshot).
    pub async fn switch_agent(&self, session_id: &str, agent_id: &str) -> Result<(), ChatError> {
        let Some(entry) = lock_ok(&self.registry).get(session_id).map(|e| e.tx.clone()) else {
            return Err(ChatError::NoSession);
        };
        let agent = {
            let stores = lock_ok(&self.stores);
            stores.agents.get(agent_id).cloned()
        };
        let Some(agent) = agent.filter(can_use_for_chat) else {
            return Err(ChatError::Message(
                "Agent 不可用，请在配置页检查".to_string(),
            ));
        };
        let current = self
            .runtime_states()
            .get(session_id)
            .map(|s| s.agent_id.clone())
            .unwrap_or_default();
        if current == agent_id {
            return Ok(()); // no-op
        }
        let _ = entry.send(RuntimeCmd::SwitchAgent(Box::new(agent))).await;
        Ok(())
    }

    /// Permission mode: persist globally (user_prefs), apply immediately to
    /// the ACTIVE session's process; background sessions pick it up on their
    /// next prompt (chat.md §3.7).
    pub async fn set_permission_mode(&self, mode: &str) -> Result<(), ChatError> {
        if !crate::store::prefs::PERMISSION_MODES.contains(&mode) {
            return Err(ChatError::Message(format!("未知权限模式: {mode}")));
        }
        {
            let mut stores = lock_ok(&self.stores);
            let paths = stores.paths.clone();
            if let Err(e) = stores.prefs.set_permission_mode(&paths, mode) {
                return Err(ChatError::Message(e.to_string()));
            }
        }
        let active = self.active_id();
        if !active.is_empty() {
            if let Some(tx) = self.entry_tx(&active) {
                let _ = tx.send(RuntimeCmd::SetMode(mode.to_string())).await;
            }
        }
        self.sink.emit("store://prefs", json!({}));
        Ok(())
    }

    /// ACP config option picker passthrough (kimi "thinking"/"model"): the
    /// runtime applies it to the session and the refreshed configOptions come
    /// back as acp://configOptions.
    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), ChatError> {
        let Some(tx) = self.entry_tx(session_id) else {
            return Err(ChatError::NoSession);
        };
        let _ = tx
            .send(RuntimeCmd::SetConfigOption {
                config_id: config_id.to_string(),
                value: value.to_string(),
            })
            .await;
        Ok(())
    }

    /// Re-push the cached configOptions for a re-activated session: the
    /// frontend clears its copy on every switch, and a live runtime would
    /// otherwise stay silent until the next picker-affecting event.
    pub async fn resend_config_options(&self, session_id: &str) -> Result<(), ChatError> {
        let Some(tx) = self.entry_tx(session_id) else {
            return Err(ChatError::NoSession);
        };
        let _ = tx.send(RuntimeCmd::ResendConfigOptions).await;
        Ok(())
    }

    /// Parallel-cap eviction hook, exposed for tests (actors call the free
    /// function directly before spawning).
    pub fn enforce_process_cap(&self, exempt: &str) {
        enforce_process_cap(&self.registry, exempt);
    }

    /// Session messages for the frontend (resident model; ensures open).
    pub fn session_messages(&self, session_id: &str) -> Vec<Value> {
        let mut stores = lock_ok(&self.stores);
        if !stores.sessions.ensure_open(session_id) {
            return Vec::new();
        }
        let mut rows: Vec<crate::store::MessageRow> = stores
            .sessions
            .messages(session_id)
            .map(|rows| rows.to_vec())
            .unwrap_or_default();
        // 历史回合：把 usage.json 的回填记录挂到缺失 usage 的 assistant 行
        // （非破坏性，重开即自愈），让旧会话的气泡也显示 ↑↓ 用量。
        crate::store::usage::attach_usage_backfill(&mut rows, &stores.usage, session_id);
        rows.iter()
            .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
            .collect()
    }
}
