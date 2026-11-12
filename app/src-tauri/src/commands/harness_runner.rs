//! The commands over the daemon's `HarnessRunner` (#731): the client asks
//! for a harness to be running and attaches its terminal; it does not spawn
//! or kill anything itself.

use std::sync::Arc;

use serde::Serialize;
use tauri::Manager;
use tauri::ipc::Channel;

use crate::harness_runner::{EnsureError, EnsureOutcome, EnsureRequest, HarnessRunner, Refusal};
use crate::pty::PtyEvent;
use crate::pty::command::PtySpawnResult;
use crate::tauri_stream_sink::TauriChannelSink;

/// What `harness_ensure_running` returns: the spawn result the client used to
/// get from `pty_spawn`, plus whether a process was started and whether it
/// has already exited.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnsureRunningDto {
    /// False when an existing process was reused.
    started: bool,
    #[serde(flatten)]
    spawn: PtySpawnResult,
    /// The process has exited (its `Exit` event also arrives on the channel
    /// when it exited before the attach).
    exited: bool,
    exit_code: Option<u32>,
    /// Pass back to `harness_detach` so a late detach from an old view
    /// cannot clear a newer view's sink.
    attach_id: Option<u64>,
}

impl From<EnsureOutcome> for EnsureRunningDto {
    fn from(o: EnsureOutcome) -> Self {
        Self {
            started: o.started,
            spawn: o.spawn,
            exited: o.exit.is_some(),
            exit_code: o.exit.and_then(|e| e.code),
            attach_id: o.attach_id,
        }
    }
}

/// `<code>: <sentence>`, so the client can branch on the code.
fn refusal_message(refusal: Refusal) -> String {
    let sentence = match refusal {
        Refusal::UnknownHarness => "no such harness in that room",
        Refusal::RoomArchived => "the room is archived",
        Refusal::NoPty => "this harness kind has no terminal",
        Refusal::NoCmd => "the harness has no command to run",
        Refusal::FolderMissing => "the room's folder does not exist",
        Refusal::RepoMismatch => "the folder holds a different repository than the room recorded",
        Refusal::StaleRecord => "the harness's command changed since the caller read it",
    };
    format!("{}: {sentence}", refusal.code())
}

fn error_message(e: &EnsureError) -> String {
    match e {
        EnsureError::Refused(r) => refusal_message(*r),
        EnsureError::SpawnFailed(_) => e.to_string(),
    }
}

/// Make sure the harness has a process, spawning one from its stored record
/// when it has none, and attach `on_event` as its terminal output.
///
/// Async + `spawn_blocking`: the spawn probes the CLI, may start a sidecar
/// and reads the Claude transcript. The errors are `<code>: <sentence>`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn harness_ensure_running(
    room_id: String,
    harness_id: String,
    rows: u16,
    cols: u16,
    env_profile: Option<String>,
    restart: bool,
    expect_cmd: Option<Vec<String>>,
    on_event: Channel<PtyEvent>,
    app: tauri::AppHandle,
) -> Result<EnsureRunningDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runner = Arc::clone(&app.state::<Arc<HarnessRunner>>());
        runner
            .ensure_running(
                &EnsureRequest {
                    room_id,
                    harness_id,
                    rows,
                    cols,
                    env_profile,
                    restart,
                    expect_cmd,
                },
                Some(Box::new(TauriChannelSink::new(on_event))),
            )
            .map(EnsureRunningDto::from)
            .map_err(|e| error_message(&e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Kill the harness's process and forget it. Idempotent; `true` when the
/// runner had it. Blocking (a kill), so off the executor.
#[tauri::command]
pub(crate) async fn harness_stop(
    harness_id: String,
    runner: tauri::State<'_, Arc<HarnessRunner>>,
) -> Result<bool, String> {
    let runner = Arc::clone(&runner);
    tauri::async_runtime::spawn_blocking(move || runner.stop_harness(&harness_id))
        .await
        .map_err(|e| e.to_string())
}

/// The view went away: clear the terminal's sink. Never kills. `true` when a
/// sink was cleared. With `attach_id` (from `harness_ensure_running`) it only
/// clears that attachment, so a late detach cannot blank a newer view.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn harness_detach(
    harness_id: String,
    attach_id: Option<u64>,
    runner: tauri::State<'_, Arc<HarnessRunner>>,
) -> bool {
    match attach_id {
        Some(id) => runner.detach_view_if(&harness_id, id),
        None => runner.detach_view(&harness_id),
    }
}
