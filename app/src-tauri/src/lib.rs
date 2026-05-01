//! Skein — Tauri shell entrypoint.
//!
//! Phase 1 wires PTY commands so the frontend can spawn a real
//! interactive terminal inside the harness pane. Each spawn produces
//! an id; subsequent writes/resizes/kills are keyed by it. Output
//! streams back over a per-spawn `tauri::ipc::Channel<String>`.

mod pty;

use std::path::Path;

use tauri::Manager;
use tauri::ipc::Channel;

use crate::pty::PtyManager;

/// Boots the Tauri runtime and blocks until the main window closes.
///
/// # Panics
///
/// Panics if the embedded Tauri context fails to build (missing config,
/// invalid capabilities, missing icon assets) — i.e. only on packaging
/// errors that would prevent the app from ever starting.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(PtyManager::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            default_shell,
            default_cwd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Smoke-test command — useful while wiring up the front/back bridge.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn ping(message: String) -> String {
    format!("pong: {message}")
}

/// Spawn a child process attached to a fresh PTY and stream its output
/// over `on_output`. Returns an opaque id the frontend uses for follow-up
/// calls.
///
/// `cmd` is argv-style: the first element is the program, the rest are
/// arguments. Empty `cmd` is rejected. `cwd` must exist.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_spawn(
    cmd: Vec<String>,
    cwd: String,
    rows: u16,
    cols: u16,
    on_output: Channel<String>,
    manager: tauri::State<'_, PtyManager>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    manager
        .spawn(
            id.clone(),
            &cmd,
            Path::new(&cwd),
            rows,
            cols,
            move |chunk| {
                // Channel send only fails if the frontend dropped the
                // channel; nothing useful we can do at that point.
                let _ = on_output.send(chunk);
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Forward stdin bytes to the child. `data` is the raw string xterm.js
/// gives us from `term.onData`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_write(
    id: String,
    data: String,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    manager
        .write(&id, data.as_bytes())
        .map_err(|e| e.to_string())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_resize(
    id: String,
    rows: u16,
    cols: u16,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    manager.resize(&id, rows, cols).map_err(|e| e.to_string())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
fn pty_kill(id: String, manager: tauri::State<'_, PtyManager>) {
    manager.kill(&id);
}

/// argv for the user's default interactive shell on this platform. Used
/// as the fallback when the new-harness picker doesn't have a more
/// specific binary in mind.
#[tauri::command]
fn default_shell() -> Vec<String> {
    if cfg!(windows) {
        // pwsh is available on most modern Windows installs (winget
        // bundles it). If it's missing, the spawn errors and the user
        // sees it in the terminal pane — they can pick another shell.
        vec!["pwsh.exe".into()]
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        vec![shell]
    }
}

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[tauri::command]
fn default_cwd() -> String {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(key)
        .ok()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
        })
        .unwrap_or_else(|| ".".into())
}
