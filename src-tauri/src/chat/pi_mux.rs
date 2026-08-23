// One Pi subprocess can host many WarDex sessions (same project + same
// Agent). RPC commands/events carry `sessionId`; `open_session` / `close_session`
// add and drop runtimes inside that process.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::acp::{AcpError, SpawnConfig, StdioTransport, Transport};
use crate::chat::driver::PiLaunch;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MuxKey {
    cwd: String,
    agent_id: String,
    /// Empty = share by (cwd, agent). Non-empty = dedicated process (old Pi
    /// without open_session, or an explicit fallback).
    solo: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachKind {
    First,
    Joined,
}

pub enum MuxMsg {
    Line(Value),
    Eof { code: i32 },
}

pub struct PiMux {
    key: MuxKey,
    transport: StdioTransport,
    bootstrap_id: String,
    sessions: Mutex<HashMap<String, mpsc::Sender<MuxMsg>>>,
}

fn registry() -> &'static Mutex<HashMap<MuxKey, Arc<PiMux>>> {
    static REG: OnceLock<Mutex<HashMap<MuxKey, Arc<PiMux>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

impl PiMux {
    pub async fn attach(
        launch: &PiLaunch,
        session_id: &str,
        inbound: mpsc::Sender<MuxMsg>,
    ) -> Result<(Arc<Self>, AttachKind), AcpError> {
        Self::attach_with_key(
            MuxKey {
                cwd: launch.cwd.clone(),
                agent_id: launch.agent_id.clone(),
                solo: String::new(),
            },
            launch,
            session_id,
            inbound,
        )
        .await
    }

    /// Own process, never shared (fallback when the live Pi binary has no
    /// `open_session`).
    pub async fn attach_solo(
        launch: &PiLaunch,
        session_id: &str,
        inbound: mpsc::Sender<MuxMsg>,
    ) -> Result<Arc<Self>, AcpError> {
        let (mux, _) = Self::attach_with_key(
            MuxKey {
                cwd: launch.cwd.clone(),
                agent_id: launch.agent_id.clone(),
                solo: session_id.to_string(),
            },
            launch,
            session_id,
            inbound,
        )
        .await?;
        Ok(mux)
    }

    async fn attach_with_key(
        key: MuxKey,
        launch: &PiLaunch,
        session_id: &str,
        inbound: mpsc::Sender<MuxMsg>,
    ) -> Result<(Arc<Self>, AttachKind), AcpError> {
        let existing = {
            let map = registry().lock().await;
            map.get(&key).cloned()
        };
        if let Some(mux) = existing {
            mux.join_session(launch, session_id, inbound).await?;
            return Ok((mux, AttachKind::Joined));
        }
        let mux = Self::spawn_first(key, launch, session_id, inbound).await?;
        Ok((mux, AttachKind::First))
    }

    async fn spawn_first(
        key: MuxKey,
        launch: &PiLaunch,
        session_id: &str,
        inbound: mpsc::Sender<MuxMsg>,
    ) -> Result<Arc<Self>, AcpError> {
        let mut args = vec!["--mode".to_string(), "rpc".to_string()];
        let persist = !launch.session_dir.is_empty()
            && crate::chat::pi::is_valid_pi_session_id(session_id);
        if persist {
            args.push("--session-dir".to_string());
            args.push(launch.session_dir.clone());
            args.push("--session-id".to_string());
            args.push(session_id.to_string());
        } else {
            args.push("--no-session".to_string());
        }
        if !launch.provider_key.is_empty() {
            args.push("--provider".to_string());
            args.push(launch.provider_key.clone());
        }
        if !launch.model.is_empty() {
            args.push("--model".to_string());
            args.push(launch.model.clone());
        }
        for ext in &launch.extensions {
            if !ext.trim().is_empty() {
                args.push("--extension".to_string());
                args.push(ext.clone());
            }
        }
        let config = SpawnConfig {
            cli_path: launch.binary.to_string_lossy().into_owned(),
            args,
            env: launch.env.clone(),
            cwd: launch.cwd.clone(),
        };
        let transport = StdioTransport::spawn(&config).await?;
        let mux = Arc::new(Self {
            key: key.clone(),
            transport,
            bootstrap_id: session_id.to_string(),
            sessions: Mutex::new(HashMap::from([(session_id.to_string(), inbound)])),
        });
        {
            let mut map = registry().lock().await;
            map.insert(key, Arc::clone(&mux));
        }
        let reader = Arc::clone(&mux);
        tokio::spawn(async move { reader.recv_loop().await });
        log::info!(
            "pi mux spawned cwd={} agent={} session={}",
            mux.key.cwd,
            mux.key.agent_id,
            session_id
        );
        Ok(mux)
    }

    async fn join_session(
        &self,
        launch: &PiLaunch,
        session_id: &str,
        inbound: mpsc::Sender<MuxMsg>,
    ) -> Result<(), AcpError> {
        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id.to_string(), inbound);
        }
        let mut cmd = json!({
            "type": "open_session",
            "sessionId": session_id,
        });
        if !launch.session_dir.is_empty() {
            cmd["sessionDir"] = json!(launch.session_dir);
        }
        if !launch.cwd.is_empty() {
            cmd["cwd"] = json!(launch.cwd);
        }
        self.send_line(&cmd.to_string()).await?;
        log::info!(
            "pi mux open_session cwd={} agent={} session={}",
            self.key.cwd,
            self.key.agent_id,
            session_id
        );
        Ok(())
    }

    pub async fn send_line(&self, line: &str) -> Result<(), AcpError> {
        self.transport.send_line(line).await
    }

    pub fn stderr_tail(&self) -> String {
        self.transport.stderr_tail()
    }

    pub async fn detach(&self, session_id: &str) {
        let remaining = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_id);
            sessions.len()
        };
        let cmd = json!({ "type": "close_session", "sessionId": session_id });
        let _ = self.send_line(&cmd.to_string()).await;
        if remaining == 0 {
            registry().lock().await.remove(&self.key);
            log::info!(
                "pi mux empty, dropping process cwd={} agent={}",
                self.key.cwd,
                self.key.agent_id
            );
        }
    }

    async fn recv_loop(self: Arc<Self>) {
        loop {
            match self.transport.recv_line().await {
                Ok(Some(line)) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        log::warn!("pi mux: unparsable line");
                        continue;
                    };
                    let sid = v
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or(&self.bootstrap_id)
                        .to_string();
                    let tx = {
                        let sessions = self.sessions.lock().await;
                        sessions
                            .get(&sid)
                            .cloned()
                            .or_else(|| sessions.get(&self.bootstrap_id).cloned())
                    };
                    if let Some(tx) = tx {
                        if tx.send(MuxMsg::Line(v)).await.is_err() {
                            self.sessions.lock().await.remove(&sid);
                        }
                    }
                }
                Ok(None) => {
                    let code = self.transport.exit_code().await.unwrap_or(-1);
                    self.broadcast_eof(code).await;
                    break;
                }
                Err(e) => {
                    log::warn!("pi mux recv: {e}");
                    self.broadcast_eof(-1).await;
                    break;
                }
            }
        }
        registry().lock().await.remove(&self.key);
    }

    async fn broadcast_eof(&self, code: i32) {
        let sessions = {
            let mut g = self.sessions.lock().await;
            std::mem::take(&mut *g)
        };
        for (_, tx) in sessions {
            let _ = tx.send(MuxMsg::Eof { code }).await;
        }
    }
}
