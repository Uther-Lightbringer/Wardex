// ACP (Agent Client Protocol) client: JSON-RPC 2.0 over stdio, NDJSON framing.
// Ported from src/AcpClient.cpp of the C++/Qt codebase.
//
// Windows-specific behaviors that MUST be preserved (compatibility with
// kimi acp / claude-code-acp / codex-acp):
// - .cmd/.bat shims are wrapped in `cmd.exe /c`
// - env overrides: a null value DELETES the variable (clearEnvs anti-nesting)
// - session/load replay notifications are discarded (local history is authoritative)
// - prompt error path order: protocolError -> messageChunk -> turnFinished("error")
//   (the chat layer's rate-limit detection depends on this order)
//
// Layout: `transport` owns the subprocess and NDJSON framing behind the
// `Transport` trait; `client` is the transport-agnostic protocol state
// machine; `types` holds the wire shapes and pure helpers; `events` is the
// mpsc-carried event enum heading to the chat layer.

pub mod client;
pub mod events;
pub mod transport;
pub mod types;

pub use client::{AcpClient, StartParams};
pub use events::AcpEvent;
pub use transport::{AcpError, LineFramer, MockTransport, SpawnConfig, StdioTransport, Transport};
