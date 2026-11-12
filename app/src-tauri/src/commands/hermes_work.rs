//! The Hermes work poller's attach/detach commands (#628). The poller
//! itself lives in `harness_work_hermes`; this file only turns the
//! frontend's channel into a sink.

use std::sync::Arc;

use crate::commands::spawn_env::SpawnEnvState;
use crate::harness_runner::HarnessRunner;
use crate::harness_work_hermes::{HermesWorkEvent, HermesWorkManager, hermes_work_attach_impl};
use crate::tauri_stream_sink::TauriChannelSink;
use skein_daemon::harness_phase::sinks::Tee;

/// Start (or replace) the work poller for a Hermes harness; the attach
/// snapshot arrives as `initial: true` starts on the first tick.
#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
pub(crate) async fn hermes_work_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    hermes_profile: Option<String>,
    resumed: Option<bool>,
    on_event: tauri::ipc::Channel<HermesWorkEvent>,
    manager: tauri::State<'_, HermesWorkManager>,
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
    runner: tauri::State<'_, Arc<HarnessRunner>>,
) -> Result<(), String> {
    let _ = (room_id, resumed); // Same arguments as hermes_actions_attach.
    // #732 shadow: `Tee` returns the client sink's result, so a dead client
    // still stops the poller.
    let phase_sink = runner.hermes_work_phase_sink(&harness_id);
    hermes_work_attach_impl(
        &manager,
        &spawn_env.snapshot(),
        harness_id,
        session_id,
        hermes_profile.as_deref(),
        Box::new(Tee::new(TauriChannelSink::new(on_event), phase_sink)),
    )
}

/// Stop the poller for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn hermes_work_detach(harness_id: String, manager: tauri::State<'_, HermesWorkManager>) {
    manager.detach(&harness_id);
}
