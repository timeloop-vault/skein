//! Enrichment from opencode's config on disk, shared by v1 and v2.
//!
//! v2 reads the plural `agents/` dir and `agents` config key; v1's
//! singular `agent/` and `agent` are still honoured. Everything is read
//! regardless of version: the CLI vouches for the names, and a def that
//! matches none is never read back out.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::agents::def::{AgentDef, AgentSource};
use crate::agents::files::read_agent_dir;

/// One definition by name, straight from disk — no CLI spawn. See
/// [`crate::agents::claude::find_definition`] for why this exists (#327).
pub fn find_definition(home: &Path, cwd: &Path, name: &str) -> Option<AgentDef> {
    definitions(home, cwd).get(name).cloned()
}

/// `<home>/.config/opencode/{agent,agents}` and
/// `<cwd>/.opencode/{agent,agents}`, plus the `agent`/`agents` objects
/// in either `opencode.json`. Within a pair the plural is read last, so
/// it wins a collision.
pub(super) fn definitions(home: &Path, cwd: &Path) -> BTreeMap<String, AgentDef> {
    let mut out = BTreeMap::new();
    let global = home.join(".config").join("opencode");
    let project = cwd.join(".opencode");
    for (dir, source) in [
        (global.join("agent"), AgentSource::User),
        (global.join("agents"), AgentSource::User),
        (project.join("agent"), AgentSource::Project),
        (project.join("agents"), AgentSource::Project),
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

/// Agents declared inline in an `opencode.json`: the v1 `agent` key,
/// then the v2 `agents` key, which wins a collision.
pub(super) fn read_config(path: &Path) -> Vec<AgentDef> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut merged: BTreeMap<String, AgentDef> = BTreeMap::new();
    for key in ["agent", "agents"] {
        let Some(agents) = json.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (name, body) in agents {
            merged.insert(name.clone(), config_def(name, body));
        }
    }
    merged.into_values().collect()
}

fn config_def(name: &str, body: &Value) -> AgentDef {
    AgentDef {
        name: name.to_owned(),
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
    }
}
