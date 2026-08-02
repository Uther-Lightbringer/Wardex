// Built-in MCP stdio server (`wardex --mcp-db`): gives ACP sessions the
// database schema surface — list_connections / list_tables / describe_table —
// WITHOUT any SQL execution tool, so the agent can study the structure of the
// project's databases but never sees or mutates data rows (PRD security
// constraint: AI 只见结构不见数据). Injected into sessions whose project has
// DB connections configured (chat/runtime.rs), project context via env.
//
// Protocol: minimal hand-rolled MCP over NDJSON stdio (same line framing as
// mcp_reminder.rs / the ACP transport). Every tool call runs allow_write=false
// through the readonly session + statement_timeout (layers 3/4), and the
// metadata queries themselves are fixed SQL — there is no free-query tool.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::db::DbManager;
use crate::store::{self, NamedConn, Paths};

/// Session context passed via env by chat/runtime.rs build_launch.
pub struct DbMcpCtx {
    project_dir: String,
    conns: Vec<NamedConn>,
    aliases: BTreeMap<String, String>,
    manager: DbManager,
}

impl DbMcpCtx {
    pub fn from_env() -> Result<Self, String> {
        let project_dir =
            std::env::var("WARDEX_PROJECT_DIR").map_err(|_| "WARDEX_PROJECT_DIR not set".to_string())?;
        if project_dir.trim().is_empty() {
            return Err("WARDEX_PROJECT_DIR must be non-empty".to_string());
        }
        let paths = Paths::production();
        let store = store::DbConnStore::load(&paths);
        let conns = store.connections(&project_dir);
        let aliases = store.aliases(&project_dir);
        Ok(Self {
            project_dir,
            conns,
            aliases,
            manager: DbManager::new(),
        })
    }

    /// Resolve the connection name (default = first configured) to (name, dsn).
    fn resolve_conn(&self, name: Option<&str>) -> Result<(String, String), String> {
        if self.conns.is_empty() {
            return Err("该项目未配置数据库连接".to_string());
        }
        let name = name.unwrap_or(&self.conns[0].name);
        let c = self
            .conns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("未找到连接「{name}」"))?;
        Ok((c.name.clone(), c.dsn.clone()))
    }
}

/// Entry point from main.rs (`--mcp-db`): serve stdio until EOF.
pub async fn run() -> Result<(), String> {
    let ctx = DbMcpCtx::from_env()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(resp) = handle_line(&ctx, line).await {
            let mut text = resp.to_string();
            text.push('\n');
            if out.write_all(text.as_bytes()).await.is_err() || out.flush().await.is_err() {
                break; // parent gone
            }
        }
    }
    Ok(())
}

/// Pure dispatcher (unit-testable without stdio). Async because tool calls
/// connect to PostgreSQL.
pub async fn handle_line(ctx: &DbMcpCtx, line: &str) -> Option<Value> {
    let req: Value = serde_json::from_str(line).ok()?;
    let method = req.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let is_request = req.get("id").is_some();

    let reply = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    let error = |code: i64, message: &str| {
        Some(json!({ "jsonrpc": "2.0", "id": id,
                     "error": { "code": code, "message": message } }))
    };

    match method {
        "initialize" => reply(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "wardex-db", "version": "0.1.0" }
        })),
        "notifications/initialized" => None,
        "tools/list" => reply(json!({ "tools": tool_defs() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(ctx, name, &args).await {
                Ok(text) => reply(json!({
                    "content": [ { "type": "text", "text": text } ]
                })),
                Err(msg) => reply(json!({
                    "content": [ { "type": "text", "text": msg } ],
                    "isError": true
                })),
            }
        }
        _ => {
            if is_request {
                error(-32601, "method not found")
            } else {
                None
            }
        }
    }
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "list_connections",
            "description": "列出当前项目已配置的数据库连接（开发环境/测试环境/预发布环境 等）。先调用它了解可选环境，再对具体环境查看表结构。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_tables",
            "description": "列出某连接的数据库表（含注释与中文别名）。conn 缺省取第一个连接；keyword 按表名/注释/别名模糊匹配（不区分大小写）。只能看到表结构，看不到数据。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "conn": { "type": "string", "description": "连接名，如 开发环境" },
                    "schema": { "type": "string", "description": "schema，缺省全部" },
                    "keyword": { "type": "string", "description": "模糊匹配关键字" }
                }
            }
        },
        {
            "name": "describe_table",
            "description": "查看某张表的列结构：列名/类型/是否非空/注释/中文别名。conn 缺省取第一个连接。只能看到结构，看不到数据。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "conn": { "type": "string", "description": "连接名，如 开发环境" },
                    "table": { "type": "string", "description": "全限定名，如 public.orders" }
                },
                "required": ["table"]
            }
        }
    ])
}

