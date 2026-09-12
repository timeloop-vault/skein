//! The agents a harness kind will accept at `--agent` spawn time, as
//! the frontend sees them. Issue #246 (sub-issue A of #219).
//!
//! Thin by design: discovery itself — which paths, which CLI, what the
//! output looks like — lives in `crates/skein-harness`, where it is
//! testable without Tauri and shared with any standalone tooling
//! (#209). This module does three things the crate deliberately cannot:
//! resolve the program the way a real spawn would, pass #215's
//! `--plugin-dir` so the list matches what the spawn accepts, and flatten
//! the result into a shape TypeScript can switch on.

use std::ffi::OsStr;
use std::path::Path;

use serde::Serialize;
use skein_harness::agents::{self, AgentDef, AgentSource};

use crate::harness_config::HarnessConfig;
use crate::spawn_settings::SpawnSettings;

/// One row of the picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDto {
    /// Exactly what goes after `--agent`, namespace included.
    pub name: String,
    pub description: Option<String>,
    /// `null` = unrestricted, which is not the same as an empty list —
    /// see `allows_review_tools`.
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
    /// `builtin` | `user` | `project` | `plugin` | `config`. Flattened
    /// from the crate's enum, whose struct variant serialises into a
    /// nested object the UI would have to unwrap.
    pub source: String,
    /// The plugin half of a namespaced name, for grouping.
    pub plugin: Option<String>,
    /// Whether this agent could see Skein's review MCP tools at all.
    ///
    /// False means selecting it silently switches off the review loop
    /// #52 exists for: the connection is healthy, `tools/list` arrives,
    /// and the agent's own allowlist filters the tools out before the
    /// model sees them — no error, nothing in any log (#215). The
    /// picker warns on this rather than letting it be discovered by its
    /// absence.
    pub allows_review_tools: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListDto {
    pub agents: Vec<AgentDto>,
    /// Why the list may be incomplete, or `null` when the CLI answered.
    /// A picker showing a degraded list must say so — an incomplete list
    /// pushes the user onto typing a name, and thus onto stale ones.
    pub degraded: Option<String>,
    /// True when this kind has no agent concept at all, in which case
    /// `agents` is empty and that is the whole truth. Distinct from a
    /// kind that has agents and none were found.
    pub unsupported: bool,
}

impl AgentListDto {
    fn unsupported() -> Self {
        Self {
            agents: Vec::new(),
            degraded: None,
            unsupported: true,
        }
    }
}

fn to_dto(def: AgentDef) -> AgentDto {
    let allows_review_tools = agents::allows_mcp_tools(def.tools.as_deref());
    let (source, plugin) = match def.source {
        AgentSource::Builtin => ("builtin", None),
        AgentSource::User => ("user", None),
        AgentSource::Project => ("project", None),
        AgentSource::Config => ("config", None),
        AgentSource::Plugin { plugin } => ("plugin", Some(plugin)),
    };
    AgentDto {
        name: def.name,
        description: def.description,
        tools: def.tools,
        model: def.model,
        source: source.to_owned(),
        plugin,
        allows_review_tools,
    }
}

/// The program Skein would spawn for a kind that has agents.
///
/// Kept as its own list rather than reused from `harness_config`'s
/// `managed_program`: that one answers "may Skein inject config here",
/// which is a different question from "does this kind take `--agent`",
/// and `copilot` is about to separate them — it is a managed program
/// with no agent concept.
fn agent_program(kind: &str) -> Option<&'static str> {
    match kind {
        "claude" => Some("claude"),
        "opencode" => Some("opencode"),
        // gh-copilot has no agent concept, a shell is not an agent, and
        // `files` never spawns anything at all.
        _ => None,
    }
}

/// List the agents `kind` will accept for a harness spawned in `cwd`.
///
/// Never returns an error for "nothing found": a picker needs a list
/// plus a reason, not a failure, so an unrunnable CLI comes back as a
/// short list with `degraded` set (#176).
pub(crate) fn list(
    kind: &str,
    cwd: &str,
    settings: &SpawnSettings,
    config: Option<&HarnessConfig>,
) -> AgentListDto {
    let Some(program) = agent_program(kind) else {
        return AgentListDto::unsupported();
    };
    let Some(home) = skein_harness::home_dir() else {
        return AgentListDto {
            agents: Vec::new(),
            degraded: Some("no home directory — cannot find the agent definitions".to_owned()),
            unsupported: false,
        };
    };
    let (resolved, _path) = crate::pty::harness_program_lookup(program, settings);
    // An unresolved program still goes to the crate as a bare name: the
    // spawn error then names the program the user is missing, which is
    // more use than a message invented here.
    let exe = resolved.unwrap_or_else(|| program.to_owned());
    let cwd_path = Path::new(cwd);

    let list = match kind {
        "claude" => {
            let extra = claude_probe_args(settings, config);
            agents::claude_agents(OsStr::new(&exe), &home, cwd_path, &extra)
        }
        _ => agents::opencode_agents(OsStr::new(&exe), &home, cwd_path),
    };
    AgentListDto {
        agents: list.agents.into_iter().map(to_dto).collect(),
        degraded: list.degraded,
        unsupported: false,
    }
}

