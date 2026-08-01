// Read a kimi-code sub-agent's on-disk transcript for the SubagentDialog
// "执行过程" section (features/chat.md §4.2).
//
// kimi CLI (interactive AND ACP-spawned) persists every sub-agent's full
// event stream at:
//   ~/.kimi-code/sessions/<project-dir>/<acpSessionId>/agents/<agentId>/wire.jsonl
// where <acpSessionId> is the id returned by ACP session/new (stored in the
// WarDex session meta) and <agentId> is the `agent_id:` line from the Agent
// tool's rawOutput (e.g. "agent-0"). This is a PRIVATE kimi-code format —
// it may change across CLI versions; on any mismatch we degrade to an
// error string the dialog shows, never a crash.
//
// Only context.append_loop_event lines are surfaced, mapped to steps:
//   tool.call   → {kind:"tool",  name, detail: description + pretty args}
//   tool.result → {kind:"result", detail: output}
//   content.part(think) → {kind:"think"}, content.part(text) → {kind:"text"}
// Caps: keep the LAST 400 steps, each detail ≤ 4000 chars.
use serde_json::{json, Map, Value};

const MAX_STEPS: usize = 400;
const MAX_DETAIL: usize = 4000;

fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_DETAIL {
        return s.to_string();
    }
    let cut: String = s.chars().take(MAX_DETAIL).collect();
    format!("{cut}\n…（已截断）")
}

/// Locate the wire.jsonl for `<acpSessionId>/<agentId>` under any project dir.
fn find_wire(acp_session_id: &str, agent_id: &str) -> Option<std::path::PathBuf> {
    // Guard against path traversal — both ids are CLI-generated.
    if !acp_session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || !agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let base = dirs::home_dir()?.join(".kimi-code").join("sessions");
    for proj in std::fs::read_dir(base).ok()?.flatten() {
        let wire = proj
            .path()
            .join(acp_session_id)
            .join("agents")
            .join(agent_id)
            .join("wire.jsonl");
        if wire.is_file() {
            return Some(wire);
        }
    }
    None
}

fn step_from(event: &Map<String, Value>) -> Option<Value> {
    match event.get("type").and_then(Value::as_str)? {
        "tool.call" => {
            let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
            let desc = event.get("description").and_then(Value::as_str).unwrap_or_default();
            let args = event
                .get("args")
                .map(|a| serde_json::to_string_pretty(a).unwrap_or_default())
                .unwrap_or_default();
            let detail = if desc.is_empty() { args } else { format!("{desc}\n{args}") };
            Some(json!({ "kind": "tool", "name": name, "detail": clip(detail.trim()) }))
        }
        "tool.result" => {
            let out = match event.get("result").and_then(|r| r.get("output")) {
                Some(Value::String(s)) => s.clone(),
                Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
                None => String::new(),
            };
            Some(json!({ "kind": "result", "name": "结果", "detail": clip(&out) }))
        }
        "content.part" => {
            let part = event.get("part")?;
            let ptype = part.get("type").and_then(Value::as_str)?;
            match ptype {
                // think parts carry the text under "think", text under "text"
                "think" => {
                    let t = part.get("think").and_then(Value::as_str).unwrap_or_default();
                    Some(json!({ "kind": "think", "name": "思考", "detail": clip(t) }))
                }
                "text" => {
                    let t = part.get("text").and_then(Value::as_str).unwrap_or_default();
                    Some(json!({ "kind": "text", "name": "回复", "detail": clip(t) }))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Parse the sub-agent wire file into a step list for the detail dialog.
pub fn read_subagent_process(acp_session_id: &str, agent_id: &str) -> Result<Value, String> {
    if acp_session_id.is_empty() {
        return Err("该会话还没有 ACP sessionId（尚未启动过回合）".to_string());
    }
    if agent_id.is_empty() {
        return Err("该子 Agent 尚未回报 agent_id（完成后才可用）".to_string());
    }
    let wire = find_wire(acp_session_id, agent_id)
        .ok_or_else(|| "找不到该子 Agent 的过程文件（可能已被清理，或非 kimi CLI）".to_string())?;
    let raw = std::fs::read_to_string(&wire).map_err(|e| format!("读取过程文件失败: {e}"))?;

    let mut steps: Vec<Value> = Vec::new();
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("context.append_loop_event") {
            continue;
        }
        let Some(event) = v.get("event").and_then(Value::as_object) else { continue };
        if let Some(s) = step_from(event) {
            steps.push(s);
        }
    }
    let total = steps.len();
    if total > MAX_STEPS {
        steps = steps.split_off(total - MAX_STEPS);
    }
    let updated = std::fs::metadata(&wire)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(json!({
        "steps": steps,
        "truncated": total > MAX_STEPS,
        "updatedAt": updated,
    }))
}
