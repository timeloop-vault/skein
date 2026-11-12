//! The app-side exit hook for a harness's `opencode serve` sidecar (#676).
//! The daemon starts the sidecar inside `pty_spawn` (#730); this file only
//! supplies what happens when it dies.

use std::sync::Arc;

use tauri::{Emitter, Manager};

use crate::opencode_sidecar::OnExit;
use crate::opencode_sidecar::start::{EXIT_EVENT, SidecarExitPayload};

/// What happens when a sidecar exits on its own: tell the frontend, then
/// end the bound TUI, which would otherwise reconnect forever to a dead
/// server. Used by the daemon-prepared `pty_spawn`.
pub(crate) fn sidecar_exit_hook(app: &tauri::AppHandle) -> OnExit {
    let exit_app = app.clone();
    Arc::new(move |notice| {
        tracing::info!(
            harness = %notice.harness_id,
            generation = notice.generation,
            code = ?notice.code,
            "opencode sidecar exited"
        );
        let (harness_id, generation) = (notice.harness_id.clone(), notice.generation);
        // Event first, so the frontend knows why the TUI died when the
        // PTY's own Exit arrives.
        let _ = exit_app.emit(EXIT_EVENT, SidecarExitPayload::from(notice));
        let ended = exit_app
            .state::<crate::pty::PtyManager>()
            .terminate_bound(&harness_id, generation);
        if ended > 0 {
            tracing::info!(harness = %harness_id, generation, ended, "terminated TUI of a dead sidecar");
        }
    })
}
