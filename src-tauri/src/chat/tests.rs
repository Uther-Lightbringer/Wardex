// Actor-level integration tests: a ChatManager whose spawner injects
// AcpClient<MockTransport>, driven by scripted NDJSON frames. Covers the
// phase-1d checklist: coalescing flush cadence, queue cap, rate-limit
// detect/backoff/cancel, interrupted-turn resume prompt synthesis, process
// cap eviction, switchAgent same/cross provider, subagent tracking.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::acp::{AcpClient, MockTransport};
use crate::chat::driver::{ClientDriver, SessionLaunch, Spawner};
use crate::chat::manager::{ChatManager, SpawnerFactory};
use crate::chat::runtime::{lock_ok, EventSink};
use crate::store::{AgentPatch, Paths, StoreRegistry};

// ---- harness ----

#[derive(Default)]
struct RecordSink {
    events: Mutex<Vec<(String, Value)>>,
}

impl EventSink for RecordSink {
    fn emit(&self, event: &str, payload: Value) {
        lock_ok(&self.events).push((event.to_string(), payload));
    }
}

impl RecordSink {
    fn find(&self, pred: impl Fn(&(String, Value)) -> bool) -> Vec<(String, Value)> {
        lock_ok(&self.events)
            .iter()
            .filter(|e| pred(e))
            .cloned()
            .collect()
    }
}

type Mocks = Arc<Mutex<Vec<MockTransport>>>;

fn mock_factory(mocks: Mocks) -> SpawnerFactory {
    Arc::new(move |_session_id: &str| {
        let mocks = mocks.clone();
        let spawner: Spawner = Box::new(move |launch: SessionLaunch, tx| {
            let mocks = mocks.clone();
            Box::pin(async move {
                let mock = MockTransport::new();
                lock_ok(&mocks).push(mock.clone());
                let mut client = AcpClient::new(mock, tx);
                client.start(launch.start).await?;
                Ok(Box::new(client) as Box<dyn ClientDriver>)
            })
        });
        spawner
    })
}

struct Harness {
    manager: Arc<ChatManager>,
    stores: Arc<Mutex<StoreRegistry>>,
    sink: Arc<RecordSink>,
    mocks: Mocks,
    #[allow(dead_code)]
    tmp: tempfile::TempDir,
}

fn harness() -> Harness {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(tmp.path().to_path_buf());
    let mut stores = StoreRegistry::init(paths.clone());
    let default_agent = stores
        .agents
        .create_agent(&paths, "Kimi")
        .expect("create agent");
    let _ = default_agent;
    let stores = Arc::new(Mutex::new(stores));
    let sink = Arc::new(RecordSink::default());
    let mocks: Mocks = Arc::new(Mutex::new(Vec::new()));
    let manager = Arc::new(ChatManager::with_factory(
        stores.clone(),
        sink.clone(),
        mock_factory(mocks.clone()),
    ));
    Harness {
        manager,
        stores,
        sink,
        mocks,
        tmp,
    }
}

async fn wait_for(mut pred: impl FnMut() -> bool) -> bool {
    for _ in 0..150 {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn update_frame(kind: &str, extra: Value) -> Value {
    let mut update = json!({ "sessionUpdate": kind });
    update
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    json!({ "jsonrpc": "2.0", "method": "session/update",
            "params": { "sessionId": "s", "update": update } })
}

/// Feed initialize + session/new responses (no load support, image yes).
async fn drive_ready(mock: &MockTransport) {
    mock.feed_json(json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "protocolVersion": 1,
                    "agentCapabilities": { "loadSession": true,
                                           "promptCapabilities": { "image": true } } }
    }))
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": 2, "result": { "sessionId": "s-1" } }))
        .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
}

/// Create a session and drive its (warm-up) ACP process to ready.
async fn ready_session(h: &Harness, projectless: &str) -> String {
    let before = lock_ok(&h.mocks).len();
    let id = h
        .manager
        .create_session(projectless)
        .await
        .expect("create session");
    assert!(
        wait_for(|| lock_ok(&h.mocks).len() > before).await,
        "warm-up spawn"
    );
    let mock = mock_at(h, before);
    drive_ready(&mock).await;
    id
}

