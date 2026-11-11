//! The review surface (#212, epic #52). The Tauri-free logic — scope,
//! diff, anchoring, comments, sign-off — lives in
//! `skein_daemon::review_surface` (#581); this module re-exports it at
//! the old path and holds the `#[tauri::command]` wrappers.

// `pub` because `tauri::generate_handler!` resolves each command
// through the module that defines it — it needs the hidden macro the
// attribute generates alongside the function, which a `pub use`
// re-export does not carry.
pub mod commands;

pub use skein_daemon::review_surface::*;
