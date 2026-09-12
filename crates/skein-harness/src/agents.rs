//! Which agents a harness will accept at `--agent` spawn time.
//!
//! Both CLIs Skein drives bind an agent at launch, and Claude Code
//! cannot change it afterwards — so the list has to be known *before*
//! the process starts. Issue #246 (sub-issue A of #219).
//!
//! **Names come from the CLI, not from globbing its config dirs.**
//! Claude has no agent-list flag (`claude agents` manages *background*
//! sessions), but an unknown `--agent` makes it enumerate its own
//! resolution — builtins, the user dir, the project dir and namespaced
//! plugin agents — in one line, in 0.35 s, with no API call:
//!
//! ```text
//! $ claude --agent __skein_agent_probe__ --print x
//! --agent '__skein_agent_probe__' not found. Available agents: claude,
//! coder, explore, Explore, general-purpose, Plan,
//! pr-review-toolkit:code-reviewer, ... reviewer, scribe
//! $ echo $?
//! 1
//! ```
//!
//! opencode has a real subcommand, `opencode agent list`, printing
//! `name (mode)` per line.
//!
//! Replicating Claude's own resolution instead would mean reading
//! `installed_plugins.json` for each plugin's install path,
//! intersecting with `enabledPlugins` across the user, project and
//! local settings files, and picking among the ten cached versions a
//! daily-driven box holds for one plugin. That drifts from the CLI
//! silently, which is the one failure this list cannot have.
//!
//! So disk parsing here is **enrichment only**: it supplies the
//! `description` a picker shows as a subtitle and the `tools`
//! allowlist that decides whether an agent can see Skein's review MCP
//! tools at all (#215). Because every name is already vouched for by
//! the CLI, enrichment is free to scan too widely — an agent file that
//! matches no name is simply never read back out.
//!
//! Everything here is synchronous, read-only and local. The harnesses
//! own these files; we never write to them.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── the model ──────────────────────────────────────────────────────

/// Where an agent definition came from. Cosmetic for the picker, but
/// it is also the only way to explain a name that has no file behind
/// it: a builtin is not missing its description, it never had one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSource {
    /// Shipped inside the CLI — no file, no description.
    Builtin,
    /// The user's own directory (`~/.claude/agents`, `~/.config/opencode/agent`).
    User,
    /// The room's worktree (`.claude/agents`, `.opencode/agent`).
    Project,
    /// An installed plugin's `agents/` directory. The name is
    /// namespaced `plugin:agent` by the CLI, so this carries the
    /// plugin half for grouping.
    Plugin { plugin: String },
    /// Declared inline in `opencode.json`'s `agent` object.
    Config,
}

/// One selectable agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDef {
    /// Exactly what goes after `--agent`, namespace included.
    pub name: String,
    /// The picker's subtitle. `None` for a builtin, or for a name the
    /// CLI accepts but whose file we could not find or read.
    pub description: Option<String>,
    /// The agent's tool allowlist.
    ///
    /// **`None` means unrestricted, which is not the same as empty.**
    /// A restricted allowlist silently hides every MCP tool — see
    /// [`allows_mcp_tools`] — so the two cases must stay
    /// distinguishable all the way to the picker.
    pub tools: Option<Vec<String>>,
    /// The model the definition pins, when it pins one.
    pub model: Option<String>,
    pub source: AgentSource,
}

impl AgentDef {
    fn bare(name: String, source: AgentSource) -> Self {
        Self {
            name,
            description: None,
            tools: None,
            model: None,
            source,
        }
    }
}

/// The answer to "what will this harness accept right now", plus an
/// honest note when we had to guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentList {
    /// Sorted by name, case-insensitively, so `Explore` and `explore`
    /// sit together instead of in separate ASCII neighbourhoods.
    pub agents: Vec<AgentDef>,
    /// Why this list may be incomplete: the CLI was missing, failed,
    /// or printed something we could not parse, and the names came
    /// from disk alone.
    ///
    /// A picker must say so rather than present a short list as the
    /// truth — an incomplete list pushes the user onto typing a name
    /// by hand, and thus onto stale names (#176, #219).
    pub degraded: Option<String>,
}

