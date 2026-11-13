//! The client-only inputs to the daemon's shadow phase machine (#732).

use std::sync::Arc;

use skein_daemon::harness_phase::{ClientPhaseInput, PhaseService};

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
    phase: tauri::State<'_, Arc<PhaseService>>,
) -> Result<(), String> {
    phase.client_input(&harness_id, input);
    Ok(())
}
