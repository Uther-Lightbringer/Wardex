// Devin cloud agent driver (provider "devin"): Devin runs remotely and is
// driven through its REST session API (docs.devin.ai/api-reference), not a
// local process — there is nothing to spawn and no stdio to frame.
//
//   POST {base}/v1/sessions              -> create a session with the prompt
//   POST {base}/v1/sessions/{id}/message -> follow-up messages
//   GET  {base}/v1/sessions/{id}         -> status + the full message list
//
// The API has no streaming channel, so a turn is a poll loop: after each
// prompt we poll the session until Devin stops working, forwarding every new
// `devin_message` as an AcpEvent::MessageChunk. That keeps the chat layer and
// the frontend provider-agnostic — they see the same event vocabulary the ACP
// and Pi drivers produce.
//
// Deliberately unsupported (no equivalent in the remote API): images,
// permission answers, permission modes, config options, token usage.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use crate::acp::{AcpError, AcpEvent};
use crate::chat::driver::{BoxFuture, ClientDriver, DevinLaunch};

/// Default API root when the agent leaves Base URL empty.
pub const DEFAULT_BASE_URL: &str = "https://api.devin.ai";

/// Poll interval of a running turn. Devin turns last minutes, so a tighter
/// interval only burns requests.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Consecutive transport failures tolerated before a turn is failed.
const MAX_POLL_ERRORS: u32 = 5;

/// Normalize the configured Base URL into an API root without a trailing
/// slash or `/v1` suffix (both spellings are natural to paste).
pub fn api_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }
    trimmed.trim_end_matches("/v1").to_string()
}

/// Blocking REST calls (ureq) moved off the runtime with spawn_blocking.
/// Errors are user-facing Chinese strings; the HTTP body is folded in because
/// Devin reports quota/permission problems there.
#[derive(Clone)]
pub struct DevinApi {
    root: String,
    key: String,
}

impl DevinApi {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            root: api_root(base_url),
            key: api_key.trim().to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.root)
    }

    pub async fn get(&self, path: &str) -> Result<Value, String> {
        let url = self.url(path);
        let key = self.key.clone();
        call_blocking(move || {
            ureq::get(&url)
                .timeout(Duration::from_secs(30))
                .set("Authorization", &format!("Bearer {key}"))
                .call()
        })
        .await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = self.url(path);
        let key = self.key.clone();
        call_blocking(move || {
            ureq::post(&url)
                .timeout(Duration::from_secs(30))
                .set("Authorization", &format!("Bearer {key}"))
                .send_json(body)
        })
        .await
    }
}

