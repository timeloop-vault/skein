//! User-configurable spawn environment — which shell Skein asks for a
//! `PATH`, how it asks, and what to add on top (issues #1, #3, #72).
//!
//! **Why this is a file and not a UI preference.** Every other Skein
//! setting lives in `localStorage`, and `prefs.ts` says why: *"we keep
//! them out of sqlite because Rust never reads them"*. That premise
//! breaks here — `pty.rs` consumes these at spawn time, and the probe is
//! pre-warmed during `setup()`, before a webview exists to be asked. A
//! frontend-push design would race room hydration and lose *silently*,
//! producing a harness with the wrong environment: precisely the bug
//! class #72 is about. Sqlite was the other candidate and was rejected
//! because settings would then ride the wipe-and-reinsert path that
//! already carries #167's scar tissue.
//!
//! A hand-editable JSON file is also a genuine feature for a one-person
//! prototype: a `PATH` you've broken can be repaired without launching
//! the app that won't start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent_api::groups::ToolGroups;

pub(crate) mod env_profile;
pub use env_profile::EnvProfile;
pub(crate) mod priority;
pub(crate) use priority::ChildPriority;

/// Lives beside `skein.db` in `app_data_dir()`, so the three build
/// profiles stay isolated from each other exactly like the database.
pub(crate) const SETTINGS_FILE: &str = "settings.json";

const SCHEMA: u32 = 1;

/// How hard Skein works to reproduce the user's interactive environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    /// Source the full rc chain — login *and* interactive files. What
    /// most people mean by "my terminal's PATH": on zsh that is
    /// `.zshenv` + `.zprofile` + `.zshrc`, and interactive-only files
    /// are where tools like pnpm and version managers install
    /// themselves.
    #[default]
    LoginInteractive,
    /// Login files only. Cheaper, and avoids interactive-shell side
    /// effects (prompt frameworks, completion loading) for people whose
    /// `.zshrc` is expensive or fragile.
    Login,
    /// Don't ask a shell at all — use the environment Skein itself was
    /// launched with, plus the additions below. The honest choice when
    /// Skein is always started from a terminal, and the escape hatch if
    /// a probe ever misbehaves.
    None,
}

/// One `KEY=VALUE` pair forced into every harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Everything the user can configure about a harness's environment.
///
/// **Field policy** (mirrors the Room blob rule in CLAUDE.md): the
/// container carries `#[serde(default)]`, so every field is
/// individually optional and a settings file written by an older Skein
/// — or hand-edited down to `{}` — still parses. A required field here
/// would make a stale file unloadable, and the failure mode of *that*
/// is a harness with no `PATH`.
// Independent on/off toggles a user sees as separate
// Settings switches, not a state machine — `struct_excessive_bools`'
// alternative (two-variant enums) would just rename the same booleans.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpawnSettings {
    pub schema: u32,
    /// Absolute path to the shell binary. `None` = resolve from
    /// `$SHELL`. A *path*, never an argv: `pwsh.exe -NoLogo` typed into
    /// one field becomes a single `argv[0]` containing a space, which no
    /// PATH search will ever resolve. Custom shell arguments are out of
    /// scope for #1.
    pub shell: Option<String>,
    pub capture: CaptureMode,
    /// Directories prepended to whatever the probe or the OS reported.
    /// Prepend-only on purpose: a mistyped *replacement* is a state you
    /// cannot recover from inside the app.
    pub path_prepend: Vec<String>,
    pub extra_env: Vec<EnvVar>,
    /// Named sets of variables to set/unset, layered on `extra_env` (#615).
    pub env_profiles: Vec<EnvProfile>,
    /// The globally active profile's name; a harness may override it.
    pub active_env_profile: Option<String>,
    /// Strip host-terminal identity variables (#192).
    pub strip_host_env: bool,
    /// Hand Claude Code the shipped Skein plugin, which registers the
    /// review MCP server (#215). Two flags rather than one because the
    /// escape hatch has to be per-harness: turning the opencode side
    /// off is how a user reclaims `OPENCODE_CONFIG` for their own file,
    /// and that must not cost them the Claude Code side.
    pub inject_claude_plugin: bool,
    /// Point opencode at the shipped config, same server (#215).
    pub inject_opencode_config: bool,
    /// Keep Skein's MCP entry in Skein's own Hermes profile `skein`
    /// (#626). Off removes the entry on the next spawn on that profile.
    pub inject_hermes_config: bool,
    /// Which families of agent-API tools harnesses may use (#534).
    /// Replaces the four `allowAgent*` kill switches; `load` migrates an
    /// old file that still carries them (see `migrate_legacy_tool_groups`).
    pub tool_groups: ToolGroups,
    /// Pass Claude Code `--settings` turning its wheel acceleration off
    /// (#569). Default off, unlike every flag above: `--settings`
    /// outranks the user's own Claude Code settings, so Skein only does
    /// it when asked.
    pub disable_claude_wheel_acceleration: bool,
    /// Spawn harnesses at below-normal CPU priority (Windows) or nice +10
    /// (macOS/Linux), so a busy build yields to Skein's UI. Default on.
    /// Applies to harnesses spawned after the change — running ones keep
    /// their priority — and is inherited by everything a harness starts.
    pub lower_harness_priority: bool,
    /// Context size in tokens above which a harness is marked (#368).
    /// `None` = off. Held here, not in the webview, so the agent API
    /// (#689) can read it later.
    pub context_threshold: Option<u64>,
}