impl AgentList {
    fn new(mut agents: Vec<AgentDef>, degraded: Option<String>) -> Self {
        agents.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        agents.dedup_by(|a, b| a.name == b.name);
        Self { agents, degraded }
    }
}

/// Whether an agent with this allowlist can see MCP tools at all.
///
/// Claude filters MCP tools against the agent's `tools` list *after*
/// the server has connected and answered `tools/list`, so a restricted
/// agent that does not name them behaves exactly as though the review
/// loop does not exist: healthy connection, no error, nothing in any
/// log (measured on #215). MCP tool names are `mcp__<server>__<tool>`,
/// and a `tools` list may allow a whole server with the `mcp__server`
/// prefix, so the test is "does anything here mention MCP at all".
///
/// `None` (no `tools` key) is unrestricted and always true.
pub fn allows_mcp_tools(tools: Option<&[String]>) -> bool {
    match tools {
        None => true,
        Some(list) => list.iter().any(|t| t.trim_start().starts_with("mcp__")),
    }
}

// ── Claude Code ────────────────────────────────────────────────────

/// The name we ask for in order to be told the real ones. Deliberately
/// unusable as a real agent name so a user cannot shadow it.
pub const CLAUDE_PROBE_AGENT: &str = "__skein_agent_probe__";

/// Claude's own agents, for the degraded path only.
///
/// The probe reports these authoritatively; this list exists so a box
/// where the CLI could not be run still offers something real. It will
/// drift as Claude adds builtins — that is acceptable precisely
/// because it is the fallback and [`AgentList::degraded`] says so.
pub const CLAUDE_BUILTINS: &[&str] = &[
    "claude",
    "Explore",
    "general-purpose",
    "Plan",
    "statusline-setup",
];

/// Pull the agent names out of the probe's refusal.
///
/// Returns `None` when the marker is absent, which is how a changed
/// message format degrades instead of yielding nonsense. The list is
/// one line today, but we join everything after the marker before
/// splitting so a wrapped or multi-line variant still parses.
pub fn parse_claude_available(text: &str) -> Option<Vec<String>> {
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

/// `<home>/.claude/agents`.
pub fn claude_user_agents_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("agents")
}

/// `<cwd>/.claude/agents` — the room's worktree.
pub fn claude_project_agents_dir(cwd: &Path) -> PathBuf {
    cwd.join(".claude").join("agents")
}