async fn call_tool(ctx: &DbMcpCtx, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "list_connections" => {
            let open = ctx.manager.open_names(&ctx.project_dir);
            let items: Vec<Value> = ctx
                .conns
                .iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "connected": open.contains(&c.name),
                    })
                })
                .collect();
            serde_json::to_string(&items).map_err(|e| e.to_string())
        }
        "list_tables" => {
            let (name, dsn) = ctx.resolve_conn(args.get("conn").and_then(Value::as_str))?;
            ctx.manager.open(&ctx.project_dir, &name, &dsn).await?;
            let schema = args.get("schema").and_then(Value::as_str);
            let keyword = args.get("keyword").and_then(Value::as_str);
            let tables = ctx
                .manager
                .tables(&ctx.project_dir, &name, schema, keyword)
                .await?;
            let out: Vec<Value> = tables
                .into_iter()
                .map(|t| {
                    let alias = ctx
                        .aliases
                        .get(&format!("{}.{}", t.schema, t.name))
                        .cloned()
                        .unwrap_or_default();
                    json!({ "schema": t.schema, "table": t.name, "comment": t.comment, "alias": alias })
                })
                .collect();
            serde_json::to_string(&out).map_err(|e| e.to_string())
        }
        "describe_table" => {
            let (name, dsn) = ctx.resolve_conn(args.get("conn").and_then(Value::as_str))?;
            ctx.manager.open(&ctx.project_dir, &name, &dsn).await?;
            let table = args.get("table").and_then(Value::as_str).ok_or("参数 table 必填")?;
            let cols = ctx.manager.columns(&ctx.project_dir, &name, table).await?;
            let out: Vec<Value> = cols
                .into_iter()
                .map(|c| {
                    let alias = ctx
                        .aliases
                        .get(&format!("{table}.{}", c.name))
                        .cloned()
                        .unwrap_or_default();
                    json!({
                        "name": c.name,
                        "type": c.type_name,
                        "not_null": c.not_null,
                        "comment": c.comment,
                        "alias": alias,
                    })
                })
                .collect();
            serde_json::to_string(&out).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NamedConn;

    fn ctx(conns: Vec<NamedConn>) -> DbMcpCtx {
        DbMcpCtx {
            project_dir: "C:/p".to_string(),
            conns,
            aliases: BTreeMap::new(),
            manager: DbManager::new(),
        }
    }

    #[tokio::test]
    async fn handshake_and_tool_list() {
        let c = ctx(vec![]);
        let init = handle_line(&c, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .expect("response");
        assert_eq!(init["result"]["serverInfo"]["name"], "wardex-db");

        assert!(
            handle_line(&c, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .await
                .is_none()
        );

        let list = handle_line(&c, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .expect("response");
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["list_connections", "list_tables", "describe_table"]);

        let err = handle_line(&c, r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#)
            .await
            .expect("response");
        assert_eq!(err["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn tools_require_connections() {
        // No connections configured → a friendly isError, not a crash.
        let empty = ctx(vec![]);
        let resp = handle_line(
            &empty,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tables","arguments":{}}}"#,
        )
        .await
        .expect("response");
        assert_eq!(resp["result"]["isError"], true);

        let cfg = ctx(vec![NamedConn {
            name: "开发环境".to_string(),
            dsn: "postgresql://invalid".to_string(),
        }]);
        let resp = handle_line(
            &cfg,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_connections","arguments":{}}}"#,
        )
        .await
        .expect("response");
        assert!(resp.get("error").is_none());
        let text = resp["result"]["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("开发环境"));
    }
}
