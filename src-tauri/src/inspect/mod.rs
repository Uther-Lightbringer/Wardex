// Info-panel data domains (docs/panels.md §1.3): one module per information
// domain surfaced in the chat page's right-hand panel dock. Each module
// exposes its own Tauri commands; no shared trait abstraction until 3+
// similar domains exist.

pub mod files;
pub mod git;
pub mod subagent;
