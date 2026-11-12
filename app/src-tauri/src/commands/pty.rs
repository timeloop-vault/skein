//! The PTY commands: spawn, write, resize, kill and the opencode scan.

use std::sync::Arc;

use tauri::Manager;
use tauri::ipc::Channel;

use super::spawn_env::SpawnEnvState;
use crate::db::Database;
use crate::pty::command::{PtySpawnArgs, PtySpawnDeps, PtySpawnResult, pty_spawn_impl};
use crate::pty::opencode::{OpencodeScanDto, scan_opencode_dto};
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
    agent: Option<String>,
    session_id: Option<String>,
    on_event: Channel<PtyEvent>,
    app: tauri::AppHandle,
) -> Result<PtySpawnResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<PtyManager>();
        let spawn_env = app.state::<Arc<SpawnEnvState>>();
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
                agent,
                session_id,
            },
            TauriChannelSink::new(on_event),
            move |binding| stop_bound_sidecar(&exit_app, binding),
            &super::opencode_sidecar::sidecar_exit_hook(&app),
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
