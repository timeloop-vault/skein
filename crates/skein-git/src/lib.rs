// All public methods that return `Result` collapse into a single
// `GitError` enum; documenting which variants each can produce is more
// noise than signal at this size.
#![allow(clippy::missing_errors_doc)]

//! `skein-git` — typed wrapper around the git operations Skein needs:
//! the worktree/branch surface used by the new-room flow, plus
//! `status` and `diff_workdir` for the live-diff watcher. Git logic
//! lands here rather than in `app/src-tauri` so it stays testable in
//! isolation, with no Tauri runtime in the way.
//!
//! All operations are synchronous and local. We deliberately disable
//! git2's default `https`/`ssh` features at the workspace level — Skein
//! does not (yet) clone, fetch, or push from inside the app, so the
//! OpenSSL/libssh2 dependency tree is wasted weight.
//!
//! Split into submodules by the question each answers (#19: a pure
//! move, no behaviour change) — `error` (the one `GitError`/`Result`
//! every operation collapses into), `repo` (open a handle, branches,
//! HEAD), `worktree` (add/restore/list/remove, and the sibling-dir
//! path proposal), `status` (worktree status), `diff` (structured
//! diffs + the HEAD-blob review baseline), `range` (commit-range
//! queries for the review surface, #212). Every type and free function
//! that was public from the crate root before the split is re-exported
//! from here unchanged.

mod diff;
mod error;
mod range;
mod repo;
mod status;
mod worktree;

pub use diff::{DiffHunk, DiffLine, DiffLineKind, FileDiff, MAX_DIFF_FILE_BYTES};
pub use error::{GitError, Result};
pub use range::{CommitInfo, MAX_RANGE_COMMITS};
pub use repo::{BranchInfo, Repo};
pub use status::{StatusEntry, StatusKind};
pub use worktree::{WorktreeInfo, propose_worktree_path};
