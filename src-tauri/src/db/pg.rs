// PostgreSQL driver (tokio-postgres). Implements `DbDriver`; defence layers
// 3/4 are enforced per-execution so the global write toggle applies instantly:
//   layer 3 — `SET default_transaction_read_only = on|off`
//   layer 4 — `SET statement_timeout = 60000`
//
// The user's statement runs through the SIMPLE query protocol (`simple_query`)
// so every result cell arrives in PostgreSQL's text format — exactly what a
// generic query tool needs (no per-type binary decoding). Column TYPE names
// come from a cheap `PREPARE` (describe only, never executes); when prepare is
// unavailable the frontend falls back to value-shape inference.
//
// Known tradeoff (PRD 03 §3.5 style): simple_query materialises the whole
// result before we truncate to `max_rows`; the gate's auto `LIMIT 10` and the
// 60s statement timeout bound the realistic exposure.

use std::sync::Arc;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::db::driver::{Col, ColumnMeta, DbDriver, Dialect, QueryResult, TableMeta};

#[derive(Clone)]
pub struct PgDriver {
    client: Arc<Client>,
}

const STATEMENT_TIMEOUT: &str = "SET statement_timeout = 60000";
const READ_ONLY_ON: &str = "SET default_transaction_read_only = on";
const READ_ONLY_OFF: &str = "SET default_transaction_read_only = off";

impl PgDriver {
    pub async fn connect(dsn: &str) -> anyhow::Result<Self> {
        // TODO(TLS): NoTls only — add rustls/native-tls when a target server
        // requires sslmode=require.
        let (client, connection) = tokio_postgres::connect(dsn, NoTls).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::warn!("postgres connection lost: {e}");
            }
        });
        Ok(Self {
            client: Arc::new(client),
        })
    }

    async fn session_setup(&self, allow_write: bool) -> anyhow::Result<()> {
        self.client.batch_execute(STATEMENT_TIMEOUT).await?;
        self.client
            .batch_execute(if allow_write { READ_ONLY_OFF } else { READ_ONLY_ON })
            .await?;
        Ok(())
    }

    /// Column type names via a describe-only PREPARE (best effort).
    async fn column_types(&self, sql: &str) -> Vec<String> {
        match self.client.prepare(sql).await {
            Ok(stmt) => stmt
                .columns()
                .iter()
                .map(|c| c.type_().name().to_string())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl DbDriver for PgDriver {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    async fn query(
        &self,
        sql: &str,
        max_rows: u32,
        allow_write: bool,
    ) -> anyhow::Result<QueryResult> {
        self.session_setup(allow_write).await?;
        let prepared_types = self.column_types(sql).await;
        let messages = self.client.simple_query(sql).await?;

        let mut columns: Vec<Col> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut truncated = false;
        let mut affected: Option<u64> = None;

        for m in messages {
            match m {
                SimpleQueryMessage::RowDescription(desc) => {
                    columns = desc
                        .iter()
                        .enumerate()
                        .map(|(i, c)| Col {
                            name: c.name().to_string(),
                            type_name: prepared_types
                                .get(i)
                                .cloned()
                                .unwrap_or_default(),
                        })
                        .collect();
                }
                SimpleQueryMessage::Row(row) => {
                    if rows.len() as u32 >= max_rows {
                        truncated = true;
                        continue;
                    }
                    let mut cells = Vec::with_capacity(columns.len());
                    for i in 0..columns.len() {
                        cells.push(row.get(i).map(|s| s.to_string()));
                    }
                    rows.push(cells);
                }
                SimpleQueryMessage::CommandComplete(n) => affected = Some(n),
                _ => {}
            }
        }
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            affected,
        })
    }

    async fn tables(
        &self,
        schema: Option<&str>,
        keyword: Option<&str>,
    ) -> anyhow::Result<Vec<TableMeta>> {
        let mut owned: Vec<String> = Vec::new();
        if let Some(s) = schema {
            owned.push(s.to_string());
        }
        let like = keyword.map(|k| format!("%{k}%"));
        if like.is_some() {
            let k = like.clone().unwrap_or_default();
            owned.push(k.clone());
            owned.push(k);
        }

        let mut sql = String::from(
            "SELECT n.nspname AS schema, c.relname AS name, \
                    COALESCE(obj_description(c.oid), '') AS comment \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r','p','v','m') \
               AND n.nspname NOT IN ('pg_catalog','information_schema')",
        );
        let mut idx = 0;
        if schema.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND n.nspname = ${idx}"));
        }
        if like.is_some() {
            sql.push_str(&format!(
                " AND (c.relname ILIKE ${0} OR COALESCE(obj_description(c.oid),'') ILIKE ${1})",
                idx + 1,
                idx + 2
            ));
        }
        sql.push_str(" ORDER BY n.nspname, c.relname");

        let params: Vec<&(dyn ToSql + Sync)> = owned
            .iter()
            .map(|s| s as &(dyn ToSql + Sync))
            .collect();
        let rows = self.client.query(&sql, &params).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(TableMeta {
                schema: r.try_get("schema")?,
                name: r.try_get("name")?,
                comment: r.try_get("comment")?,
            });
        }
        Ok(out)
    }

    async fn columns(&self, qualified: &str) -> anyhow::Result<Vec<ColumnMeta>> {
        let (schema, table) = split_qualified(qualified);
        let rows = self
            .client
            .query(
                "SELECT a.attname AS name, \
                        format_type(a.atttypid, a.atttypmod) AS type_name, \
                        a.attnotnull AS not_null, \
                        COALESCE(col_description(a.attrelid, a.attnum), '') AS comment \
                 FROM pg_catalog.pg_attribute a \
                 JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2 \
                   AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY a.attnum",
                &[&schema, &table],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(ColumnMeta {
                name: r.try_get("name")?,
                type_name: r.try_get("type_name")?,
                not_null: r.try_get("not_null")?,
                comment: r.try_get("comment")?,
            });
        }
        Ok(out)
    }
}

/// `schema.table` → (schema, table); a bare `table` defaults to `public`.
fn split_qualified(qualified: &str) -> (String, String) {
    if let Some((s, t)) = qualified.rsplit_once('.') {
        (
            s.trim_matches('"').to_string(),
            t.trim_matches('"').to_string(),
        )
    } else {
        ("public".to_string(), qualified.trim_matches('"').to_string())
    }
}
