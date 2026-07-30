// Session persistence: sessions/<uuid>/meta.json + messages.jsonl, the
// sessions index scan, LRU-resident message models, and full-text search
// (data-formats.md §2, §3, §4, §10, §12).
//
// Write timing is a hard compatibility contract (§4.4):
//   - append_message: ONE compact JSON line appended, WITHOUT the `segments`
//     key (user rows, assistant placeholder rows).
//   - rewrite_messages_file: atomic full rewrite of every in-memory row,
//     WITH `segments` (turn flush / error replacement / field fix-ups).
//   - streaming chunk mutations (append_*_content/thinking, upsert_*_tool):
//     memory only, ZERO disk I/O. A process killed mid-stream leaves the
//     placeholder row (content "…", status "pending") on disk — that is
//     compatible data, not corruption.
//
// Loading is tolerant: empty lines skipped, corrupt lines treated as empty
// objects, timestamps parsed via f64, missing `status` defaults to "done",
// and rows without a `segments` key get the legacy synthesis
// thinking → text → tools (§4.1). The stray leading "…" baked into old
// history by the placeholder bug is scrubbed both on load and in search.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::store::json::{
    append_json_line, contains_case_insensitive, de_ms_i64, ellipsize,
    find_case_insensitive, left_chars, now_ms, snippet_around, write_text_atomic,
    write_value_atomic, JsonError,
};
use crate::store::paths::{canonical_dir, Paths};

/// In-memory cap of loaded session message models (performance.md §3:
/// 上限 5 / LRU / 会话删除或重开时失效). Disk is the only source of truth,
/// so eviction is just a drop — every mutation writes through immediately.
pub const MAX_OPEN_SESSIONS: usize = 5;

const DEFAULT_TITLE: &str = "新会话";
const PLACEHOLDER: &str = "…";
const EMPTY_REPLY: &str = "（空回复）";

#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
    #[error("项目目录不存在: {0}")]
    ProjectDirMissing(String),
    #[error("无法写入会话元数据")]
    MetaWrite,
    #[error("无法删除会话目录")]
    DeleteFailed,
}