impl Default for SpawnSettings {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            shell: None,
            capture: CaptureMode::default(),
            path_prepend: default_path_prepend(),
            extra_env: Vec::new(),
            env_profiles: Vec::new(),
            active_env_profile: None,
            strip_host_env: true,
            inject_claude_plugin: true,
            inject_opencode_config: true,
            inject_hermes_config: true,
            tool_groups: ToolGroups::default(),
            disable_claude_wheel_acceleration: false,
            lower_harness_priority: true,
            context_threshold: None,
        }
    }
}

/// The `PATH` additions Skein ships with.
///
/// `~/.local/bin` is the de-facto install location for `pip install
/// --user`, `pipx`, `uv tool` and Claude Code's own installer — and, as
/// verified on this machine, it is routinely absent from the rc chain,
/// because it is normally put there by something further up (a display
/// manager, VS Code's terminal integration) that a Finder-launched
/// bundle never sees. `~/bin` is the same story for hand-rolled scripts.
///
/// Windows gets `%USERPROFILE%\.local\bin` because that is where Claude
/// Code's native Windows installer puts `claude.exe`. It deliberately
/// does *not* get `%LOCALAPPDATA%\Microsoft\WindowsApps`: that is
/// already in the registry `PATH`, and it holds the Store
/// execution-alias stubs — prepending it would put the stub
/// `python.exe`, which opens the Microsoft Store, ahead of a real
/// Python for every harness.
pub(crate) fn default_path_prepend() -> Vec<String> {
    if cfg!(windows) {
        vec![r"%USERPROFILE%\.local\bin".to_owned()]
    } else {
        vec!["~/.local/bin".to_owned(), "~/bin".to_owned()]
    }
}

impl SpawnSettings {
    /// The configured shell, but only if it is actually runnable.
    ///
    /// A bad value here is unusually costly: the Enter-for-shell prompt
    /// writes the resolved shell into `Harness.cmd`, which is persisted
    /// to sqlite and never rewritten, so one keystroke against a typo'd
    /// setting would convert a harness into a permanently broken one
    /// that survives restart. Falling back is the safe direction.
    pub(crate) fn valid_shell(&self) -> Option<&str> {
        let shell = self.shell.as_deref()?.trim();
        if shell.is_empty() {
            return None;
        }
        if is_runnable(Path::new(shell)) {
            Some(shell)
        } else {
            tracing::warn!(
                shell = %shell,
                "spawn settings: configured shell is not a runnable file, ignoring"
            );
            None
        }
    }
}

