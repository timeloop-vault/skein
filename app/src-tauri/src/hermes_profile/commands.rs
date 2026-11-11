//! Tauri commands for Hermes profiles (#626/#627): list them with the
//! state of Skein's MCP entry, preview the edit, and make it with the
//! user's consent. The DTOs and the listing/preview/inject logic live in
//! `skein_daemon::hermes_profile::entries`.

use skein_daemon::hermes_profile::entries::{
    EntryPreviewDto, ProfileListDto, hook_inputs, inject_in, list_profiles_in, preview_in,
    root_or_err,
};
use skein_daemon::hermes_profile::hooks::HookInputs;

/// Hermes profiles and the state of Skein's MCP entry in each.
#[tauri::command]
pub async fn hermes_list_profiles(
    spawn_env: tauri::State<'_, std::sync::Arc<crate::commands::spawn_env::SpawnEnvState>>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<ProfileListDto, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = skein_daemon::hermes_profile::effective_root(&settings);
        let inputs = if root.is_some() {
            hook_inputs(&settings, &config)
        } else {
            HookInputs::default()
        };
        list_profiles_in(root.as_deref(), &inputs)
    })
    .await
    .map_err(|e| e.to_string())
}

/// What adding Skein's entry to `profile` would insert.
#[tauri::command]
pub async fn hermes_profile_entry_preview(
    profile: String,
    spawn_env: tauri::State<'_, std::sync::Arc<crate::commands::spawn_env::SpawnEnvState>>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<EntryPreviewDto, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = root_or_err(&settings)?;
        preview_in(&root, &profile, &hook_inputs(&settings, &config))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn hermes_profile_inject(
    profile: String,
    spawn_env: tauri::State<'_, std::sync::Arc<crate::commands::spawn_env::SpawnEnvState>>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<&'static str, String> {
    let settings = spawn_env.snapshot();
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = root_or_err(&settings)?;
        inject_in(&root, &profile, &hook_inputs(&settings, &config))
    })
    .await
    .map_err(|e| e.to_string())?
}