fn mock_at(h: &Harness, i: usize) -> MockTransport {
    lock_ok(&h.mocks).get(i).expect("mock index").clone()
}

fn chunk_events(h: &Harness, session_id: &str, kind: &str) -> Vec<String> {
    h.sink
        .find(|(ev, p)| {
            ev == "acp://chunk"
                && p.get("sessionId").and_then(Value::as_str) == Some(session_id)
                && p.get("kind").and_then(Value::as_str) == Some(kind)
        })
        .into_iter()
        .filter_map(|(_, p)| p.get("text").and_then(Value::as_str).map(str::to_string))
        .collect()
}

// ---- tests ----

#[tokio::test]
async fn flush_coalesces_chunks_into_single_event() {
    let h = harness();
    let id = ready_session(&h, "").await;
    h.manager.send_prompt(&id, "hello", &[]).await.expect("send");
    assert!(wait_for(|| !mock_at(&h, 0).sent_json().is_empty()).await);
    let mock = mock_at(&h, 0);
    // Prompt went out (id 3 after init/new).
    assert!(
        wait_for(|| {
            mock.sent_json()
                .iter()
                .any(|m| m.get("method").and_then(Value::as_str) == Some("session/prompt"))
        })
        .await
    );

    // Two chunks 10ms apart merge into ONE 50ms flush.
    mock.feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "AB" } })))
        .await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    mock.feed_json(update_frame("agent_message_chunk", json!({ "content": { "text": "CD" } })))
        .await;
    assert!(wait_for(|| !chunk_events(&h, &id, "text").is_empty()).await);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let chunks = chunk_events(&h, &id, "text");
    assert_eq!(chunks, vec!["ABCD".to_string()], "50ms merge window");
}

#[tokio::test]
async fn queue_caps_at_ten_with_chinese_error() {
    let h = harness();
    let id = ready_session(&h, "").await;
    // First prompt keeps the turn busy (no turnFinished scripted).
    h.manager.send_prompt(&id, "first", &[]).await.expect("send");
    assert!(wait_for(|| {
        mock_at(&h, 0)
            .sent_json()
            .iter()
            .any(|m| m.get("method").and_then(Value::as_str) == Some("session/prompt"))
    })
    .await);
    for i in 0..11 {
        let _ = h
            .manager
            .send_prompt(&id, &format!("queued {i}"), &[])
            .await;
    }
    assert!(wait_for(|| {
        h.sink.find(|(ev, p)| {
            ev == "chat://status"
                && p.get("lastError").and_then(Value::as_str) == Some("队列已满（最多 10 条）")
        })
        .into_iter()
        .next()
        .is_some()
    })
    .await);
    let states = h.manager.runtime_states();
    assert_eq!(states.get(&id).map(|s| s.queue_len), Some(10));
}