/// A file that exists *and* can be executed.
///
/// The executable bit matters: a spawn resolves the program with
/// `access(X_OK)`, so a readable-but-not-executable path would pass an
/// `is_file` check here and then fail at spawn time with a much less
/// obvious error.
fn is_runnable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Read the settings file.
///
/// Returns the settings plus an optional human-readable problem to show
/// in the UI. **Never fails**: a missing file is the normal first-run
/// case, and an unparseable one must not stop Skein booting — it takes
/// the defaults and says so, loudly enough that the user knows their
/// edits aren't in effect.
pub(crate) fn load(dir: &Path) -> (SpawnSettings, Option<String>) {
    let path = file_path(dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (SpawnSettings::default(), None);
        }
        Err(e) => {
            let msg = format!("Could not read {}: {e}. Using defaults.", path.display());
            tracing::warn!(path = %path.display(), error = %e, "spawn settings: read failed");
            return (SpawnSettings::default(), Some(msg));
        }
    };
    match serde_json::from_str::<SpawnSettings>(&raw) {
        Ok(mut settings) => {
            migrate_legacy_tool_groups(&raw, &mut settings);
            (settings, None)
        }
        Err(e) => {
            let msg = format!(
                "{} is not valid JSON ({e}). Using defaults — saving will overwrite it.",
                path.display()
            );
            tracing::warn!(path = %path.display(), error = %e, "spawn settings: parse failed");
            (SpawnSettings::default(), Some(msg))
        }
    }
}

/// Derive `tool_groups` from the four pre-#534 `allowAgent*` kill
/// switches when the file has no `toolGroups` key. Conservative: a group
/// stays on only if every switch that used to gate any of its tools was
/// on. A file that has `toolGroups` ignores the legacy keys entirely.
fn migrate_legacy_tool_groups(raw: &str, settings: &mut SpawnSettings) {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    if map.contains_key("toolGroups") {
        return;
    }
    let flag = |key: &str| {
        map.get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    };
    let messaging = flag("allowAgentMessaging");
    let creation = flag("allowAgentRoomCreation");
    let closing = flag("allowAgentRoomClosing");
    let harness_control = flag("allowAgentHarnessControl");
    settings.tool_groups = ToolGroups {
        mail: messaging,
        rooms: creation && closing && harness_control,
        design: harness_control,
        ..ToolGroups::default()
    };
}

/// Write the settings file via a temp file + rename, so an interrupted
/// save can't leave a half-written file that the next boot refuses.
pub(crate) fn save(dir: &Path, settings: &SpawnSettings) -> Result<(), String> {
    let path = file_path(dir);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), "spawn settings: saved");
    Ok(())
}

