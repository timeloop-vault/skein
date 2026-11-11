//! Process manager for the per-harness `opencode serve` sidecar (#676). The
//! Tauri-free part moved to `skein_daemon::opencode_sidecar` (#581); only the
//! `tauri::Resource` exit guard stays here.

mod exit_guard;

pub use exit_guard::{SidecarExitGuard, exit_cleanup};
pub use skein_daemon::opencode_sidecar::*;
