// todos.json (data-formats.md §8.1): top-level `todos` array, entries in
// insertion order on disk (the pending/done sorted views are in-memory
// only). Entries with an empty id or title are dropped on load.

use serde::{Deserialize, Serialize};

use crate::store::json::{de_ms_i64, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum TodosError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TodoRow {
    #[serde(rename = "createdAt", deserialize_with = "de_ms_i64")]
    pub created_at: i64,
    pub done: bool,
    /// 0 while not done; toggling back to pending resets it to 0.
    #[serde(rename = "doneAt", deserialize_with = "de_ms_i64")]
    pub done_at: i64,
    pub id: String,
    pub title: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct TodosFile {
    todos: Vec<TodoRow>,
}

#[derive(Debug, Default)]
pub struct TodoStore {
    rows: Vec<TodoRow>,
}

impl TodoStore {
    pub fn load(paths: &Paths) -> Self {
        let file: TodosFile = std::fs::read(paths.todos_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            rows: file
                .todos
                .into_iter()
                .filter(|r| !r.id.is_empty() && !r.title.is_empty())
                .collect(),
        }
    }

    /// Disk order = insertion order.
    pub fn rows(&self) -> &[TodoRow] {
        &self.rows
    }

    /// Pending view: createdAt desc (in-memory only, never persisted).
    pub fn pending(&self) -> Vec<&TodoRow> {
        let mut v: Vec<&TodoRow> = self.rows.iter().filter(|r| !r.done).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        v
    }

    /// Done view: doneAt desc.
    pub fn done(&self) -> Vec<&TodoRow> {
        let mut v: Vec<&TodoRow> = self.rows.iter().filter(|r| r.done).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.done_at));
        v
    }

    pub fn add(&mut self, paths: &Paths, title: &str) -> Result<(), TodosError> {
        let t = title.trim();
        if t.is_empty() {
            return Ok(());
        }
        self.rows.push(TodoRow {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            title: t.to_string(),
            done: false,
            created_at: now_ms(),
            done_at: 0,
        });
        self.save(paths)
    }

    pub fn toggle(&mut self, paths: &Paths, id: &str) -> Result<(), TodosError> {
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.done = !r.done;
            r.done_at = if r.done { now_ms() } else { 0 };
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn remove(&mut self, paths: &Paths, id: &str) -> Result<(), TodosError> {
        let before = self.rows.len();
        self.rows.retain(|r| r.id != id);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn clear_done(&mut self, paths: &Paths) -> Result<(), TodosError> {
        let before = self.rows.len();
        self.rows.retain(|r| !r.done);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), TodosError> {
        paths.ensure_layout();
        let file = TodosFile {
            todos: self.rows.clone(),
        };
        write_value_atomic(&paths.todos_path(), &file)?;
        Ok(())
    }
}