#[tokio::test]
async fn rate_limit_schedules_retry_and_cancel_settles() {
    let h = harness();
    let id = ready_session(&h, "").await;
    h.manager.send_prompt(&id, "work", &[]).await.expect("send");
    let mock = mock_at(&h, 0);
    assert!(
        wait_for(|| {
            mock.sent_json()
                .iter()
                .any(|m| m.get("method").and_then(Value::as_str) == Some("session/prompt"))
        })
        .await
    );
    // Prompt fails with a 429: protocolError -> messageChunk -> turnFinished.
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": 4,
                           "error": { "code": -32000, "message": "HTTP 429 rate limit" } }))
        .await;
    assert!(wait_for(|| {
        !h.sink
            .find(|(ev, p)| ev == "chat://retry" && p.get("active").and_then(Value::as_bool) == Some(true))
            .is_empty()
    })
    .await);
    let retry = h
        .sink
        .find(|(ev, p)| ev == "chat://retry" && p.get("active").and_then(Value::as_bool) == Some(true));
    let payload = retry.last().expect("retry event").1.clone();
    assert_eq!(payload.get("countdown").and_then(Value::as_u64), Some(20));
    assert_eq!(payload.get("attempt").and_then(Value::as_u64), Some(1));
    // Bubble carries the static notice.
    let notice = h.sink.find(|(ev, p)| {
        ev == "chat://bubbleSet"
            && p.pointer("/row/content")
                .and_then(Value::as_str)
                .is_some_and(|c| c.starts_with("请求被限流，20 秒后自动重试（第 1/3 次）"))
    });
    assert_eq!(notice.len(), 1);

    // Cancel settles the bubble as a plain failure and closes the turn.
    h.manager.retry_cancel(&id).await.expect("cancel retry");
    assert!(wait_for(|| {
        !h.sink
            .find(|(ev, p)| {
                ev == "chat://bubbleSet"
                    && p.pointer("/row/content").and_then(Value::as_str)
                        == Some("回合失败：请求被限流，已取消自动重试")
            })
            .is_empty()
    })
    .await);
}

#[tokio::test]
async fn process_exit_with_partial_output_resumes_with_tail_prompt() {
    let h = harness();
    let id = ready_session(&h, "").await;
    h.manager.send_prompt(&id, "long task", &[]).await.expect("send");
    let mock = mock_at(&h, 0);
    mock.feed_json(update_frame(
        "agent_message_chunk",
        json!({ "content": { "text": "PARTIAL-OUTPUT" } }),
    ))
    .await;
    assert!(wait_for(|| !chunk_events(&h, &id, "text").is_empty()).await);

    // Process dies mid-turn: a second spawn should follow automatically.
    mock.feed_eof(-1).await;
    assert!(wait_for(|| lock_ok(&h.mocks).len() >= 2).await);
    let mock2 = mock_at(&h, 1);
    drive_ready(&mock2).await;
    // Continuation prompt: carries the template + tail, into the same bubble.
    assert!(wait_for(|| {
        mock2.sent_json().iter().any(|m| {
            m.get("method").and_then(Value::as_str) == Some("session/prompt")
                && m.pointer("/params/prompt/0/text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| {
                        t.starts_with("上一条回复因连接中断被截断。") && t.ends_with("PARTIAL-OUTPUT")
                    })
        })
    })
    .await);
    // The synthetic continuation prompt is NOT persisted to history.
    let rows = h.manager.session_messages(&id);
    let user_rows = rows
        .iter()
        .filter(|r| r.get("role").and_then(Value::as_str) == Some("user"))
        .count();
    assert_eq!(user_rows, 1);
}

#[tokio::test]
async fn process_cap_stops_lru_idle_process() {
    let h = harness();
    // Empty sessions would be discarded when the next one is created, so each
    // session first completes one turn (busy -> idle, messageCount > 0).
    let mut ids = Vec::new();
    for _ in 0..4 {
        let before = lock_ok(&h.mocks).len();
        let id = ready_session(&h, "").await;
        h.manager.send_prompt(&id, "one turn", &[]).await.expect("send");
        let mock = mock_at(&h, before);
        // prompt id: 1=init, 2=session/new, 3=set_config_option, 4=prompt.
        assert!(wait_for(|| {
            mock.sent_json()
                .iter()
                .any(|m| m.get("method").and_then(Value::as_str) == Some("session/prompt"))
        })
        .await);
        mock.feed_json(json!({ "jsonrpc": "2.0", "id": 4, "result": { "stopReason": "end_turn" } }))
            .await;
        assert!(
            wait_for(|| h.manager.runtime_states().get(&id).is_some_and(|s| !s.busy)).await,
            "turn finished"
        );
        ids.push(id);
    }
    // Cap is 3: the 4th spawn evicted the least-recently-active idle process.
    assert!(
        wait_for(|| {
            let states = h.manager.runtime_states();
            states.values().filter(|s| s.acp_running).count() == 3
                && states.get(&ids[0]).is_some_and(|s| !s.acp_running)
        })
        .await,
        "exactly 3 processes stay alive, oldest evicted"
    );
}

