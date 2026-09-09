//! How a harness comes up already knowing how to reach Skein's review
//! API (#215, epic #52 E).
//!
//! #213 put `SKEIN_REVIEW_URL` and the room's bearer token into every
//! harness's environment, but a variable nobody reads changes nothing:
//! each agent CLI still had to be told, by hand, that there is an MCP
//! server at that address. This module is the telling.
//!
//! # Session-scoped injection, not a file in the user's repo
//!
//! Both CLIs can be pointed at a configuration for one session only:
//!
//! | harness | mechanism |
//! | :-- | :-- |
//! | Claude Code | `--plugin-dir <dir>` appended to the argv |
//! | opencode | `OPENCODE_CONFIG=<file>` in the environment |
//!
//! Skein ships the bundle they point at as a Tauri resource, so nothing
//! is ever written into the worktree. That is not merely tidier — a
//! `.mcp.json` in the worktree would land in the review's own diff,
//! would need a `.git/info/exclude` entry, would make Claude Code ask
//! the user to approve the server on first run, and would still be
//! sitting there pointing at a dead port after the room was archived.
//! With nothing per-room on disk, that last problem cannot occur: the
//! port and the token reach the harness through the environment, and
//! both die with the process that minted them.
//!
//! # Both mechanisms are additive
//!
//! Verified against both CLIs before this was built, because a
//! mechanism that silently replaced the user's own configuration would
//! be disqualifying:
//!
//! - `OPENCODE_CONFIG` is **merged**, between the user's global config
//!   and the project's. The user's providers and credentials survive
//!   intact, and a repo's own `opencode.json` still wins over ours —
//!   which is the right way round.
//! - `--plugin-dir` is **additive**. Installed plugins and connectors
//!   all still load; the documented shadowing rule is same-name-only,
//!   so the only exposure is a user plugin literally named `skein`.
//!
//! Additive is still not the same as *known about*, which is why
//! [`HarnessConfigStatus`] exists and the Settings panel renders it
//! with a switch (#215 E3).
//!
//! # No endpoint, no injection
//!
//! [`injection_for`] takes `agent`, and returns nothing without it. The
//! configs interpolate `${SKEIN_REVIEW_URL}` / `{env:SKEIN_REVIEW_URL}`
//! from the environment, and those variables are set only when the
//! agent API actually bound (#213). Injecting anyway would register an
//! MCP server whose URL is the literal string `${SKEIN_REVIEW_URL}` and
//! hand the agent a connection error instead of an absence.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::agent_api::state::HarnessIdentity;
use crate::spawn_settings::SpawnSettings;

/// The environment variable opencode reads a custom config path from.
///
/// Public because `pty.rs` has to treat it as reserved *while we are
/// writing it* — see [`Injection::env`].
pub(crate) const OPENCODE_CONFIG_VAR: &str = "OPENCODE_CONFIG";

/// Where the shipped bundle lives inside the resource directory, and
/// the two entry points inside it. Kept as constants because
/// `tauri.conf.json` names the same paths and the two have to agree.
const BUNDLE_DIR: &str = "harness-config";
const CLAUDE_PLUGIN_SUBDIR: &str = "claude-plugin";
/// The file whose presence proves the plugin directory survived
/// bundling. A plugin without its manifest is not a plugin, and
/// dot-directories are exactly what a resource glob tends to drop.
const CLAUDE_PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";
const OPENCODE_CONFIG_FILE: &str = "opencode/opencode.json";

/// The resolved bundle. Held as managed state, resolved once in
/// `setup()`.
#[derive(Debug, Clone, Default)]
pub struct HarnessConfig {
    /// The Claude Code plugin directory, when its manifest was found.
    claude_plugin: Option<PathBuf>,
    /// The opencode config file, when it was found.
    opencode_config: Option<PathBuf>,
    /// Why either is missing. A packaging mistake is silent otherwise:
    /// the agent simply has no review tools and nothing says why
    /// (#176).
    error: Option<String>,
}

