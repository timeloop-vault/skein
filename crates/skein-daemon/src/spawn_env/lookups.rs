//! The Tauri-free bodies of the spawn-environment commands: the CLI
//! version probes, the settings save and the default shell / cwd lookups.

use super::state::{SpawnEnvState, SpawnSettingsPayload, home_dir, spawn_settings_payload};
use crate::spawn_settings::SpawnSettings;

/// Persist the environment settings and immediately re-probe. Blocks on
/// the settings file write.
///
/// # Errors
/// The settings file could not be written.
pub fn spawn_settings_save_impl(
    spawn_env: &SpawnEnvState,
    settings: SpawnSettings,
) -> Result<SpawnSettingsPayload, String> {
    crate::spawn_settings::save(&spawn_env.data_dir, &settings)?;
    *spawn_env.settings.write() = settings.clone();
    // A successful save replaces whatever was unreadable before.
    *spawn_env.degraded.write() = None;
    crate::pty::reprobe(spawn_env.data_dir.clone(), settings);
    Ok(spawn_settings_payload(spawn_env))
}

/// The installed Claude Code version; `None` when anything stops an answer.
#[must_use]
pub fn claude_cli_version_impl(settings: &SpawnSettings) -> Option<String> {
    let program = crate::harness_kind::HarnessKind::Claude
        .program()
        .expect("claude has a program");
    let (resolved, _path) = crate::pty::harness_program_lookup(program, settings);
    let exe = resolved.unwrap_or_else(|| program.to_owned());
    let version = skein_harness::claude::cli_version::probe_version(
        std::ffi::OsStr::new(&exe),
        std::time::Duration::from_secs(10),
    );
    if version.is_none() {
        tracing::debug!(program = %exe, "claude --version gave no usable version");
    }
    version
}

/// The major opencode version a spawn would run; `None` = unknown.
#[must_use]
pub fn opencode_cli_version_impl(settings: &SpawnSettings) -> Option<OpencodeVersionDto> {
    let program = crate::harness_kind::HarnessKind::Opencode
        .program()
        .expect("opencode has a program");
    let (resolved, path) = crate::pty::harness_program_lookup(program, settings);
    let Some(exe) = resolved else {
        tracing::info!(
            program,
            "opencode --version gave no usable version; treating as v1"
        );
        return None;
    };
    let version = skein_harness::opencode::version::probe_version(
        std::ffi::OsStr::new(&exe),
        std::time::Duration::from_secs(10),
        Some(path.as_os_str()),
    );
    if let Some(v) = &version {
        tracing::debug!(
            program = %exe,
            major = v.major,
            version = %v.version,
            "opencode version probed"
        );
    } else {
        tracing::info!(
            program = %exe,
            "opencode --version gave no usable version; treating as v1"
        );
    }
    version.map(|v| OpencodeVersionDto {
        major: v.major,
        version: v.version,
    })
}

/// Wire shape of `opencode_cli_version`.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeVersionDto {
    pub major: u32,
    pub version: String,
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
#[must_use]
pub fn default_shell_impl(spawn_env: &SpawnEnvState) -> Vec<String> {
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

/// User's home directory as a path string. Used as the default cwd
/// for newly-spawned harnesses until Phase 4 wires real worktrees.
#[must_use]
pub fn default_cwd_impl() -> String {
    home_dir()
        .and_then(|p| p.to_str().map(str::to_owned))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
        })
        .unwrap_or_else(|| ".".into())
}

#[cfg(test)]
mod opencode_version_tests {
    use super::OpencodeVersionDto;

    #[test]
    fn opencode_version_dto_is_camel_case() {
        let dto = OpencodeVersionDto {
            major: 2,
            version: "2.2.0".into(),
        };
        assert_eq!(
            serde_json::to_string(&dto).unwrap(),
            r#"{"major":2,"version":"2.2.0"}"#
        );
    }
}
