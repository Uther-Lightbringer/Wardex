// Database feature root: driver abstraction + PostgreSQL backend + SQL gate.
//
// Two surfaces share this module:
//   - Tauri commands (`commands.rs`) power the query window (schema tree,
//     editor execution, write confirmations).
//   - `mcp.rs` exposes metadata-only tools (`--mcp-db`) to ACP sessions so a
//     chat agent can inspect table structure without ever seeing data rows.
//
// Connection model (project-bound): DbManager keeps open drivers keyed by
// project dir → connection name (开发环境 / 测试环境 / 预发布环境 …). Each
// named connection is an independent pool with its own session state.

pub mod commands;
pub mod driver;
pub mod gate;
pub mod mcp;
pub mod pg;

use std::collections::HashMap;
use std::sync::Mutex;

use driver::{ColumnMeta, DbDriver, Dialect, Driver, QueryResult, TableMeta};

/// Open connection registry. `Driver` is Clone (sqlx pools are cheap clones),
/// so a driver is handed out under a short lock and used without holding it.
pub struct DbManager {
    projects: Mutex<HashMap<String, HashMap<String, Driver>>>,
}

impl DbManager {
    pub fn new() -> Self {
        Self {
            projects: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, HashMap<String, Driver>>> {
        self.projects.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn driver(&self, project: &str, name: &str) -> Option<Driver> {
        self.lock().get(project)?.get(name).cloned()
    }

    pub async fn open(&self, project: &str, name: &str, dsn: &str) -> Result<(), String> {
        if self.driver(project, name).is_some() {
            return Ok(());
        }
        let d = Driver::connect(dsn, Dialect::Postgres)
            .await
            .map_err(|e| format!("连接「{name}」失败：{e}"))?;
        self.lock()
            .entry(project.to_string())
            .or_default()
            .insert(name.to_string(), d);
        Ok(())
    }

    pub fn close(&self, project: &str, name: &str) {
        if let Some(map) = self.lock().get_mut(project) {
            map.remove(name);
        }
    }

    pub fn close_project(&self, project: &str) {
        self.lock().remove(project);
    }

    pub fn open_names(&self, project: &str) -> Vec<String> {
        self.lock()
            .get(project)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn query(
        &self,
        project: &str,
        name: &str,
        sql: &str,
        max_rows: u32,
        allow_write: bool,
    ) -> Result<QueryResult, String> {
        let d = self.driver(project, name).ok_or("连接未打开")?;
        d.query(sql, max_rows, allow_write).await.map_err(|e| e.to_string())
    }

    pub async fn tables(
        &self,
        project: &str,
        name: &str,
        schema: Option<&str>,
        keyword: Option<&str>,
    ) -> Result<Vec<TableMeta>, String> {
        let d = self.driver(project, name).ok_or("连接未打开")?;
        d.tables(schema, keyword).await.map_err(|e| e.to_string())
    }

    pub async fn columns(
        &self,
        project: &str,
        name: &str,
        qualified: &str,
    ) -> Result<Vec<ColumnMeta>, String> {
        let d = self.driver(project, name).ok_or("连接未打开")?;
        d.columns(qualified).await.map_err(|e| e.to_string())
    }
}

impl Default for DbManager {
    fn default() -> Self {
        Self::new()
    }
}
