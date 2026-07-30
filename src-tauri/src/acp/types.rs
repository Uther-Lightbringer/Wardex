// ACP wire types and pure helpers: JSON-RPC 2.0 message skeletons, the
// pinned `initialize` request, image mime mapping, tool_call normalization
// and fs line clipping. Everything here is pure (no IO, no state) so both
// the client state machine and the probe/testAgent handshake share it.
//
// Ported from AcpClient.cpp:119-135 (initialize), 152-164 (mime),
// 250-288 (message skeletons), 505-514 + ChatController.cpp:466-483
// (two-level tool normalization), 436-443 (line clipping).

use serde_json::{json, Value};

/// JSON-RPC error code used for fs/* reverse RPC failures
/// (AcpClient.cpp:225, 236).
pub const RPC_ERROR_FS: i64 = -32000;
/// JSON-RPC "method not found" for unknown agent->client requests with an id
/// (AcpClient.cpp:463-465).
pub const RPC_ERROR_METHOD_NOT_FOUND: i64 = -32601;

/// clientInfo is pinned (AcpClient.cpp:127-129). probe.rs's testAgent
/// handshake shares this same value via initialize_request() — bump the
/// version in THIS ONE place and both stay in sync (docs/providers-and-cli.md
/// §4.3).
pub const CLIENT_NAME: &str = "WarDex";
pub const CLIENT_VERSION: &str = "0.2";

/// initialize params (AcpClient.cpp:119-135): protocolVersion 1, pinned
/// clientInfo, fs read/write capabilities, no terminal.
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION },
        "clientCapabilities": {
            "fs": { "readTextFile": true, "writeTextFile": true },
            "terminal": false
        }
    })
}

/// Full initialize request frame; used by probe.rs's testAgent and as the
/// reference for the client's first request.
pub fn initialize_request(id: i64) -> Value {
    request(id, "initialize", initialize_params())
}

/// Request skeleton (AcpClient.cpp:250-258).
pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Notification skeleton (AcpClient.cpp:260-267).
pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// Success response skeleton (AcpClient.cpp:269-276).
pub fn response(id: i64, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Error response skeleton (AcpClient.cpp:278-288).
pub fn error_response(id: i64, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Image mime by file extension (AcpClient.cpp:152-164): jpg/jpeg, webp,
/// gif, bmp get their own mime; EVERYTHING else defaults to image/png.
pub fn mime_for_image(path: &str) -> &'static str {
    // QString::section('.', -1) on a dotless path returns the whole string,
    // which simply matches nothing and falls through to png — rsplit does
    // the same.
    let ext = path.rsplit('.').next().unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// Two-level tool_call normalization, merged into one place as
/// acp-protocol.md §6 suggests:
///
/// Level 1 (AcpClient.cpp:505-514): if the update has no top-level
/// `toolCallId` but carries a nested `toolCall` object (some adapters nest
/// one level), unwrap to that sub-object.
///
/// Level 2 (ChatController.cpp:466-483 `toolFromUpdate`): fill a missing
/// `name` from `title` (non-empty), else from `kind`. A present-but-null
/// `name` key counts as present, matching QVariantMap::contains semantics.
pub fn normalize_tool_call(update: &Value) -> Value {
    let mut out = update.clone();
    if out.get("toolCallId").is_none() {
        if let Some(inner) = out.get("toolCall").filter(|v| v.is_object()) {
            out = inner.clone();
        }
    }
    if out.get("name").is_none() {
        let title = out.get("title").and_then(Value::as_str).unwrap_or_default();
        if !title.is_empty() {
            out["name"] = Value::String(title.to_string());
        } else if let Some(kind) = out.get("kind").cloned() {
            out["name"] = kind;
        }
    }
    out
}

/// 1-based line/limit clipping for fs/read_text_file (AcpClient.cpp:436-443):
/// split on '\n' (empty parts kept, like QString::split), start at line-1
/// when line > 0, take at most `limit` lines when limit > 0, rejoin with
/// '\n'. A start past the end yields an empty string (Qt mid() clamps).
pub fn clip_lines(content: &str, line: i64, limit: i64) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let start = if line > 0 { (line - 1) as usize } else { 0 };
    let start = start.min(lines.len());
    let end = if limit > 0 {
        start.saturating_add(limit as usize).min(lines.len())
    } else {
        lines.len()
    };
    lines[start..end].join("\n")
}

/// JSON integer tolerant of float-shaped values (Qt toInt() accepts doubles).
pub fn json_int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_request_matches_pinned_shape() {
        let req = initialize_request(1);
        assert_eq!(
            req,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientInfo": { "name": "WarDex", "version": "0.2" },
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true },
                        "terminal": false
                    }
                }
            })
        );
    }

    #[test]
    fn mime_mapping_defaults_to_png() {
        assert_eq!(mime_for_image("a.jpg"), "image/jpeg");
        assert_eq!(mime_for_image("a.JPEG"), "image/jpeg");
        assert_eq!(mime_for_image("a.webp"), "image/webp");
        assert_eq!(mime_for_image("a.gif"), "image/gif");
        assert_eq!(mime_for_image("a.bmp"), "image/bmp");
        assert_eq!(mime_for_image("a.png"), "image/png");
        // The "everything else" default: unknown ext, no ext, dotfile.
        assert_eq!(mime_for_image("a.tiff"), "image/png");
        assert_eq!(mime_for_image("noext"), "image/png");
        assert_eq!(mime_for_image(r"C:\dir.with.dot\file"), "image/png");
    }

    #[test]
    fn normalize_tool_call_unwraps_nested_tool_call_object() {
        let nested = json!({
            "sessionUpdate": "tool_call",
            "toolCall": { "toolCallId": "t1", "title": "Read" }
        });
        let out = normalize_tool_call(&nested);
        assert_eq!(out["toolCallId"], "t1");
        assert_eq!(out["name"], "Read", "level 2: name filled from title");

        // Top-level toolCallId wins — no unwrap.
        let flat = json!({
            "toolCallId": "t2",
            "toolCall": { "toolCallId": "WRONG" },
            "kind": "edit"
        });
        let out = normalize_tool_call(&flat);
        assert_eq!(out["toolCallId"], "t2");
        assert_eq!(out["name"], "edit", "level 2: name filled from kind");

        // Existing name is never overwritten.
        let named = json!({ "toolCallId": "t3", "name": "Bash", "title": "ignored" });
        assert_eq!(normalize_tool_call(&named)["name"], "Bash");
    }

    #[test]
    fn clip_lines_is_1_based_and_matches_qt_mid() {
        let content = "l1\nl2\nl3\nl4";
        assert_eq!(clip_lines(content, 0, 0), content, "no clipping without line/limit semantics (caller guards)");
        assert_eq!(clip_lines(content, 2, 0), "l2\nl3\nl4");
        assert_eq!(clip_lines(content, 1, 2), "l1\nl2");
        assert_eq!(clip_lines(content, 2, 2), "l2\nl3");
        assert_eq!(clip_lines(content, 3, 99), "l3\nl4");
        assert_eq!(clip_lines(content, 99, 0), "", "start past end -> empty");
        // Trailing newline produces a trailing empty part, like Qt split.
        assert_eq!(clip_lines("a\n", 0, 0), "a\n");
    }

    #[test]
    fn json_int_accepts_int_and_float() {
        assert_eq!(json_int(&json!(3)), Some(3));
        assert_eq!(json_int(&json!(3.0)), Some(3));
        assert_eq!(json_int(&json!("3")), None);
        assert_eq!(json_int(&Value::Null), None);
    }
}
