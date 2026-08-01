// Session runtime management: stream coalescing (50ms flush, 250ms under >64KB
// backlog), queue cap 10, max 3 parallel ACP processes, interrupted-turn resume,
// rate-limit retry with 20/40/80s backoff, subagent tracking.
// Ported from src/ChatController.cpp.
//
// Memory design (fixes P2/P3 from memory-audit-streaming.md):
// - segments are the single source of truth; no duplicate content/thinking copies
// - tool payloads truncated to 64KB in memory (full payload only on disk)
// - opened session models are LRU-evicted; only ≤3 ACP processes stay alive
//
// Layout: `driver` abstracts the ACP client behind an object-safe trait (tests
// inject MockTransport); `runtime` is the per-session actor (turn state
// machine, coalescing flush, retry/resume/subagents); `manager` is the
// HashMap<sessionId, Runtime> plus session-lifecycle entry points the Tauri
// command layer calls.

pub mod driver;
pub mod manager;
pub mod runtime;
pub mod wire;

#[cfg(test)]
mod tests;

pub use manager::{ChatError, ChatManager, SpawnerFactory};
pub use runtime::{EventSink, RuntimeSnap, SubagentEntry};
