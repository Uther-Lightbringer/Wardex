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

// ---- AskUserQuestion permission requests (kimi acp adapter wire format) ----
//
// kimi's ACP adapter bridges the AskUserQuestion tool through
// session/request_permission (ACP has no dedicated question method), tagging
// option ids with a `q{n}_*` namespace so the round-trip is unambiguous:
//   q{questionIndex}_opt_{optionIndex}  (kind allow_once, name = option label)
//   q{questionIndex}_skip               (kind reject_once, name "Skip")
// The question text rides in toolCall.content blocks; a future adapter may
// also carry the full question array in toolCall.rawInput.questions (with
// multi_select flags). This parser groups everything the wire carries so the
// dialog can render EVERY question — nothing is dropped client-side.
//
// Verified against kimi 0.29.1: that version's adapter itself degrades a
// multi-question call to the first question before anything reaches the
// wire, so today only q0 groups ever arrive; the parser is the forward-
// compatible half of the fix.
//
// Answer narrowing (kept verbatim from the adapter's contract): the ACP
// response carries exactly ONE optionId, so each request is answered with a
// single selection — even for multi_select questions (the adapter narrows
// them the same way). claude-code-acp disallows AskUserQuestion entirely
// and codex-acp never emits this namespace, so non-matching options leave
// the regular permission flow untouched.

use serde::Serialize;

/// One selectable answer of an AskUserQuestion question.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct QuestionOption {
    pub option_id: String,
    pub label: String,
}

/// One question of an AskUserQuestion permission request, grouped from the
/// `q{n}_*` option namespace.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct QuestionGroup {
    /// Question index from the option-id namespace (0-based).
    pub index: usize,
    /// Question text (content block / rawInput; may be empty).
    pub text: String,
    /// rawInput.questions[i].multi_select|multiSelect; default single-select.
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
    /// optionId of the group's Skip entry ("" when the agent offers none).
    pub skip_id: String,
}

/// Parse one `q{n}_opt_{i}` / `q{n}_skip` option id → (question, option?).
fn parse_question_option_id(id: &str) -> Option<(usize, Option<usize>)> {
    let rest = id.strip_prefix('q')?;
    let (q, rest) = rest.split_once('_')?;
    let question: usize = q.parse().ok()?;
    if rest == "skip" {
        return Some((question, None));
    }
    let opt = rest.strip_prefix("opt_")?.parse().ok()?;
    Some((question, Some(opt)))
}

