//! The design preview server and its commands (#433). The Tauri-free
//! logic lives in `skein_daemon::design` (#581); this module re-exports it
//! at the old path and holds the `#[tauri::command]` wrappers.

pub mod commands;

pub use skein_daemon::design::*;
