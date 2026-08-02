// todos.json: the unified todo/reminder model (data-formats.md §8.1).
// Top-level `todos` array, insertion order on disk (the sorted views are
// in-memory only). Each row carries a scope:
//
//   session — belongs to a session; due → popup (or push into the session
//             when notifyMode == "push", the MCP agent self-wakeup path)
//   project — belongs to a project (projectDir); due → auto-create a new
//             session in that project and ask the user how to proceed
//   global  — app-level personal board; due → popup notification only
//
// Legacy data migration (idempotent, runs on load): the old reminders.json
// (per-session one-shot reminders, features/chat.md §6.6) merges into
// todos.json — rows keep their ids so re-running skips already-migrated
// rows; source=="agent" rows keep notifyMode="push" (到期发回会话, MCP
// self-wakeup semantics), source=="user" rows become notifyMode="popup".
// The legacy file is left on disk untouched (no longer written).

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use crate::store::json::now_ms;
use crate::store::json::{de_ms_i64, write_value_atomic, JsonError};
use crate::store::paths::Paths;

pub const SCOPE_SESSION: &str = "session";
pub const SCOPE_PROJECT: &str = "project";
pub const SCOPE_GLOBAL: &str = "global";
pub const NOTIFY_POPUP: &str = "popup";
pub const NOTIFY_PUSH: &str = "push";

#[derive(Debug, thiserror::Error)]
pub enum TodosError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TodoRow {
    pub id: String,
    pub title: String,
    pub done: bool,
    #[serde(deserialize_with = "de_ms_i64")]
    pub created_at: i64,
    /// 0 while not done; toggling back to pending resets it to 0.
    #[serde(deserialize_with = "de_ms_i64")]
    pub done_at: i64,
    /// "session" | "project" | "global" (legacy rows without the key → global).
    pub scope: String,
    /// scope == session.
    pub session_id: String,
    /// scope == project (canonical dir).
    pub project_dir: String,
    /// 0 = no due time (plain list item).
    #[serde(deserialize_with = "de_ms_i64")]
    pub due_at_ms: i64,
    /// When the due notification fired (0 = not yet) — popup dedup guard.
    #[serde(deserialize_with = "de_ms_i64")]
    pub notified_at_ms: i64,
    /// "popup" (due → notification, user ticks it off) | "push" (due → send
    /// into the session as a chat prompt, MCP agent self-wakeup).
    pub notify_mode: String,
}