// ---------------------------------------------------------------------------
// meta.json model (§3). Unknown keys are preserved verbatim via `extra` so
// cross-version writes never drop fields (§0).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMeta {
    #[serde(rename = "acpSessionId", skip_serializing_if = "Option::is_none")]
    pub acp_session_id: Option<String>,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "agentName")]
    pub agent_name: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "cliPath")]
    pub cli_path: String,
    #[serde(rename = "createdAt", deserialize_with = "de_ms_i64")]
    pub created_at: i64,
    pub id: String,
    #[serde(rename = "messageCount", deserialize_with = "de_ms_i64")]
    pub message_count: i64,
    pub model: String,
    // Later-added key: absent in old files. Once set (even to false) the key
    // is written back, matching the old QVariantMap insert behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(rename = "projectDir")]
    pub project_dir: String,
    pub provider: String,
    pub status: String,
    pub summary: String,
    pub title: String,
    #[serde(rename = "updatedAt", deserialize_with = "de_ms_i64")]
    pub updated_at: i64,
    #[serde(rename = "workDir")]
    pub work_dir: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SessionMeta {
    pub fn pinned(&self) -> bool {
        self.pinned.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// messages.jsonl row model (§4.1). `tool_calls` / `segments` elements are
// passthrough JSON maps (ACP payloads flow through untouched). Row-level
// unknown keys are NOT preserved: the old rewriteMessagesFile also wrote
// only the known key set.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub provider: String,
    pub status: String,
    pub thinking: String,
    pub tool_calls: Vec<Value>,
    /// Arrival-ordered stream: maps of {kind: "thinking"|"text"|"tool", ...}.
    pub segments: Vec<Value>,
    /// Local file paths attached to a user message (images etc.).
    pub attachments: Vec<String>,
}

/// Wire shape of one JSONL line. `status` defaults to "done" when the key is
/// missing (§4.1).
#[derive(Debug, Deserialize)]
#[serde(default)]
struct MessageLine {
    id: String,
    role: String,
    content: String,
    #[serde(rename = "createdAt", deserialize_with = "de_ms_i64")]
    created_at: i64,
    provider: String,
    #[serde(default = "default_status")]
    status: String,
    thinking: String,
    #[serde(rename = "toolCalls")]
    tool_calls: Vec<Value>,
    segments: Vec<Value>,
    attachments: Vec<String>,
}

impl Default for MessageLine {
    fn default() -> Self {
        Self {
            id: String::new(),
            role: String::new(),
            content: String::new(),
            created_at: 0,
            provider: String::new(),
            status: default_status(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            segments: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

fn default_status() -> String {
    "done".to_string()
}

impl MessageLine {
    fn into_row(self) -> MessageRow {
        let mut row = MessageRow {
            id: self.id,
            role: self.role,
            content: scrub_leading_ellipsis(self.content),
            created_at: self.created_at,
            provider: self.provider,
            status: self.status,
            thinking: self.thinking,
            tool_calls: self.tool_calls,
            segments: self.segments,
            attachments: self.attachments,
        };
        if row.segments.is_empty() {
            synthesize_legacy_segments(&mut row);
        }
        row
    }
}

/// Scrub the stray leading ellipsis a pre-fix placeholder bug baked into old
/// history (placeholder "…" was never cleared before append). Done both on
/// load and in search (§4.1).
fn scrub_leading_ellipsis(mut content: String) -> String {
    if content.chars().count() > 1 && content.starts_with(PLACEHOLDER) {
        content.remove(0);
    }
    content
}

/// Legacy rows (appended, never rewritten) carry no `segments` key: rebuild
/// the ordered stream as thinking → text → tools (§4.1).
fn synthesize_legacy_segments(row: &mut MessageRow) {
    if !row.thinking.trim().is_empty() {
        row.segments.push(Value::Object(Map::from_iter([
            ("kind".to_string(), Value::String("thinking".to_string())),
            ("text".to_string(), Value::String(row.thinking.clone())),
        ])));
    }
    if !row.content.is_empty() && row.content != PLACEHOLDER {
        row.segments.push(Value::Object(Map::from_iter([
            ("kind".to_string(), Value::String("text".to_string())),
            ("text".to_string(), Value::String(row.content.clone())),
        ])));
    }
    for tc in &row.tool_calls {
        if let Value::Object(map) = tc {
            let mut seg = map.clone();
            seg.insert("kind".to_string(), Value::String("tool".to_string()));
            row.segments.push(Value::Object(seg));
        }
    }
}

/// Serialize one row as a compact JSONL object. Keys come out alphabetically
/// (serde_json's BTreeMap). `segments` is only included for full rewrites —
/// appended rows must NOT carry the key (§4.4).
fn message_line_value(row: &MessageRow, include_segments: bool) -> Value {
    let mut o = Map::new();
    o.insert(
        "attachments".to_string(),
        Value::Array(row.attachments.iter().map(|a| Value::String(a.clone())).collect()),
    );
    o.insert("content".to_string(), Value::String(row.content.clone()));
    o.insert("createdAt".to_string(), Value::Number(row.created_at.into()));
    o.insert("id".to_string(), Value::String(row.id.clone()));
    o.insert("provider".to_string(), Value::String(row.provider.clone()));
    o.insert("role".to_string(), Value::String(row.role.clone()));
    if include_segments {
        o.insert("segments".to_string(), Value::Array(row.segments.clone()));
    }
    o.insert("status".to_string(), Value::String(row.status.clone()));
    o.insert("thinking".to_string(), Value::String(row.thinking.clone()));
    o.insert("toolCalls".to_string(), Value::Array(row.tool_calls.clone()));
    Value::Object(o)
}

/// Extend a trailing text segment, or open a new one after thinking/tools.
fn append_text_segment(segments: &mut Vec<Value>, chunk: &str) {
    let extend = segments
        .last()
        .and_then(|s| s.get("kind"))
        .and_then(Value::as_str)
        == Some("text");
    if extend {
        if let Some(Value::String(text)) = segments.last_mut().and_then(|s| s.get_mut("text")) {
            text.push_str(chunk);
            return;
        }
    }
    segments.push(Value::Object(Map::from_iter([
        ("kind".to_string(), Value::String("text".to_string())),
        ("text".to_string(), Value::String(chunk.to_string())),
    ])));
}

// ---------------------------------------------------------------------------
// Sessions index (§2): no index file — scan sessions/*/meta.json, skip
// unreadable meta, sort updatedAt desc.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionIndexRow {
    pub id: String,
    pub title: String,
    #[serde(rename = "agentName")]
    pub agent_name: String,
    pub provider: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "messageCount")]
    pub message_count: i64,
    pub status: String,
    pub summary: String,
    #[serde(rename = "projectDir")]
    pub project_dir: String,
    pub pinned: bool,
}

/// Agent fields snapshotted into meta at session creation (§3).
#[derive(Debug, Clone, Default)]
pub struct AgentSnapshot {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub cli_path: String,
}

struct OpenSession {
    meta: SessionMeta,
    messages: Vec<MessageRow>,
}

/// Per-project session listing entry (sessions_for_project).
#[derive(Debug, Clone, Serialize)]
pub struct SessionForProject {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub title: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "messageCount")]
    pub message_count: i64,
    pub pinned: bool,
}