/// What Skein adds to one spawn.
///
/// Split by *where it goes* rather than by harness kind, because the
/// spawn path applies the two halves in different places: `args` before
/// the command is built, `env` inside `apply_env` after the user's own
/// additions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Injection {
    /// Appended to the argv.
    pub args: Vec<String>,
    /// Forced into the environment. These keys are reserved *for this
    /// spawn* — the user is told they were ignored rather than having
    /// them silently overwritten — but only while we are actually
    /// writing them, so turning the injection off in Settings hands the
    /// key back.
    pub env: Vec<(String, String)>,
}

impl Injection {
    pub(crate) fn is_empty(&self) -> bool {
        self.args.is_empty() && self.env.is_empty()
    }
}

/// What the Settings panel renders (#215 E3).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessConfigStatus {
    /// Absolute path to the plugin directory, or `None` when it did not
    /// resolve.
    pub claude_plugin: Option<String>,
    pub opencode_config: Option<String>,
    pub error: Option<String>,
    /// The flag Skein appends for Claude Code, spelled out so the panel
    /// shows the argument rather than describing it.
    pub claude_flag: String,
    pub opencode_var: String,
}

impl HarnessConfig {
    /// Resolve the bundle out of the app's resource directory.
    ///
    /// Never fails the boot: a missing bundle costs the agent its
    /// review tools, which is worse than nothing but much better than
    /// an app that will not start.
    pub fn resolve(resource_dir: &Path) -> Self {
        let root = resource_dir.join(BUNDLE_DIR);
        let plugin = root.join(CLAUDE_PLUGIN_SUBDIR);
        let opencode = root.join(OPENCODE_CONFIG_FILE);

        let mut missing = Vec::new();
        let claude_plugin = if plugin.join(CLAUDE_PLUGIN_MANIFEST).is_file() {
            Some(plugin)
        } else {
            missing.push(format!(
                "{}/{CLAUDE_PLUGIN_MANIFEST}",
                plugin.display().to_string().replace('\\', "/")
            ));
            None
        };
        let opencode_config = if opencode.is_file() {
            Some(opencode)
        } else {
            missing.push(opencode.display().to_string().replace('\\', "/"));
            None
        };

        let error = if missing.is_empty() {
            None
        } else {
            Some(format!(
                "harness config bundle incomplete — not found: {}",
                missing.join(", ")
            ))
        };
        if let Some(ref e) = error {
            tracing::error!(error = %e, "harness config: bundle did not resolve");
        } else {
            tracing::info!(root = %root.display(), "harness config: bundle resolved");
        }

        Self {
            claude_plugin,
            opencode_config,
            error,
        }
    }

    /// A bundle that could not even be looked for, because the resource
    /// directory itself did not resolve. Distinct from "looked, and the
    /// files are missing" so the Settings pane can say which.
    pub fn unavailable(error: &str) -> Self {
        Self {
            claude_plugin: None,
            opencode_config: None,
            error: Some(error.to_owned()),
        }
    }

    pub fn status(&self) -> HarnessConfigStatus {
        HarnessConfigStatus {
            claude_plugin: self.claude_plugin.as_ref().map(|p| p.display().to_string()),
            opencode_config: self
                .opencode_config
                .as_ref()
                .map(|p| p.display().to_string()),
            error: self.error.clone(),
            claude_flag: "--plugin-dir".to_owned(),
            opencode_var: OPENCODE_CONFIG_VAR.to_owned(),
        }
    }
}

