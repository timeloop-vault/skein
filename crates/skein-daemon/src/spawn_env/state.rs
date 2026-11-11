//! The spawn environment's managed state and its Settings wire shape.

use crate::spawn_settings::SpawnSettings;

/// Everything the spawn path needs to know that isn't per-spawn: the
/// user's environment settings and where they live on disk.
///
/// Held as Tauri managed state rather than passed from the frontend
/// because the probe is kicked off during `setup()`, before a webview
/// exists to be asked — and because a frontend-push design would race
/// room hydration and lose *silently*, spawning harnesses with the
/// wrong environment. See `spawn_settings`.
pub struct SpawnEnvState {
    pub data_dir: std::path::PathBuf,
    pub settings: parking_lot::RwLock<SpawnSettings>,
    /// Set when the settings file existed but couldn't be used, so the
    /// UI can say "your edits aren't in effect" instead of quietly
    /// showing defaults.
    pub degraded: parking_lot::RwLock<Option<String>>,
}

impl SpawnEnvState {
    pub fn snapshot(&self) -> SpawnSettings {
        self.settings.read().clone()
    }
}

/// Wire shape for the Settings "Shell & environment" section.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSettingsPayload {
    settings: SpawnSettings,
    /// Non-null when the settings file exists but couldn't be used.
    degraded: Option<String>,
    /// Shown in the UI so the file can be hand-edited or backed up.
    settings_path: String,
}

pub fn spawn_settings_payload(state: &SpawnEnvState) -> SpawnSettingsPayload {
    SpawnSettingsPayload {
        settings: state.snapshot(),
        degraded: state.degraded.read().clone(),
        settings_path: crate::spawn_settings::file_path(&state.data_dir)
            .display()
            .to_string(),
    }
}

/// User's home directory, cross-platform. Windows keeps it in
/// `USERPROFILE`; Unix in `HOME`. `None` only in exotic environments
/// where neither is set (some CI images / sandboxes). The single
/// source of truth for "where does this tool keep its dotfiles" —
/// reading `HOME` directly is wrong on Windows, where it's usually
/// unset.
pub fn home_dir() -> Option<std::path::PathBuf> {
    skein_harness::home_dir()
}