impl Default for TodoRow {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            done: false,
            created_at: 0,
            done_at: 0,
            scope: SCOPE_GLOBAL.to_string(),
            session_id: String::new(),
            project_dir: String::new(),
            due_at_ms: 0,
            notified_at_ms: 0,
            notify_mode: NOTIFY_POPUP.to_string(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct TodosFile {
    todos: Vec<TodoRow>,
}

/// Legacy reminders.json row (camelCase on disk, same field names).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LegacyReminder {
    id: String,
    session_id: String,
    content: String,
    #[serde(deserialize_with = "de_ms_i64")]
    due_at_ms: i64,
    #[serde(deserialize_with = "de_ms_i64")]
    created_at_ms: i64,
    source: String,
    done: bool,
}

impl Default for LegacyReminder {
    fn default() -> Self {
        Self {
            id: String::new(),
            session_id: String::new(),
            content: String::new(),
            due_at_ms: 0,
            created_at_ms: 0,
            source: "user".to_string(),
            done: false,
        }
    }
}

// ---- pure functions shared with the MCP subprocess ----

/// Tolerant read: missing/corrupt file loads as empty.
pub fn load_file(path: &Path) -> Vec<TodoRow> {
    let file: TodosFile = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    file.todos
        .into_iter()
        .filter(|r| !r.id.is_empty() && !r.title.is_empty())
        .collect()
}

/// Atomic full-file write (tmp + rename).
pub fn save_file(path: &Path, rows: &[TodoRow]) -> Result<(), JsonError> {
    write_value_atomic(
        path,
        &TodosFile {
            todos: rows.to_vec(),
        },
    )
}

/// Build a new todo row (uuid id, created_at = now). None on invalid input.
pub fn new_todo(
    title: &str,
    scope: &str,
    session_id: &str,
    project_dir: &str,
    due_at_ms: i64,
    notify_mode: &str,
) -> Option<TodoRow> {
    let title = title.trim();
    if title.is_empty() || !matches!(scope, SCOPE_SESSION | SCOPE_PROJECT | SCOPE_GLOBAL) {
        return None;
    }
    if scope == SCOPE_SESSION && session_id.is_empty() {
        return None;
    }
    if scope == SCOPE_PROJECT && project_dir.is_empty() {
        return None;
    }
    let now = now_ms();
    Some(TodoRow {
        id: uuid::Uuid::new_v4().hyphenated().to_string(),
        title: title.to_string(),
        done: false,
        created_at: now,
        done_at: 0,
        scope: scope.to_string(),
        session_id: session_id.to_string(),
        project_dir: project_dir.to_string(),
        due_at_ms: if due_at_ms > 0 { due_at_ms } else { 0 },
        notified_at_ms: 0,
        notify_mode: notify_mode.to_string(),
    })
}

// ---- in-app store ----

#[derive(Debug, Default)]
pub struct TodoStore {
    rows: Vec<TodoRow>,
}

impl TodoStore {
    pub fn load(paths: &Paths) -> Self {
        let mut rows = load_file(&paths.todos_path());
        // Idempotent legacy migration: reminders.json → session-scoped todos.
        let legacy = std::fs::read(paths.reminders_path())
            .ok()
            .and_then(|b| {
                serde_json::from_slice::<serde_json::Value>(&b).ok().map(|v| {
                    let rows: Vec<LegacyReminder> = v
                        .get("reminders")
                        .and_then(|r| serde_json::from_value(r.clone()).ok())
                        .unwrap_or_default();
                    rows
                })
            })
            .unwrap_or_default();
        for r in legacy {
            if r.id.is_empty() || r.content.is_empty() || r.session_id.is_empty() {
                continue;
            }
            if rows.iter().any(|x| x.id == r.id) {
                continue; // already migrated
            }
            rows.push(TodoRow {
                id: r.id,
                title: r.content,
                done: r.done,
                created_at: r.created_at_ms,
                done_at: 0,
                scope: SCOPE_SESSION.to_string(),
                session_id: r.session_id,
                project_dir: String::new(),
                due_at_ms: r.due_at_ms,
                notified_at_ms: 0,
                // Agent-set reminders kept their push semantics (到期发回会话).
                notify_mode: if r.source == "agent" {
                    NOTIFY_PUSH.to_string()
                } else {
                    NOTIFY_POPUP.to_string()
                },
            });
        }
        Self { rows }
    }

    /// Re-read the file from disk (captures rows written by the MCP
    /// subprocess since the last load).
    pub fn reload(&mut self, paths: &Paths) {
        *self = Self::load(paths);
    }

    /// Disk order = insertion order.
    pub fn rows(&self) -> &[TodoRow] {
        &self.rows
    }

    pub fn pending(&self) -> Vec<&TodoRow> {
        let mut v: Vec<&TodoRow> = self.rows.iter().filter(|r| !r.done).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        v
    }

