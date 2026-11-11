//! Hermes profiles for Skein (#626/#627). The Tauri-free logic moved to
//! `skein_daemon::hermes_profile` (#581); the `#[tauri::command]`s stay here.

pub(crate) use skein_daemon::hermes_profile::*;

pub(crate) mod commands;
