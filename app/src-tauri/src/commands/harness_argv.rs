//! Argv construction commands (#730): the daemon builds the argv, so the
//! webview no longer does. It picks no opencode port: the spawn does (#754).

use std::sync::Arc;

use skein_daemon::harness_argv::{
    FreshCmdInput, FreshCmdOutput, ResumeOneInput, ResumeOneOutput, ResumeRoomsInput,
    ResumeRoomsOutput, fresh_cmd, resume_one, resume_rooms,
};
use tauri::Manager;

use super::spawn_env::SpawnEnvState;

/// Argv for a brand-new harness; probes the
/// default shell, hence `spawn_blocking`.
#[tauri::command]
pub(crate) async fn harness_fresh_cmd(
    input: FreshCmdInput,
    app: tauri::AppHandle,
) -> Result<FreshCmdOutput, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<Arc<SpawnEnvState>>();
        fresh_cmd(&spawn_env, &input)
    })
    .await
    .map_err(|e| e.to_string())
}

/// One harness rebuilt into resume form (restart).
#[tauri::command]
pub(crate) async fn harness_resume_cmd(input: ResumeOneInput) -> Result<ResumeOneOutput, String> {
    tauri::async_runtime::spawn_blocking(move || resume_one(input))
        .await
        .map_err(|e| e.to_string())
}

/// Rooms rebuilt into resume form (boot, or unarchive).
#[tauri::command]
pub(crate) async fn rooms_resume_cmds(
    input: ResumeRoomsInput,
) -> Result<ResumeRoomsOutput, String> {
    tauri::async_runtime::spawn_blocking(move || resume_rooms(input))
        .await
        .map_err(|e| e.to_string())
}
