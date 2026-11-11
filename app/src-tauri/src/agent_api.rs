//! The agent-facing review API (#213, epic #52 D8). The server, verbs
//! and MCP surface live in `skein_daemon::agent_api` (#582); this module
//! re-exports them at the old path and holds the `#[tauri::command]`
//! wrappers.

// `pub` because `tauri::generate_handler!` resolves each command
// through the module that defines it — it needs the hidden macro the
// attribute generates alongside the function, which a `pub use`
// re-export does not carry.
pub mod commands;

pub use skein_daemon::agent_api::*;
