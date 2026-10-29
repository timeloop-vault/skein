//! opencode's agent list, and why only primaries are offered.
//!
//! Agent *discovery* — not to be confused with [`crate::opencode`],
//! which reads the session database.
//!
//! Unlike Claude, opencode answers directly. v1 has `opencode agent
//! list` ([`v1`]); v2 dropped it for `opencode debug agents
//! --standalone`, which prints JSON ([`v2`]). Either way there is no
//! probe trick, and the only work is dropping what `--agent` would
//! refuse. [`disk`] is the shared enrichment (descriptions, tools).

mod disk;
mod v1;
mod v2;

#[cfg(test)]
mod tests;

pub use disk::find_definition;
pub use v1::list;
pub use v2::list_v2;

/// opencode's internal primaries.
///
/// `opencode agent list` marks these `(primary)` and gives no way to
/// tell them apart from `build` and `plan` — the output carries a name,
/// a mode and a permission blob, nothing else. They are machinery
/// (titling a session, summarising, compacting), never something a
/// user starts a harness as, so they are excluded by name. Documented
/// rather than clever: if opencode ever labels them, drop this. (v2
/// does label them `hidden`, and also filters on that.)
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

impl Mode {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "primary" => Some(Self::Primary),
            "subagent" => Some(Self::Subagent),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether `--agent` accepts it.
    fn selectable(self) -> bool {
        matches!(self, Self::Primary | Self::All)
    }
}
