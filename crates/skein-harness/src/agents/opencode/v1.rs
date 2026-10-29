//! opencode v1: `opencode agent list`, `name (mode)` per line.

use std::ffi::OsStr;
use std::path::Path;

use super::disk::definitions;
use super::{INTERNAL, Mode};
use crate::agents::clean_command;
use crate::agents::def::{AgentDef, AgentList, AgentSource};

/// The agents opencode will accept for a harness spawned in `cwd`.
///
/// `program` is the resolved `opencode` executable; see
/// [`crate::agents::claude::list`] for why resolution is the caller's job.
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
            let mode = Mode::parse(mode)?;
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            Some((name.to_owned(), mode))
        })
        .collect()
}

/// Names from [`parse_list`] that a picker should offer.
pub(super) fn selectable(pairs: Vec<(String, Mode)>) -> Vec<String> {
    pairs
        .into_iter()
        .filter(|(_, mode)| mode.selectable())
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
