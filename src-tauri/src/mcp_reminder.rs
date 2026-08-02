// Built-in MCP stdio server (`wardex --mcp-reminder`): gives the agent a
// set_reminder / cancel_reminder / list_reminders tool surface without any
// external MCP dependency. Injected into every ACP session via build_launch
// (chat/runtime.rs) as an mcpServers entry with WARDEX_SESSION_ID /
// WARDEX_TODOS_PATH env, so one subprocess serves exactly one session.
//
// Protocol: minimal hand-rolled MCP over NDJSON stdio (same line framing as
// the ACP transport — one compact JSON-RPC object per line). Implements
// initialize / notifications/initialized / tools/list / tools/call; every
// other request gets -32601. No rmcp crate (plan: keep deps flat).
//
// Persistence: reads/writes todos.json directly through store::todos' pure
// load/save functions (tmp + rename atomic writes). Agent reminders are
// session-scoped rows with notifyMode == "push": when due, the session
// runtime sends the content back into the chat as a prompt (self-wakeup).
// The main app re-reads the file on every scheduling point, so no IPC is
// needed between this subprocess and the app.

use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::store::todos::{self, NOTIFY_PUSH, SCOPE_SESSION};

/// Session context, passed via env by build_launch.
pub struct ReminderCtx {
    pub session_id: String,
    pub path: PathBuf,
}

impl ReminderCtx {
    pub fn from_env() -> Result<Self, String> {
        let session_id = std::env::var("WARDEX_SESSION_ID")
            .map_err(|_| "WARDEX_SESSION_ID not set".to_string())?;
        let path = std::env::var("WARDEX_TODOS_PATH")
            .map_err(|_| "WARDEX_TODOS_PATH not set".to_string())?;
        if session_id.trim().is_empty() || path.trim().is_empty() {
            return Err("WARDEX_SESSION_ID / WARDEX_TODOS_PATH must be non-empty".to_string());
        }
        Ok(Self {
            session_id,
            path: PathBuf::from(path),
        })
    }
}

/// Entry point from main.rs (`--mcp-reminder`): serve stdio until EOF.
pub async fn run() -> Result<(), String> {
    let ctx = ReminderCtx::from_env()?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(resp) = handle_line(&ctx, line) {
            let mut text = resp.to_string();
            text.push('\n');
            if out.write_all(text.as_bytes()).await.is_err() || out.flush().await.is_err() {
                break; // parent gone
            }
        }
    }
    Ok(())
}

