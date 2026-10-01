//! The spawn environment: managed state, the Settings "Shell &
//! environment" commands, and the small platform lookups (default
//! shell, home, cwd) a harness spawn starts from.

use tauri::Manager;

use crate::spawn_settings::SpawnSettings;

/// Everything the spawn path needs to know that isn't per-spawn: the
/// user's environment settings and where they live on disk.
///
/// Held as Tauri managed state rather than passed from the frontend
/// because the probe is kicked off during `setup()`, before a webview
/// exists to be asked — and because a frontend-push design would race
/// room hydration and lose *silently*, spawning harnesses with the
/// wrong environment. See `spawn_settings`.
pub(crate) struct SpawnEnvState {
    pub(crate) data_dir: std::path::PathBuf,
    pub(crate) settings: parking_lot::RwLock<SpawnSettings>,
    /// Set when the settings file existed but couldn't be used, so the
    /// UI can say "your edits aren't in effect" instead of quietly
    /// showing defaults.
    pub(crate) degraded: parking_lot::RwLock<Option<String>>,
}

impl SpawnEnvState {
    pub(crate) fn snapshot(&self) -> SpawnSettings {
        self.settings.read().clone()
    }
}

/// Wire shape for the Settings "Shell & environment" section.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpawnSettingsPayload {
    settings: SpawnSettings,
    /// Non-null when the settings file exists but couldn't be used.
    degraded: Option<String>,
    /// Shown in the UI so the file can be hand-edited or backed up.
    settings_path: String,
}

pub(crate) fn spawn_settings_payload(state: &SpawnEnvState) -> SpawnSettingsPayload {
    SpawnSettingsPayload {
        settings: state.snapshot(),
        degraded: state.degraded.read().clone(),
        settings_path: crate::spawn_settings::file_path(&state.data_dir)
            .display()
            .to_string(),
    }
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn spawn_settings_load(
    spawn_env: tauri::State<'_, SpawnEnvState>,
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
/// Async: writes the settings file to disk (#171). `SpawnEnvState`
/// isn't `Arc`-wrapped, so the blocking closure re-resolves it off an
/// owned `AppHandle` rather than trying to move the `State` borrow
/// into a `'static` closure.
#[tauri::command]
pub(crate) async fn spawn_settings_save(
    settings: SpawnSettings,
    app: tauri::AppHandle,
) -> Result<SpawnSettingsPayload, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spawn_env = app.state::<SpawnEnvState>();
        crate::spawn_settings::save(&spawn_env.data_dir, &settings)?;
        *spawn_env.settings.write() = settings.clone();
        // A successful save replaces whatever was unreadable before.
        *spawn_env.degraded.write() = None;
        crate::pty::reprobe(spawn_env.data_dir.clone(), settings);
        Ok(spawn_settings_payload(&spawn_env))
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
    spawn_env: tauri::State<'_, SpawnEnvState>,
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
    spawn_env: tauri::State<'_, SpawnEnvState>,
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> Result<crate::agents::AgentListDto, String> {
    let settings = spawn_env.snapshot();
    // Cloned rather than borrowed: the closure outlives this frame as
    // far as the compiler is concerned, and the config is a handful of
    // paths.
    let config = harness_config.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::agents::list(kind, &cwd, &settings, Some(&config))
    })
    .await
    .map_err(|e| format!("agent discovery panicked: {e}"))
}

/// What Skein injects into each agent CLI so it can reach the review
/// API, and where the shipped bundle resolved to (#215 E3). Read by the
/// spawn-environment panel — the injection is additive, but the user
/// still gets to see it and switch it off.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn harness_config_status(
    harness_config: tauri::State<'_, crate::harness_config::HarnessConfig>,
) -> crate::harness_config::HarnessConfigStatus {
    harness_config.status()
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn spawn_env_reprobe(spawn_env: tauri::State<'_, SpawnEnvState>) {
    crate::pty::reprobe(spawn_env.data_dir.clone(), spawn_env.snapshot());
}

/// argv for the user's default interactive shell on this platform. Used
/// as the fallback when the new-harness picker doesn't have a more
/// specific binary in mind.
///
/// On Windows we prefer `pwsh.exe` (`PowerShell` 7) when it's on PATH —
/// it has better ANSI/UTF-8 handling — and fall back to `powershell.exe`
/// (`PowerShell` 5.1, which ships with every modern Windows install).
///
/// On Unix this delegates to the same resolution the `PATH` probe uses,
/// so a harness shell and the shell we asked for a `PATH` can never
/// disagree. It previously had its own copy that fell back to
/// `/bin/bash` when `$SHELL` was unset — the fix for that landed in the
/// probe in 2026-05 and was never applied here, leaving a bundled-app
/// shell harness reading `~/.bash_profile`. On a machine whose Homebrew
/// setup lives in `~/.zprofile` (the default `brew shellenv` install),
/// that yields a shell with no `/opt/homebrew/bin` at all.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn default_shell(spawn_env: tauri::State<'_, SpawnEnvState>) -> Vec<String> {
    #[cfg(windows)]
    {
        if let Some(shell) = spawn_env.snapshot().valid_shell() {
            return vec![shell.to_owned()];
        }
        let pwsh_on_path = std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|dir| dir.join("pwsh.exe").is_file()));
        if pwsh_on_path {
            vec!["pwsh.exe".into()]
        } else {
            vec!["powershell.exe".into()]
        }
    }
    #[cfg(not(windows))]
    {
        // `probe_shell` already prefers the configured shell, so a
        // harness shell and the shell we asked for a PATH can never
        // disagree.
        vec![crate::pty::probe_shell(&spawn_env.snapshot())]
    }
}

/// User's home directory, cross-platform. Windows keeps it in
/// `USERPROFILE`; Unix in `HOME`. `None` only in exotic environments
/// where neither is set (some CI images / sandboxes). The single
/// source of truth for "where does this tool keep its dotfiles" —
/// reading `HOME` directly is wrong on Windows, where it's usually
/// unset.
pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    skein_harness::home_dir()
}

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[tauri::command]
pub(crate) fn default_cwd() -> String {
    home_dir()
        .and_then(|p| p.to_str().map(str::to_owned))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
        })
        .unwrap_or_else(|| ".".into())
}