#[tokio::test]
async fn switch_agent_keeps_acp_session_same_provider_drops_cross_provider() {
    let h = harness();
    // A second kimi agent and a claude agent.
    let (kimi2, claude) = {
        let paths = h.stores_paths();
        let mut stores = lock_ok(&h.stores);
        let kimi2 = stores.agents.create_agent(&paths, "Kimi2").expect("kimi2");
        let claude = stores.agents.create_agent(&paths, "Claude").expect("claude");
        stores
            .agents
            .update_agent(
                &paths,
                &claude,
                &AgentPatch {
                    provider: Some("claude".to_string()),
                    ..Default::default()
                },
            )
            .expect("patch provider");
        (kimi2, claude)
    };
    let id = ready_session(&h, "").await;
    // Handshake persisted the agent-side session id.
    let acp_id = acp_session_id(&h, &id);
    assert_eq!(acp_id, "s-1");

    // Same provider: acpSessionId kept, new process resumes via session/load.
    let before = lock_ok(&h.mocks).len();
    h.manager.switch_agent(&id, &kimi2).await.expect("switch same");
    assert!(wait_for(|| lock_ok(&h.mocks).len() > before).await);
    assert_eq!(acp_session_id(&h, &id), "s-1", "same provider keeps id");
    let mock2 = mock_at(&h, before);
    mock2
        .feed_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "protocolVersion": 1,
                        "agentCapabilities": { "loadSession": true,
                                               "promptCapabilities": { "image": true } } }
        }))
        .await;
    assert!(wait_for(|| {
        mock2.sent_json().iter().any(|m| {
            m.get("method").and_then(Value::as_str) == Some("session/load")
                && m.pointer("/params/sessionId").and_then(Value::as_str) == Some("s-1")
        })
    })
    .await);

    // Cross provider: stored id dropped, fresh session/new.
    let before = lock_ok(&h.mocks).len();
    h.manager
        .switch_agent(&id, &claude)
        .await
        .expect("switch cross");
    assert!(wait_for(|| acp_session_id(&h, &id).is_empty()).await);
    assert!(wait_for(|| lock_ok(&h.mocks).len() > before).await);
    let mock3 = mock_at(&h, before);
    mock3
        .feed_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "protocolVersion": 1,
                        "agentCapabilities": { "loadSession": true,
                                               "promptCapabilities": { "image": false } } }
        }))
        .await;
    assert!(wait_for(|| {
        mock3.sent_json().iter().any(|m| {
            m.get("method").and_then(Value::as_str) == Some("session/new")
        })
    })
    .await);
}

#[tokio::test]
async fn subagent_tracked_from_tool_call_through_turn_end() {
    let h = harness();
    let id = ready_session(&h, "").await;
    h.manager.send_prompt(&id, "swarm please", &[]).await.expect("send");
    let mock = mock_at(&h, 0);
    mock.feed_json(update_frame(
        "tool_call",
        json!({ "toolCallId": "t1", "title": "AgentSwarm", "status": "pending" }),
    ))
    .await;
    mock.feed_json(update_frame(
        "tool_call_update",
        json!({ "toolCallId": "t1", "status": "in_progress", "content": [
            { "type": "content", "content": { "text": "{\"description\":\"研究方案\",\"items\":[\"a\",\"b\"]}" } }
        ]}),
    ))
    .await;
    mock.feed_json(update_frame(
        "tool_call_update",
        json!({ "toolCallId": "t1", "status": "completed",
                "rawOutput": "<agent_swarm_result><subagent outcome=\"completed\"/><subagent outcome=\"failed\"/>" }),
    ))
    .await;
    assert!(wait_for(|| {
        !h.sink
            .find(|(ev, p)| {
                ev == "acp://subagent"
                    && p.pointer("/subagents/0/status").and_then(Value::as_str) == Some("completed")
            })
            .is_empty()
    })
    .await);
    let events = h.sink.find(|(ev, _)| ev == "acp://subagent");
    let last = events.last().expect("subagent events").1.clone();
    let entry = &last["subagents"][0];
    assert_eq!(entry["kind"], "AgentSwarm");
    assert_eq!(entry["title"], "研究方案");
    assert_eq!(entry["children"], 2);
    assert_eq!(entry["childNames"], json!(["a", "b"]));
    assert_eq!(entry["summary"], "完成 1/2");
    assert!(entry["finishedAt"].as_i64().unwrap_or(0) > 0);
}

