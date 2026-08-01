// Client driver abstraction: the runtime actor drives the ACP client through
// this object-safe trait so tests can substitute AcpClient<MockTransport>
// without touching chat logic. Production wraps AcpClient<StdioTransport>
// (spawned per ensureAcp via `stdio_spawner`).

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::acp::{AcpClient, AcpError, AcpEvent, SpawnConfig, StartParams, Transport};

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

/// Factory producing a started client bound to a fresh event channel. The
/// event receiver side stays with the actor across respawns.
pub type Spawner = Box<
    dyn FnMut(SessionLaunch, mpsc::Sender<AcpEvent>) -> BoxFuture<'static, Result<Box<dyn ClientDriver>, AcpError>>
        + Send,
>;

/// Production spawner: real stdio subprocess (kill-on-drop replaces the old
/// stop() kill+waitForFinished).
pub fn stdio_spawner() -> Spawner {
    Box::new(|launch, tx| {
        Box::pin(async move {
            let client = AcpClient::spawn(launch.spawn, launch.start, tx).await?;
            Ok(Box::new(client) as Box<dyn ClientDriver>)
        })
    })
}
