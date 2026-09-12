//! What an agent is once discovered, and the one rule that reads its
//! `tools` list.

use serde::{Deserialize, Serialize};

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
    /// A name the CLI vouched for with no file behind it.
    pub(super) fn bare(name: String, source: AgentSource) -> Self {
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
    pub(super) fn new(mut agents: Vec<AgentDef>, degraded: Option<String>) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
