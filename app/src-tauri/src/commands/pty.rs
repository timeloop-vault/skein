//! The PTY commands: spawn, write, resize, kill, and the free-port
//! picker opencode's embedded server needs.

use std::path::Path;
use std::sync::Arc;

use tauri::Manager;
use tauri::ipc::Channel;

use super::spawn_env::SpawnEnvState;
use crate::db::Database;
use crate::pty::{PtyEvent, PtyManager};

/// Wire shape for `pty_spawn`'s return value.
///
/// `injected` names whether #215's config injection actually happened
/// for this spawn (a non-empty `Injection`) — the one thing the
/// frontend cannot compute itself, since `injection_for` lives entirely
/// on the Rust side. #238's nudge gate refuses to send a prompt to a
/// harness whose CLI might not even have the review tools wired up.
///
/// `mouse_clicks_disabled` is whether `CLAUDE_CODE_DISABLE_MOUSE_CLICKS`
/// is truthy in the final spawn environment. Only Rust sees that env
/// (login shell, inherited, user extras): the frontend's link-click
/// deferral (#269) must not stand aside when Claude ignores clicks (#401).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySpawnResult {
    id: String,
    injected: bool,
    mouse_clicks_disabled: bool,
}

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
    on_event: Channel<PtyEvent>,
    app: tauri::AppHandle,
) -> Result<PtySpawnResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<PtyManager>();
        let spawn_env = app.state::<SpawnEnvState>();
        let db = app.state::<Arc<Database>>();
        let endpoint = app.state::<crate::agent_api::state::AgentApiEndpoint>();
        let harness_config = app.state::<crate::harness_config::HarnessConfig>();

        let id = uuid::Uuid::new_v4().to_string();
        let settings = spawn_env.snapshot();
        // #213: mint (or reuse) the room's review token and hand the
        // harness its endpoint. A failure here is not a reason to
        // refuse the spawn — the terminal still works, the agent
        // simply has no review tools — but it is logged rather than
        // swallowed (#176).
        let agent = endpoint.mcp_url().and_then(|url| {
            match db.ensure_room_token(&room_id, crate::review::now_ms()) {
                Ok(token) => Some(crate::agent_api::state::HarnessIdentity {
                    url,
                    token,
                    room_id: room_id.clone(),
                    harness_id: harness_id.clone(),
                }),
                Err(e) => {
                    tracing::error!(room_id, error = %e, "agent api: minting a room token failed");
                    None
                }
            }
        });
        let outcome = manager
            .spawn(
                crate::pty::SpawnRequest {
                    id: id.clone(),
                    cmd: &cmd,
                    cwd: Path::new(&cwd),
                    rows,
                    cols,
                    settings: &settings,
                    agent: agent.as_ref(),
                    kind,
                    harness_config: Some(&harness_config),
                },
                move |event| {
                    // Channel send only fails if the frontend dropped
                    // the channel; nothing useful we can do at that
                    // point.
                    let _ = on_event.send(event);
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(PtySpawnResult {
            id,
            injected: outcome.injected,
            mouse_clicks_disabled: outcome.mouse_clicks_disabled,
        })
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
    manager
        .write(&id, data.as_bytes())
        .map_err(|e| e.to_string())
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
pub(crate) fn pty_kill(id: String, manager: tauri::State<'_, PtyManager>) {
    manager.kill(&id);
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

#[cfg(test)]
mod tests {
    use super::PtySpawnResult;

    /// #238: the frontend's `harnessInput` gate reads `injected` off
    /// `pty_spawn`'s resolved value — pin the wire shape so a field
    /// rename here doesn't silently turn into `undefined` there.
    #[test]
    fn pty_spawn_result_serializes_as_camel_case() {
        let json = serde_json::to_value(PtySpawnResult {
            id: "abc".to_owned(),
            injected: true,
            mouse_clicks_disabled: true,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({ "id": "abc", "injected": true, "mouseClicksDisabled": true })
        );
    }
}