/// The program Skein spawns for a kind, where Skein chooses it. Mirrors
/// `managedProgram` in `app/src/harnessCmd.ts`; `byoh` runs the user's
/// shell and `files` runs nothing, so neither has one.
fn managed_program(kind: &str) -> Option<&'static str> {
    match kind {
        "claude" => Some("claude"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

/// Whether `program` really is the CLI this kind is about.
///
/// The kind and the argv can legitimately disagree: a harness whose
/// child exited offers "press Enter for shell", which rewrites `cmd`
/// while the record stays `claude`. Appending `--plugin-dir` to a shell
/// would break the spawn outright, so this is a correctness gate and
/// not a tidiness one.
///
/// Compared on the file stem, because the argv may carry a path
/// (`/opt/homebrew/bin/claude`) or a Windows extension
/// (`claude.cmd`) — and on Windows, case-insensitively.
fn program_is(program: &str, expected: &str) -> bool {
    let stem = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    if cfg!(windows) {
        stem.eq_ignore_ascii_case(expected)
    } else {
        stem == expected
    }
}

/// What to inject for one harness.
///
/// `kind` comes from the harness record and decides *which* CLI's
/// mechanism applies; `program` is the argv's head and decides whether
/// that CLI is actually the thing being spawned. Both have to agree.
pub(crate) fn injection_for(
    kind: &str,
    program: &str,
    config: Option<&HarnessConfig>,
    settings: &SpawnSettings,
    agent: Option<&HarnessIdentity>,
) -> Injection {
    // The record says `claude` but the argv says `bash`: the user
    // swapped the command, and Skein's configuration for a program that
    // is not running would at best be noise and at worst a broken
    // spawn.
    if !managed_program(kind).is_some_and(|p| program_is(program, p)) {
        return Injection::default();
    }
    // No live endpoint means the config would interpolate a variable
    // that is not set — see the module docs.
    if agent.is_none() {
        return Injection::default();
    }
    let Some(config) = config else {
        return Injection::default();
    };
    match kind {
        "claude" if settings.inject_claude_plugin => {
            config
                .claude_plugin
                .as_ref()
                .map_or_else(Injection::default, |dir| Injection {
                    args: vec!["--plugin-dir".to_owned(), dir.display().to_string()],
                    env: Vec::new(),
                })
        }
        "opencode" if settings.inject_opencode_config => config
            .opencode_config
            .as_ref()
            .map_or_else(Injection::default, |file| Injection {
                args: Vec::new(),
                env: vec![(OPENCODE_CONFIG_VAR.to_owned(), file.display().to_string())],
            }),
        // copilot, byoh and files reach no review API: gh-copilot has
        // no MCP configuration Skein can inject, a shell is not an
        // agent, and `files` never spawns anything at all.
        _ => Injection::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> HarnessIdentity {
        HarnessIdentity {
            url: "http://127.0.0.1:4242/mcp".to_owned(),
            token: "tok".to_owned(),
            room_id: "s_1".to_owned(),
            harness_id: "h_1".to_owned(),
        }
    }

    fn bundle() -> (tempfile::TempDir, HarnessConfig) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join(BUNDLE_DIR);
        let manifest = root.join(CLAUDE_PLUGIN_SUBDIR).join(CLAUDE_PLUGIN_MANIFEST);
        std::fs::create_dir_all(manifest.parent().expect("manifest parent")).expect("mkdir");
        std::fs::write(&manifest, "{}").expect("write manifest");
        let oc = root.join(OPENCODE_CONFIG_FILE);
        std::fs::create_dir_all(oc.parent().expect("oc parent")).expect("mkdir");
        std::fs::write(&oc, "{}").expect("write opencode config");
        let config = HarnessConfig::resolve(dir.path());
        (dir, config)
    }

    #[test]
    fn a_complete_bundle_resolves_both_halves() {
        let (_tmp, config) = bundle();
        assert!(config.error.is_none());
        assert!(config.claude_plugin.is_some());
        assert!(config.opencode_config.is_some());
    }

    #[test]
    fn a_missing_bundle_reports_which_paths_it_looked_for() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = HarnessConfig::resolve(dir.path());
        let error = config
            .error
            .expect("an empty resource dir must report an error");
        assert!(error.contains(CLAUDE_PLUGIN_MANIFEST), "{error}");
        assert!(error.contains("opencode.json"), "{error}");
    }

    #[test]
    fn claude_gets_the_plugin_dir_and_nothing_in_the_environment() {
        let (_tmp, config) = bundle();
        let injection = injection_for(
            "claude",
            "claude",
            Some(&config),
            &SpawnSettings::default(),
            Some(&identity()),
        );
        assert_eq!(
            injection.args.first().map(String::as_str),
            Some("--plugin-dir")
        );
        assert!(
            injection
                .args
                .get(1)
                .is_some_and(|p| p.ends_with(CLAUDE_PLUGIN_SUBDIR)),
            "{:?}",
            injection.args
        );
        assert!(injection.env.is_empty());
    }

    #[test]
    fn opencode_gets_the_config_variable_and_nothing_in_the_argv() {
        let (_tmp, config) = bundle();
        let injection = injection_for(
            "opencode",
            "opencode",
            Some(&config),
            &SpawnSettings::default(),
            Some(&identity()),
        );
        assert!(injection.args.is_empty());
        assert_eq!(
            injection.env.first().map(|(k, _)| k.as_str()),
            Some(OPENCODE_CONFIG_VAR)
        );
    }

    #[test]
    fn kinds_with_no_review_path_get_nothing() {
        let (_tmp, config) = bundle();
        for kind in ["copilot", "byoh", "files", "something-new"] {
            let injection = injection_for(
                kind,
                kind,
                Some(&config),
                &SpawnSettings::default(),
                Some(&identity()),
            );
            assert!(injection.is_empty(), "{kind} should get no injection");
        }
    }

    #[test]
    fn each_toggle_turns_off_only_its_own_harness() {
        let (_tmp, config) = bundle();
        let settings = SpawnSettings {
            inject_claude_plugin: false,
            ..SpawnSettings::default()
        };
        assert!(
            injection_for(
                "claude",
                "claude",
                Some(&config),
                &settings,
                Some(&identity())
            )
            .is_empty()
        );
        assert!(
            !injection_for(
                "opencode",
                "opencode",
                Some(&config),
                &settings,
                Some(&identity())
            )
            .is_empty(),
            "turning off the Claude plugin must not disable opencode's config"
        );
    }

    #[test]
    fn a_kind_whose_command_was_swapped_gets_nothing() {
        // "Press Enter for shell" rewrites cmd but not the record, so
        // the kind still says `claude`. Appending --plugin-dir to bash
        // would break the spawn outright.
        let (_tmp, config) = bundle();
        let injection = injection_for(
            "claude",
            "/bin/bash",
            Some(&config),
            &SpawnSettings::default(),
            Some(&identity()),
        );
        assert!(injection.is_empty());
    }

    /// An absolute path is the normal case once #207's Windows
    /// re-resolution has run, and `.cmd` is what an npm-installed
    /// harness resolves to there. Backslash separators only parse as
    /// separators on Windows, so that case is gated rather than
    /// asserted everywhere.
    #[test]
    fn a_resolved_path_still_counts_as_the_program() {
        let (_tmp, config) = bundle();
        let mut programs = vec!["/opt/homebrew/bin/claude", "claude"];
        if cfg!(windows) {
            programs.push(r"C:\Users\x\.local\bin\claude.cmd");
            programs.push("CLAUDE.EXE");
        }
        for program in programs {
            let injection = injection_for(
                "claude",
                program,
                Some(&config),
                &SpawnSettings::default(),
                Some(&identity()),
            );
            assert!(!injection.is_empty(), "{program} should be recognised");
        }
    }

    #[test]
    fn without_a_bound_endpoint_nothing_is_injected() {
        // The configs interpolate SKEIN_REVIEW_URL, which is only set
        // when the agent API bound. Injecting anyway would register an
        // MCP server pointing at the literal variable name.
        let (_tmp, config) = bundle();
        for kind in ["claude", "opencode"] {
            let injection =
                injection_for(kind, kind, Some(&config), &SpawnSettings::default(), None);
            assert!(injection.is_empty(), "{kind} without an endpoint");
        }
    }
}