/// 子 Agent 批次唤醒：回合内启动的子 Agent 在回合结束后才全部完成时，
/// runtime 自动发一条 kind:"reminder" 的追问 prompt 唤醒 agent；一批只发
/// 一次。同时验证 runtime 在回合外确实能收到 tool_call_update（recv 循环
/// 不门控 busy），这是事件驱动路径成立的前提。
#[tokio::test]
async fn subagent_batch_completion_outside_turn_wakes_agent() {
    let h = harness();
    let id = ready_session(&h, "").await;
    let mock = mock_at(&h, 0);
    h.manager.send_prompt(&id, "spawn agents", &[]).await.expect("send");
    assert!(wait_for(|| !prompt_msgs(&mock).is_empty()).await);
    // 回合内：2 个子 Agent 启动（pending，未等到完成）。
    for tid in ["bg1", "bg2"] {
        mock.feed_json(update_frame(
            "tool_call",
            json!({ "toolCallId": tid, "title": "Agent", "status": "pending" }),
        ))
        .await;
    }
    // 回合结束（仍有 2 个子 Agent 未完成 → 记入 bg_pending）。
    let pid = prompt_msgs(&mock)
        .last()
        .and_then(|m| m.get("id").and_then(Value::as_u64))
        .expect("prompt id");
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                           "result": { "stopReason": "end_turn" } }))
        .await;
    assert!(wait_for(|| h.manager.runtime_states().get(&id).map(|s| !s.busy) == Some(true)).await);
    let turns = prompt_msgs(&mock).len();

    // 回合外：第 1 个完成 —— 批未齐，不触发。
    mock.feed_json(update_frame(
        "tool_call_update",
        json!({ "toolCallId": "bg1", "status": "completed", "rawOutput": "done-1" }),
    ))
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(prompt_msgs(&mock).len(), turns, "partial batch must not wake");

    // 第 2 个完成 —— 批齐，自动追问一次。
    mock.feed_json(update_frame(
        "tool_call_update",
        json!({ "toolCallId": "bg2", "status": "completed", "rawOutput": "done-2" }),
    ))
    .await;
    assert!(wait_for(|| prompt_msgs(&mock).len() == turns + 1).await, "batch done wakes the agent");
    let wake = prompt_msgs(&mock)
        .last()
        .and_then(|m| m.pointer("/params/prompt/0/text").and_then(Value::as_str).map(str::to_string))
        .expect("wake prompt text");
    assert_eq!(wake, "你的子 Agent 已全部完成，请读取结果并继续。");
    // 追问回合的 user 行带 reminder 标记（前端系统样式）。
    assert!(wait_for(|| {
        h.manager.session_messages(&id).iter().any(|r| {
            r.get("role").and_then(Value::as_str) == Some("user")
                && r.get("content").and_then(Value::as_str) == Some(wake.as_str())
                && r.get("kind").and_then(Value::as_str) == Some("reminder")
        })
    })
    .await);
}

