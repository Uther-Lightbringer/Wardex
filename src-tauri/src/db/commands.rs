// Tauri command layer for the database feature. These power the query
// window (schema tree / editor execution / write confirmation). The write
// toggle is a frontend concern — the backend only asks whether writes are
// allowed for THIS call (PRD M6: 全局写开关对所有连接即时生效).
//
// Execution flow: prepare_batch (gate + auto LIMIT) → if the batch contains
// write statements and writes are allowed but not yet confirmed, return
// `need_confirm` with the final statements; the frontend shows its dialog
// and re-calls with `confirmed = true`. Readonly mode never reaches that
// branch (the gate rejects writes outright).

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;

use crate::db::driver::QueryResult;
use crate::db::gate;

#[derive(Debug, Deserialize)]
pub struct NamedConnInput {
    pub name: String,
    pub dsn: String,
}

#[derive(serde::Serialize)]
pub struct ExecuteOutcome {
    pub need_confirm: bool,
    pub has_write: bool,
    pub statements: Vec<String>,
    pub results: Option<Vec<QueryResult>>,
}

/// Saved connections + aliases + which connections are currently open.
#[tauri::command]
pub fn db_conns(state: State<'_, crate::AppState>, project_dir: String) -> Value {
    let stores = crate::lock(&state.stores);
    json!({
        "connections": stores.db_conns.connections(&project_dir),
        "aliases": stores.db_conns.aliases(&project_dir),
        "open": state.db.open_names(&project_dir),
    })
}

#[tauri::command]
pub fn db_save_conns(
    state: State<'_, crate::AppState>,
    project_dir: String,
    connections: Vec<NamedConnInput>,
) -> Result<(), String> {
    let mut stores = crate::lock(&state.stores);
    let paths = stores.paths.clone();
    let conns = connections
        .into_iter()
        .map(|c| crate::store::NamedConn {
            name: c.name,
            dsn: c.dsn,
        })
        .collect();
    stores.db_conns.set_connections(&paths, &project_dir, conns).map_err(crate::err)
}

#[tauri::command]
pub fn db_set_alias(
    state: State<'_, crate::AppState>,
    project_dir: String,
    key: String,
    alias: String,
) -> Result<(), String> {
    let mut stores = crate::lock(&state.stores);
    let paths = stores.paths.clone();
    stores
        .db_conns
        .set_alias(&paths, &project_dir, &key, &alias)
        .map_err(crate::err)
}

#[tauri::command]
pub async fn db_open(
    state: State<'_, crate::AppState>,
    project_dir: String,
    name: String,
    dsn: String,
) -> Result<(), String> {
    state.db.open(&project_dir, &name, &dsn).await
}

#[tauri::command]
pub fn db_close(state: State<'_, crate::AppState>, project_dir: String, name: String) {
    state.db.close(&project_dir, &name);
}

#[tauri::command]
pub fn db_close_all(state: State<'_, crate::AppState>, project_dir: String) {
    state.db.close_project(&project_dir);
}

#[tauri::command]
pub async fn db_tables(
    state: State<'_, crate::AppState>,
    project_dir: String,
    name: String,
    schema: Option<String>,
    keyword: Option<String>,
) -> Result<Value, String> {
    let tables = state
        .db
        .tables(&project_dir, &name, schema.as_deref(), keyword.as_deref())
        .await?;
    Ok(serde_json::to_value(tables).map_err(crate::err)?)
}

#[tauri::command]
pub async fn db_columns(
    state: State<'_, crate::AppState>,
    project_dir: String,
    name: String,
    qualified: String,
) -> Result<Value, String> {
    let cols = state.db.columns(&project_dir, &name, &qualified).await?;
    Ok(serde_json::to_value(cols).map_err(crate::err)?)
}

/// Execute a batch through the gate. See module doc for the confirm dance.
#[tauri::command]
pub async fn db_execute(
    state: State<'_, crate::AppState>,
    project_dir: String,
    name: String,
    sql: String,
    allow_write: bool,
    confirmed: bool,
) -> Result<Value, String> {
    let stmts = gate::prepare_batch(&sql, allow_write)?;
    let has_write = gate::contains_write(&stmts);
    if has_write && allow_write && !confirmed {
        return Ok(json!(ExecuteOutcome {
            need_confirm: true,
            has_write,
            statements: stmts,
            results: None,
        }));
    }
    let mut results = Vec::with_capacity(stmts.len());
    for s in &stmts {
        let r = state.db.query(&project_dir, &name, s, 100, allow_write).await?;
        results.push(r);
    }
    Ok(json!(ExecuteOutcome {
        need_confirm: false,
        has_write,
        statements: stmts,
        results: Some(results),
    }))
}
