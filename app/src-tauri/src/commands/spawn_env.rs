//! The spawn environment: managed state, the Settings "Shell &
//! environment" commands, and the small platform lookups (default
//! shell, home, cwd) a harness spawn starts from.

use std::sync::Arc;

use tauri::Manager;

use crate::spawn_settings::SpawnSettings;
use skein_daemon::spawn_env::default_agents::{
    DefaultAgentsImport, default_agent_set_impl, default_agents_import_impl,
};
use skein_daemon::spawn_env::lookups::{
    OpencodeVersionDto, claude_cli_version_impl, default_cwd_impl, default_shell_impl,
    opencode_cli_version_impl, spawn_settings_save_impl,
};

pub(crate) use skein_daemon::spawn_env::state::{
    SpawnEnvState, SpawnSettingsPayload, home_dir, spawn_settings_payload,
};

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn spawn_settings_load(
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
) -> SpawnSettingsPayload {
    spawn_settings_payload(&spawn_env)
}

/// Persist the environment settings and immediately re-probe.
///
/// Re-probing here rather than lazily matters: the shell the user just
/// picked is the shell whose `PATH` the next harness should get, and a
/// spawn racing the save would otherwise keep the old shell's answer for
/// the rest of the process's life.
///
/// Note the two different lifetimes, which the UI states explicitly:
/// `PATH` additions and the captured environment apply to the *next
/// spawn of any harness*, whereas the shell is baked into `Harness.cmd`
/// when a shell harness is created and persisted from there — so
/// existing harnesses are never rewritten behind the user's back.
///
/// Async: writes the settings file to disk (#171). A `State` borrow
/// can't move into a `'static` closure, so the blocking closure
/// re-resolves the managed `Arc` off an owned `AppHandle`.
#[tauri::command]
pub(crate) async fn spawn_settings_save(
    settings: SpawnSettings,
    app: tauri::AppHandle,
) -> Result<SpawnSettingsPayload, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<Arc<SpawnEnvState>>();
        spawn_settings_save_impl(&spawn_env, settings)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One-time, non-overwriting import of the old frontend `defaultAgents`
/// pref into settings.json (#730). Returns `{imported, defaultAgents}`.
#[tauri::command]
pub(crate) async fn default_agents_import(
    default_agents: std::collections::BTreeMap<String, String>,
    app: tauri::AppHandle,
) -> Result<DefaultAgentsImport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<Arc<SpawnEnvState>>();
        default_agents_import_impl(&spawn_env, default_agents)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Set (or, with a null / blank `agent`, clear) one kind's default agent;
/// returns the whole new `{kind: agent}` map (#730).
#[tauri::command]
pub(crate) async fn default_agent_set(
    kind: crate::harness_kind::HarnessKind,
    agent: Option<String>,
    app: tauri::AppHandle,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<Arc<SpawnEnvState>>();
        default_agent_set_impl(&spawn_env, kind, agent.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The environment a harness would get if it spawned right now.
///
/// Built by the *same* code the spawn path runs, so the panel can't
/// drift from reality.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn spawn_env_preview(
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
) -> crate::pty::EnvPreview {
    crate::pty::env_preview(&spawn_env.snapshot())
}

/// Which agents `kind` will accept at `--agent` for a harness spawned
/// in `cwd` (#246). Answers the picker in #247.
///
/// `async` deliberately: this shells out to the harness CLI, so a sync
/// command would run the subprocess on the main thread and stall the
/// event loop for as long as the CLI takes to start (#171). Never
/// `Err` for "found nothing" — the DTO carries `degraded` and
/// `unsupported` instead, because a picker needs a list plus a reason
/// rather than a failure (#176).
///
/// The body then goes to `spawn_blocking`, because `async` alone only
/// moves the block off the main thread and onto a tokio worker. #247
/// made this a per-spawn call rather than a per-picker-open one, so a
/// boot that restores several agent-bearing harnesses fires several at
/// once — and the axum agent API (#213) runs on those same workers.
///
/// `kind` deserializes as `HarnessKind` (#116): an unknown kind string
/// fails the invoke instead of falling through to `unsupported`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) async fn list_harness_agents(
    kind: crate::harness_kind::HarnessKind,
    cwd: String,
    check: Option<String>,
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<skein_daemon::harness_argv::AgentListReply, String> {
    let settings = spawn_env.snapshot();
    // Cloned rather than borrowed: the closure outlives this frame as
    // far as the compiler is concerned, and the config is a handful of
    // paths.
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let listing = crate::agents::list(kind, &cwd, &settings, Some(&config));
        skein_daemon::harness_argv::with_verdict(listing, check.as_deref())
    })
    .await
    .map_err(|e| format!("agent discovery panicked: {e}"))
}

/// The installed Claude Code version, from `claude --version` (#491).
/// Resolves the program exactly like `list_harness_agents`. Anything that
/// stops an answer (not installed, non-zero exit, unrecognised output,
/// the 10 s timeout) is `Ok(None)`: the caller just has no version to
/// compare, which is not an error worth a toast.
#[tauri::command]
pub(crate) async fn claude_cli_version(
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
) -> Result<Option<String>, String> {
    let settings = spawn_env.snapshot();
    tauri::async_runtime::spawn_blocking(move || claude_cli_version_impl(&settings))
        .await
        .map_err(|e| format!("claude version probe panicked: {e}"))
}

/// The major opencode version a spawn would run (#675), from
/// `opencode --version`, cached per (path, size, mtime) because the v2
/// binary is huge. `Ok(None)` = unknown; the caller treats it as v1.
#[tauri::command]
pub(crate) async fn opencode_cli_version(
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
) -> Result<Option<OpencodeVersionDto>, String> {
    let settings = spawn_env.snapshot();
    tauri::async_runtime::spawn_blocking(move || opencode_cli_version_impl(&settings))
        .await
        .map_err(|e| format!("opencode version probe panicked: {e}"))
}

/// What Skein injects into each agent CLI so it can reach the review
/// API, and where the shipped bundle resolved to (#215 E3). Read by the
/// spawn-environment panel — the injection is additive, but the user
/// still gets to see it and switch it off.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn harness_config_status(
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
    spawn_env: tauri::State<'_, Arc<SpawnEnvState>>,
) -> crate::harness_config::HarnessConfigStatus {
    harness_config.status(crate::hermes_profile::effective_root(&spawn_env.snapshot()))
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn spawn_env_reprobe(spawn_env: tauri::State<'_, Arc<SpawnEnvState>>) {
    crate::pty::reprobe(spawn_env.data_dir.clone(), spawn_env.snapshot());
}

/// argv for the user's default interactive shell on this platform. Used
/// as the fallback when the new-harness picker doesn't have a more
/// specific binary in mind. See `skein_daemon::spawn_env::lookups`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn default_shell(spawn_env: tauri::State<'_, Arc<SpawnEnvState>>) -> Vec<String> {
    default_shell_impl(&spawn_env)
}

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[tauri::command]
pub(crate) fn default_cwd() -> String {
    default_cwd_impl()
}