/// 回合内完成的子 Agent 是常态：绝不触发追问。
#[tokio::test]
async fn subagent_completion_inside_turn_does_not_wake() {
    let h = harness();
    let id = ready_session(&h, "").await;
    let mock = mock_at(&h, 0);
    h.manager.send_prompt(&id, "one agent", &[]).await.expect("send");
    assert!(wait_for(|| !prompt_msgs(&mock).is_empty()).await);
    mock.feed_json(update_frame(
        "tool_call",
        json!({ "toolCallId": "fg1", "title": "Agent", "status": "pending" }),
    ))
    .await;
    // 回合内就完成了。
    mock.feed_json(update_frame(
        "tool_call_update",
        json!({ "toolCallId": "fg1", "status": "completed", "rawOutput": "ok" }),
    ))
    .await;
    let pid = prompt_msgs(&mock)
        .last()
        .and_then(|m| m.get("id").and_then(Value::as_u64))
        .expect("prompt id");
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                           "result": { "stopReason": "end_turn" } }))
        .await;
    assert!(wait_for(|| h.manager.runtime_states().get(&id).map(|s| !s.busy) == Some(true)).await);
    let turns = prompt_msgs(&mock).len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(prompt_msgs(&mock).len(), turns, "in-turn completion must not wake");
}

#[tokio::test]
async fn queue_drains_head_first_until_empty() {
    let h = harness();
    let id = ready_session(&h, "").await;
    let mock = mock_at(&h, 0);
    h.manager.send_prompt(&id, "task-1", &[]).await.expect("send");
    assert!(wait_for(|| !prompt_msgs(&mock_at(&h, 0)).is_empty()).await);
    h.manager.send_prompt(&id, "task-2", &[]).await.expect("q2");
    h.manager.send_prompt(&id, "task-3", &[]).await.expect("q3");
    assert!(wait_for(|| {
        h.manager.runtime_states().get(&id).map(|s| s.queue_len) == Some(2)
    })
    .await);

    // Each turn finish must send the NEXT queued prompt, in FIFO order.
    for (text, remaining) in [("task-2", 1), ("task-3", 0)] {
        let pid = prompt_msgs(&mock_at(&h, 0))
            .last()
            .and_then(|m| m.get("id").and_then(Value::as_u64))
            .expect("prompt id");
        mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                               "result": { "stopReason": "end_turn" } }))
            .await;
        assert!(
            wait_for(|| {
                prompt_msgs(&mock_at(&h, 0)).iter().any(|m| {
                    m.pointer("/params/prompt/0/text")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t == text)
                })
            })
            .await,
            "queued prompt {text} is sent after the previous turn finishes"
        );
        assert!(wait_for(|| {
            h.manager.runtime_states().get(&id).map(|s| s.queue_len) == Some(remaining)
        })
        .await);
    }
    // Finishing the last drained turn leaves the queue empty.
    let pid = prompt_msgs(&mock_at(&h, 0))
        .last()
        .and_then(|m| m.get("id").and_then(Value::as_u64))
        .expect("prompt id");
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                           "result": { "stopReason": "end_turn" } }))
        .await;
    assert!(wait_for(|| {
        h.manager.runtime_states().get(&id).map(|s| s.queue_len) == Some(0)
    })
    .await);
    // Exactly three prompts went out: task-1, task-2, task-3.
    assert_eq!(prompt_msgs(&mock_at(&h, 0)).len(), 3);
}

