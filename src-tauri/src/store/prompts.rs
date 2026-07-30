// prompts.json (data-formats.md §8.2): top-level `prompts` array.
//
// First-launch seed: ONLY when the file has never existed AND parses empty,
// three built-in Chinese templates are written verbatim (trailing '\n' is
// part of the text — the template is meant to prefix the actual code).
// A user who deletes all templates leaves an empty-but-existing file, which
// is never re-seeded.

use serde::{Deserialize, Serialize};

use crate::store::json::{de_ms_i64, left_chars, now_ms, write_value_atomic, JsonError};
use crate::store::paths::Paths;

#[derive(Debug, thiserror::Error)]
pub enum PromptsError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptRow {
    #[serde(rename = "createdAt", deserialize_with = "de_ms_i64")]
    pub created_at: i64,
    pub id: String,
    pub name: String,
    pub text: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PromptsFile {
    prompts: Vec<PromptRow>,
}

/// Built-in seed templates (verbatim; trailing newline included).
const SEEDS: [(&str, &str); 3] = [
    (
        "代码审查",
        "请审查以下代码，指出潜在的 bug、边界条件问题和可改进点，并给出具体的修改建议：\n",
    ),
    (
        "解释代码",
        "请逐段解释以下代码的作用、实现思路和关键细节：\n",
    ),
    (
        "重构建议",
        "请分析以下代码的结构，在保持行为不变的前提下给出具体的重构方案：\n",
    ),
];

#[derive(Debug, Default)]
pub struct PromptStore {
    rows: Vec<PromptRow>,
}

impl PromptStore {
    pub fn load(paths: &Paths) -> Self {
        // Seed only when the file has never existed; an empty or hand-trimmed
        // file is a deliberate user choice and stays untouched.
        let first_launch = !paths.prompts_path().exists();
        let mut store = Self::load_no_seed(paths);
        if first_launch && store.rows.is_empty() {
            let now = now_ms();
            for (name, text) in SEEDS {
                store.rows.push(PromptRow {
                    created_at: now,
                    id: uuid::Uuid::new_v4().hyphenated().to_string(),
                    name: name.to_string(),
                    text: text.to_string(),
                });
            }
            // Best-effort seed write; a failed write just retries next launch.
            let _ = store.save(paths);
        }
        store
    }

    /// Load without the seed step (for tests of the no-reseed rule).
    fn load_no_seed(paths: &Paths) -> Self {
        let file: PromptsFile = std::fs::read(paths.prompts_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            rows: file
                .prompts
                .into_iter()
                .filter(|r| !r.id.is_empty() && !r.text.is_empty())
                .collect(),
        }
    }

    pub fn rows(&self) -> &[PromptRow] {
        &self.rows
    }

    /// add: text trimmed (empty → ignored); an empty name falls back to the
    /// text's first line, left(20) — the menu must always have something
    /// readable to show.
    pub fn add(&mut self, paths: &Paths, name: &str, text: &str) -> Result<(), PromptsError> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(());
        }
        let n = name.trim();
        let n = if n.is_empty() {
            let first_line = t.split('\n').next().unwrap_or_default();
            left_chars(first_line, 20)
        } else {
            n.to_string()
        };
        self.rows.push(PromptRow {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            name: n,
            text: t.to_string(),
            created_at: now_ms(),
        });
        self.save(paths)
    }

    pub fn remove(&mut self, paths: &Paths, id: &str) -> Result<(), PromptsError> {
        let before = self.rows.len();
        self.rows.retain(|r| r.id != id);
        if self.rows.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    pub fn save(&self, paths: &Paths) -> Result<(), PromptsError> {
        paths.ensure_layout();
        let file = PromptsFile {
            prompts: self.rows.clone(),
        };
        write_value_atomic(&paths.prompts_path(), &file)?;
        Ok(())
    }
}