    pub fn done(&self) -> Vec<&TodoRow> {
        let mut v: Vec<&TodoRow> = self.rows.iter().filter(|r| r.done).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.done_at));
        v
    }

    pub fn list_session(&self, session_id: &str) -> Vec<TodoRow> {
        self.rows
            .iter()
            .filter(|r| !r.done && r.scope == SCOPE_SESSION && r.session_id == session_id)
            .cloned()
            .collect()
    }

    pub fn list_project(&self, project_dir: &str) -> Vec<TodoRow> {
        self.rows
            .iter()
            .filter(|r| !r.done && r.scope == SCOPE_PROJECT && r.project_dir == project_dir)
            .cloned()
            .collect()
    }

    pub fn list_global(&self) -> Vec<TodoRow> {
        self.rows
            .iter()
            .filter(|r| !r.done && r.scope == SCOPE_GLOBAL)
            .cloned()
            .collect()
    }

    /// All pending rows, split by scope (frontend 待办页 grouping).
    pub fn pending_grouped(&self) -> (Vec<TodoRow>, Vec<TodoRow>, Vec<TodoRow>) {
        let mut session: Vec<TodoRow> = Vec::new();
        let mut project: Vec<TodoRow> = Vec::new();
        let mut global: Vec<TodoRow> = Vec::new();
        for r in self.rows.iter().filter(|r| !r.done) {
            match r.scope.as_str() {
                SCOPE_SESSION => session.push(r.clone()),
                SCOPE_PROJECT => project.push(r.clone()),
                _ => global.push(r.clone()),
            }
        }
        for v in [&mut session, &mut project, &mut global] {
            v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        }
        (session, project, global)
    }

    /// Due rows that have not been notified yet (not done, due_at in the
    /// past, notified_at == 0). Dedup guard for the manager-level tick.
    pub fn due_not_notified(&self, now: i64) -> Vec<TodoRow> {
        self.rows
            .iter()
            .filter(|r| {
                !r.done && r.due_at_ms > 0 && r.due_at_ms <= now && r.notified_at_ms == 0
            })
            .cloned()
            .collect()
    }

    /// Rows the session runtime pushes into ITS session: not done, session
    /// scope, notifyMode == push, due in the past.
    pub fn push_due_for(&self, session_id: &str, now: i64) -> Vec<TodoRow> {
        self.rows
            .iter()
            .filter(|r| {
                !r.done
                    && r.scope == SCOPE_SESSION
                    && r.session_id == session_id
                    && r.notify_mode == NOTIFY_PUSH
                    && r.due_at_ms > 0
                    && r.due_at_ms <= now
            })
            .cloned()
            .collect()
    }

    /// Earliest due time of the session's push rows (arm-the-timer helper).
    pub fn next_push_due(&self, session_id: &str) -> Option<i64> {
        self.rows
            .iter()
            .filter(|r| {
                !r.done
                    && r.scope == SCOPE_SESSION
                    && r.session_id == session_id
                    && r.notify_mode == NOTIFY_PUSH
                    && r.due_at_ms > 0
            })
            .map(|r| r.due_at_ms)
            .min()
    }

    pub fn add(
        &mut self,
        paths: &Paths,
        title: &str,
        scope: &str,
        session_id: &str,
        project_dir: &str,
        due_at_ms: i64,
        notify_mode: &str,
    ) -> Result<Option<TodoRow>, TodosError> {
        // Reload first so a concurrent MCP write is not clobbered.
        self.reload(paths);
        let Some(r) = new_todo(title, scope, session_id, project_dir, due_at_ms, notify_mode)
        else {
            return Ok(None);
        };
        self.rows.push(r.clone());
        self.save(paths)?;
        Ok(Some(r))
    }

    pub fn toggle(&mut self, paths: &Paths, id: &str) -> Result<(), TodosError> {
        self.reload(paths);
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.done = !r.done;
            r.done_at = if r.done { now_ms() } else { 0 };
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn remove(&mut self, paths: &Paths, id: &str) -> Result<(), TodosError> {
        self.reload(paths);
        let before = self.rows.len();
        self.rows.retain(|r| r.id != id);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn clear_done(&mut self, paths: &Paths) -> Result<(), TodosError> {
        self.reload(paths);
        let before = self.rows.len();
        self.rows.retain(|r| !r.done);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    /// Mark a due row as notified (dedup guard; returns true if it changed).
    pub fn mark_notified(&mut self, paths: &Paths, id: &str, now: i64) -> Result<bool, TodosError> {
        self.reload(paths);
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            if r.notified_at_ms == 0 && !r.done {
                r.notified_at_ms = now;
                self.save(paths)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Mark a due row done without toggling back (used after a project due
    /// fires a new session: the todo is considered handled once the session
    /// exists — the user can still un-tick it in the list).
    pub fn settle(&mut self, paths: &Paths, id: &str) -> Result<(), TodosError> {
        self.reload(paths);
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            if !r.done {
                r.done = true;
                r.done_at = now_ms();
                self.save(paths)?;
            }
        }
        Ok(())
    }

    /// Session deletion cleanup: drop every session-scoped row of the session.
    pub fn remove_session(&mut self, paths: &Paths, session_id: &str) -> Result<(), TodosError> {
        self.reload(paths);
        let before = self.rows.len();
        self.rows
            .retain(|r| !(r.scope == SCOPE_SESSION && r.session_id == session_id));
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), TodosError> {
        paths.ensure_layout();
        save_file(&paths.todos_path(), &self.rows)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(tmp.path().to_path_buf());
        (tmp, paths)
    }

    #[test]
    fn add_list_by_scope() {
        let (_tmp, paths) = tmp_paths();
        let mut store = TodoStore::load(&paths);
        store
            .add(&paths, "提交代码", SCOPE_SESSION, "s1", "", 0, NOTIFY_POPUP)
            .expect("add");
        store
            .add(&paths, "修 bug", SCOPE_PROJECT, "", "C:/proj", 0, NOTIFY_POPUP)
            .expect("add");
        store
            .add(&paths, "喝水", SCOPE_GLOBAL, "", "", 0, NOTIFY_POPUP)
            .expect("add");

        assert_eq!(store.list_session("s1").len(), 1);
        assert_eq!(store.list_project("C:/proj").len(), 1);
        assert_eq!(store.list_global().len(), 1);
        // Wrong scope keys see nothing.
        assert!(store.list_session("s2").is_empty());
        assert!(store.list_project("C:/other").is_empty());

        // Invalid rows are rejected.
        assert!(store
            .add(&paths, "  ", SCOPE_GLOBAL, "", "", 0, NOTIFY_POPUP)
            .expect("ok")
            .is_none());
        assert!(store
            .add(&paths, "无会话", SCOPE_SESSION, "", "", 0, NOTIFY_POPUP)
            .expect("ok")
            .is_none());
        assert!(store
            .add(&paths, "无项目", SCOPE_PROJECT, "", "", 0, NOTIFY_POPUP)
            .expect("ok")
            .is_none());
        assert!(store
            .add(&paths, "bad scope", "weird", "", "", 0, NOTIFY_POPUP)
            .expect("ok")
            .is_none());
    }

    #[test]
    fn due_and_notified_dedup() {
        let (_tmp, paths) = tmp_paths();
        let mut store = TodoStore::load(&paths);
        let due = store
            .add(&paths, "到点", SCOPE_GLOBAL, "", "", now_ms() - 1000, NOTIFY_POPUP)
            .expect("add")
            .expect("some");
        store
            .add(&paths, "未到", SCOPE_GLOBAL, "", "", now_ms() + 60_000, NOTIFY_POPUP)
            .expect("add");
        store
            .add(&paths, "无到期", SCOPE_GLOBAL, "", "", 0, NOTIFY_POPUP)
            .expect("add");

        assert_eq!(store.due_not_notified(now_ms()).len(), 1);
        // mark_notified → no longer in the due set.
        assert!(store
            .mark_notified(&paths, &due.id, now_ms())
            .expect("mark"));
        assert!(store.due_not_notified(now_ms()).is_empty());
        // Second mark is a no-op.
        assert!(!store
            .mark_notified(&paths, &due.id, now_ms())
            .expect("mark2"));
    }

    #[test]
    fn push_due_filters_by_session_and_mode() {
        let (_tmp, paths) = tmp_paths();
        let mut store = TodoStore::load(&paths);
        store
            .add(&paths, "push 到期", SCOPE_SESSION, "s1", "", now_ms() - 1, NOTIFY_PUSH)
            .expect("add");
        store
            .add(&paths, "popup 到期", SCOPE_SESSION, "s1", "", now_ms() - 1, NOTIFY_POPUP)
            .expect("add");
        store
            .add(&paths, "其它会话", SCOPE_SESSION, "s2", "", now_ms() - 1, NOTIFY_PUSH)
            .expect("add");
        let due = store.push_due_for("s1", now_ms());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].title, "push 到期");
    }

    #[test]
    fn toggle_remove_settle_session_cleanup() {
        let (_tmp, paths) = tmp_paths();
        let mut store = TodoStore::load(&paths);
        let r = store
            .add(&paths, "待办", SCOPE_SESSION, "s1", "", 0, NOTIFY_POPUP)
            .expect("add")
            .expect("some");
        store.toggle(&paths, &r.id).expect("toggle");
        assert!(store.rows()[0].done);

        store.settle(&paths, &r.id).expect("settle");
        assert!(store.rows()[0].done);

        store
            .add(&paths, "其它会话", SCOPE_SESSION, "s2", "", 0, NOTIFY_POPUP)
            .expect("add");
        store.remove_session(&paths, "s1").expect("remove_session");
        assert_eq!(store.rows().len(), 1, "s1 rows dropped, s2 kept");

        store.remove(&paths, &r.id).expect("remove");
        assert_eq!(store.rows().len(), 1);
    }

    #[test]
    fn legacy_reminders_migrate_once() {
        let (_tmp, paths) = tmp_paths();
        // Fixture: one agent (push) + one user (popup) reminder in the legacy file.
        let now = now_ms();
        let file = serde_json::json!({
            "reminders": [
                {
                    "id": "r-agent",
                    "sessionId": "s1",
                    "content": "agent 提醒",
                    "dueAtMs": now + 300_000,
                    "createdAtMs": now,
                    "source": "agent",
                    "done": false
                },
                {
                    "id": "r-user",
                    "sessionId": "s1",
                    "content": "用户提醒",
                    "dueAtMs": now + 600_000,
                    "createdAtMs": now,
                    "source": "user",
                    "done": false
                }
            ]
        });
        std::fs::write(&paths.reminders_path(), file.to_string()).expect("write");

        let store = TodoStore::load(&paths);
        let migrated: Vec<_> = store
            .rows()
            .iter()
            .filter(|r| r.scope == SCOPE_SESSION && r.session_id == "s1")
            .collect();
        assert_eq!(migrated.len(), 2);
        assert!(migrated.iter().any(|r| r.notify_mode == NOTIFY_PUSH && r.title == "agent 提醒"));
        assert!(migrated.iter().any(|r| r.notify_mode == NOTIFY_POPUP && r.title == "用户提醒"));
        assert!(migrated.iter().all(|r| r.due_at_ms > 0));

        // Reload: ids already present → no duplicates.
        let again = TodoStore::load(&paths);
        assert_eq!(
            again.rows().iter().filter(|r| r.scope == SCOPE_SESSION).count(),
            2,
            "idempotent migration"
        );
    }

    #[test]
    fn legacy_todos_default_to_global_popup() {
        let (_tmp, paths) = tmp_paths();
        let file = serde_json::json!({
            "todos": [{
                "id": "t1",
                "title": "旧待办",
                "done": false,
                "createdAt": 1000,
                "doneAt": 0
            }]
        });
        std::fs::write(&paths.todos_path(), file.to_string()).expect("write");
        let store = TodoStore::load(&paths);
        let r = &store.rows()[0];
        assert_eq!(r.scope, SCOPE_GLOBAL);
        assert_eq!(r.notify_mode, NOTIFY_POPUP);
        assert_eq!(r.due_at_ms, 0);
    }
}