/// Run one ureq call on the blocking pool and normalize its outcome. A 2xx
/// with an empty/non-JSON body yields Value::Null (the message endpoint
/// answers `null` on success).
async fn call_blocking<F>(f: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<ureq::Response, ureq::Error> + Send + 'static,
{
    let joined = tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("请求未能执行: {e}"))?;
    match joined {
        Ok(resp) => Ok(resp.into_json::<Value>().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {code}: {}", truncate(body.trim(), 400)))
        }
        Err(e) => Err(format!("网络错误: {e}")),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// What the poll task reports back to the driver's event loop.
enum PollMsg {
    /// A new agent message (Devin's reply text).
    Chunk(String),
    /// Session title from the API (Devin names the session itself).
    Title(String),
    /// The turn ended: Devin is blocked on the user, finished or expired.
    Finished { stop_reason: String },
    /// Transport gave up; the turn is failed with this message.
    Failed(String),
}

pub struct DevinDriver {
    api: DevinApi,
    tx: mpsc::Sender<AcpEvent>,
    inbound: mpsc::Receiver<PollMsg>,
    poll_tx: mpsc::Sender<PollMsg>,
    /// Remote session id; empty until the first prompt creates one.
    devin_session: String,
    /// event_ids already forwarded (the API returns the whole history on
    /// every poll, including messages from earlier turns and other clients).
    seen: Arc<Mutex<HashSet<String>>>,
    poller: Option<JoinHandle<()>>,
}

impl DevinDriver {
    /// Mirrors AcpClient::spawn's contract: any pre-session failure emits
    /// StartFailed into the event channel before returning Err.
    pub async fn spawn(launch: DevinLaunch, tx: mpsc::Sender<AcpEvent>) -> Result<Self, AcpError> {
        if launch.api_key.trim().is_empty() {
            let error =
                "未配置 Devin API Key（在 https://app.devin.ai/settings/api-keys 生成 apk_… 密钥）"
                    .to_string();
            let _ = tx
                .send(AcpEvent::StartFailed {
                    error: error.clone(),
                })
                .await;
            return Err(AcpError::Spawn(error));
        }
        let api = DevinApi::new(&launch.base_url, &launch.api_key);
        let (poll_tx, inbound) = mpsc::channel(64);
        let mut driver = Self {
            api,
            tx,
            inbound,
            poll_tx,
            devin_session: String::new(),
            seen: Arc::new(Mutex::new(HashSet::new())),
            poller: None,
        };
        // Resume: adopt the stored remote session and mark its existing
        // messages as seen so a respawn doesn't replay the whole history.
        let resume = launch.resume_session_id.trim().to_string();
        if !resume.is_empty() {
            match driver.api.get(&session_path(&resume)).await {
                Ok(detail) => {
                    let mut seen = driver.seen.lock().await;
                    for id in event_ids(&detail) {
                        seen.insert(id);
                    }
                    drop(seen);
                    driver.devin_session = resume;
                }
                Err(e) => log::warn!("devin: 无法恢复会话 {resume}，将新建：{e}"),
            }
        }
        let session_id = driver.devin_session.clone();
        driver.emit(AcpEvent::Started { session_id }).await;
        Ok(driver)
    }

    async fn emit(&self, ev: AcpEvent) {
        let _ = self.tx.send(ev).await;
    }

    /// Start (or restart) the poll task that watches the remote session for
    /// the duration of one turn.
    fn start_poll(&mut self) {
        self.stop_poll();
        let api = self.api.clone();
        let session = self.devin_session.clone();
        let seen = Arc::clone(&self.seen);
        let out = self.poll_tx.clone();
        self.poller = Some(tokio::spawn(async move {
            poll_turn(api, session, seen, out).await;
        }));
    }

    fn stop_poll(&mut self) {
        if let Some(h) = self.poller.take() {
            h.abort();
        }
    }
}

fn session_path(id: &str) -> String {
    format!("/v1/sessions/{id}")
}

/// event_ids of every message in a GET session payload.
fn event_ids(detail: &Value) -> Vec<String> {
    detail
        .get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter()
                .filter_map(|m| m.get("event_id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A message the user should see: everything Devin emits, minus the echo of
/// the prompts WarDex itself sent (`user_message` and, for the prompt that
/// created the session, `initial_user_message`).
fn is_agent_message(msg: &Value) -> bool {
    let ty = msg.get("type").and_then(Value::as_str).unwrap_or_default();
    !ty.ends_with("user_message")
}

/// Map the API's status_enum to a WarDex stop reason. None = the session is
/// still running (working / suspend / resume transitions), so keep polling.
fn stop_reason_for(status: &str) -> Option<&'static str> {
    match status {
        "blocked" | "finished" => Some("end_turn"),
        "expired" => Some("cancelled"),
        _ => None,
    }
}

/// One turn's poll loop: forward new agent messages until Devin stops
/// working, then report the stop reason. Errors are tolerated up to
/// MAX_POLL_ERRORS so a transient network blip never kills a long turn.
async fn poll_turn(
    api: DevinApi,
    session: String,
    seen: Arc<Mutex<HashSet<String>>>,
    out: mpsc::Sender<PollMsg>,
) {
    let path = session_path(&session);
    let mut errors = 0u32;
    let mut title_sent = false;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let detail = match api.get(&path).await {
            Ok(v) => {
                errors = 0;
                v
            }
            Err(e) => {
                errors += 1;
                if errors >= MAX_POLL_ERRORS {
                    let _ = out
                        .send(PollMsg::Failed(format!("轮询 Devin 会话失败: {e}")))
                        .await;
                    return;
                }
                continue;
            }
        };
        if !title_sent {
            if let Some(t) = detail.get("title").and_then(Value::as_str) {
                if !t.trim().is_empty() {
                    title_sent = true;
                    let _ = out.send(PollMsg::Title(t.to_string())).await;
                }
            }
        }
        if let Some(msgs) = detail.get("messages").and_then(Value::as_array) {
            let mut seen = seen.lock().await;
            for m in msgs {
                let Some(id) = m.get("event_id").and_then(Value::as_str) else {
                    continue;
                };
                if !seen.insert(id.to_string()) || !is_agent_message(m) {
                    continue;
                }
                let text = m.get("message").and_then(Value::as_str).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                if out.send(PollMsg::Chunk(format!("{text}\n"))).await.is_err() {
                    return;
                }
            }
        }
        let status = detail
            .get("status_enum")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(stop) = stop_reason_for(status) {
            let _ = out
                .send(PollMsg::Finished {
                    stop_reason: stop.to_string(),
                })
                .await;
            return;
        }
    }
}

impl Drop for DevinDriver {
    fn drop(&mut self) {
        self.stop_poll();
    }
}

impl ClientDriver for DevinDriver {
    fn recv_once(&mut self) -> BoxFuture<'_, Result<bool, AcpError>> {
        Box::pin(async move {
            match self.inbound.recv().await {
                Some(PollMsg::Chunk(text)) => {
                    self.emit(AcpEvent::MessageChunk { text }).await;
                    Ok(true)
                }
                Some(PollMsg::Title(title)) => {
                    self.emit(AcpEvent::SessionInfo { title: Some(title) })
                        .await;
                    Ok(true)
                }
                Some(PollMsg::Finished { stop_reason }) => {
                    self.stop_poll();
                    self.emit(AcpEvent::TurnFinished {
                        stop_reason,
                        usage: None,
                    })
                    .await;
                    Ok(true)
                }
                Some(PollMsg::Failed(error)) => {
                    self.stop_poll();
                    self.emit(AcpEvent::ProtocolError { error }).await;
                    self.emit(AcpEvent::TurnFinished {
                        stop_reason: "error".to_string(),
                        usage: None,
                    })
                    .await;
                    Ok(true)
                }
                // The driver owns a sender clone, so this only happens once
                // the driver itself is being torn down.
                None => Ok(false),
            }
        })
    }

    fn prompt<'a>(
        &'a mut self,
        text: &'a str,
        image_paths: &'a [String],
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async move {
            if !image_paths.is_empty() {
                self.emit(AcpEvent::MessageChunk {
                    text: "（Devin API 不支持图片附件，已忽略）\n".to_string(),
                })
                .await;
            }
            if self.devin_session.is_empty() {
                let created = self
                    .api
                    .post("/v1/sessions", json!({ "prompt": text }))
                    .await
                    .map_err(AcpError::Spawn)?;
                let id = created
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return Err(AcpError::Spawn(
                        "Devin 创建会话未返回 session_id".to_string(),
                    ));
                }
                self.devin_session = id.clone();
                // Re-announce so the chat layer persists the remote id and a
                // respawn resumes this session instead of opening a new one.
                self.emit(AcpEvent::Started { session_id: id }).await;
                if let Some(url) = created.get("url").and_then(Value::as_str) {
                    self.emit(AcpEvent::MessageChunk {
                        text: format!("Devin 会话已创建: {url}\n\n"),
                    })
                    .await;
                }
            } else {
                let path = format!("{}/message", session_path(&self.devin_session));
                self.api
                    .post(&path, json!({ "message": text }))
                    .await
                    .map_err(AcpError::Spawn)?;
            }
            self.start_poll();
            Ok(())
        })
    }

    fn cancel_turn(&mut self) -> BoxFuture<'_, Result<(), AcpError>> {
        // The v1 API has no cancel: stop watching and close the turn locally.
        // Devin keeps working remotely; its output lands on the next poll.
        Box::pin(async move {
            self.stop_poll();
            self.emit(AcpEvent::TurnFinished {
                stop_reason: "cancelled".to_string(),
                usage: None,
            })
            .await;
            Ok(())
        })
    }

    fn answer_permission<'a>(
        &'a mut self,
        _request_id: i64,
        _option_id: &'a str,
        _cancelled: bool,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async { Ok(()) })
    }

    fn set_mode<'a>(&'a mut self, _mode_id: &'a str) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async { Ok(()) })
    }

    fn set_config_option<'a>(
        &'a mut self,
        _config_id: &'a str,
        _value: &'a str,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(async { Ok(()) })
    }

    fn image_supported(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_root_normalizes_base_url() {
        assert_eq!(api_root(""), DEFAULT_BASE_URL);
        assert_eq!(api_root("  "), DEFAULT_BASE_URL);
        assert_eq!(api_root("https://api.devin.ai"), "https://api.devin.ai");
        assert_eq!(api_root("https://api.devin.ai/"), "https://api.devin.ai");
        assert_eq!(api_root("https://api.devin.ai/v1"), "https://api.devin.ai");
        assert_eq!(api_root("https://api.devin.ai/v1/"), "https://api.devin.ai");
        assert_eq!(
            api_root("https://proxy.example.com/devin"),
            "https://proxy.example.com/devin"
        );
    }

    #[test]
    fn stop_reason_maps_status_enum() {
        assert_eq!(stop_reason_for("working"), None);
        assert_eq!(stop_reason_for("suspend_requested"), None);
        assert_eq!(stop_reason_for("resumed"), None);
        assert_eq!(stop_reason_for("blocked"), Some("end_turn"));
        assert_eq!(stop_reason_for("finished"), Some("end_turn"));
        assert_eq!(stop_reason_for("expired"), Some("cancelled"));
    }

    #[test]
    fn only_non_user_messages_are_forwarded() {
        assert!(is_agent_message(&json!({ "type": "devin_message" })));
        assert!(!is_agent_message(&json!({ "type": "user_message" })));
        assert!(!is_agent_message(
            &json!({ "type": "initial_user_message" })
        ));
    }

    #[test]
    fn event_ids_tolerates_missing_messages() {
        assert!(event_ids(&json!({})).is_empty());
        assert_eq!(
            event_ids(&json!({ "messages": [{ "event_id": "a" }, { "message": "no id" }] })),
            vec!["a".to_string()]
        );
    }
}