/// `<home>/.claude/plugins/installed_plugins.json`.
pub fn claude_installed_plugins_path(home: &Path) -> PathBuf {
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
pub fn claude_plugin_agents_dirs(home: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(raw) = fs::read_to_string(claude_installed_plugins_path(home)) else {
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

/// The agents Claude Code will accept for a harness spawned in `cwd`.
///
/// `program` is the resolved `claude` executable — resolved by the
/// caller, because picking it out of `PATH` on Windows needs `PATHEXT`
/// handling that already exists (and is already tested) in
/// `app/src-tauri/src/pty.rs`. `extra_args` is for #215's
/// `--plugin-dir`, so an agent shipped by Skein's own plugin shows up
/// under the same namespace the CLI would give it.
pub fn claude_agents(program: &OsStr, home: &Path, cwd: &Path, extra_args: &[String]) -> AgentList {
    let enrich = claude_definitions(home, cwd);
    match probe_claude(program, cwd, extra_args) {
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
            for b in CLAUDE_BUILTINS {
                agents.push(AgentDef::bare((*b).to_owned(), AgentSource::Builtin));
            }
            AgentList::new(agents, Some(why))
        }
    }
}

/// Run the probe and return the names, or why we could not.
fn probe_claude(program: &OsStr, cwd: &Path, extra_args: &[String]) -> Result<Vec<String>, String> {
    let mut cmd = clean_command(program);
    cmd.current_dir(cwd)
        .arg("--agent")
        .arg(CLAUDE_PROBE_AGENT)
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
    parse_claude_available(&text).ok_or_else(|| {
        "the Claude Code CLI did not list its agents — the message format may have changed"
            .to_owned()
    })
}

/// Every agent definition on disk, keyed by the name the CLI would use.
fn claude_definitions(home: &Path, cwd: &Path) -> BTreeMap<String, AgentDef> {
    let mut out = BTreeMap::new();
    for (dir, source) in [
        (claude_user_agents_dir(home), AgentSource::User),
        (claude_project_agents_dir(cwd), AgentSource::Project),
    ] {
        for def in read_agent_dir(&dir, &source, None) {
            out.insert(def.name.clone(), def);
        }
    }
    for (plugin, dir) in claude_plugin_agents_dirs(home) {
        let source = AgentSource::Plugin {
            plugin: plugin.clone(),
        };
        for def in read_agent_dir(&dir, &source, Some(&plugin)) {
            out.insert(def.name.clone(), def);
        }
    }
    out
}

// ── opencode ───────────────────────────────────────────────────────

/// opencode's internal primaries.
///
/// `opencode agent list` marks these `(primary)` and gives no way to
/// tell them apart from `build` and `plan` — the output carries a name,
/// a mode and a permission blob, nothing else. They are machinery
/// (titling a session, summarising, compacting), never something a
/// user starts a harness as, so they are excluded by name. Documented
/// rather than clever: if opencode ever labels them, drop this.
pub const OPENCODE_INTERNAL: &[&str] = &["title", "summary", "compaction"];

/// How opencode classifies an agent. Only a primary can be handed to
/// `--agent`: passing a subagent is refused at spawn with
/// `agent "explore" is a subagent, not a primary agent` and falls back
/// to the default, so offering one in a picker would be offering a
/// silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpencodeMode {
    Primary,
    Subagent,
    All,
}

/// Parse `opencode agent list`: `name (mode)` lines, each followed by
/// an indented permission blob we ignore.
pub fn parse_opencode_agents(stdout: &str) -> Vec<(String, OpencodeMode)> {
    stdout
        .lines()
        // The permission JSON is indented; agent names are not.
        .filter(|l| !l.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end();
            let rest = line.strip_suffix(')')?;
            let (name, mode) = rest.rsplit_once(" (")?;
            let mode = match mode {
                "primary" => OpencodeMode::Primary,
                "subagent" => OpencodeMode::Subagent,
                "all" => OpencodeMode::All,
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

/// Names from `parse_opencode_agents` that a picker should offer.
fn selectable_opencode(pairs: Vec<(String, OpencodeMode)>) -> Vec<String> {
    pairs
        .into_iter()
        .filter(|(_, mode)| matches!(mode, OpencodeMode::Primary | OpencodeMode::All))
        .map(|(name, _)| name)
        .filter(|name| !OPENCODE_INTERNAL.contains(&name.as_str()))
        .collect()
}

/// `<home>/.config/opencode/agent` and `<cwd>/.opencode/agent`, plus
/// the `agent` object in either `opencode.json`.
fn opencode_definitions(home: &Path, cwd: &Path) -> BTreeMap<String, AgentDef> {
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
        for def in read_opencode_config(&cfg) {
            out.insert(def.name.clone(), def);
        }
    }
    out
}

/// Agents declared inline in an `opencode.json`.
fn read_opencode_config(path: &Path) -> Vec<AgentDef> {
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

/// The agents opencode will accept for a harness spawned in `cwd`.
///
/// Unlike Claude, opencode answers directly — `opencode agent list` is
/// a real subcommand — so there is no probe trick here and the only
/// filtering is dropping what `--agent` would refuse.
pub fn opencode_agents(program: &OsStr, home: &Path, cwd: &Path) -> AgentList {
    let enrich = opencode_definitions(home, cwd);
    match probe_opencode(program, cwd) {
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

fn probe_opencode(program: &OsStr, cwd: &Path) -> Result<Vec<String>, String> {
    let out = clean_command(program)
        .current_dir(cwd)
        .arg("agent")
        .arg("list")
        .output()
        .map_err(|e| format!("could not run the opencode CLI: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pairs = parse_opencode_agents(&text);
    if pairs.is_empty() {
        return Err(
            "the opencode CLI listed no agents — the output format may have changed".to_owned(),
        );
    }
    Ok(selectable_opencode(pairs))
}

// ── agent definition files ─────────────────────────────────────────

/// Markdown agent definitions in one directory, recursively.
///
/// Claude allows subdirectories under `agents/`, and the name comes
/// from the frontmatter when it has one and the file stem otherwise —
/// so a nested file is named by its stem, not its path. `namespace`
/// prefixes plugin agents the way the CLI does (`plugin:agent`).
fn read_agent_dir(dir: &Path, source: &AgentSource, namespace: Option<&str>) -> Vec<AgentDef> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(read_agent_dir(&path, source, namespace));
            continue;
        }
        if path.extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let fm = parse_frontmatter(&text);
        let stem = path
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_owned();
        let bare = fm.as_ref().and_then(|f| f.name.clone()).unwrap_or(stem);
        if bare.is_empty() {
            continue;
        }
        let name = match namespace {
            Some(ns) => format!("{ns}:{bare}"),
            None => bare,
        };
        out.push(AgentDef {
            name,
            description: fm.as_ref().and_then(|f| f.description.clone()),
            tools: fm.as_ref().and_then(|f| f.tools.clone()),
            model: fm.as_ref().and_then(|f| f.model.clone()),
            source: source.clone(),
        });
    }
    out
}

/// The fields we read out of an agent file's YAML frontmatter.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
}

/// Parse the `---`-fenced frontmatter of an agent definition.
///
/// Deliberately not a YAML parser: agent frontmatter is a handful of
/// scalars, and the shapes seen in the wild are a one-line scalar
/// (`model: sonnet`), an inline comma list (`tools: Read, Glob`), an
/// inline flow sequence (`tools: [Read, Glob]`), a block sequence, and
/// a block scalar (`description: >-`). Pulling in a YAML dependency to
/// read four keys would cost more than it explains. Anything unknown
/// is skipped, never guessed at.
pub fn parse_frontmatter(md: &str) -> Option<FrontMatter> {
    let body = frontmatter_block(md)?;
    let lines: Vec<&str> = body.lines().collect();
    let mut fm = FrontMatter::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        // Only top-level keys; anything indented belongs to the value
        // above and is consumed there.
        if line.trim().is_empty() || line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let rest = rest.trim();
        // Gather any block that follows: indented continuation lines.
        let start = i;
        while i < lines.len() && (lines[i].starts_with(char::is_whitespace) || lines[i].is_empty())
        {
            i += 1;
        }
        let block = &lines[start..i];
        match key {
            "name" => fm.name = scalar(rest, block),
            "description" => fm.description = scalar(rest, block),
            "model" => fm.model = scalar(rest, block),
            "tools" => fm.tools = Some(string_list(rest, block)),
            _ => {}
        }
    }
    Some(fm)
}

/// The text between the opening and closing `---`, or `None` when the
/// file has no frontmatter.
fn frontmatter_block(md: &str) -> Option<&str> {
    // A BOM survives `read_to_string` on Windows-authored files and
    // would hide the opening fence.
    let text = md.strip_prefix('\u{feff}').unwrap_or(md);
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest
        .split_inclusive('\n')
        .scan(0usize, |at, line| {
            let here = *at;
            *at += line.len();
            Some((here, line))
        })
        .find(|(_, line)| matches!(line.trim_end(), "---" | "..."))
        .map(|(at, _)| at)?;
    Some(&rest[..end])
}

/// A scalar value: inline when there is one, otherwise the block
/// underneath folded into a single line.
fn scalar(inline: &str, block: &[&str]) -> Option<String> {
    let inline = inline.trim();
    // `>` / `|` (with any chomping indicator) say the value is the
    // block below; a bare empty value means the same thing.
    let is_block_marker = inline.is_empty() || inline.starts_with('>') || inline.starts_with('|');
    if !is_block_marker {
        return Some(unquote(inline));
    }
    let literal = inline.starts_with('|');
    let joined: Vec<&str> = block
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if joined.is_empty() {
        return None;
    }
    Some(joined.join(if literal { "\n" } else { " " }))
}

/// A list value: an inline flow sequence, an inline comma list, or a
/// block sequence of `- item` lines.
fn string_list(inline: &str, block: &[&str]) -> Vec<String> {
    let inline = inline.trim();
    if !inline.is_empty() {
        let inner = inline
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(inline);
        return inner
            .split(',')
            .map(|s| unquote(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    block
        .iter()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|s| unquote(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Strip one layer of matching quotes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return s[1..s.len() - 1].to_owned();
        }
    }
    s.to_owned()
}

// ── spawning ───────────────────────────────────────────────────────

/// A `Command` that cannot be hijacked by an ambient git environment.
///
/// Git exports `GIT_DIR`, `GIT_INDEX_FILE` and friends to every hook it
/// runs, so anything spawned from the pre-commit gate inherits them.
/// Neither CLI reads them today, but both shell out to git, and the
/// consequence when it bites is that a child silently operates on *this
/// repository* instead of the one it was pointed at — which is how a
/// `.git` folder appears to vanish (see Conventions in `CLAUDE.md`, and
/// `2bcae5a` on `feat/214-land-actions`). Stripping them costs nothing.
fn clean_command(program: &OsStr) -> Command {
    let mut cmd = Command::new(program);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        cmd.env_remove(key);
    }
    cmd.stdin(Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real refusal, copied from a live run (2026-09-12).
    const REFUSAL: &str = "--agent '__skein_agent_probe__' not found. Available agents: claude, coder, explore, Explore, general-purpose, orchestrator, Plan, pr-review-toolkit:code-reviewer, reviewer, scribe, statusline-setup";

    #[test]
    fn parses_the_real_refusal() {
        let names = parse_claude_available(REFUSAL).expect("marker present");
        assert_eq!(names.first().map(String::as_str), Some("claude"));
        assert!(names.contains(&"pr-review-toolkit:code-reviewer".to_owned()));
        assert!(names.contains(&"statusline-setup".to_owned()));
        assert_eq!(names.len(), 11);
    }

    #[test]
    fn parses_a_wrapped_refusal() {
        let wrapped = "--agent 'x' not found. Available agents: claude,\ncoder, explore,\nPlan";
        let names = parse_claude_available(wrapped).expect("marker present");
        assert_eq!(names, vec!["claude", "coder", "explore", "Plan"]);
    }

    #[test]
    fn a_changed_message_degrades_rather_than_inventing_names() {
        assert!(parse_claude_available("some entirely different error").is_none());
        assert!(parse_claude_available("Available agents:").is_none());
    }

    #[test]
    fn trailing_prose_is_not_an_agent() {
        // Multi-word fragments are prose; a name never contains a space.
        let text = "Available agents: coder, scribe, see the docs for more";
        let names = parse_claude_available(text).expect("marker present");
        assert_eq!(names, vec!["coder", "scribe"]);
    }

    // `opencode agent list` interleaves each name with an indented
    // permission blob; the indentation is what separates them.
    const OPENCODE_LIST: &str = "build (primary)\n  [\n  {\n    \"permission\": \"*\"\n  }\n  ]\ncompaction (primary)\n  [ ]\nexplore (subagent)\n  [ ]\ngeneral (subagent)\nplan (primary)\nsummary (primary)\ntitle (primary)\n";

    #[test]
    fn parses_the_real_opencode_list() {
        let pairs = parse_opencode_agents(OPENCODE_LIST);
        assert_eq!(
            pairs,
            vec![
                ("build".to_owned(), OpencodeMode::Primary),
                ("compaction".to_owned(), OpencodeMode::Primary),
                ("explore".to_owned(), OpencodeMode::Subagent),
                ("general".to_owned(), OpencodeMode::Subagent),
                ("plan".to_owned(), OpencodeMode::Primary),
                ("summary".to_owned(), OpencodeMode::Primary),
                ("title".to_owned(), OpencodeMode::Primary),
            ]
        );
    }

    #[test]
    fn offers_only_what_opencode_would_accept() {
        // Subagents are refused by `--agent`; the three internal
        // primaries are machinery. What is left is what a user starts a
        // harness as.
        let names = selectable_opencode(parse_opencode_agents(OPENCODE_LIST));
        assert_eq!(names, vec!["build", "plan"]);
    }

    #[test]
    fn an_unknown_mode_is_skipped_not_guessed() {
        let pairs = parse_opencode_agents("weird (someday)\nbuild (primary)\n");
        assert_eq!(pairs, vec![("build".to_owned(), OpencodeMode::Primary)]);
    }

    // ── frontmatter ────────────────────────────────────────────────

    #[test]
    fn parses_an_inline_comma_tools_list() {
        // The shape `~/.claude/agents/coder.md` actually uses.
        let md = "---\nname: coder\ndescription: Use proactively for implementation work.\ntools: Read, Glob, Grep, Edit, Write, Bash, PowerShell\nmodel: sonnet\ncolor: green\n---\n\nYou are an implementation subagent.\n";
        let fm = parse_frontmatter(md).expect("has frontmatter");
        assert_eq!(fm.name.as_deref(), Some("coder"));
        assert_eq!(
            fm.description.as_deref(),
            Some("Use proactively for implementation work.")
        );
        assert_eq!(fm.model.as_deref(), Some("sonnet"));
        let expected = [
            "Read",
            "Glob",
            "Grep",
            "Edit",
            "Write",
            "Bash",
            "PowerShell",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(fm.tools, Some(expected));
    }

    #[test]
    fn parses_a_flow_sequence_and_a_block_sequence() {
        let flow = parse_frontmatter("---\ntools: [Read, \"Glob\"]\n---\n").expect("frontmatter");
        assert_eq!(flow.tools, Some(vec!["Read".to_owned(), "Glob".to_owned()]));
        let block =
            parse_frontmatter("---\ntools:\n  - Read\n  - Glob\n---\n").expect("frontmatter");
        assert_eq!(
            block.tools,
            Some(vec!["Read".to_owned(), "Glob".to_owned()])
        );
    }

    #[test]
    fn folds_a_block_scalar_description() {
        let md = "---\nname: x\ndescription: >-\n  first line\n  second line\n---\n";
        let fm = parse_frontmatter(md).expect("frontmatter");
        assert_eq!(fm.description.as_deref(), Some("first line second line"));
    }

    #[test]
    fn no_tools_key_means_unrestricted_not_empty() {
        let fm = parse_frontmatter("---\nname: x\n---\n").expect("frontmatter");
        // The distinction the picker's warning depends on.
        assert!(fm.tools.is_none());
        assert!(allows_mcp_tools(fm.tools.as_deref()));
    }

    #[test]
    fn a_file_without_frontmatter_is_not_an_agent() {
        assert!(parse_frontmatter("# just markdown\n").is_none());
        assert!(
            parse_frontmatter("---\nname: x\n").is_none(),
            "unterminated fence"
        );
    }

    #[test]
    fn survives_a_bom_and_crlf() {
        let md = "\u{feff}---\r\nname: x\r\ndescription: hi\r\n---\r\n";
        let fm = parse_frontmatter(md).expect("frontmatter");
        assert_eq!(fm.name.as_deref(), Some("x"));
        assert_eq!(fm.description.as_deref(), Some("hi"));
    }

    // ── the MCP allowlist rule (#215) ──────────────────────────────

    #[test]
    fn a_restricted_allowlist_hides_the_review_tools() {
        let narrow = ["Read".to_owned(), "Glob".to_owned()];
        assert!(!allows_mcp_tools(Some(&narrow)));
        let with_mcp = [
            "Read".to_owned(),
            "mcp__skein_review__list_comments".to_owned(),
        ];
        assert!(allows_mcp_tools(Some(&with_mcp)));
        assert!(allows_mcp_tools(None));
    }

    // ── disk ───────────────────────────────────────────────────────

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    #[test]
    fn reads_a_directory_of_definitions_recursively() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("agents");
        write(
            &dir.join("coder.md"),
            "---\nname: coder\ndescription: d1\n---\n",
        );
        // Frontmatter with no name falls back to the file stem.
        write(
            &dir.join("nested").join("scribe.md"),
            "---\ndescription: d2\n---\n",
        );
        // Not markdown, and markdown that is not an agent.
        write(&dir.join("notes.txt"), "ignore me");
        write(&dir.join("README.md"), "no frontmatter here");
        let mut defs = read_agent_dir(&dir, &AgentSource::User, None);
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["README", "coder", "scribe"]);
        assert_eq!(defs[1].description.as_deref(), Some("d1"));
        assert_eq!(defs[2].description.as_deref(), Some("d2"));
    }

    #[test]
    fn plugin_agents_get_the_namespace_the_cli_uses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("agents");
        write(
            &dir.join("code-reviewer.md"),
            "---\nname: code-reviewer\ndescription: d\n---\n",
        );
        let source = AgentSource::Plugin {
            plugin: "pr-review-toolkit".to_owned(),
        };
        let defs = read_agent_dir(&dir, &source, Some("pr-review-toolkit"));
        assert_eq!(defs.len(), 1);
        // Must match the probe's spelling exactly, or enrichment misses.
        assert_eq!(defs[0].name, "pr-review-toolkit:code-reviewer");
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
        write(&claude_installed_plugins_path(home), &json.to_string());
        let dirs = claude_plugin_agents_dirs(home);
        // Keyed by the plugin half of `name@marketplace`.
        assert_eq!(dirs.get("pr-review-toolkit"), Some(&new.join("agents")));
    }

    #[test]
    fn a_missing_or_broken_plugin_index_is_simply_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(claude_plugin_agents_dirs(tmp.path()).is_empty());
        write(
            &claude_installed_plugins_path(tmp.path()),
            "not json at all",
        );
        assert!(claude_plugin_agents_dirs(tmp.path()).is_empty());
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
        write(&cfg, &json.to_string());
        let defs = read_opencode_config(&cfg);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "reviewer");
        assert_eq!(defs[0].description.as_deref(), Some("reviews"));
        // opencode states tools as a bool map; the allowlist is the
        // enabled keys, so a disabled one must not count as allowed.
        assert_eq!(defs[0].tools, Some(vec!["read".to_owned()]));
    }

    #[test]
    fn the_list_is_sorted_case_insensitively_and_deduped() {
        let list = AgentList::new(
            vec![
                AgentDef::bare("scribe".to_owned(), AgentSource::User),
                AgentDef::bare("Explore".to_owned(), AgentSource::Builtin),
                AgentDef::bare("explore".to_owned(), AgentSource::User),
                AgentDef::bare("scribe".to_owned(), AgentSource::User),
            ],
            None,
        );
        let names: Vec<&str> = list.agents.iter().map(|a| a.name.as_str()).collect();
        // `Explore` and `explore` are different agents and both stay,
        // but they sit together instead of in separate ASCII blocks.
        assert_eq!(names, vec!["Explore", "explore", "scribe"]);
    }

    #[test]
    fn a_missing_cli_degrades_to_disk_plus_builtins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("repo");
        write(
            &claude_user_agents_dir(&home).join("coder.md"),
            "---\nname: coder\n---\n",
        );
        fs::create_dir_all(&cwd).expect("mkdir");
        let list = claude_agents(OsStr::new("skein-no-such-binary-xyz"), &home, &cwd, &[]);
        assert!(
            list.degraded.is_some(),
            "must say the list may be incomplete"
        );
        let names: Vec<&str> = list.agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"coder"), "the user's own agents still show");
        assert!(names.contains(&"general-purpose"), "builtins still show");
    }
}
