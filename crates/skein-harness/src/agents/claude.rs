//! Claude Code's agent list: the probe, and the four sources behind it.
//!
//! Agent *discovery* — not to be confused with [`crate::claude`], which
//! reads the session store.
//!
//! Claude resolves `--agent` against builtins, `~/.claude/agents`,
//! `<cwd>/.claude/agents` and every installed plugin's `agents/`. Only
//! the CLI knows the whole answer, so [`list`] asks it and uses the
//! directories for descriptions alone. See the module docs in
//! [`super`] for why.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::clean_command;
use super::def::{AgentDef, AgentList, AgentSource};
use super::files::read_agent_dir;

/// The name we ask for in order to be told the real ones. Deliberately
/// unusable as a real agent name so a user cannot shadow it.
pub const PROBE_AGENT: &str = "__skein_agent_probe__";

/// Claude's own agents, for the degraded path only.
///
/// The probe reports these authoritatively; this list exists so a box
/// where the CLI could not be run still offers something real. It will
/// drift as Claude adds builtins — that is acceptable precisely
/// because it is the fallback and [`AgentList::degraded`] says so.
pub const BUILTINS: &[&str] = &[
    "claude",
    "Explore",
    "general-purpose",
    "Plan",
    "statusline-setup",
];

/// The agents Claude Code will accept for a harness spawned in `cwd`.
///
/// `program` is the resolved `claude` executable — resolved by the
/// caller, because picking it out of `PATH` on Windows needs `PATHEXT`
/// handling that already exists (and is already tested) in
/// `app/src-tauri/src/pty.rs`. `extra_args` is for #215's
/// `--plugin-dir`, so an agent shipped by Skein's own plugin shows up
/// under the same namespace the CLI would give it.
pub fn list(program: &OsStr, home: &Path, cwd: &Path, extra_args: &[String]) -> AgentList {
    let enrich = definitions(home, cwd);
    match probe(program, cwd, extra_args) {
        Ok(names) => AgentList::new(
            names
                .into_iter()
                .map(|name| match enrich.get(&name) {
                    Some(def) => def.clone(),
                    // Vouched for by the CLI with no file behind it:
                    // that is what a builtin looks like from here.
                    None => AgentDef::bare(name, AgentSource::Builtin),
                })
                .collect(),
            None,
        ),
        Err(why) => {
            let mut agents: Vec<AgentDef> = enrich.into_values().collect();
            for b in BUILTINS {
                agents.push(AgentDef::bare((*b).to_owned(), AgentSource::Builtin));
            }
            AgentList::new(agents, Some(why))
        }
    }
}

