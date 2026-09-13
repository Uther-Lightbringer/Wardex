// Client driver abstraction: the runtime actor drives the ACP client through
// this object-safe trait so tests can substitute AcpClient<MockTransport>
// without touching chat logic. Production wraps AcpClient<StdioTransport>
// (spawned per ensureAcp via `stdio_spawner`).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::acp::{AcpClient, AcpError, AcpEvent, SpawnConfig, StartParams, Transport};
use crate::provider::EnvOverrides;

/// Boxed future alias for the object-safe trait (no async-trait dependency).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Everything the actor needs from the protocol state machine. `start()` is
/// not here: it runs inside the spawner so the actor always receives a
/// client whose handshake is already in flight.
pub trait ClientDriver: Send {
    /// Read and dispatch one inbound line. Ok(false) = process exited.
    fn recv_once(&mut self) -> BoxFuture<'_, Result<bool, AcpError>>;
    fn prompt<'a>(&'a mut self, text: &'a str, image_paths: &'a [String])
        -> BoxFuture<'a, Result<(), AcpError>>;
    fn cancel_turn(&mut self) -> BoxFuture<'_, Result<(), AcpError>>;
    fn answer_permission<'a>(
        &'a mut self,
        request_id: i64,
        option_id: &'a str,
        cancelled: bool,
    ) -> BoxFuture<'a, Result<(), AcpError>>;
    fn set_mode<'a>(&'a mut self, mode_id: &'a str) -> BoxFuture<'a, Result<(), AcpError>>;
    fn set_config_option<'a>(
        &'a mut self,
        config_id: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<(), AcpError>>;
    /// From initialize -> promptCapabilities.image (attachment split rule).
    fn image_supported(&self) -> bool;
    /// Tail of the child CLI's stderr, folded into failure bubbles; empty
    /// for drivers without a captured stderr (tests).
    fn stderr_tail(&self) -> String {
        String::new()
    }
}

impl<T: Transport + Sync + 'static> ClientDriver for AcpClient<T> {
    fn recv_once(&mut self) -> BoxFuture<'_, Result<bool, AcpError>> {
        Box::pin(AcpClient::recv_once(self))
    }
    fn prompt<'a>(
        &'a mut self,
        text: &'a str,
        image_paths: &'a [String],
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(AcpClient::prompt(self, text, image_paths))
    }
    fn cancel_turn(&mut self) -> BoxFuture<'_, Result<(), AcpError>> {
        Box::pin(AcpClient::cancel_turn(self))
    }
    fn answer_permission<'a>(
        &'a mut self,
        request_id: i64,
        option_id: &'a str,
        cancelled: bool,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(AcpClient::answer_permission(
            self, request_id, option_id, cancelled,
        ))
    }
    fn set_mode<'a>(&'a mut self, mode_id: &'a str) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(AcpClient::set_mode(self, mode_id))
    }
    fn set_config_option<'a>(
        &'a mut self,
        config_id: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<(), AcpError>> {
        Box::pin(AcpClient::set_config_option(self, config_id, value))
    }
    fn image_supported(&self) -> bool {
        AcpClient::image_supported(self)
    }
    fn stderr_tail(&self) -> String {
        AcpClient::stderr_tail(self)
    }
}

/// Everything an ensureAcp (re)start needs, recomputed from the agent
/// snapshot + session meta at each call (ChatController.cpp:836-896).
pub struct SessionLaunch {
    pub spawn: SpawnConfig,
    pub start: StartParams,
}

/// Launch parameters for the embedded Pi agent (provider "pi"): no ACP
/// handshake — spawn `<binary> --mode rpc --no-session` and speak the Pi RPC
/// JSONL protocol over stdin/stdout (chat/pi.rs). The binary is a bun-compiled
/// self-contained executable (bundle-pi.mjs); Node is not required. Env
/// overrides use the same None-deletes semantics as ACP.
#[derive(Clone)]
pub struct PiLaunch {
    /// Absolute path to the compiled pi binary (pi.exe on Windows).
    pub binary: PathBuf,
    /// Custom-provider key written into ~/.pi/agent/models.json ("" = none,
    /// pi uses its own config).
    pub provider_key: String,
    /// Model id passed via `--model` ("" = pi default).
    pub model: String,
    /// Allowed thinking levels (models.rs EFFORT_LEVELS); empty = every level.
    /// Drives the thinking picker and the initial `set_thinking_level`.
    pub effort_options: Vec<String>,
    /// Default thinking level ("" = the first allowed one). Sent to pi at
    /// spawn so thinking is ON by default.
    pub default_effort: String,
    pub env: EnvOverrides,
    /// WarDex agent id: sessions with the same cwd + agent share one pi.exe.
    pub agent_id: String,
    /// Child working directory (the session's project dir).
    pub cwd: String,
    /// Pi session persistence: --session-dir (isolated per-Wardex-session
    /// dir) + --session-id (Wardex session uuid; pi creates-if-missing /
    /// resumes-if-exists). Empty = keep --no-session (ephemeral fallback).
    pub session_dir: String,
    /// Wardex session id, passed as pi --session-id when session_dir is set.
    pub session_id: String,
    /// Absolute paths to WarDex-bundled Pi extensions (`--extension`, repeatable).
    pub extensions: Vec<String>,
}

/// Everything the Devin cloud agent needs: it has no local process, only an
/// API key and (optionally) a remote session to resume.
pub struct DevinLaunch {
    pub api_key: String,
    /// Empty = https://api.devin.ai.
    pub base_url: String,
    /// Remote Devin session id stored on the WarDex session; empty creates a
    /// new Devin session on the first prompt.
    pub resume_session_id: String,
}

/// Everything a spawn needs: an ACP subprocess, the embedded pi agent, or a
/// remote Devin session.
pub enum Launch {
    Acp(SessionLaunch),
    Pi(PiLaunch),
    Devin(DevinLaunch),
}

/// Factory producing a started client bound to a fresh event channel. The
/// event receiver side stays with the actor across respawns.
pub type Spawner = Box<
    dyn FnMut(Launch, mpsc::Sender<AcpEvent>) -> BoxFuture<'static, Result<Box<dyn ClientDriver>, AcpError>>
        + Send,
>;

/// Production spawner: dispatches on the launch kind. ACP agents become a
/// real stdio subprocess (kill-on-drop replaces the old stop()
/// kill+waitForFinished); the pi agent becomes a node rpc-entry subprocess
/// driven through PiDriver.
pub fn production_spawner() -> Spawner {
    Box::new(|launch, tx| {
        Box::pin(async move {
            match launch {
                Launch::Acp(l) => {
                    let client = AcpClient::spawn(l.spawn, l.start, tx).await?;
                    Ok(Box::new(client) as Box<dyn ClientDriver>)
                }
                Launch::Pi(p) => {
                    let client = crate::chat::pi::PiDriver::spawn(p, tx).await?;
                    Ok(Box::new(client) as Box<dyn ClientDriver>)
                }
                Launch::Devin(d) => {
                    let client = crate::chat::devin::DevinDriver::spawn(d, tx).await?;
                    Ok(Box::new(client) as Box<dyn ClientDriver>)
                }
            }
        })
    })
}