#[tokio::test]
async fn attachments_enqueue_while_busy_and_drain_with_image_block() {
    use crate::chat::runtime::SendOutcome;

    let h = harness();
    let id = ready_session(&h, "").await;
    let mock = mock_at(&h, 0);
    h.manager.send_prompt(&id, "task-1", &[]).await.expect("send");
    assert!(wait_for(|| !prompt_msgs(&mock_at(&h, 0)).is_empty()).await);

    // Attachment must exist on disk (the manager drops missing paths); the
    // mock advertises promptCapabilities.image so this rides as an image
    // block rather than an inline "[附件]" line.
    let png = h.tmp.path().join("queued.png");
    std::fs::write(&png, b"png").expect("write png");
    let png_str = png.to_string_lossy().into_owned();
    let outcome = h
        .manager
        .send_prompt(&id, "with-img", &[png_str])
        .await
        .expect("enqueue");
    assert!(
        matches!(outcome, SendOutcome::Enqueued),
        "attachments may queue while busy"
    );
    // Snapshot annotates the entry with 📎1.
    assert!(wait_for(|| {
        h.manager
            .runtime_states()
            .get(&id)
            .and_then(|s| s.queue.first())
            .is_some_and(|q| q.contains("with-img") && q.contains("📎1"))
    })
    .await);

    // Finishing the turn drains the queued message WITH its image block.
    let pid = prompt_msgs(&mock_at(&h, 0))
        .last()
        .and_then(|m| m.get("id").and_then(Value::as_u64))
        .expect("prompt id");
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                           "result": { "stopReason": "end_turn" } }))
        .await;
    assert!(wait_for(|| {
        prompt_msgs(&mock_at(&h, 0)).iter().any(|m| {
            m.pointer("/params/prompt/0/text").and_then(Value::as_str) == Some("with-img")
                && m
                    .pointer("/params/prompt/1/type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t == "image" || t == "image_url")
        })
    })
    .await);
}

// ---- small helpers ----

/// 首条用户消息注入提醒工具引导语：首条 session/prompt 文本 = 引导语 +
/// 原文，第二条起不再注入；聊天显示/落盘的用户行始终是原文。
#[tokio::test]
async fn first_user_message_carries_reminder_guide_prefix() {
    use crate::chat::runtime::REMINDER_GUIDE_PREFIX;

    // 引导语同时覆盖"用户提醒"与"子 Agent/后台任务自我唤醒"两个场景。
    assert!(REMINDER_GUIDE_PREFIX.contains("set_reminder"));
    assert!(REMINDER_GUIDE_PREFIX.contains("子 Agent"));

    let h = harness();
    let id = ready_session(&h, "").await;
    let mock = mock_at(&h, 0);

    h.manager.send_prompt(&id, "第一条", &[]).await.expect("send");
    assert!(wait_for(|| !prompt_msgs(&mock_at(&h, 0)).is_empty()).await);
    let first = prompt_msgs(&mock_at(&h, 0))
        .last()
        .and_then(|m| m.pointer("/params/prompt/0/text").and_then(Value::as_str).map(str::to_string))
        .expect("first prompt text");
    assert!(
        first.starts_with(REMINDER_GUIDE_PREFIX) && first.ends_with("\n\n第一条"),
        "first prompt prefixed with the reminder guide, got: {first}"
    );

    // Finish the turn, then send the second message.
    let pid = prompt_msgs(&mock_at(&h, 0))
        .last()
        .and_then(|m| m.get("id").and_then(Value::as_u64))
        .expect("prompt id");
    mock.feed_json(json!({ "jsonrpc": "2.0", "id": pid,
                           "result": { "stopReason": "end_turn" } }))
        .await;
    h.manager.send_prompt(&id, "第二条", &[]).await.expect("send 2");
    assert!(wait_for(|| prompt_msgs(&mock_at(&h, 0)).len() == 2).await);
    let second = prompt_msgs(&mock_at(&h, 0))
        .last()
        .and_then(|m| m.pointer("/params/prompt/0/text").and_then(Value::as_str).map(str::to_string))
        .expect("second prompt text");
    assert_eq!(second, "第二条", "no guide prefix on subsequent prompts");

    // Store 里的用户行保持原文（引导语不进历史/显示）。
    assert!(wait_for(|| {
        let rows = h.manager.session_messages(&id);
        let users: Vec<&str> = rows
            .iter()
            .filter(|r| r.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|r| r.get("content").and_then(Value::as_str))
            .collect();
        users == ["第一条", "第二条"]
    })
    .await);
}

