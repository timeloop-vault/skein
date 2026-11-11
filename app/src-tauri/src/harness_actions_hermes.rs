//! The Tauri command wrappers over the Hermes `state.db` tail. The tail
//! itself (and its tests) live in `skein_daemon::harness_actions_hermes`.

pub(crate) use skein_daemon::harness_actions_hermes::*;

/// Start (or replace) the state.db tail for a Hermes harness. The same
/// session again is a no-op; a different one replaces the tail.
/// `resumed`: the session already had spend, so per-turn cost starts from
/// its current totals instead of zero (unless a baseline was recorded).
#[tauri::command]
pub(crate) async fn hermes_actions_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    hermes_profile: Option<String>,
    resumed: Option<bool>,
    manager: tauri::State<'_, HermesActionsManager>,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<(), String> {
    let settings = spawn_env.snapshot();
    let state_db = crate::resume::profile_db(&settings, hermes_profile.as_deref())
        .ok_or_else(|| "no Hermes home or invalid profile".to_string())?;
    manager
        .attach(
            harness_id,
            room_id,
            session_id,
            state_db,
            resumed.unwrap_or(false),
        )
        .await
}

/// Stop the tail for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn hermes_actions_detach(
    harness_id: String,
    manager: tauri::State<'_, HermesActionsManager>,
) {
    manager.detach(&harness_id);
}

/// A real end of turn: record the turn's cost. `true` when a `turn_cost`
/// row was recorded; `false` for no tail, no session row or no growth.
#[tauri::command]
pub(crate) async fn hermes_actions_turn_end(
    harness_id: String,
    manager: tauri::State<'_, HermesActionsManager>,
) -> Result<bool, String> {
    let Some(tail) = manager.tail(&harness_id) else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || tail.turn_end())
        .await
        .map_err(|e| e.to_string())?
}
