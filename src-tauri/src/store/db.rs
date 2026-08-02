// db_conns.json — project-scoped database connections and table/column
// aliases (the PGAssistant `config.py` equivalent, but per project).
//
// Format:
//   { "projects": {
//       "C:/workspace/orders": {
//         "connections": [ { "name": "开发环境", "dsn": "postgresql://…" }, … ],
//         "aliases": { "public.orders": "订单", "public.orders.user_id": "用户ID" }
//       }
//     } }
//
// Connections are an ordered Vec (insertion order = tab order, dev/test/pre
// as the user arranges them); aliases are shared across every connection of a
// project since they describe the schema, not the environment.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::store::json::write_value_atomic;
use crate::store::paths::{canonical_dir, Paths};

#[derive(Debug, thiserror::Error)]
pub enum DbConnsError {
    #[error("io/json error: {0}")]
    Json(#[from] crate::store::json::JsonError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedConn {
    pub name: String,
    pub dsn: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectDbConns {
    pub connections: Vec<NamedConn>,
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct DbConnsFile {
    projects: BTreeMap<String, ProjectDbConns>,
}

#[derive(Debug, Default)]
pub struct DbConnStore {
    projects: BTreeMap<String, ProjectDbConns>,
}

impl DbConnStore {
    /// Tolerant load: missing/corrupt file → empty store; connection entries
    /// without a name are dropped.
    pub fn load(paths: &Paths) -> Self {
        let file: DbConnsFile = std::fs::read(paths.db_conns_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut projects = file.projects;
        for p in projects.values_mut() {
            p.connections.retain(|c| !c.name.is_empty());
        }
        Self { projects }
    }

    pub fn save(&self, paths: &Paths) -> Result<(), DbConnsError> {
        paths.ensure_layout();
        let file = DbConnsFile {
            projects: self.projects.clone(),
        };
        write_value_atomic(&paths.db_conns_path(), &file)?;
        Ok(())
    }

    fn key(dir: &str) -> String {
        canonical_dir(dir)
    }

    /// Named connections for a project (in stored order).
    pub fn connections(&self, dir: &str) -> Vec<NamedConn> {
        self.projects
            .get(&Self::key(dir))
            .map(|p| p.connections.clone())
            .unwrap_or_default()
    }

    /// Table/column aliases for a project.
    pub fn aliases(&self, dir: &str) -> BTreeMap<String, String> {
        self.projects
            .get(&Self::key(dir))
            .map(|p| p.aliases.clone())
            .unwrap_or_default()
    }

    /// Replace the project's whole connection list (preserves order).
    pub fn set_connections(
        &mut self,
        paths: &Paths,
        dir: &str,
        conns: Vec<NamedConn>,
    ) -> Result<(), DbConnsError> {
        let key = Self::key(dir);
        if key.is_empty() {
            return Ok(());
        }
        let proj = self.projects.entry(key).or_default();
        proj.connections = conns.into_iter().filter(|c| !c.name.is_empty()).collect();
        self.save(paths)
    }

    /// Set or clear (empty value) one alias key `schema.table[.column]`.
    pub fn set_alias(
        &mut self,
        paths: &Paths,
        dir: &str,
        key: &str,
        alias: &str,
    ) -> Result<(), DbConnsError> {
        let dir_key = Self::key(dir);
        if dir_key.is_empty() || key.is_empty() {
            return Ok(());
        }
        let a = alias.trim().to_string();
        let proj = self.projects.entry(dir_key).or_default();
        if a.is_empty() {
            proj.aliases.remove(key);
        } else {
            proj.aliases.insert(key.to_string(), a);
        }
        self.save(paths)
    }

    pub fn remove_project(&mut self, paths: &Paths, dir: &str) -> Result<(), DbConnsError> {
        self.projects.remove(&Self::key(dir));
        self.save(paths)
    }
}