/// Pull the agent names out of the probe's refusal.
///
/// Returns `None` when the marker is absent, which is how a changed
/// message format degrades instead of yielding nonsense. The list is
/// one line today, but we join everything after the marker before
/// splitting so a wrapped or multi-line variant still parses.
pub(super) fn parse_available(text: &str) -> Option<Vec<String>> {
    const MARKER: &str = "Available agents:";
    let tail = text.split_once(MARKER)?.1;
    let names: Vec<String> = tail
        .split(['\n', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // Anything with whitespace inside is prose, not a name — the
        // guard that stops a trailing sentence becoming an "agent".
        .filter(|s| !s.contains(char::is_whitespace))
        .map(str::to_owned)
        .collect();
    if names.is_empty() { None } else { Some(names) }
}

/// Run the probe and return the names, or why we could not.
fn probe(program: &OsStr, cwd: &Path, extra_args: &[String]) -> Result<Vec<String>, String> {
    let mut cmd = clean_command(program);
    cmd.current_dir(cwd)
        .arg("--agent")
        .arg(PROBE_AGENT)
        // `--print` keeps it non-interactive. The refusal happens
        // before any model call, so this costs nothing and needs no
        // credentials — but without a prompt the CLI would wait on
        // stdin if the refusal ever stopped happening.
        .arg("--print")
        .arg("x")
        .args(extra_args);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run the Claude Code CLI: {e}"))?;
    // The refusal goes to stderr, but read both: which stream carries
    // it is not part of any contract we can rely on.
    let mut text = String::from_utf8_lossy(&out.stderr).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    parse_available(&text).ok_or_else(|| {
        "the Claude Code CLI did not list its agents — the message format may have changed"
            .to_owned()
    })
}

/// `<home>/.claude/agents`.
pub fn user_agents_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("agents")
}

/// `<cwd>/.claude/agents` — the room's worktree.
pub fn project_agents_dir(cwd: &Path) -> PathBuf {
    cwd.join(".claude").join("agents")
}

/// `<home>/.claude/plugins/installed_plugins.json`.
pub fn installed_plugins_path(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("plugins")
        .join("installed_plugins.json")
}

/// Every installed plugin's `agents/` directory, keyed by the plugin
/// name the CLI namespaces with (`pr-review-toolkit`, from the
/// `pr-review-toolkit@claude-plugins-official` key).
///
/// A plugin can have several cached versions installed at once; we
/// take the most recently updated, since that is the one the CLI
/// loads. Enablement is deliberately **not** consulted: a disabled
/// plugin's agents never appear in the probe's list, so they can never
/// be matched, and reading them costs nothing.
pub fn plugin_agents_dirs(home: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(raw) = fs::read_to_string(installed_plugins_path(home)) else {
        return out;
    };
    let Ok(json) = serde_json::from_str::<Value>(&raw) else {
        return out;
    };
    let Some(plugins) = json.get("plugins").and_then(Value::as_object) else {
        return out;
    };
    for (key, entries) in plugins {
        // `name@marketplace` → `name`, which is the namespace the CLI
        // prefixes onto the agent name.
        let plugin = key.split('@').next().unwrap_or(key).to_owned();
        let Some(list) = entries.as_array() else {
            continue;
        };
        let newest = list
            .iter()
            .filter_map(|e| {
                let path = e.get("installPath").and_then(Value::as_str)?;
                let stamp = e
                    .get("lastUpdated")
                    .or_else(|| e.get("installedAt"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some((stamp.to_owned(), PathBuf::from(path)))
            })
            .max_by(|a, b| a.0.cmp(&b.0));
        if let Some((_, path)) = newest {
            out.insert(plugin, path.join("agents"));
        }
    }
    out
}

/// Every agent definition on disk, keyed by the name the CLI would use.
fn definitions(home: &Path, cwd: &Path) -> BTreeMap<String, AgentDef> {
    let mut out = BTreeMap::new();
    for (dir, source) in [
        (user_agents_dir(home), AgentSource::User),
        (project_agents_dir(cwd), AgentSource::Project),
    ] {
        for def in read_agent_dir(&dir, &source, None) {
            out.insert(def.name.clone(), def);
        }
    }
    for (plugin, dir) in plugin_agents_dirs(home) {
        let source = AgentSource::Plugin {
            plugin: plugin.clone(),
        };
        for def in read_agent_dir(&dir, &source, Some(&plugin)) {
            out.insert(def.name.clone(), def);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real refusal, copied from a live run (2026-09-12).
    const REFUSAL: &str = "--agent '__skein_agent_probe__' not found. Available agents: claude, coder, explore, Explore, general-purpose, orchestrator, Plan, pr-review-toolkit:code-reviewer, reviewer, scribe, statusline-setup";

    #[test]
    fn parses_the_real_refusal() {
        let names = parse_available(REFUSAL).expect("marker present");
        assert_eq!(names.first().map(String::as_str), Some("claude"));
        assert!(names.contains(&"pr-review-toolkit:code-reviewer".to_owned()));
        assert!(names.contains(&"statusline-setup".to_owned()));
        assert_eq!(names.len(), 11);
    }

    #[test]
    fn parses_a_wrapped_refusal() {
        let wrapped = "--agent 'x' not found. Available agents: claude,\ncoder, explore,\nPlan";
        let names = parse_available(wrapped).expect("marker present");
        assert_eq!(names, vec!["claude", "coder", "explore", "Plan"]);
    }

    #[test]
    fn a_changed_message_degrades_rather_than_inventing_names() {
        assert!(parse_available("some entirely different error").is_none());
        assert!(parse_available("Available agents:").is_none());
    }

    #[test]
    fn trailing_prose_is_not_an_agent() {
        // Multi-word fragments are prose; a name never contains a space.
        let text = "Available agents: coder, scribe, see the docs for more";
        let names = parse_available(text).expect("marker present");
        assert_eq!(names, vec!["coder", "scribe"]);
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn picks_the_most_recently_updated_plugin_install() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let old = home.join("old");
        let new = home.join("new");
        let json = serde_json::json!({
            "version": 2,
            "plugins": {
                "pr-review-toolkit@claude-plugins-official": [
                    { "installPath": old, "lastUpdated": "2025-01-01T00:00:00.000Z" },
                    { "installPath": new, "lastUpdated": "2026-09-12T00:00:00.000Z" },
                ]
            }
        });
        write(&installed_plugins_path(home), &json.to_string());
        let dirs = plugin_agents_dirs(home);
        // Keyed by the plugin half of `name@marketplace`.
        assert_eq!(dirs.get("pr-review-toolkit"), Some(&new.join("agents")));
    }

    #[test]
    fn a_missing_or_broken_plugin_index_is_simply_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(plugin_agents_dirs(tmp.path()).is_empty());
        write(&installed_plugins_path(tmp.path()), "not json at all");
        assert!(plugin_agents_dirs(tmp.path()).is_empty());
    }

    #[test]
    fn a_missing_cli_degrades_to_disk_plus_builtins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        write(
            &user_agents_dir(&home).join("coder.md"),
            "---\nname: coder\n---\n",
        );
        fs::create_dir_all(&cwd).expect("mkdir");
        let list = list(OsStr::new("skein-no-such-binary-xyz"), &home, &cwd, &[]);
        assert!(
            list.degraded.is_some(),
            "must say the list may be incomplete"
        );
        let names: Vec<&str> = list.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"coder"), "the user's own agents still show");
        assert!(names.contains(&"general-purpose"), "builtins still show");
    }
}