pub struct SessionStore {
    paths: Paths,
    index: Vec<SessionIndexRow>,
    open: HashMap<String, OpenSession>,
    /// LRU order: front = most recently used.
    lru: VecDeque<String>,
}

impl SessionStore {
    /// Startup load (old SessionStore ctor): ensure layout, discard empty
    /// sessions, build the index. Tolerant — never fails hard.
    pub fn load(paths: Paths) -> Self {
        paths.ensure_layout();
        let mut store = Self {
            paths,
            index: Vec::new(),
            open: HashMap::new(),
            lru: VecDeque::new(),
        };
        store.discard_empty_sessions();
        store.reload_index();
        store
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn list(&self) -> &[SessionIndexRow] {
        &self.index
    }

    /// Delete leftover session dirs that were created but never used:
    /// meta readable AND messageCount == 0 → recursive delete; unreadable
    /// meta → left alone (§2). Runs once at startup (no sessions open).
    pub fn discard_empty_sessions(&self) {
        let Ok(entries) = fs::read_dir(self.paths.sessions_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if self.open.contains_key(&id) {
                continue;
            }
            match self.read_meta(&id) {
                Some(meta) if meta.message_count == 0 => {
                    let _ = fs::remove_dir_all(self.paths.session_dir(&id));
                }
                _ => {}
            }
        }
    }

    pub fn reload_index(&mut self) {
        self.index.clear();
        if let Ok(entries) = fs::read_dir(self.paths.sessions_dir()) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().into_owned();
                let Some(meta) = self.read_meta(&id) else {
                    continue; // unreadable/corrupt meta → not in the list
                };
                let pinned = meta.pinned();
                self.index.push(SessionIndexRow {
                    id,
                    title: meta.title,
                    agent_name: meta.agent_name,
                    provider: meta.provider,
                    updated_at: meta.updated_at,
                    created_at: meta.created_at,
                    message_count: meta.message_count,
                    status: meta.status,
                    summary: meta.summary,
                    project_dir: meta.project_dir,
                    pinned,
                });
            }
        }
        self.index.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    }

    // ---- meta I/O ----

    /// writeMeta: always a full atomic rewrite of every known key (+ unknown
    /// extras preserved). Empty id is refused (§3).
    pub fn write_meta(&self, meta: &SessionMeta) -> Result<(), SessionsError> {
        if meta.id.is_empty() {
            return Err(SessionsError::MetaWrite);
        }
        let path = self.paths.session_meta_path(&meta.id);
        write_value_atomic(&path, meta)?;
        Ok(())
    }

    /// readMeta: missing/corrupt → None (caller treats as "skip this dir").
    pub fn read_meta(&self, session_id: &str) -> Option<SessionMeta> {
        let bytes = fs::read(self.paths.session_meta_path(session_id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    // ---- session lifecycle ----

    /// createSession (§3.2): meta + EMPTY messages.jsonl land together; a
    /// projectless session gets an app-managed workspace dir. Returns the id.
    pub fn create_session(
        &mut self,
        agent: &AgentSnapshot,
        project_dir: &str,
    ) -> Result<String, SessionsError> {
        self.paths.ensure_layout();
        // QUuid::createUuid().toString(WithoutBraces): lowercase hyphenated.
        let id = uuid::Uuid::new_v4().hyphenated().to_string();
        let now = now_ms();

        let clean_project = if project_dir.is_empty() {
            String::new()
        } else {
            let c = canonical_dir(project_dir);
            if !std::path::Path::new(&c).is_dir() {
                return Err(SessionsError::ProjectDirMissing(project_dir.to_string()));
            }
            c
        };
        let work_dir = if clean_project.is_empty() {
            self.paths
                .session_workspace_dir(&id)
                .to_string_lossy()
                .replace('\\', "/")
        } else {
            clean_project.clone()
        };
        if clean_project.is_empty() {
            fs::create_dir_all(&work_dir).map_err(|source| JsonError::Io {
                path: std::path::PathBuf::from(&work_dir),
                source,
            })?;
        }

        let meta = SessionMeta {
            id: id.clone(),
            title: DEFAULT_TITLE.to_string(),
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            provider: agent.provider.clone(),
            model: agent.model.clone(),
            base_url: agent.base_url.clone(),
            cli_path: agent.cli_path.clone(),
            work_dir,
            project_dir: clean_project,
            ..Default::default()
        };
        self.write_meta(&meta)?;

        // Empty messages file (0 bytes) created up-front.
        let messages_path = self.paths.session_messages_path(&id);
        fs::write(&messages_path, b"").map_err(|source| JsonError::Io {
            path: messages_path,
            source,
        })?;

        self.reload_index();
        self.ensure_open(&id);
        Ok(id)
    }

    /// ensureOpen: load meta + messages into the LRU cache. False when the
    /// session's meta is unreadable (does not exist / corrupt).
    pub fn ensure_open(&mut self, session_id: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        if self.open.contains_key(session_id) {
            self.touch_lru(session_id);
            return true;
        }
        let Some(meta) = self.read_meta(session_id) else {
            return false;
        };
        let messages = load_messages(&self.paths, session_id);
        self.open.insert(session_id.to_string(), OpenSession { meta, messages });
        self.lru.push_front(session_id.to_string());
        self.evict_lru();
        true
    }

    /// Drop a resident model (chat calls this when a runtime is dropped).
    /// All mutations write through immediately, so eviction cannot lose
    /// data; if a future chat layer keeps unflushed streaming state it must
    /// rewrite the file BEFORE releasing (§12.2).
    pub fn release_session(&mut self, session_id: &str) {
        self.open.remove(session_id);
        self.lru.retain(|id| id != session_id);
    }

    pub fn is_open(&self, session_id: &str) -> bool {
        self.open.contains_key(session_id)
    }

    /// Number of resident message models (for tests / memory HUD).
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    pub fn delete_session(&mut self, session_id: &str) -> Result<bool, SessionsError> {
        if session_id.is_empty() {
            return Ok(false);
        }
        self.release_session(session_id);
        let dir = self.paths.session_dir(session_id);
        if dir.exists() && fs::remove_dir_all(&dir).is_err() {
            return Err(SessionsError::DeleteFailed);
        }
        self.index.retain(|r| r.id != session_id);
        Ok(true)
    }

    /// Rename only — no updatedAt bump, no resort (§3.1). Trim + left(48).
    pub fn rename_session(&mut self, session_id: &str, title: &str) -> Result<bool, SessionsError> {
        let t = left_chars(title.trim(), 48);
        if session_id.is_empty() || t.is_empty() {
            return Ok(false);
        }
        let Some(meta) = self.meta_mut(session_id) else {
            return Ok(false);
        };
        meta.title = t.clone();
        let meta = meta.clone();
        self.write_meta(&meta)?;
        if let Some(row) = self.index.iter_mut().find(|r| r.id == session_id) {
            row.title = t;
        }
        Ok(true)
    }

    /// Pin flag — like rename: no updatedAt bump, no resort (§3.1).
    pub fn set_session_pinned(&mut self, session_id: &str, pinned: bool) -> Result<bool, SessionsError> {
        if session_id.is_empty() {
            return Ok(false);
        }
        let Some(meta) = self.meta_mut(session_id) else {
            return Ok(false);
        };
        meta.pinned = Some(pinned);
        let meta = meta.clone();
        self.write_meta(&meta)?;
        if let Some(row) = self.index.iter_mut().find(|r| r.id == session_id) {
            row.pinned = pinned;
        }
        Ok(true)
    }

    /// Agent switch (§3.1): updates agentId/agentName/provider only.
    pub fn set_session_agent_id(
        &mut self,
        session_id: &str,
        agent_id: &str,
        agent_name: &str,
        provider: &str,
    ) -> Result<bool, SessionsError> {
        if session_id.is_empty() || agent_id.is_empty() {
            return Ok(false);
        }
        let Some(meta) = self.meta_mut(session_id) else {
            return Ok(false);
        };
        meta.agent_id = agent_id.to_string();
        meta.agent_name = agent_name.to_string();
        meta.provider = provider.to_string();
        let meta = meta.clone();
        self.write_meta(&meta)?;
        if let Some(row) = self.index.iter_mut().find(|r| r.id == session_id) {
            row.agent_name = agent_name.to_string();
            row.provider = provider.to_string();
        }
        Ok(true)
    }

    pub fn set_acp_session_id(&mut self, session_id: &str, acp_session_id: &str) -> Result<(), SessionsError> {
        let Some(meta) = self.meta_mut(session_id) else {
            return Ok(());
        };
        meta.acp_session_id = Some(acp_session_id.to_string());
        let meta = meta.clone();
        self.write_meta(&meta)
    }

    pub fn acp_session_id_for(&mut self, session_id: &str) -> String {
        self.meta_for(session_id)
            .and_then(|m| m.acp_session_id)
            .unwrap_or_default()
    }

    /// Meta of a session: open cache first, else read from disk.
    pub fn meta_for(&mut self, session_id: &str) -> Option<SessionMeta> {
        if self.open.contains_key(session_id) {
            return self.open.get(session_id).map(|o| o.meta.clone());
        }
        self.read_meta(session_id)
    }

    /// Resident messages (does NOT re-order the LRU — read-only view).
    pub fn messages(&self, session_id: &str) -> Option<&[MessageRow]> {
        self.open.get(session_id).map(|o| o.messages.as_slice())
    }

    // ---- workspace / project ----

    /// workspacePathFor (§3): projectDir > workDir > sessions/<id>/workspace.
    pub fn workspace_path_for(&mut self, session_id: &str) -> String {
        if session_id.is_empty() {
            return String::new();
        }
        let Some(meta) = self.meta_for(session_id) else {
            return String::new();
        };
        if !meta.project_dir.is_empty() {
            return meta.project_dir;
        }
        if !meta.work_dir.is_empty() {
            return meta.work_dir;
        }
        self.paths
            .session_workspace_dir(session_id)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn project_dir_of(&mut self, session_id: &str) -> String {
        self.meta_for(session_id)
            .map(|m| m.project_dir)
            .unwrap_or_default()
    }

    /// Sessions of one project ("" = projectless), pinned first, then
    /// newest first (stable: the index is already updatedAt-desc).
    pub fn sessions_for_project(&self, project_dir: &str) -> Vec<SessionForProject> {
        let key = if project_dir.is_empty() {
            String::new()
        } else {
            canonical_dir(project_dir)
        };
        let mut out: Vec<SessionForProject> = self
            .index
            .iter()
            .filter(|r| {
                if key.is_empty() {
                    r.project_dir.is_empty()
                } else {
                    r.project_dir.eq_ignore_ascii_case(&key)
                }
            })
            .map(|r| SessionForProject {
                session_id: r.id.clone(),
                title: r.title.clone(),
                updated_at: r.updated_at,
                message_count: r.message_count,
                pinned: r.pinned,
            })
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.pinned));
        out
    }

    // ---- message mutation (write timing per §4.4) ----

    /// appendMessageTo: append ONE compact line WITHOUT `segments`, push the
    /// row, refresh meta (summary/messageCount/updatedAt, plus the title
    /// hint for the first user message). Status defaults to "done".
    pub fn append_message(
        &mut self,
        session_id: &str,
        role: &str,
        content: &str,
        provider: &str,
        status: &str,
        attachments: &[String],
    ) -> Result<bool, SessionsError> {
        if !self.ensure_open(session_id) {
            return Ok(false);
        }
        let status = if status.is_empty() { "done" } else { status };
        let row = MessageRow {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: now_ms(),
            provider: provider.to_string(),
            status: status.to_string(),
            attachments: attachments.to_vec(),
            ..Default::default()
        };
        let path = self.paths.session_messages_path(session_id);
        append_json_line(&path, &message_line_value(&row, false))?;

        let title_hint = {
            let open = self.open.get_mut(session_id);
            match open {
                None => None,
                Some(open) => {
                    open.messages.push(row);
                    if role == "user" && open.meta.title == DEFAULT_TITLE {
                        Some(ellipsize(content, 24))
                    } else {
                        None
                    }
                }
            }
        };
        self.update_meta_after_write(session_id, content, title_hint.as_deref())?;
        Ok(true)
    }

    /// updateLastAssistantTo: replace content/status of the last assistant
    /// row (error replacement, "（已中断）" writes) and REWRITE the file.
    pub fn update_last_assistant(
        &mut self,
        session_id: &str,
        content: &str,
        status: &str,
    ) -> Result<bool, SessionsError> {
        if !self.ensure_open(session_id) {
            return Ok(false);
        }
        let content_for_meta;
        {
            let Some(open) = self.open.get_mut(session_id) else {
                return Ok(false);
            };
            let Some(row) = open.messages.last_mut() else {
                return Ok(false);
            };
            if row.role != "assistant" {
                return Ok(false);
            }
            // Keep the ordered segment stream consistent with the replaced content.
            let old = row.content.clone();
            if !old.is_empty() && old != PLACEHOLDER && content.starts_with(&old) {
                let delta = &content[old.len()..];
                if !delta.is_empty() {
                    append_text_segment(&mut row.segments, delta);
                }
            } else {
                row.segments
                    .retain(|s| s.get("kind").and_then(Value::as_str) != Some("text"));
                if !content.is_empty() {
                    append_text_segment(&mut row.segments, content);
                }
            }
            row.content = content.to_string();
            row.status = status.to_string();
            content_for_meta = row.content.clone();
        }
        self.rewrite_messages_file(session_id)?;
        self.update_meta_after_write(session_id, &content_for_meta, None)?;
        Ok(true)
    }

    /// Streaming text chunk — MEMORY ONLY, zero disk I/O (§4.4).
    pub fn append_last_assistant_content(&mut self, session_id: &str, chunk: &str) -> bool {
        if !self.ensure_open(session_id) || chunk.is_empty() {
            return false;
        }
        let Some(open) = self.open.get_mut(session_id) else {
            return false;
        };
        let Some(row) = open.messages.last_mut() else {
            return false;
        };
        if row.role != "assistant" {
            return false;
        }
        if row.content == PLACEHOLDER {
            row.content.clear();
        }
        row.content.push_str(chunk);
        append_text_segment(&mut row.segments, chunk);
        row.status = "streaming".to_string();
        true
    }

    /// Streaming thinking chunk — MEMORY ONLY, zero disk I/O.
    pub fn append_last_assistant_thinking(&mut self, session_id: &str, chunk: &str) -> bool {
        if !self.ensure_open(session_id) || chunk.is_empty() {
            return false;
        }
        let Some(open) = self.open.get_mut(session_id) else {
            return false;
        };
        let Some(row) = open.messages.last_mut() else {
            return false;
        };
        if row.role != "assistant" {
            return false;
        }
        row.thinking.push_str(chunk);
        let extend = row
            .segments
            .last()
            .and_then(|s| s.get("kind"))
            .and_then(Value::as_str)
            == Some("thinking");
        if extend {
            if let Some(Value::String(text)) =
                row.segments.last_mut().and_then(|s| s.get_mut("text"))
            {
                text.push_str(chunk);
            }
        } else {
            row.segments.push(Value::Object(Map::from_iter([
                ("kind".to_string(), Value::String("thinking".to_string())),
                ("text".to_string(), Value::String(chunk.to_string())),
            ])));
        }
        row.status = "streaming".to_string();
        true
    }

    /// upsertLastAssistantToolTo — MEMORY ONLY. Merge by `toolCallId`
    /// (non-null fields overwrite; empty toolCallId drops the update),
    /// mirrored into the segment stream where `kind` is forced to "tool".
    pub fn upsert_last_assistant_tool(&mut self, session_id: &str, tool: &Map<String, Value>) -> bool {
        if tool.is_empty() || !self.ensure_open(session_id) {
            return false;
        }
        let id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if id.is_empty() {
            return false;
        }
        let Some(open) = self.open.get_mut(session_id) else {
            return false;
        };
        let Some(row) = open.messages.last_mut() else {
            return false;
        };
        if row.role != "assistant" {
            return false;
        }

        // Merge into toolCalls.
        let mut found = false;
        for tc in row.tool_calls.iter_mut() {
            let Value::Object(m) = tc else { continue };
            if m.get("toolCallId").and_then(Value::as_str) != Some(id) {
                continue;
            }
            merge_non_null(m, tool);
            found = true;
            break;
        }
        if !found {
            row.tool_calls.push(Value::Object(tool.clone()));
        }

        // Mirror into the ordered segment stream (update in place, else append).
        let mut seg_found = false;
        for seg in row.segments.iter_mut().rev() {
            let Value::Object(m) = seg else { continue };
            if m.get("kind").and_then(Value::as_str) != Some("tool")
                || m.get("toolCallId").and_then(Value::as_str) != Some(id)
            {
                continue;
            }
            merge_non_null(m, tool);
            m.insert("kind".to_string(), Value::String("tool".to_string()));
            seg_found = true;
            break;
        }
        if !seg_found {
            let mut seg = tool.clone();
            seg.insert("kind".to_string(), Value::String("tool".to_string()));
            row.segments.push(Value::Object(seg));
        }

        row.status = "streaming".to_string();
        true
    }

    /// setLastAssistantFieldsTo: full field replacement + REWRITE.
    pub fn set_last_assistant_fields(
        &mut self,
        session_id: &str,
        content: &str,
        thinking: &str,
        tool_calls: Vec<Value>,
        status: &str,
    ) -> Result<bool, SessionsError> {
        if !self.ensure_open(session_id) {
            return Ok(false);
        }
        let content_for_meta;
        {
            let Some(open) = self.open.get_mut(session_id) else {
                return Ok(false);
            };
            let Some(row) = open.messages.last_mut() else {
                return Ok(false);
            };
            if row.role != "assistant" {
                return Ok(false);
            }
            row.content = content.to_string();
            row.thinking = thinking.to_string();
            row.tool_calls = tool_calls;
            row.status = status.to_string();
            content_for_meta = row.content.clone();
        }
        self.rewrite_messages_file(session_id)?;
        self.update_meta_after_write(session_id, &content_for_meta, None)?;
        Ok(true)
    }

    /// flushLastAssistantTo (turn end — the one regular path that persists
    /// segments): REWRITE the whole file, then refresh meta. A done row whose
    /// content is still the placeholder becomes "（空回复）".
    pub fn flush_last_assistant(
        &mut self,
        session_id: &str,
        status: Option<&str>,
    ) -> Result<bool, SessionsError> {
        if !self.ensure_open(session_id) {
            return Ok(false);
        }
        let content_for_meta;
        {
            let Some(open) = self.open.get_mut(session_id) else {
                return Ok(false);
            };
            let Some(row) = open.messages.last_mut() else {
                return Ok(false);
            };
            if row.role != "assistant" {
                return Ok(false);
            }
            if let Some(s) = status {
                if !s.is_empty() {
                    row.status = s.to_string();
                }
            }
            if row.content == PLACEHOLDER && row.status == "done" {
                row.content = EMPTY_REPLY.to_string();
                append_text_segment(&mut row.segments, EMPTY_REPLY);
            }
            content_for_meta = row.content.clone();
        }
        self.rewrite_messages_file(session_id)?;
        self.update_meta_after_write(session_id, &content_for_meta, None)?;
        Ok(true)
    }

    // ---- internals ----

    fn meta_mut(&mut self, session_id: &str) -> Option<&mut SessionMeta> {
        if !self.ensure_open(session_id) {
            return None;
        }
        self.open.get_mut(session_id).map(|o| &mut o.meta)
    }

    /// rewriteMessagesFile: atomic full rewrite of every in-memory row, in
    /// model order, WITH segments (§4.4).
    fn rewrite_messages_file(&self, session_id: &str) -> Result<(), SessionsError> {
        let Some(open) = self.open.get(session_id) else {
            return Ok(());
        };
        let mut buf = Vec::new();
        for row in &open.messages {
            let line = serde_json::to_string(&message_line_value(row, true)).map_err(|e| {
                JsonError::Serde {
                    path: self.paths.session_messages_path(session_id),
                    source: e,
                }
            })?;
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        write_text_atomic(&self.paths.session_messages_path(session_id), &buf)?;
        Ok(())
    }

    /// summary/messageCount/updatedAt (+optional title) bookkeeping after any
    /// message write, then writeMeta. summary = source.left(80) + "…" (§3.1).
    fn update_meta_after_write(
        &mut self,
        session_id: &str,
        summary_source: &str,
        title_hint: Option<&str>,
    ) -> Result<(), SessionsError> {
        let Some(open) = self.open.get_mut(session_id) else {
            return Ok(());
        };
        open.meta.summary = ellipsize(summary_source, 80);
        open.meta.message_count = open.messages.len() as i64;
        open.meta.updated_at = now_ms();
        if let Some(hint) = title_hint {
            if !hint.is_empty() {
                open.meta.title = hint.to_string();
            }
        }
        let meta = open.meta.clone();
        self.write_meta(&meta)?;
        // Index row: bump updatedAt + move to top (touchSession).
        if let Some(pos) = self.index.iter().position(|r| r.id == session_id) {
            let mut row = self.index.remove(pos);
            row.updated_at = meta.updated_at;
            row.message_count = meta.message_count;
            row.summary = meta.summary.clone();
            row.title = meta.title.clone();
            self.index.insert(0, row);
        }
        Ok(())
    }

    fn touch_lru(&mut self, session_id: &str) {
        self.lru.retain(|id| id != session_id);
        self.lru.push_front(session_id.to_string());
    }

    fn evict_lru(&mut self) {
        while self.open.len() > MAX_OPEN_SESSIONS {
            match self.lru.pop_back() {
                Some(victim) => {
                    self.open.remove(&victim);
                }
                None => break,
            }
        }
    }

    /// Snapshot of the index as search targets (updatedAt-desc already).
    pub fn search_targets(&self) -> Vec<SearchTarget> {
        self.index
            .iter()
            .map(|r| SearchTarget {
                id: r.id.clone(),
                title: r.title.clone(),
                project_dir: r.project_dir.clone(),
                updated_at: r.updated_at,
                messages_path: self.paths.session_messages_path(&r.id),
            })
            .collect()
    }
}

/// Load + clean messages.jsonl (§4.1): empty lines skipped, corrupt lines
/// parsed as empty objects, legacy synthesis for missing `segments`.
pub fn load_messages(paths: &Paths, session_id: &str) -> Vec<MessageRow> {
    let Ok(bytes) = fs::read(paths.session_messages_path(session_id)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw in bytes.split(|&b| b == b'\n') {
        let line = trim_ascii(raw);
        if line.is_empty() {
            continue;
        }
        let parsed: MessageLine = serde_json::from_slice(line).unwrap_or_default();
        out.push(parsed.into_row());
    }
    out
}

fn trim_ascii(mut b: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = b.split_first() {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = b.split_last() {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Merge non-null fields of `patch` into `base` (upsert semantics §4.3).
fn merge_non_null(base: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (k, v) in patch {
        if !v.is_null() {
            base.insert(k.clone(), v.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Full-text search (§10). Synchronous scan core; the Tauri command layer
// runs it on a blocking task and checks `is_current(generation)` before
// emitting "search://results". A shared AtomicU64 generation supersedes
// stale scans exactly like the old QAtomicInt counter.
// ---------------------------------------------------------------------------

/// Max hits delivered per session (the full hit count is still reported).
pub const SEARCH_MAX_HITS_PER_SESSION: usize = 3;
/// Chars of context on each side of a hit.
pub const SEARCH_SNIPPET_CONTEXT: usize = 40;

#[derive(Debug, Clone)]
pub struct SearchTarget {
    pub id: String,
    pub title: String,
    pub project_dir: String,
    pub updated_at: i64,
    pub messages_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "sessionTitle")]
    pub session_title: String,
    #[serde(rename = "projectDir")]
    pub project_dir: String,
    pub snippet: String,
    pub timestamp: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(rename = "hitCount")]
    pub hit_count: usize,
    #[serde(rename = "titleOnly")]
    pub title_only: bool,
}

pub enum SearchOutcome {
    /// A newer query bumped the generation while this scan was running;
    /// results must be dropped.
    Superseded,
    Done {
        generation: u64,
        results: Vec<SearchHit>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SearchEngine {
    gen: Arc<AtomicU64>,
}

impl SearchEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.gen.load(Ordering::Acquire) == generation
    }

    /// Scan all targets (files only, no model state). Empty query cancels
    /// any in-flight search and returns empty results immediately.
    pub fn search(
        &self,
        targets: &[SearchTarget],
        query: &str,
        max_results: usize,
    ) -> SearchOutcome {
        let q = query.trim();
        let generation = self.gen.fetch_add(1, Ordering::AcqRel) + 1;
        if q.is_empty() || max_results == 0 {
            return SearchOutcome::Done {
                generation,
                results: Vec::new(),
            };
        }

        let mut results: Vec<SearchHit> = Vec::new();
        for t in targets {
            if !self.is_current(generation) {
                return SearchOutcome::Superseded;
            }
            // Collect every hit in this session's file (chronological order).
            let mut hits: Vec<(String, i64)> = Vec::new(); // (snippet, createdAt)
            if let Ok(bytes) = fs::read(&t.messages_path) {
                for raw in bytes.split(|&b| b == b'\n') {
                    let line = trim_ascii(raw);
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(line) else {
                        continue;
                    };
                    let role = o.get("role").and_then(Value::as_str).unwrap_or_default();
                    if role != "user" && role != "assistant" {
                        continue;
                    }
                    let content = scrub_leading_ellipsis(
                        o.get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    );
                    if content.is_empty() || content == PLACEHOLDER {
                        continue; // pending placeholder, no body yet
                    }
                    let Some((from, to)) = find_case_insensitive(&content, q) else {
                        continue;
                    };
                    let snippet = snippet_around(&content, from, to, SEARCH_SNIPPET_CONTEXT);
                    let ts = o
                        .get("createdAt")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as i64;
                    hits.push((snippet, ts));
                }
            }
            let title_hit = contains_case_insensitive(&t.title, q);
            if !title_hit && hits.is_empty() {
                continue;
            }
            if hits.is_empty() {
                // Title-only match: no message snippet to show.
                results.push(SearchHit {
                    session_id: t.id.clone(),
                    session_title: t.title.clone(),
                    project_dir: t.project_dir.clone(),
                    snippet: t.title.clone(),
                    timestamp: t.updated_at,
                    updated_at: t.updated_at,
                    hit_count: 0,
                    title_only: true,
                });
            } else {
                // Newest hits first; the JSONL file is chronological.
                for (snippet, ts) in hits
                    .iter()
                    .rev()
                    .take(SEARCH_MAX_HITS_PER_SESSION)
                {
                    results.push(SearchHit {
                        session_id: t.id.clone(),
                        session_title: t.title.clone(),
                        project_dir: t.project_dir.clone(),
                        snippet: snippet.clone(),
                        timestamp: *ts,
                        updated_at: t.updated_at,
                        hit_count: hits.len(),
                        title_only: false,
                    });
                }
            }
            if results.len() >= max_results {
                break;
            }
        }
        SearchOutcome::Done { generation, results }
    }
}

#[cfg(test)]
mod tests;
