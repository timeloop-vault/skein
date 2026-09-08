//! The review surface — scope, diff and comments (#212, epic #52 D1,
//! D5, D6, D7).
//!
//! [`crate::review`] owns the #211 baseline: what a harness changed
//! since the user last accepted it. This module owns the other half —
//! the review the user actually reads, which is scoped to the *branch*.
//!
//! # Three scopes, one renderer
//!
//! The epic asks for one diff renderer over two scopes; in practice
//! there are three views of the same range, and every one of them
//! returns the same [`skein_review::Hunk`] shape the pane renders:
//!
//! * [`Scope::Branch`] — `merge-base(HEAD, base) → working tree`. The
//!   default, and the honest one: it counts what the agent has
//!   committed *and* what it has not, so the pane does not go blank the
//!   moment the agent commits, nor lie about work still on disk.
//! * [`Scope::Commit`] — one commit against its first parent, for
//!   reading a multi-commit branch the way it was written.
//! * [`Scope::Pending`] — #211's `baseline → disk`, which is where
//!   accept and reject still live. Uncommitted work only.
//!
//! # Where things are
//!
//! Split by what a reader is likely to be after, rather than by layer:
//!
//! | module | question it answers |
//! | :-- | :-- |
//! | [`dto`] | what does the frontend receive |
//! | [`git`] | what does this scope actually cover, and what did git say |
//! | [`anchoring`] | where does this thread sit now |
//! | [`query`] | the two read commands' logic |
//! | [`write`] | opening a thread, which is where an anchor is captured |
//! | [`commands`] | the Tauri boundary, and nothing else |
//!
//! The invariant worth carrying between them: a thread is re-matched
//! against live text on every file open and its new position written
//! back, but its anchor *text* never is. That text is the comment's
//! evidence, and D6 requires an unplaceable thread to render against it
//! rather than against whatever now occupies its old line numbers.

use serde::{Deserialize, Serialize};

mod anchoring;
// `pub` because `tauri::generate_handler!` resolves each command
// through the module that defines it — it needs the hidden macro the
// attribute generates alongside the function, which a `pub use`
// re-export does not carry.
pub mod commands;
mod dto;
mod git;
mod query;
mod write;

/// Which view of the room's work the caller wants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// merge-base → working tree. Everything this branch does.
    #[default]
    Branch,
    /// One commit against its first parent.
    Commit,
    /// #211's baseline → disk. Uncommitted, and the only scope with
    /// accept and reject.
    Pending,
}

/// Thread scopes, as stored in `review_threads.scope` (D5).
mod thread_scope {
    pub const LINE: &str = "line";
    pub const FILE: &str = "file";
    pub const COMMIT: &str = "commit";
    pub const REVIEW: &str = "review";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_defaults_to_branch() {
        assert_eq!(Scope::default(), Scope::Branch);
        assert_eq!(
            serde_json::from_str::<Scope>("\"pending\"").unwrap(),
            Scope::Pending
        );
        assert_eq!(
            serde_json::from_str::<Scope>("\"commit\"").unwrap(),
            Scope::Commit
        );
        assert_eq!(
            serde_json::from_str::<Scope>("\"branch\"").unwrap(),
            Scope::Branch
        );
    }
}