pub(crate) fn file_path(dir: &Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_api::groups::ToolGroup;

    #[test]
    fn defaults_are_what_the_ui_promises() {
        let d = SpawnSettings::default();
        // Pinned so a future `#[derive(Default)]` can't silently flip
        // stripping off or empty the additions.
        assert!(d.strip_host_env);
        assert_eq!(d.schema, SCHEMA);
        assert_eq!(d.capture, CaptureMode::LoginInteractive);
        assert!(d.shell.is_none());
        assert_eq!(d.path_prepend, default_path_prepend());
        assert_ne!(d.path_prepend, Vec::<String>::new());
    }

    #[test]
    fn an_empty_document_parses_to_defaults() {
        let parsed: SpawnSettings = serde_json::from_str("{}").expect("empty object parses");
        assert_eq!(parsed, SpawnSettings::default());
    }

    #[test]
    fn every_field_is_individually_optional() {
        // A settings file written by an older Skein must keep loading.
        // A required field here would make it unparseable, and the
        // failure mode of that is a harness with no PATH.
        let full = serde_json::to_value(SpawnSettings::default()).expect("serialize");
        let obj = full.as_object().expect("object");
        assert!(obj.len() >= 6, "unexpectedly few fields: {obj:?}");
        for key in obj.keys() {
            let mut trimmed = obj.clone();
            trimmed.remove(key);
            let doc = serde_json::Value::Object(trimmed);
            serde_json::from_value::<SpawnSettings>(doc)
                .unwrap_or_else(|e| panic!("removing {key} broke parsing: {e}"));
        }
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let parsed: SpawnSettings =
            serde_json::from_str(r#"{"schema":1,"somethingNew":true}"#).expect("parses");
        assert_eq!(parsed.schema, 1);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut settings = SpawnSettings::default();
        settings.path_prepend.push("~/tools/bin".to_owned());
        settings.extra_env.push(EnvVar {
            key: "SKEIN_TEST".to_owned(),
            value: "1".to_owned(),
        });
        settings.capture = CaptureMode::None;
        save(dir.path(), &settings).expect("save");

        let (loaded, degraded) = load(dir.path());
        assert_eq!(loaded, settings);
        assert!(degraded.is_none());
    }

    #[test]
    fn a_missing_file_is_not_a_problem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (loaded, degraded) = load(dir.path());
        assert_eq!(loaded, SpawnSettings::default());
        assert!(degraded.is_none(), "first run must not look like an error");
    }

    #[test]
    fn a_corrupt_file_degrades_instead_of_failing_boot() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(file_path(dir.path()), "{ this is not json").expect("write");
        let (loaded, degraded) = load(dir.path());
        assert_eq!(loaded, SpawnSettings::default());
        assert!(
            degraded.is_some(),
            "the user must be told their edits are inert"
        );
    }

    #[test]
    fn a_truncated_file_degrades_instead_of_failing_boot() {
        // What an interrupted hand-edit leaves behind.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(file_path(dir.path()), r#"{"schema":1,"pathPrepend":["~/b"#).expect("write");
        let (_, degraded) = load(dir.path());
        assert!(degraded.is_some());
    }

    #[test]
    fn a_nonexistent_shell_is_refused() {
        let settings = SpawnSettings {
            shell: Some("/definitely/not/a/shell".to_owned()),
            ..SpawnSettings::default()
        };
        assert_eq!(settings.valid_shell(), None);
    }

    #[test]
    fn a_blank_shell_reads_as_unset() {
        for value in ["", "   "] {
            let settings = SpawnSettings {
                shell: Some(value.to_owned()),
                ..SpawnSettings::default()
            };
            assert_eq!(settings.valid_shell(), None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_real_shell_is_accepted() {
        let settings = SpawnSettings {
            shell: Some("/bin/sh".to_owned()),
            ..SpawnSettings::default()
        };
        assert_eq!(settings.valid_shell(), Some("/bin/sh"));
    }

    #[test]
    fn an_old_settings_file_without_the_messaging_key_defaults_to_allowed() {
        // #327 added the field after settings.json was already shipping
        // in the wild — a file written by an older Skein must still
        // load, and the mailbox must default open like the two
        // injection flags it sits beside.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            file_path(dir.path()),
            r#"{"schema":1,"injectClaudePlugin":true,"injectOpencodeConfig":false}"#,
        )
        .expect("write");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert_eq!(loaded.tool_groups, ToolGroups::default());
        // And the fields that *were* present still round-trip, so this
        // is genuinely the old-file case and not just an empty object.
        assert!(loaded.inject_claude_plugin);
        assert!(!loaded.inject_opencode_config);
    }

    #[test]
    fn an_old_settings_file_leaves_claude_wheel_acceleration_alone() {
        // #569 added the field after settings.json was already shipping.
        // Unlike the flags above it defaults OFF: Skein only overrides a
        // Claude Code setting when asked.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            file_path(dir.path()),
            r#"{"schema":1,"injectClaudePlugin":true}"#,
        )
        .expect("write");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert!(!loaded.disable_claude_wheel_acceleration);
    }

    fn load_str(json: &str) -> SpawnSettings {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(file_path(dir.path()), json).expect("write");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        loaded
    }

    #[test]
    fn fresh_default_has_every_tool_group_on() {
        let d = SpawnSettings::default();
        assert!(ToolGroup::ALL.iter().all(|g| d.tool_groups.allows(*g)));
        let parsed: SpawnSettings = serde_json::from_str("{}").expect("parses");
        assert_eq!(parsed.tool_groups, ToolGroups::default());
    }

    #[test]
    fn legacy_messaging_off_turns_only_mail_off() {
        let g = load_str(r#"{"schema":1,"allowAgentMessaging":false}"#).tool_groups;
        assert!(!g.mail);
        assert!(g.review && g.rooms && g.directory && g.design);
    }

    #[test]
    fn legacy_harness_control_off_turns_rooms_and_design_off() {
        let g = load_str(r#"{"allowAgentHarnessControl":false}"#).tool_groups;
        assert!(!g.rooms && !g.design);
        assert!(g.review && g.mail && g.directory);
    }

    #[test]
    fn legacy_room_creation_or_closing_off_turns_rooms_off_only() {
        for key in ["allowAgentRoomCreation", "allowAgentRoomClosing"] {
            let g = load_str(&format!(r#"{{"{key}":false}}"#)).tool_groups;
            assert!(!g.rooms, "{key}");
            assert!(g.design && g.mail && g.review && g.directory, "{key}");
        }
    }

    #[test]
    fn tool_groups_present_ignores_legacy_keys() {
        let s = load_str(
            r#"{"allowAgentMessaging":false,"allowAgentHarnessControl":false,"toolGroups":{"review":false}}"#,
        );
        assert!(!s.tool_groups.review);
        assert!(s.tool_groups.mail && s.tool_groups.rooms && s.tool_groups.design);
    }

    #[test]
    fn tool_groups_round_trip_and_legacy_keys_are_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = SpawnSettings {
            tool_groups: ToolGroups {
                mail: false,
                ..ToolGroups::default()
            },
            ..SpawnSettings::default()
        };
        save(dir.path(), &settings).expect("save");
        let written = std::fs::read_to_string(file_path(dir.path())).expect("read");
        assert!(written.contains("toolGroups"));
        assert!(!written.contains("allowAgent"));
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert_eq!(loaded.tool_groups, settings.tool_groups);
    }

    #[test]
    fn lower_harness_priority_defaults_on() {
        assert!(SpawnSettings::default().lower_harness_priority);
        let parsed: SpawnSettings = serde_json::from_str("{}").expect("empty object parses");
        assert!(parsed.lower_harness_priority);
    }

    #[test]
    fn an_old_settings_file_without_lower_harness_priority_defaults_on() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            file_path(dir.path()),
            r#"{"schema":1,"stripHostEnv":false,"injectClaudePlugin":true,"injectOpencodeConfig":false,"allowAgentMessaging":true,"allowAgentRoomCreation":true,"allowAgentRoomClosing":true,"allowAgentHarnessControl":true,"disableClaudeWheelAcceleration":true}"#,
        )
        .expect("write");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert!(loaded.lower_harness_priority);
        assert!(loaded.disable_claude_wheel_acceleration);
    }

    #[test]
    fn lower_harness_priority_false_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = SpawnSettings {
            lower_harness_priority: false,
            ..SpawnSettings::default()
        };
        save(dir.path(), &settings).expect("save");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert!(!loaded.lower_harness_priority);
    }

    #[test]
    fn context_threshold_defaults_off_and_round_trips() {
        let parsed: SpawnSettings = serde_json::from_str("{}").expect("empty object parses");
        assert_eq!(parsed.context_threshold, None);
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = SpawnSettings {
            context_threshold: Some(150_000),
            ..SpawnSettings::default()
        };
        save(dir.path(), &settings).expect("save");
        let (loaded, degraded) = load(dir.path());
        assert!(degraded.is_none());
        assert_eq!(loaded.context_threshold, Some(150_000));
    }

    #[test]
    fn capture_mode_survives_the_wire() {
        for mode in [
            CaptureMode::LoginInteractive,
            CaptureMode::Login,
            CaptureMode::None,
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let back: CaptureMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(mode, back, "{json}");
        }
        // The frontend writes these literals; pin the spelling.
        assert_eq!(
            serde_json::to_string(&CaptureMode::LoginInteractive).expect("serialize"),
            "\"login-interactive\""
        );
    }
}
