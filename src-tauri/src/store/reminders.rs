// reminders.json: top-level `reminders` array, insertion order on disk,
// filtered by sessionId in memory. One-shot reminders only (no recurrence).
//
// Two writers share the single file: the main app (via ReminderStore) and
// the `wardex --mcp-reminder` MCP stdio subprocess (via the pure load/save
// functions below — the subprocess has no StoreRegistry). Both write
// atomically (tmp + rename), and the app re-reads the file at every reload
// point (turn end / tick / manual add-cancel), so last-writer-wins races
// stay benign: a reminder is never fired twice because mark_done persists
// before the prompt goes out.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::json::{de_ms_i64, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum RemindersError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    pub session_id: String,
    pub content: String,
    #[serde(deserialize_with = "de_ms_i64")]
    pub due_at_ms: i64,
    #[serde(deserialize_with = "de_ms_i64")]
    pub created_at_ms: i64,
    /// "agent" (set via the MCP tool) | "user" (set from the UI).
    pub source: String,
    pub done: bool,
}

impl Default for Reminder {
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

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct RemindersFile {
    reminders: Vec<Reminder>,
}

// ---- pure functions shared with the MCP subprocess ----

/// Tolerant read: missing/corrupt file loads as empty.
pub fn load_file(path: &Path) -> Vec<Reminder> {
    let file: RemindersFile = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    file.reminders
        .into_iter()
        .filter(|r| !r.id.is_empty() && !r.session_id.is_empty() && !r.content.is_empty())
        .collect()
}

/// Atomic full-file write (tmp + rename).
pub fn save_file(path: &Path, rows: &[Reminder]) -> Result<(), JsonError> {
    write_value_atomic(
        path,
        &RemindersFile {
            reminders: rows.to_vec(),
        },
    )
}

/// Build a new reminder row (uuid id, created_at = now).
pub fn new_reminder(session_id: &str, content: &str, minutes: f64, source: &str) -> Option<Reminder> {
    let content = content.trim();
    if session_id.is_empty() || content.is_empty() || minutes <= 0.0 {
        return None;
    }
    let now = now_ms();
    Some(Reminder {
        id: uuid::Uuid::new_v4().hyphenated().to_string(),
        session_id: session_id.to_string(),
        content: content.to_string(),
        due_at_ms: now + (minutes * 60_000.0) as i64,
        created_at_ms: now,
        source: source.to_string(),
        done: false,
    })
}

// ---- in-app store ----

#[derive(Debug, Default)]
pub struct ReminderStore {
    rows: Vec<Reminder>,
}

impl ReminderStore {
    pub fn load(paths: &Paths) -> Self {
        Self {
            rows: load_file(&paths.reminders_path()),
        }
    }

    /// Re-read the file from disk (captures rows written by the MCP
    /// subprocess since the last load).
    pub fn reload(&mut self, paths: &Paths) {
        self.rows = load_file(&paths.reminders_path());
    }

    pub fn rows(&self) -> &[Reminder] {
        &self.rows
    }

    pub fn list(&self, session_id: &str) -> Vec<Reminder> {
        self.rows
            .iter()
            .filter(|r| r.session_id == session_id && !r.done)
            .cloned()
            .collect()
    }

    pub fn add(
        &mut self,
        paths: &Paths,
        session_id: &str,
        content: &str,
        minutes: f64,
        source: &str,
    ) -> Result<Option<Reminder>, RemindersError> {
        // Reload first so a concurrent MCP write is not clobbered.
        self.reload(paths);
        let Some(r) = new_reminder(session_id, content, minutes, source) else {
            return Ok(None);
        };
        self.rows.push(r.clone());
        self.save(paths)?;
        Ok(Some(r))
    }

    pub fn cancel(&mut self, paths: &Paths, id: &str) -> Result<bool, RemindersError> {
        self.reload(paths);
        let before = self.rows.len();
        self.rows.retain(|r| r.id != id);
        if self.rows.len() != before {
            self.save(paths)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn mark_done(&mut self, paths: &Paths, id: &str) -> Result<bool, RemindersError> {
        self.reload(paths);
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            if !r.done {
                r.done = true;
                self.save(paths)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Session deletion cleanup: drop every reminder of the session.
    pub fn remove_session(&mut self, paths: &Paths, session_id: &str) -> Result<(), RemindersError> {
        self.reload(paths);
        let before = self.rows.len();
        self.rows.retain(|r| r.session_id != session_id);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), RemindersError> {
        save_file(&paths.reminders_path(), &self.rows)?;
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
    fn add_list_cancel_by_session() {
        let (_tmp, paths) = tmp_paths();
        let mut store = ReminderStore::load(&paths);
        let r1 = store
            .add(&paths, "s1", "喝水", 1.0, "user")
            .expect("add")
            .expect("some");
        store
            .add(&paths, "s2", "其它会话", 5.0, "agent")
            .expect("add");
        assert_eq!(store.list("s1").len(), 1);
        assert_eq!(store.list("s2").len(), 1);
        assert_eq!(r1.content, "喝水");

        // Reload from disk (MCP-subprocess view) sees the same rows.
        let fresh = ReminderStore::load(&paths);
        assert_eq!(fresh.rows().len(), 2);

        assert!(store.cancel(&paths, &r1.id).expect("cancel"));
        assert!(store.list("s1").is_empty());
        assert!(!store.cancel(&paths, "no-such-id").expect("cancel"));

        // remove_session drops the other session's rows too.
        store.remove_session(&paths, "s2").expect("remove_session");
        assert!(store.rows().is_empty());
    }

    #[test]
    fn mark_done_persists_and_filters() {
        let (_tmp, paths) = tmp_paths();
        let mut store = ReminderStore::load(&paths);
        let r = store
            .add(&paths, "s1", "休息", 1.0, "agent")
            .expect("add")
            .expect("some");
        assert!(store.mark_done(&paths, &r.id).expect("done"));
        assert!(store.list("s1").is_empty(), "done rows leave the pending list");
        let fresh = ReminderStore::load(&paths);
        assert!(fresh.rows()[0].done, "done flag survived the round-trip");
        // JSON keys are camelCase on disk.
        let text = std::fs::read_to_string(paths.reminders_path()).expect("read");
        assert!(text.contains("\"dueAtMs\""), "{text}");
        assert!(text.contains("\"sessionId\""), "{text}");
    }

    #[test]
    fn invalid_input_is_rejected_and_missing_file_loads_empty() {
        let (_tmp, paths) = tmp_paths();
        let mut store = ReminderStore::load(&paths);
        assert!(store.rows().is_empty());
        assert!(store
            .add(&paths, "s1", "   ", 1.0, "user")
            .expect("ok")
            .is_none());
        assert!(store
            .add(&paths, "s1", "x", 0.0, "user")
            .expect("ok")
            .is_none());
    }
}
