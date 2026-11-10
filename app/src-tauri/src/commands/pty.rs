//! The PTY commands: spawn, write, resize, kill, and the free-port
//! picker opencode's embedded server needs.

use std::sync::Arc;

use tauri::Manager;
use tauri::ipc::Channel;

use super::spawn_env::SpawnEnvState;
use crate::db::Database;
use crate::pty::command::{PtySpawnArgs, PtySpawnDeps, PtySpawnResult, pty_spawn_impl};
use crate::pty::{PtyEvent, PtyManager};
use crate::tauri_stream_sink::TauriChannelSink;

/// Spawn a child process attached to a fresh PTY and stream its output
/// over `on_output`. Returns an opaque id the frontend uses for follow-up
/// calls, plus whether #215's config injection happened for this spawn.
///
/// `cmd` is argv-style: the first element is the program, the rest are
/// arguments. Empty `cmd` is rejected. `cwd` must exist.
///
/// `kind` is the harness kind, which decides what #215 injects so the
/// agent can reach the review API. It is passed separately from `cmd`
/// because the two can legitimately disagree — the user can swap a
/// harness's command without changing what the harness is. Tauri
/// deserializes it as `HarnessKind` (#116), so an unknown kind string
/// fails the invoke outright rather than silently injecting nothing.
///
/// Async: `PtyManager::spawn` calls into `apply_env`, which reads the
/// login-shell probe result and can block waiting on it for up to
/// `PROBE_WAIT` (6 s, #171/#177) — plus the actual child-process spawn.
/// None of the managed state here is `Arc`-wrapped, so the blocking
/// closure re-resolves each one off an owned `AppHandle` instead of
/// trying to move a borrowed `State` into a `'static` closure.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn pty_spawn(
    cmd: Vec<String>,
    cwd: String,
    rows: u16,
    cols: u16,
    room_id: String,
    harness_id: String,
    kind: crate::harness_kind::HarnessKind,
    env_profile: Option<String>,
    sidecar_generation: Option<u64>,
    on_event: Channel<PtyEvent>,
    app: tauri::AppHandle,
) -> Result<PtySpawnResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<PtyManager>();
        let spawn_env = app.state::<SpawnEnvState>();
        let db = app.state::<Arc<Database>>();
        let endpoint = app.state::<crate::agent_api::state::AgentApiEndpoint>();
        let harness_config = app.state::<crate::harness_config::HarnessConfig>();
        let sidecars = app.state::<crate::opencode_sidecar::OpencodeSidecarManager>();
        let exit_app = app.clone();
        pty_spawn_impl(
            PtySpawnDeps {
                manager: &manager,
                spawn_env: &spawn_env,
                db: &db,
                endpoint: &endpoint,
                harness_config: &harness_config,
                sidecars: &sidecars,
            },
            PtySpawnArgs {
                cmd,
                cwd,
                rows,
                cols,
                room_id,
                harness_id,
                kind,
                env_profile,
                sidecar_generation,
            },
            TauriChannelSink::new(on_event),
            move |binding| stop_bound_sidecar(&exit_app, binding),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

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

// Deliberately sync (#171): same FIFO-ordering reasoning as `pty_write`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn pty_kill(
    id: String,
    manager: tauri::State<'_, PtyManager>,
    sidecars: tauri::State<'_, crate::opencode_sidecar::OpencodeSidecarManager>,
) {
    if let Some((harness_id, generation)) = manager.kill(&id) {
        sidecars.stop_if_generation(&harness_id, generation);
    }
}

/// End the sidecar generation a PTY was bound to (#676). Generation-keyed,
/// so a late exit of an old PTY cannot stop a restart's newer sidecar.
fn stop_bound_sidecar(app: &tauri::AppHandle, binding: &crate::pty::SidecarBinding) {
    let (harness_id, generation) = binding;
    let sidecars = app.state::<crate::opencode_sidecar::OpencodeSidecarManager>();
    if sidecars.stop_if_generation(harness_id, *generation) {
        tracing::info!(harness = %harness_id, generation, "opencode sidecar stopped with its PTY");
    }
}

/// Wire shape for `pty_scan_opencode` (#517).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpencodeScanDto {
    pid: u32,
    session_id: Option<String>,
    port: Option<u16>,
    port_confirmed: bool,
    continue_last: bool,
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
        crate::pty::procscan::scan_opencode(pid).map(|f| OpencodeScanDto {
            pid: f.pid,
            session_id: f.session_id,
            port: f.port,
            port_confirmed: f.port_confirmed,
            continue_last: f.continue_last,
        })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Allocate a free TCP port on `127.0.0.1` for opencode's embedded
/// HTTP server. Epic #50 L2c-2.
///
/// Implementation: bind a `TcpListener` to port 0, read the OS-assigned
/// port, drop the listener. There's a small race window between drop
/// and the next process binding it (~microseconds typically), but it
/// only matters if another process simultaneously asks for a free
/// port and beats opencode to bind. In practice we've never observed
/// it; if it ever happens opencode reports the bind error in the PTY
/// and the user can restart the harness.
#[tauri::command]
pub(crate) fn pick_free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("pick_free_port: {e}"))
}
