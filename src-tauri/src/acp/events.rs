// Events emitted by the ACP client towards the chat layer — the Rust
// counterpart of the old Qt signals (AcpClient.h:54-65). Serialized with an
// internally-tagged `event` key in camelCase so they can be forwarded to the
// frontend via Tauri emit as-is; the chat layer (phase 1d) is just a
// consumer of an mpsc::Receiver<AcpEvent>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Token usage reported by the agent in a session/prompt result's `usage`
/// object (kimi: {inputTokens, outputTokens, totalTokens, cachedReadTokens?,
/// cachedWriteTokens?, thoughtTokens?}). Missing fields default to 0/None so
/// a partial payload still parses; callers treat a wholly absent/broken usage
/// as None (see TurnUsage::from_acp).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u64>,
}

impl TurnUsage {
    /// Tolerant parse of a prompt result's `usage` value: anything that isn't
    /// an object with numeric counts yields None — a broken usage must never
    /// fail the turn itself. A missing totalTokens falls back to
    /// input + output.
    pub fn from_acp(value: &Value) -> Option<Self> {
        let mut usage: Self = serde_json::from_value(value.clone()).ok()?;
        if usage.total_tokens == 0 {
            usage.total_tokens = usage.input_tokens + usage.output_tokens;
        }
        Some(usage)
    }

    /// Tolerant parse of pi's `get_session_stats` cumulative `tokens` object
    /// ({input, output, total, cacheRead?, cacheWrite?}). Numeric fields
    /// default to 0, total falls back to input+output. A wholly empty object
    /// (or a non-object) yields None so a missing/broken payload never invents
    /// usage.
    pub fn from_pi(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let num = |k: &str| obj.get(k).and_then(Value::as_u64).unwrap_or(0);
        let input = num("input");
        let output = num("output");
        let mut total = num("total");
        if total == 0 {
            total = input + output;
        }
        let usage = Self {
            input_tokens: input,
            output_tokens: output,
            total_tokens: total,
            cached_read_tokens: obj.get("cacheRead").and_then(Value::as_u64),
            cached_write_tokens: obj.get("cacheWrite").and_then(Value::as_u64),
            thought_tokens: None,
        };
        if usage.total_tokens == 0
            && usage.cached_read_tokens.is_none()
            && usage.cached_write_tokens.is_none()
        {
            return None;
        }
        Some(usage)
    }
}

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
    TurnFinished {
        stop_reason: String,
        /// Token usage from the prompt result; absent on error paths or for
        /// agents that don't report it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
    },
    ProtocolError { error: String },
    /// available_commands_update: the session's slash-command list (raw ACP
    /// {name, description, input?} objects passed through). State-type: kept
    /// during session/load replay, latest value wins.
    AvailableCommands { commands: Vec<Value> },
    /// plan update: agent's task plan entries ([{content, priority?, status}]).
    /// State-type like AvailableCommands — replay-safe, latest value wins.
    Plan { entries: Vec<Value> },
    /// usage_update notification: mid/post-turn token usage reported outside
    /// the prompt result. Parsed with the same tolerant TurnUsage path.
    UsageUpdate { usage: TurnUsage },
    /// session_info_update: session metadata (title). Absent/empty title
    /// never overwrites the stored one (handled by the chat layer).
    SessionInfo { title: Option<String> },
    /// session/load failed and the client silently fell back to session/new:
    /// the chat layer must surface this (with the load error verbatim) so a
    /// lost-context resume is never invisible to the user.
    SessionLoadFallback { error: String },
    /// Process died at any point; chat does resume/interrupt bookkeeping.
    /// -1 = exit code unavailable (e.g. killed, or a mock transport).
    ProcessExited { code: i32 },
    /// Pi extension `notify` (fire-and-forget). Chat layer shows a desktop
    /// notification; no answer is expected.
    Notify { title: String, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_with_camel_case_tag_and_fields() {
        let v = serde_json::to_value(AcpEvent::TurnFinished {
            stop_reason: "end_turn".into(),
            usage: None,
        })
        .expect("serialize");
        assert_eq!(v, json!({ "event": "turnFinished", "stopReason": "end_turn" }));

        let v = serde_json::to_value(AcpEvent::TurnFinished {
            stop_reason: "end_turn".into(),
            usage: Some(TurnUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..Default::default()
            }),
        })
        .expect("serialize");
        assert_eq!(
            v,
            json!({
                "event": "turnFinished",
                "stopReason": "end_turn",
                "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
            })
        );

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

    #[test]
    fn turn_usage_from_acp_tolerant() {
        // Full kimi payload.
        let u = TurnUsage::from_acp(&json!({
            "inputTokens": 1200, "outputTokens": 300, "totalTokens": 1500,
            "cachedReadTokens": 800, "thoughtTokens": 42,
        }))
        .expect("usage");
        assert_eq!(u.input_tokens, 1200);
        assert_eq!(u.total_tokens, 1500);
        assert_eq!(u.cached_read_tokens, Some(800));
        assert_eq!(u.cached_write_tokens, None);
        assert_eq!(u.thought_tokens, Some(42));

        // Missing totalTokens falls back to input + output.
        let u = TurnUsage::from_acp(&json!({ "inputTokens": 7, "outputTokens": 3 }))
            .expect("usage");
        assert_eq!(u.total_tokens, 10);

        // No usage key / broken shapes → None, never an error.
        assert_eq!(TurnUsage::from_acp(&json!("oops")), None);
        assert_eq!(TurnUsage::from_acp(&Value::Null), None);
    }

    #[test]
    fn turn_usage_from_pi_tolerant() {
        // Full pi get_session_stats tokens payload.
        let u = TurnUsage::from_pi(&json!({
            "input": 50000, "output": 10000, "total": 105000,
            "cacheRead": 40000, "cacheWrite": 5000,
        }))
        .expect("usage");
        assert_eq!(u.input_tokens, 50000);
        assert_eq!(u.output_tokens, 10000);
        assert_eq!(u.total_tokens, 105000);
        assert_eq!(u.cached_read_tokens, Some(40000));
        assert_eq!(u.cached_write_tokens, Some(5000));
        assert_eq!(u.thought_tokens, None);

        // Missing total falls back to input + output.
        let u = TurnUsage::from_pi(&json!({ "input": 7, "output": 3 })).expect("usage");
        assert_eq!(u.total_tokens, 10);

        // Broken / empty shapes → None, never an error.
        assert_eq!(TurnUsage::from_pi(&json!("oops")), None);
        assert_eq!(TurnUsage::from_pi(&Value::Null), None);
        assert_eq!(TurnUsage::from_pi(&json!({ "other": 1 })), None);
    }
}