/// 提醒调度：store 里注入一条短到期提醒 + RemindersReload 后，runtime 自动
/// 以一条普通 prompt 发起提醒回合（user + assistant 占位行落盘），提醒
/// 本体在发 prompt 前已 mark_done（防崩溃重发）。
#[tokio::test]
async fn due_reminder_fires_as_prompt_turn() {
    let h = harness();
    let id = ready_session(&h, "").await;
    {
        let paths = h.stores_paths();
        let mut stores = lock_ok(&h.stores);
        stores
            .reminders
            .add(&paths, &id, "喝水", 0.005, "user")
            .expect("add reminder");
    }
    h.manager.reminders_reload(&id).await;
    let mock = mock_at(&h, 0);
    assert!(
        wait_for(|| {
            prompt_msgs(&mock).iter().any(|m| {
                // 提醒回合若是本会话首条用户消息，prompt 文本带引导语前缀。
                m.pointer("/params/prompt/0/text").and_then(Value::as_str)
                    .is_some_and(|t| t.ends_with("⏰ 提醒时间到：喝水"))
            })
        })
        .await,
        "reminder prompt sent to the agent"
    );
    // user 行（带 reminder 标记）+ assistant 占位行出现在历史里。
    assert!(wait_for(|| {
        let rows = h.manager.session_messages(&id);
        rows.iter().any(|r| {
            r.get("role").and_then(Value::as_str) == Some("user")
                && r.get("content").and_then(Value::as_str) == Some("⏰ 提醒时间到：喝水")
                && r.get("kind").and_then(Value::as_str) == Some("reminder")
        }) && rows.iter().any(|r| {
            r.get("role").and_then(Value::as_str) == Some("assistant")
                && r.get("status").and_then(Value::as_str) == Some("pending")
        })
    })
    .await);
    // chat://messageAppended 的 user 行 payload 也带 reminder 标记。
    assert!(!h
        .sink
        .find(|(ev, p)| ev == "chat://messageAppended"
            && p.pointer("/row/kind").and_then(Value::as_str) == Some("reminder"))
        .is_empty());
    // 触发即 done，pending 列表清空，且前端收到 chat://reminders。
    assert!(lock_ok(&h.stores).reminders.list(&id).is_empty());
    assert!(!h
        .sink
        .find(|(ev, p)| ev == "chat://reminders"
            && p.get("sessionId").and_then(Value::as_str) == Some(id.as_str()))
        .is_empty());
}

/// 过期提醒（重启恢复场景）：reload 时 due_at 已过的提醒立即触发。
#[tokio::test]
async fn overdue_reminder_fires_immediately() {
    let h = harness();
    let id = h.manager.create_session("").await.expect("create session");
    // 先正常 add，再把落盘的 dueAtMs 改到过去（等价于重启后读到的过期行）。
    {
        let paths = h.stores_paths();
        let mut stores = lock_ok(&h.stores);
        stores
            .reminders
            .add(&paths, &id, "过期提醒", 1.0, "user")
            .expect("add");
        let mut rows = crate::store::reminders::load_file(&paths.reminders_path());
        for r in &mut rows {
            r.due_at_ms = crate::store::json::now_ms() - 60_000;
        }
        crate::store::reminders::save_file(&paths.reminders_path(), &rows).expect("save");
    }
    h.manager.reminders_reload(&id).await;
    assert!(
        wait_for(|| lock_ok(&h.stores).reminders.list(&id).is_empty()).await,
        "overdue reminder fired and marked done"
    );
    // 提醒回合的 user 行进了历史。
    assert!(wait_for(|| {
        h.manager.session_messages(&id).iter().any(|r| {
            r.get("content").and_then(Value::as_str) == Some("⏰ 提醒时间到：过期提醒")
        })
    })
    .await);
}

// ---- small helpers ----

fn prompt_msgs(mock: &MockTransport) -> Vec<Value> {
    mock.sent_json()
        .into_iter()
        .filter(|m| m.get("method").and_then(Value::as_str) == Some("session/prompt"))
        .collect()
}

impl Harness {
    fn stores_paths(&self) -> Paths {
        lock_ok(&self.stores).paths.clone()
    }
}

fn acp_session_id(h: &Harness, session_id: &str) -> String {
    let mut stores = lock_ok(&h.stores);
    stores.sessions.acp_session_id_for(session_id)
}
