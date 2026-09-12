//! opencode's agent list, and why only primaries are offered.
//!
//! Agent *discovery* — not to be confused with [`crate::opencode`],
//! which reads the session database.
//!
//! Unlike Claude, opencode answers directly: `opencode agent list` is
//! a real subcommand. So there is no probe trick here, and the only
//! work is dropping what `--agent` would refuse.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::clean_command;
use super::def::{AgentDef, AgentList, AgentSource};
use super::files::read_agent_dir;

/// opencode's internal primaries.
///
/// `opencode agent list` marks these `(primary)` and gives no way to
/// tell them apart from `build` and `plan` — the output carries a name,
/// a mode and a permission blob, nothing else. They are machinery
/// (titling a session, summarising, compacting), never something a
/// user starts a harness as, so they are excluded by name. Documented
/// rather than clever: if opencode ever labels them, drop this.
pub const INTERNAL: &[&str] = &["title", "summary", "compaction"];

/// How opencode classifies an agent. Only a primary can be handed to
/// `--agent`: passing a subagent is refused at spawn with
/// `agent "explore" is a subagent, not a primary agent` and falls back
/// to the default, so offering one in a picker would be offering a
/// silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Primary,
    Subagent,
    All,
}

/// The agents opencode will accept for a harness spawned in `cwd`.
///
/// `program` is the resolved `opencode` executable; see
/// [`super::claude::list`] for why resolution is the caller's job.
pub fn list(program: &OsStr, home: &Path, cwd: &Path) -> AgentList {
    let enrich = definitions(home, cwd);
    match probe(program, cwd) {
        Ok(names) => AgentList::new(
            names
                .into_iter()
                .map(|name| match enrich.get(&name) {
                    Some(def) => def.clone(),
                    None => AgentDef::bare(name, AgentSource::Builtin),
                })
                .collect(),
            None,
        ),
        Err(why) => AgentList::new(enrich.into_values().collect(), Some(why)),
    }
}

/// Parse `opencode agent list`: `name (mode)` lines, each followed by
/// an indented permission blob we ignore.
pub(super) fn parse_list(stdout: &str) -> Vec<(String, Mode)> {
    stdout
        .lines()
        // The permission JSON is indented; agent names are not.
        .filter(|l| !l.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end();
            let rest = line.strip_suffix(')')?;
            let (name, mode) = rest.rsplit_once(" (")?;
            let mode = match mode {
                "primary" => Mode::Primary,
                "subagent" => Mode::Subagent,
                "all" => Mode::All,
                _ => return None,
            };
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            Some((name.to_owned(), mode))
        })
        .collect()
}

/// Names from [`parse_list`] that a picker should offer.
fn selectable(pairs: Vec<(String, Mode)>) -> Vec<String> {
    pairs
        .into_iter()
        .filter(|(_, mode)| matches!(mode, Mode::Primary | Mode::All))
        .map(|(name, _)| name)
        .filter(|name| !INTERNAL.contains(&name.as_str()))
        .collect()
}

fn probe(program: &OsStr, cwd: &Path) -> Result<Vec<String>, String> {
    let out = clean_command(program)
        .current_dir(cwd)
        .arg("agent")
        .arg("list")
        .output()
        .map_err(|e| format!("could not run the opencode CLI: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pairs = parse_list(&text);
    if pairs.is_empty() {
        return Err(
            "the opencode CLI listed no agents — the output format may have changed".to_owned(),
        );
    }
    Ok(selectable(pairs))
}

/// `<home>/.config/opencode/agent` and `<cwd>/.opencode/agent`, plus
/// the `agent` object in either `opencode.json`.
fn definitions(home: &Path, cwd: &Path) -> BTreeMap<String, AgentDef> {
    let mut out = BTreeMap::new();
    let global = home.join(".config").join("opencode");
    for (dir, source) in [
        (global.join("agent"), AgentSource::User),
        (cwd.join(".opencode").join("agent"), AgentSource::Project),
    ] {
        for def in read_agent_dir(&dir, &source, None) {
            out.insert(def.name.clone(), def);
        }
    }
    // Project config last so a repo's own definition wins, which is the
    // precedence opencode itself applies.
    for cfg in [global.join("opencode.json"), cwd.join("opencode.json")] {
        for def in read_config(&cfg) {
            out.insert(def.name.clone(), def);
        }
    }
    out
}

/// Agents declared inline in an `opencode.json`.
fn read_config(path: &Path) -> Vec<AgentDef> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(agents) = json.get("agent").and_then(Value::as_object) else {
        return Vec::new();
    };
    agents
        .iter()
        .map(|(name, body)| AgentDef {
            name: name.clone(),
            description: body
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            // opencode states tools as `{name: bool}`, so the allowlist
            // is the keys that are enabled.
            tools: body.get("tools").and_then(Value::as_object).map(|t| {
                t.iter()
                    .filter(|(_, v)| v.as_bool().unwrap_or(false))
                    .map(|(k, _)| k.clone())
                    .collect()
            }),
            model: body.get("model").and_then(Value::as_str).map(str::to_owned),
            source: AgentSource::Config,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `opencode agent list` interleaves each name with an indented
    // permission blob; the indentation is what separates them.
    const OPENCODE_LIST: &str = "build (primary)\n  [\n  {\n    \"permission\": \"*\"\n  }\n  ]\ncompaction (primary)\n  [ ]\nexplore (subagent)\n  [ ]\ngeneral (subagent)\nplan (primary)\nsummary (primary)\ntitle (primary)\n";

    #[test]
    fn parses_the_real_opencode_list() {
        let pairs = parse_list(OPENCODE_LIST);
        assert_eq!(
            pairs,
            vec![
                ("build".to_owned(), Mode::Primary),
                ("compaction".to_owned(), Mode::Primary),
                ("explore".to_owned(), Mode::Subagent),
                ("general".to_owned(), Mode::Subagent),
                ("plan".to_owned(), Mode::Primary),
                ("summary".to_owned(), Mode::Primary),
                ("title".to_owned(), Mode::Primary),
            ]
        );
    }

    #[test]
    fn offers_only_what_opencode_would_accept() {
        // Subagents are refused by `--agent`; the three internal
        // primaries are machinery. What is left is what a user starts a
        // harness as.
        let names = selectable(parse_list(OPENCODE_LIST));
        assert_eq!(names, vec!["build", "plan"]);
    }

    #[test]
    fn an_unknown_mode_is_skipped_not_guessed() {
        let pairs = parse_list("weird (someday)\nbuild (primary)\n");
        assert_eq!(pairs, vec![("build".to_owned(), Mode::Primary)]);
    }

    #[test]
    fn reads_agents_declared_in_opencode_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("opencode.json");
        let json = serde_json::json!({
            "agent": {
                "reviewer": {
                    "description": "reviews",
                    "model": "m",
                    "tools": { "read": true, "write": false }
                }
            }
        });
        fs::write(&cfg, json.to_string()).expect("write");
        let defs = read_config(&cfg);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "reviewer");
        assert_eq!(defs[0].description.as_deref(), Some("reviews"));
        // opencode states tools as a bool map; the allowlist is the
        // enabled keys, so a disabled one must not count as allowed.
        assert_eq!(defs[0].tools, Some(vec!["read".to_owned()]));
    }
}
