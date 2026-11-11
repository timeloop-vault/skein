//! The review baseline (#211). The Tauri-free logic lives in
//! `skein_daemon::review` (#581); this module re-exports it at the old
//! path and holds the `#[tauri::command]` wrappers.

pub mod commands;

pub use skein_daemon::review::*;
