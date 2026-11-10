//! The harness telemetry attach/detach commands: Claude's JSONL tail
//! and opencode's SSE subscription.

use tauri::Manager;
use tauri::ipc::Channel;

use crate::harness_events_claude::{
    ClaudeEvent, ClaudeEventsManager, ReattachOutcome, claude_events_attach_impl,
};
use crate::harness_events_opencode::{
    OpencodeAttachArgs, OpencodeEvent, OpencodeEventsManager, opencode_events_attach_impl,
};
use crate::opencode_sidecar::OpencodeSidecarManager;
use crate::tauri_stream_sink::TauriChannelSink;

/// Start tailing the Claude JSONL session log for a harness. Emits
/// semantic `ClaudeEvent` values over `on_event` whenever the file
/// grows. Epic #50 L2c-1.
///
/// Purely additive: failing to attach (HOME unset, parent dir
/// unwriteable, watcher init failure) just means the harness falls
/// back to the L2a idle heuristic. We surface the error as a string
/// for the frontend to log, but the frontend treats it as soft —
/// notifications keep working from the chunk-based path.
///
/// Async: on first attach, `ClaudeEventsManager::attach` reads the
/// whole existing JSONL file to derive the current phase and backfill
/// actions (#80) before it starts tailing — for a long-running resumed
/// conversation that is real disk I/O, off the main thread (#171).
/// `ClaudeEventsManager` isn't `Arc`-wrapped, so the blocking closure
/// re-resolves it off an owned `AppHandle`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn claude_events_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    cwd: String,
    fresh_process: bool,
    on_event: Channel<ClaudeEvent>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let log_harness_id = harness_id.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<ClaudeEventsManager>();
        claude_events_attach_impl(
            &manager,
            harness_id,
            room_id,
            &session_id,
            &cwd,
            fresh_process,
            TauriChannelSink::new(on_event),
        )
    })
    .await;

    join_result.unwrap_or_else(|join_error| {
        // The blocking closure panicked — `attach`'s own error
        // path never runs, so without this the frontend just sees
        // its request hang, then error out with no explanation.
        tracing::warn!(
            harness_id = %log_harness_id,
            error = %join_error,
            "claude_events_attach: spawn_blocking join failed (panic inside attach?)"
        );
        Err(join_error.to_string())
    })
}

/// Stop tailing the JSONL for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn claude_events_detach(
    harness_id: String,
    manager: tauri::State<'_, ClaudeEventsManager>,
) {
    manager.detach(&harness_id);
}

/// Manually trigger the #410 dead-tail recovery check for `harness_id`
/// — the same re-arm-then-check the background supervisor runs every
/// `SUPERVISE_INTERVAL`, but on demand and without its automatic-path
/// backoff. Returns which of `ReattachOutcome`'s three strings applies;
/// the frontend (`reattachClaudeTelemetry` in `harnessEvents.ts`)
/// matches on the exact string.
///
/// Modelled on `claude_events_attach` above: `async fn` +
/// `spawn_blocking`, since a dead tail actually being reattached runs a
/// full `attach_at`'s worth of disk I/O (`scan_history` over the whole
/// transcript) — `ClaudeEventsManager` isn't `Arc`-wrapped, so the
/// blocking closure re-resolves it off an owned `AppHandle`, same as
/// `claude_events_attach` does.
#[tauri::command]
pub(crate) async fn claude_events_reattach(
    harness_id: String,
    app: tauri::AppHandle,
) -> Result<ReattachOutcome, String> {
    tracing::info!(harness_id, "claude_events_reattach: called");
    let log_harness_id = harness_id.clone();
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<ClaudeEventsManager>();
        manager.reattach(&harness_id)
    })
    .await;

    match join_result {
        Ok(Ok(outcome)) => {
            tracing::info!(
                harness_id = %log_harness_id,
                ?outcome,
                "claude_events_reattach: done"
            );
            Ok(outcome)
        }
        Ok(Err(e)) => {
            tracing::warn!(
                harness_id = %log_harness_id,
                error = %e,
                "claude_events_reattach: failed"
            );
            Err(e.to_string())
        }
        Err(join_error) => {
            // The blocking closure panicked — mirrors
            // `claude_events_attach`'s identical handling below it.
            tracing::warn!(
                harness_id = %log_harness_id,
                error = %join_error,
                "claude_events_reattach: spawn_blocking join failed (panic inside reattach?)"
            );
            Err(join_error.to_string())
        }
    }
}

/// Start subscribing to opencode's `/event` SSE stream on `127.0.0.1:<port>`.
/// Epic #50 L2c-2.
///
/// Symmetrical with `claude_events_attach`: pass an `on_event` Channel
/// that receives semantic `OpencodeEvent` values. The adapter handles
/// initial-connect race (opencode hasn't bound the port yet) and
/// mid-session disconnects via exponential backoff — see
/// `harness_events_opencode::run_adapter`.
///
/// `async fn` so Tauri runs us on its tokio executor — the manager
/// calls `tokio::spawn` internally to launch the background SSE
/// reader, and that requires a runtime context. The function itself
/// returns immediately; the spawned task lives on until
/// `opencode_events_detach` is called or the manager is dropped.
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
#[tauri::command]
pub(crate) async fn opencode_events_attach(
    harness_id: String,
    room_id: String,
    cwd: String,
    port: u16,
    session_id: Option<String>,
    sidecar_generation: Option<u64>,
    on_event: Channel<OpencodeEvent>,
    manager: tauri::State<'_, OpencodeEventsManager>,
    sidecars: tauri::State<'_, OpencodeSidecarManager>,
) -> Result<(), String> {
    opencode_events_attach_impl(
        &manager,
        &sidecars,
        OpencodeAttachArgs {
            harness_id,
            room_id,
            cwd,
            port,
            session_id,
            sidecar_generation,
        },
        TauriChannelSink::new(on_event),
    )
}

/// Stop the SSE subscription for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn opencode_events_detach(
    harness_id: String,
    manager: tauri::State<'_, OpencodeEventsManager>,
) {
    manager.detach(&harness_id);
}
