//! The PTY commands: write, resize and the opencode scan. Spawning and
//! killing belong to the daemon's `HarnessRunner` (#731).

use tauri::Manager;

use crate::pty::PtyManager;
use crate::pty::opencode::{OpencodeScanDto, scan_opencode_dto};

/// Forward stdin bytes to the child. `data` is the raw string xterm.js
/// gives us from `term.onData`.
///
/// Deliberately sync (#171): keystroke ordering to a PTY must stay
/// FIFO, and async scheduling could let two writes to the same PTY
/// invert.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn pty_write(
    id: String,
    data: String,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let res = manager
        .write(&id, data.as_bytes())
        .map_err(|e| e.to_string());
    crate::perf_spans::PTY_WRITE.record(t0.elapsed());
    res
}

// Deliberately sync (#171): same FIFO-ordering reasoning as `pty_write`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn pty_resize(
    id: String,
    rows: u16,
    cols: u16,
    manager: tauri::State<'_, PtyManager>,
) -> Result<(), String> {
    manager.resize(&id, rows, cols).map_err(|e| e.to_string())
}

/// Look for an opencode TUI among the descendants of one PTY's child and
/// report the session id and port from its argv (#517). Unknown PTY, or
/// none found, is `Ok(None)`. No caching: the frontend decides when to ask.
/// Async because the process scan takes tens of milliseconds.
#[tauri::command]
pub(crate) async fn pty_scan_opencode(
    id: String,
    app: tauri::AppHandle,
) -> Result<Option<OpencodeScanDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let pid = app.state::<PtyManager>().pid(&id)?;
        scan_opencode_dto(pid)
    })
    .await
    .map_err(|e| e.to_string())
}
