//! The client-only inputs to the daemon's phase machine (#732).

use std::sync::Arc;

use skein_daemon::harness_phase::{ClientPhaseInput, PhaseService, PhaseUpdate};
use skein_daemon::harness_runner::HarnessRunner;

/// One input only the webview can observe (keystrokes, the rendered Hermes
/// screen, a seam-submitted prompt). Fire-and-forget: a short mutex section.
///
/// `async` so it runs off the main thread (a sync command runs on it, once per
/// keystroke); Tauri requires a `Result` from an async command that borrows
/// its state. The section is short, so no `spawn_blocking`.
#[allow(clippy::needless_pass_by_value, clippy::unused_async)]
#[tauri::command]
pub(crate) async fn harness_phase_input(
    harness_id: String,
    input: ClientPhaseInput,
    runner: tauri::State<'_, Arc<HarnessRunner>>,
) -> Result<(), String> {
    runner.client_phase_input(&harness_id, input);
    Ok(())
}

/// Every known harness's current phase state, each at the stream's current
/// seq. A client listens to `skein://harness-phase` first, then reads this,
/// and drops stream updates whose `seq` is not above the snapshot's.
#[allow(clippy::unused_async)]
#[tauri::command]
pub(crate) async fn harness_phase_snapshot(
    phase: tauri::State<'_, Arc<PhaseService>>,
) -> Result<Vec<PhaseUpdate>, String> {
    Ok(phase.stream_snapshot())
}