/// Group a request_permission params' options by question. Empty when no
/// option id matches the namespace (regular approvals: allow_once etc.).
pub fn parse_question_request(params: &Value) -> Vec<QuestionGroup> {
    let Some(options) = params.get("options").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut groups: Vec<QuestionGroup> = Vec::new();
    for opt in options {
        let id = opt.get("optionId").and_then(Value::as_str).unwrap_or_default();
        let Some((qi, oi)) = parse_question_option_id(id) else {
            continue;
        };
        if groups.iter().all(|g| g.index != qi) {
            groups.push(QuestionGroup {
                index: qi,
                ..Default::default()
            });
        }
        let group = groups.iter_mut().find(|g| g.index == qi).expect("just pushed");
        match oi {
            Some(_) => group.options.push(QuestionOption {
                option_id: id.to_string(),
                label: opt
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            None => group.skip_id = id.to_string(),
        }
    }
    if groups.is_empty() {
        return groups;
    }
    groups.sort_by_key(|g| g.index);

    // Question texts + multi_select: rawInput.questions wins (it is the
    // tool's original payload); content blocks are the kimi fallback (one
    // text block per question, or a single block for a single question).
    let tool = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let raw_questions = tool
        .get("rawInput")
        .and_then(|r| r.get("questions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let content_texts: Vec<String> = tool
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .map(|b| {
                    b.get("content")
                        .and_then(|c| c.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let group_count = groups.len();
    for (pos, group) in groups.iter_mut().enumerate() {
        if let Some(rq) = raw_questions.get(group.index).or_else(|| raw_questions.get(pos)) {
            if group.text.is_empty() {
                group.text = rq
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            group.multi_select = rq.get("multi_select").and_then(Value::as_bool).unwrap_or(false)
                || rq.get("multiSelect").and_then(Value::as_bool).unwrap_or(false);
        }
        if group.text.is_empty() {
            group.text = if group_count == 1 {
                content_texts.join("\n")
            } else {
                content_texts.get(pos).cloned().unwrap_or_default()
            };
        }
    }
    groups
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

    /// Captured verbatim from kimi 0.29.1 (`kimi acp`, AskUserQuestion with
    /// two questions — the adapter itself degrades to the first; the client
    /// only ever sees q0 today).
    #[test]
    fn parse_question_request_kimi_single_question_wire() {
        let params = json!({
            "sessionId": "s",
            "options": [
                { "optionId": "q0_opt_0", "name": "Red", "kind": "allow_once" },
                { "optionId": "q0_opt_1", "name": "Blue", "kind": "allow_once" },
                { "optionId": "q0_skip", "name": "Skip", "kind": "reject_once" }
            ],
            "toolCall": {
                "toolCallId": "0:tool_1",
                "title": "AskUserQuestion",
                "content": [{ "type": "content", "content": { "type": "text", "text": "Pick a color" } }]
            }
        });
        let groups = parse_question_request(&params);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.index, 0);
        assert_eq!(g.text, "Pick a color");
        assert!(!g.multi_select);
        assert_eq!(g.skip_id, "q0_skip");
        assert_eq!(
            g.options,
            vec![
                QuestionOption { option_id: "q0_opt_0".into(), label: "Red".into() },
                QuestionOption { option_id: "q0_opt_1".into(), label: "Blue".into() },
            ]
        );
    }

    /// Forward-compatible shape: several question groups in ONE request
    /// (the kimi adapter's documented no-wire-change multi-question path).
    /// Every group must survive — the rendering layer drops nothing.
    #[test]
    fn parse_question_request_groups_multiple_questions() {
        let params = json!({
            "options": [
                { "optionId": "q0_opt_0", "name": "Red" },
                { "optionId": "q0_skip", "name": "Skip" },
                { "optionId": "q1_opt_0", "name": "Big" },
                { "optionId": "q1_opt_1", "name": "Small" },
                { "optionId": "q1_skip", "name": "Skip" }
            ],
            "toolCall": {
                "title": "AskUserQuestion",
                "content": [
                    { "content": { "text": "Pick a color" } },
                    { "content": { "text": "Pick a size" } }
                ],
                "rawInput": {
                    "questions": [
                        { "question": "Pick a color", "multi_select": false },
                        { "question": "Pick a size", "multi_select": true }
                    ]
                }
            }
        });
        let groups = parse_question_request(&params);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].text, "Pick a color");
        assert!(!groups[0].multi_select);
        assert_eq!(groups[0].skip_id, "q0_skip");
        assert_eq!(groups[1].text, "Pick a size");
        assert!(groups[1].multi_select, "multi_select from rawInput");
        assert_eq!(groups[1].options.len(), 2);
        assert_eq!(groups[1].skip_id, "q1_skip");
    }

    #[test]
    fn parse_question_request_ignores_regular_approvals() {
        // Standard approval options never match the q{n}_* namespace.
        let params = json!({
            "options": [
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
            ],
            "toolCall": { "title": "Bash", "kind": "execute" }
        });
        assert!(parse_question_request(&params).is_empty());
        assert!(parse_question_request(&json!({})).is_empty());
        // Malformed q-ids are skipped, valid siblings still group.
        let params = json!({
            "options": [
                { "optionId": "q_opt_0", "name": "bad" },
                { "optionId": "q0_opt_x", "name": "bad" },
                { "optionId": "q0_opt_2", "name": "ok" }
            ]
        });
        let groups = parse_question_request(&params);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].options.len(), 1);
        assert_eq!(groups[0].options[0].label, "ok");
        assert!(groups[0].skip_id.is_empty());
    }

    #[test]
    fn parse_question_option_id_shapes() {
        assert_eq!(parse_question_option_id("q0_opt_0"), Some((0, Some(0))));
        assert_eq!(parse_question_option_id("q12_opt_3"), Some((12, Some(3))));
        assert_eq!(parse_question_option_id("q2_skip"), Some((2, None)));
        assert_eq!(parse_question_option_id("allow"), None);
        assert_eq!(parse_question_option_id("q0"), None);
        assert_eq!(parse_question_option_id("q0_opt_"), None);
        assert_eq!(parse_question_option_id("q0_skip_1"), None);
    }
}
