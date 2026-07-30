// projects.json: recent project list + independent alias map
// (data-formats.md §6).
//
// Compatibility quirks preserved:
//   - `recent` is newest-first, capped at 8; entries carry only
//     {path, lastOpenedAt} — never the alias.
//   - Dedup/matching is CASE-INSENSITIVE (Windows semantics)…
//   - …but the aliases map keys are inserted CASE-SENSITIVELY (old QHash
//     exact-match behavior): the same directory with different casing
//     produces two alias keys. Do NOT "clean this up".
//   - Aliases are stored separately from recent so they survive a project
//     falling off the recent list.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::store::json::{de_ms_i64, left_chars, now_ms, write_value_atomic, JsonError};
use crate::store::paths::{canonical_dir, Paths};

pub const MAX_RECENT: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ProjectsError {
    #[error("io/json error: {0}")]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentEntry {
    #[serde(rename = "lastOpenedAt", deserialize_with = "de_ms_i64")]
    pub last_opened_at: i64,
    pub path: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ProjectsFile {
    aliases: BTreeMap<String, String>,
    recent: Vec<RecentEntry>,
}

#[derive(Debug, Default)]
pub struct ProjectStore {
    recent: Vec<RecentEntry>,
    aliases: BTreeMap<String, String>,
}

impl ProjectStore {
    /// Tolerant load: missing/corrupt file → empty store; empty alias values
    /// dropped; recent truncated to the cap; entries without a path skipped.
    pub fn load(paths: &Paths) -> Self {
        let file: ProjectsFile = std::fs::read(paths.projects_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let aliases = file
            .aliases
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let recent = file
            .recent
            .into_iter()
            .filter(|r| !r.path.is_empty())
            .take(MAX_RECENT)
            .collect();
        Self { recent, aliases }
    }

    pub fn recent(&self) -> &[RecentEntry] {
        &self.recent
    }

    pub fn aliases(&self) -> &BTreeMap<String, String> {
        &self.aliases
    }

    /// touchProject: case-insensitive dedupe, prepend, truncate to 8.
    pub fn touch_project(&mut self, paths: &Paths, dir: &str) -> Result<(), ProjectsError> {
        let key = canonical_dir(dir);
        if key.is_empty() {
            return Ok(());
        }
        self.recent
            .retain(|r| !r.path.eq_ignore_ascii_case(&key));
        self.recent.insert(
            0,
            RecentEntry {
                path: key,
                last_opened_at: now_ms(),
            },
        );
        self.recent.truncate(MAX_RECENT);
        self.save(paths)
    }

    /// setAlias: trim + left(24); empty string removes the key. NOTE: the
    /// alias map key is matched CASE-SENSITIVELY (old QHash quirk), while the
    /// recent-list update below matches case-insensitively.
    pub fn set_alias(&mut self, paths: &Paths, dir: &str, alias: &str) -> Result<(), ProjectsError> {
        let key = canonical_dir(dir);
        if key.is_empty() {
            return Ok(());
        }
        let a = left_chars(alias.trim(), 24);
        if self.aliases.get(&key).map(String::as_str) == Some(a.as_str()) {
            return Ok(());
        }
        if a.is_empty() {
            self.aliases.remove(&key);
        } else {
            self.aliases.insert(key, a);
        }
        self.save(paths)
    }

    pub fn remove_project(&mut self, paths: &Paths, dir: &str) -> Result<(), ProjectsError> {
        let key = canonical_dir(dir);
        let before = self.recent.len();
        self.recent.retain(|r| !r.path.eq_ignore_ascii_case(&key));
        if self.recent.len() != before {
            self.save(paths)?;
        }
        Ok(())
    }

    /// Display name fallback chain: alias (exact key) → directory basename →
    /// whole path in native separators (drive roots like "C:/" have no
    /// basename).
    pub fn display_name_for(&self, dir: &str) -> String {
        let key = canonical_dir(dir);
        if let Some(a) = self.aliases.get(&key) {
            if !a.is_empty() {
                return a.clone();
            }
        }
        display_name_of(&key)
    }

    /// Basename-or-native-path fallback shared with the recent list rows.
    pub fn display_name_of_path(path: &str) -> String {
        display_name_of(path)
    }

    pub fn save(&self, paths: &Paths) -> Result<(), ProjectsError> {
        paths.ensure_layout();
        let file = ProjectsFile {
            aliases: self.aliases.clone(),
            recent: self.recent.clone(),
        };
        write_value_atomic(&paths.projects_path(), &file)?;
        Ok(())
    }
}

fn display_name_of(canonical_path: &str) -> String {
    match canonical_path.rsplit('/').next() {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => canonical_path.replace('/', "\\"),
    }
}
