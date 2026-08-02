// Database driver abstraction (PGAssistant `db_tools` + driver port).
//
// The abstraction boundary lives here: `DbDriver` is the contract every
// concrete backend implements, and `Driver` is the owned enum that gets
// stored in `DbManager`. Adding a new database later = one new variant in
// `Driver`, one new impl of `DbDriver` — the SQL gate (pure text), the MCP
// server and every Tauri command stay untouched.
//
// Concurrency: all methods are `&self` and thread-safe (`Send + Sync`); the
// Postgres impl uses an sqlx pool internally. `query()` carries `allow_write`
// per call so a global write toggle (PRD M6) takes effect immediately on
// every already-open connection — the server-side readonly + timeout session
// settings are re-applied right before each statement runs (layers 3/4).

use crate::db::pg::PgDriver;

/// Target database dialect. Extend when a new backend lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
}

/// One result column as surfaced to the UI / MCP: name + server type name.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Col {
    pub name: String,
    pub type_name: String,
}

/// A query result: rows of text-formatted cells, NULL as `None`.
/// `truncated` is set when more than `max_rows` rows existed (the UI shows
/// "仅显示前 100 行（已被截断）"); `affected` carries the row count for
/// non-query statements.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryResult {
    pub columns: Vec<Col>,
    pub rows: Vec<Vec<Option<String>>>,
    pub truncated: bool,
    pub affected: Option<u64>,
}

/// A table (or view) surfaced by the schema tree / MCP `list_tables`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableMeta {
    pub schema: String,
    pub name: String,
    pub comment: String,
}

/// A column surfaced by the schema tree / MCP `describe_table`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
    pub not_null: bool,
    pub comment: String,
}

/// The backend contract. Implemented by `PgDriver`; `Driver` delegates.
pub trait DbDriver: Send + Sync {
    fn dialect(&self) -> Dialect;

    /// Execute one (already gated) statement. `allow_write` toggles the
    /// server-side readonly session for THIS call (defense layer 3) and the
    /// 60s statement timeout is always applied (defense layer 4).
    async fn query(
        &self,
        sql: &str,
        max_rows: u32,
        allow_write: bool,
    ) -> anyhow::Result<QueryResult>;

    /// Schema metadata: `schema`/`keyword` narrow the listing.
    async fn tables(
        &self,
        schema: Option<&str>,
        keyword: Option<&str>,
    ) -> anyhow::Result<Vec<TableMeta>>;

    /// Columns of `schema.table`.
    async fn columns(&self, qualified: &str) -> anyhow::Result<Vec<ColumnMeta>>;
}

/// Owned, storable driver. This is what `DbManager` holds per connection.
#[derive(Clone)]
pub enum Driver {
    Postgres(PgDriver),
}

impl Driver {
    pub async fn connect(dsn: &str, dialect: Dialect) -> anyhow::Result<Self> {
        match dialect {
            Dialect::Postgres => Ok(Driver::Postgres(PgDriver::connect(dsn).await?)),
        }
    }
}

impl DbDriver for Driver {
    fn dialect(&self) -> Dialect {
        match self {
            Driver::Postgres(_) => Dialect::Postgres,
        }
    }

    async fn query(
        &self,
        sql: &str,
        max_rows: u32,
        allow_write: bool,
    ) -> anyhow::Result<QueryResult> {
        match self {
            Driver::Postgres(p) => p.query(sql, max_rows, allow_write).await,
        }
    }

    async fn tables(
        &self,
        schema: Option<&str>,
        keyword: Option<&str>,
    ) -> anyhow::Result<Vec<TableMeta>> {
        match self {
            Driver::Postgres(p) => p.tables(schema, keyword).await,
        }
    }

    async fn columns(&self, qualified: &str) -> anyhow::Result<Vec<ColumnMeta>> {
        match self {
            Driver::Postgres(p) => p.columns(qualified).await,
        }
    }
}