/// The extra argv the probe needs so its list matches what the spawn
/// will accept — today that is only #215's `--plugin-dir`, and only
/// while the user has that injection switched on.
fn claude_probe_args(settings: &SpawnSettings, config: Option<&HarnessConfig>) -> Vec<String> {
    if !settings.inject_claude_plugin {
        return Vec::new();
    }
    config
        .and_then(HarnessConfig::claude_plugin_dir)
        .map(|dir| vec!["--plugin-dir".to_owned(), dir.display().to_string()])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, tools: Option<Vec<String>>, source: AgentSource) -> AgentDef {
        AgentDef {
            name: name.to_owned(),
            description: None,
            tools,
            model: None,
            source,
        }
    }

    #[test]
    fn only_the_two_agent_kinds_have_a_program() {
        assert_eq!(agent_program("claude"), Some("claude"));
        assert_eq!(agent_program("opencode"), Some("opencode"));
        for kind in ["copilot", "byoh", "files", ""] {
            assert_eq!(
                agent_program(kind),
                None,
                "{kind} must have no agent picker"
            );
        }
    }

    #[test]
    fn a_kind_without_agents_is_unsupported_not_empty() {
        // The frontend has to tell "this kind has no agents" from "this
        // kind has agents and we found none", or it would render an
        // empty picker for a shell.
        let dto = AgentListDto::unsupported();
        assert!(dto.unsupported);
        assert!(dto.agents.is_empty());
        assert!(dto.degraded.is_none());
    }

    #[test]
    fn flattens_a_plugin_source_into_name_plus_plugin() {
        let dto = to_dto(def(
            "pr-review-toolkit:code-reviewer",
            None,
            AgentSource::Plugin {
                plugin: "pr-review-toolkit".to_owned(),
            },
        ));
        assert_eq!(dto.source, "plugin");
        assert_eq!(dto.plugin.as_deref(), Some("pr-review-toolkit"));
    }

    #[test]
    fn marks_a_restricted_agent_as_losing_the_review_tools() {
        let narrow = to_dto(def(
            "narrow",
            Some(vec!["Read".to_owned(), "Glob".to_owned()]),
            AgentSource::User,
        ));
        assert!(!narrow.allows_review_tools);
        // No `tools` key at all is unrestricted, and must not be
        // reported as a restriction.
        let open = to_dto(def("open", None, AgentSource::User));
        assert!(open.allows_review_tools);
        assert!(open.tools.is_none());
    }

    #[test]
    fn the_plugin_dir_rides_along_only_while_injection_is_on() {
        let on = SpawnSettings {
            inject_claude_plugin: true,
            ..SpawnSettings::default()
        };
        // No resolved bundle: nothing to pass, and not a failure.
        assert!(claude_probe_args(&on, None).is_empty());
        let off = SpawnSettings {
            inject_claude_plugin: false,
            ..SpawnSettings::default()
        };
        assert!(claude_probe_args(&off, None).is_empty());
    }
}

/// Discovery against the CLIs actually installed on this machine,
/// through the same program resolution a real spawn uses.
///
/// `#[ignore]` on purpose: these shell out to `claude` and `opencode`,
/// so they depend on the box rather than on the repository, and the
/// pre-commit gate must stay hermetic. They catch the one class of
/// failure no unit test can — an upstream change to Claude's refusal
/// message or to `opencode agent list` — so run them by hand after
/// upgrading either CLI:
///
/// ```text
/// cargo test --manifest-path app/src-tauri/Cargo.toml -- --ignored --nocapture agents::smoke
/// ```
///
/// They live here rather than in `skein-harness` because resolution is
/// the caller's job: on Windows `npm i -g` leaves `opencode.ps1`,
/// `opencode.cmd` and an extensionless `#!/bin/sh` shim, none of which
/// a bare `Command::new("opencode")` can start (#207). Calling the
/// crate directly with a bare name reports "program not found" on
/// Windows and proves nothing about the product.
#[cfg(test)]
mod smoke {
    use super::*;

    fn probe(kind: &str) -> AgentListDto {
        let cwd = std::env::current_dir().expect("a cwd");
        list(
            kind,
            &cwd.display().to_string(),
            &SpawnSettings::default(),
            None,
        )
    }

    #[test]
    #[ignore = "shells out to the installed CLIs; run by hand"]
    fn claude_lists_its_agents_on_this_machine() {
        let dto = probe("claude");
        println!("degraded: {:?}", dto.degraded);
        for a in &dto.agents {
            println!(
                "  {:<42} {:<8} review_tools={}",
                a.name, a.source, a.allows_review_tools
            );
        }
        assert!(dto.degraded.is_none(), "the probe should have answered");
        assert!(!dto.unsupported);
        assert!(
            dto.agents.iter().any(|a| a.name == "general-purpose"),
            "builtins must be present"
        );
    }

    #[test]
    #[ignore = "shells out to the installed CLIs; run by hand"]
    fn opencode_lists_its_primary_agents_on_this_machine() {
        let dto = probe("opencode");
        println!("degraded: {:?}", dto.degraded);
        for a in &dto.agents {
            println!("  {:<20} {}", a.name, a.source);
        }
        assert!(dto.degraded.is_none(), "agent list should have answered");
        assert!(
            dto.agents.iter().any(|a| a.name == "build"),
            "opencode always has `build`"
        );
        for internal in skein_harness::agents::OPENCODE_INTERNAL {
            assert!(
                !dto.agents.iter().any(|a| a.name == *internal),
                "{internal} is machinery and must not be offered"
            );
        }
    }

    #[test]
    #[ignore = "shells out to the installed CLIs; run by hand"]
    fn a_kind_without_agents_needs_no_cli_at_all() {
        for kind in ["copilot", "byoh", "files"] {
            let dto = probe(kind);
            assert!(dto.unsupported, "{kind} must report unsupported");
            assert!(dto.agents.is_empty());
        }
    }
}
