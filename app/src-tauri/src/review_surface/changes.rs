//! Which new-side lines a review scope changed (#547).
//!
//! The design pane's `show_changes` maps changed source lines onto the
//! rendered elements created there, so it needs only paths and line
//! numbers — not the diff text. [`changed_lines`] is the pure rule over
//! hunks; [`scope_changes`] feeds it from git (branch, commit) or from
//! the baselines (pending).

use skein_git::{Repo, StatusKind};
use skein_review::{Hunk, LineKind};

use super::Scope;
use super::git::{norm, resolve_range, scope_diffs, to_review_hunk};
use crate::db::Database;
use crate::review::pending_impl;

/// At most this many files go to the frontend.
pub(crate) const MAX_CHANGED_FILES: usize = 300;
/// At most this many line numbers, over all files.
pub(crate) const MAX_CHANGED_LINES: usize = 20_000;

/// One changed file: worktree-relative forward-slash path, the sorted
/// unique new-side lines that changed, and whether the file is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    pub path: String,
    pub lines: Vec<usize>,
    pub deleted: bool,
}

/// The new-side lines a set of hunks touches: every added line, plus —
/// for each run of deleted lines — the first line after the run (or the
/// last one before it, when the run ends the hunk), so a deleted
/// attribute line still touches the tag it sat in. Sorted, unique.
pub(crate) fn changed_lines(hunks: &[Hunk]) -> Vec<usize> {
    let mut out = Vec::new();
    for h in hunks {
        let mut in_run = false;
        let mut last_new: Option<usize> = None;
        for l in &h.lines {
            match l.kind {
                LineKind::Delete => {
                    in_run = true;
                    continue;
                }
                LineKind::Add => out.extend(l.new_lineno),
                LineKind::Context => {
                    if in_run {
                        out.extend(l.new_lineno);
                    }
                }
            }
            in_run = false;
            if l.new_lineno.is_some() {
                last_new = l.new_lineno;
            }
        }
        if in_run {
            out.extend(last_new);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Apply the file and line caps, in order. `true` when anything was cut.
pub(crate) fn cap(
    mut files: Vec<ChangedFile>,
    max_files: usize,
    max_lines: usize,
) -> (Vec<ChangedFile>, bool) {
    let mut truncated = files.len() > max_files;
    files.truncate(max_files);
    let mut budget = max_lines;
    let mut kept = Vec::with_capacity(files.len());
    for mut f in files {
        if budget == 0 && !f.lines.is_empty() {
            truncated = true;
            break;
        }
        if f.lines.len() > budget {
            f.lines.truncate(budget);
            truncated = true;
        }
        budget -= f.lines.len();
        kept.push(f);
    }
    (kept, truncated)
}

/// The changed files of a scope, as the room's worktree sees them. A
/// file with nothing to say (binary, or an empty diff) is left out.
pub(crate) fn scope_changes(
    db: &Database,
    room_id: &str,
    cwd: &str,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<Vec<ChangedFile>, String> {
    let mut out = Vec::new();
    if scope == Scope::Pending {
        for p in pending_impl(db, room_id, cwd)? {
            let deleted = p.change == "deleted";
            let lines = if deleted {
                Vec::new()
            } else {
                changed_lines(&p.hunks)
            };
            if deleted || !lines.is_empty() {
                out.push(ChangedFile {
                    path: norm(&p.path),
                    lines,
                    deleted,
                });
            }
        }
    } else {
        let repo = Repo::open(std::path::Path::new(cwd))
            .map_err(|e| format!("this room's folder is not a git repository: {e}"))?;
        let range = resolve_range(db, room_id, &repo)?;
        for d in scope_diffs(&repo, &range, scope, commit_sha, &[])? {
            let deleted = d.kind == StatusKind::Deleted;
            let hunks: Vec<Hunk> = d.hunks.iter().map(to_review_hunk).collect();
            let lines = if deleted {
                Vec::new()
            } else {
                changed_lines(&hunks)
            };
            if deleted || !lines.is_empty() {
                out.push(ChangedFile {
                    path: norm(&d.path),
                    lines,
                    deleted,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
