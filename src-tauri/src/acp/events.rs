// Events emitted by the ACP client towards the chat layer — the Rust
// counterpart of the old Qt signals (AcpClient.h:54-65). Serialized with an
// internally-tagged `event` key in camelCase so they can be forwarded to the
// frontend via Tauri emit as-is; the chat layer (phase 1d) is just a
// consumer of an mpsc::Receiver<AcpEvent>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum AcpEvent {
    /// Handshake finished; the agent-side session id is ready. Chat sends
    /// queued prompts only after this (acp-protocol.md §3.5).
    #[serde(rename_all = "camelCase")]
    Started { session_id: String },
    /// Any failure before the session is ready (spawn, initialize,
    /// session/new).
    StartFailed { error: String },
    ModeChanged { mode: String },
    /// Available config options (model / thinking / mode pickers) from
    /// session/new|load's result, refreshed on set_config_option responses
    /// and config_option_update notifications. Raw ACP option objects
    /// ({type, id, name, category, currentValue, options[]}) passed through.
    ConfigOptions { options: Vec<Value> },
    ThoughtChunk { text: String },
    MessageChunk { text: String },
    /// Normalized tool_call payload (see types::normalize_tool_call).
    ToolCall { call: Value },
    ToolCallUpdate { update: Value },
    /// Agent asked for tool approval; answer via
    /// AcpClient::answer_permission with this request id.
    #[serde(rename_all = "camelCase")]
    PermissionRequested { request_id: i64, params: Value },
    #[serde(rename_all = "camelCase")]
    TurnFinished { stop_reason: String },
    ProtocolError { error: String },
    /// Process died at any point; chat does resume/interrupt bookkeeping.
    /// -1 = exit code unavailable (e.g. killed, or a mock transport).
    ProcessExited { code: i32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_with_camel_case_tag_and_fields() {
        let v = serde_json::to_value(AcpEvent::TurnFinished {
            stop_reason: "end_turn".into(),
        })
        .expect("serialize");
        assert_eq!(v, json!({ "event": "turnFinished", "stopReason": "end_turn" }));

        let v = serde_json::to_value(AcpEvent::PermissionRequested {
            request_id: 7,
            params: json!({ "toolCall": {} }),
        })
        .expect("serialize");
        assert_eq!(
            v,
            json!({ "event": "permissionRequested", "requestId": 7, "params": { "toolCall": {} } })
        );

        // Round trip keeps Deserialize honest for future use.
        let ev: AcpEvent = serde_json::from_value(v).expect("deserialize");
        assert_eq!(
            ev,
            AcpEvent::PermissionRequested {
                request_id: 7,
                params: json!({ "toolCall": {} })
            }
        );
    }
}