/// Pure dispatcher (unit-testable without stdio): one request line in,
/// Some(response) for requests, None for notifications / unparseable input.
pub fn handle_line(ctx: &ReminderCtx, line: &str) -> Option<Value> {
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
            "serverInfo": { "name": "wardex-reminder", "version": "0.2.0" }
        })),
        "notifications/initialized" => None,
        "tools/list" => reply(json!({ "tools": tool_defs() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(ctx, name, &args) {
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
            "name": "set_reminder",
            "description": "Set a one-shot reminder for this chat session. When the time comes, Wardex posts the reminder content back into this chat as a new prompt so you can report to the user. Use this whenever the user asks to be reminded about something later.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "minutes": { "type": "number", "description": "Minutes from now until the reminder fires (must be > 0)" },
                    "content": { "type": "string", "description": "What to remind about" }
                },
                "required": ["minutes", "content"]
            }
        },
        {
            "name": "cancel_reminder",
            "description": "Cancel a pending reminder by its id (from list_reminders).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Reminder id to cancel" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "list_reminders",
            "description": "List all pending reminders of this chat session.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(ctx: &ReminderCtx, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "set_reminder" => {
            let minutes = args.get("minutes").and_then(Value::as_f64).unwrap_or(0.0);
            let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
            if minutes <= 0.0 || content.trim().is_empty() {
                return Err("invalid arguments: minutes must be > 0 and content non-empty".to_string());
            }
            let due_at = todos::now_ms() + (minutes * 60_000.0) as i64;
            let row = todos::new_todo(
                content,
                SCOPE_SESSION,
                &ctx.session_id,
                "",
                due_at,
                NOTIFY_PUSH,
            )
            .ok_or_else(|| {
                "invalid arguments: minutes must be > 0 and content non-empty".to_string()
            })?;
            let mut rows = todos::load_file(&ctx.path);
            rows.push(row.clone());
            todos::save_file(&ctx.path, &rows).map_err(|e| e.to_string())?;
            serde_json::to_string(&row).map_err(|e| e.to_string())
        }
        "cancel_reminder" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or_default();
            let mut rows = todos::load_file(&ctx.path);
            let before = rows.len();
            rows.retain(|r| r.id != id);
            if rows.len() == before {
                return Err(format!("reminder not found: {id}"));
            }
            todos::save_file(&ctx.path, &rows).map_err(|e| e.to_string())?;
            Ok(format!("cancelled {id}"))
        }
        "list_reminders" => {
            let all = todos::load_file(&ctx.path);
            let rows: Vec<&todos::TodoRow> = all
                .iter()
                .filter(|r| {
                    r.scope == SCOPE_SESSION
                        && r.session_id == ctx.session_id
                        && r.notify_mode == NOTIFY_PUSH
                        && !r.done
                })
                .collect();
            serde_json::to_string(&rows).map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::todos as store;

    fn ctx(tmp: &tempfile::TempDir) -> ReminderCtx {
        ReminderCtx {
            session_id: "sess-1".to_string(),
            path: tmp.path().join("todos.json"),
        }
    }

    #[test]
    fn initialize_and_tools_list() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ctx = ctx(&tmp);
        let init = handle_line(&ctx, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("response");
        assert_eq!(init["result"]["serverInfo"]["name"], "wardex-reminder");

        // Notification: no response.
        assert!(handle_line(&ctx, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());

        let list = handle_line(&ctx, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .expect("response");
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert_eq!(names, ["set_reminder", "cancel_reminder", "list_reminders"]);

        // Unknown request → -32601.
        let err = handle_line(&ctx, r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#)
            .expect("response");
        assert_eq!(err["error"]["code"], -32601);
        // Unknown notification → ignored.
        assert!(handle_line(&ctx, r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#).is_none());
    }

    #[test]
    fn set_cancel_list_reminder_roundtrip_writes_disk() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ctx = ctx(&tmp);

        let set = handle_line(
            &ctx,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"set_reminder","arguments":{"minutes":5,"content":"喝水"}}}"#,
        )
        .expect("response");
        assert!(set.get("error").is_none(), "{set}");
        let text = set["result"]["content"][0]["text"].as_str().expect("text");
        let row: store::TodoRow = serde_json::from_str(text).expect("todo json");
        assert_eq!(row.session_id, "sess-1");
        assert_eq!(row.scope, SCOPE_SESSION);
        assert_eq!(row.notify_mode, NOTIFY_PUSH);
        assert!(!row.done);
        assert!(row.due_at_ms > row.created_at);

        // Persisted to disk.
        let on_disk = store::load_file(&ctx.path);
        assert_eq!(on_disk.len(), 1);
        assert_eq!(on_disk[0].title, "喝水");

        // list_reminders sees it.
        let list = handle_line(
            &ctx,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_reminders","arguments":{}}}"#,
        )
        .expect("response");
        assert!(list["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("喝水"));

        // cancel removes it; a second cancel is an error result.
        let cancel_line = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"cancel_reminder","arguments":{{"id":"{}"}}}}}}"#,
            row.id
        );
        let cancel = handle_line(&ctx, &cancel_line).expect("response");
        assert!(cancel.get("error").is_none());
        assert!(store::load_file(&ctx.path).is_empty());
        let again = handle_line(&ctx, &cancel_line).expect("response");
        assert_eq!(again["result"]["isError"], true);

        // Bad arguments → isError result, nothing written.
        let bad = handle_line(
            &ctx,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"set_reminder","arguments":{"minutes":0,"content":"x"}}}"#,
        )
        .expect("response");
        assert_eq!(bad["result"]["isError"], true);
    }
}
