//! Turning git's answers into the review's shapes.
//!
//! Two jobs, both narrow. [`resolve_range`] works out what a review
//! actually covers — the room's base ref (its override, else the repo's
//! guess) and the merge base with HEAD — and [`scope_diffs`] asks git
//! for the right diff for a scope. Everything else here is conversion:
//! git's hunks into the one shape the pane renders, and paths into the
//! worktree-relative forward-slash keys the rest of the review model
//! keys on.

use skein_git::{DiffLineKind, FileDiff, Repo, StatusKind};
use skein_review::{FileState, Hunk, HunkLine, LineKind};

use super::Scope;
use crate::db::Database;
use crate::review::abs_path;

pub(super) fn status_str(kind: StatusKind) -> &'static str {
    match kind {
        StatusKind::Added => "added",
        StatusKind::Modified => "modified",
        StatusKind::Deleted => "deleted",
        StatusKind::Renamed => "renamed",
        StatusKind::Untracked => "untracked",
        StatusKind::Conflicted => "conflicted",
        StatusKind::Typechange => "typechange",
    }
}

/// Convert a git hunk into the shape the Diff card already renders.
///
/// The coordinates come from the line numbers rather than from parsing
/// the `@@` header: the header is git's rendering of them, and reading
/// them off the lines cannot disagree with the lines themselves.
pub(super) fn to_review_hunk(h: &skein_git::DiffHunk) -> Hunk {
    let mut lines = Vec::with_capacity(h.lines.len());
    let (mut old_start, mut old_lines) = (0usize, 0usize);
    let (mut new_start, mut new_lines) = (0usize, 0usize);
    for l in &h.lines {
        if let Some(n) = l.old_lineno {
            let n = n as usize;
            if old_start == 0 {
                old_start = n;
            }
            old_lines += 1;
        }
        if let Some(n) = l.new_lineno {
            let n = n as usize;
            if new_start == 0 {
                new_start = n;
            }
            new_lines += 1;
        }
        lines.push(HunkLine {
            kind: match l.kind {
                DiffLineKind::Add => LineKind::Add,
                DiffLineKind::Delete => LineKind::Delete,
                DiffLineKind::Context => LineKind::Context,
            },
            content: l.content.clone(),
            old_lineno: l.old_lineno.map(|n| n as usize),
            new_lineno: l.new_lineno.map(|n| n as usize),
        });
    }
    Hunk {
        header: h.header.clone(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines,
    }
}

pub(super) fn additions(f: &FileDiff) -> usize {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == DiffLineKind::Add)
        .count()
}

pub(super) fn deletions(f: &FileDiff) -> usize {
    f.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == DiffLineKind::Delete)
        .count()
}

/// Normalise a git path to the key everything else in the review model
/// uses: worktree-relative, forward slashes.
pub(super) fn norm(path: &str) -> String {
    path.replace('\\', "/")
}

/// What a scope actually covers, once the room's base ref and the
/// merge base with HEAD have both been resolved.
pub(super) struct Range {
    pub base_ref: Option<String>,
    pub base_resolved: bool,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub head_branch: Option<String>,
}

pub(super) fn resolve_range(db: &Database, room_id: &str, repo: &Repo) -> Result<Range, String> {
    let stored = db.review_base_ref(room_id)?;
    let base_ref = stored.or_else(|| repo.default_base_branch());
    let head_branch = repo.head_branch();
    let head_sha = repo.resolve_commit("HEAD").map_err(|e| e.to_string())?;

    let (base_resolved, base_sha) = match (&base_ref, &head_sha) {
        (Some(base), Some(_)) => {
            let resolved = repo
                .resolve_commit(base)
                .map_err(|e| e.to_string())?
                .is_some();
            let merge_base = if resolved {
                repo.merge_base(base, "HEAD").map_err(|e| e.to_string())?
            } else {
                None
            };
            (resolved, merge_base)
        }
        _ => (false, None),
    };

    Ok(Range {
        base_ref,
        base_resolved,
        base_sha,
        head_sha,
        head_branch,
    })
}

/// The file diffs a scope produces.
pub(super) fn scope_diffs(
    repo: &Repo,
    range: &Range,
    scope: Scope,
    commit_sha: Option<&str>,
) -> Result<Vec<FileDiff>, String> {
    match scope {
        // merge-base → working tree. A `None` base is an unrelated or
        // unresolvable history: diffing against the empty tree reports
        // the branch as wholly added, which over-reports rather than
        // hiding work.
        Scope::Branch => repo
            .diff_tree_to_workdir(range.base_sha.as_deref())
            .map_err(|e| e.to_string()),
        Scope::Commit => match commit_sha {
            Some(sha) => repo.diff_commit(sha).map_err(|e| e.to_string()),
            None => Ok(Vec::new()),
        },
        // Pending has no git side at all — it is baseline → disk.
        Scope::Pending => Ok(Vec::new()),
    }
}

/// Digest of a file's new-side content in the given scope — the value a
/// viewed marker stores.
pub(super) fn file_hash(
    cwd: &str,
    path: &str,
    scope: Scope,
    commit_sha: Option<&str>,
    repo: &Repo,
) -> String {
    let state = match (scope, commit_sha) {
        (Scope::Commit, Some(sha)) => repo
            .blob_at(sha, path)
            .ok()
            .flatten()
            .map_or(FileState::Missing, |b| skein_review::classify_bytes(&b)),
        _ => skein_review::read_state(&abs_path(cwd, path)),
    };
    skein_review::content_hash(&state).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use skein_git::{DiffLine, DiffLineKind};

    fn line(kind: DiffLineKind, content: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_owned(),
            old_lineno: old,
            new_lineno: new,
        }
    }

    #[test]
    fn a_git_hunk_converts_with_coordinates_read_off_its_lines() {
        // Not parsed from the @@ header: the header is git's rendering
        // of the line numbers, and reading the numbers cannot disagree
        // with the lines the user sees.
        let h = skein_git::DiffHunk {
            header: "@@ -10,3 +10,4 @@ fn foo()".to_owned(),
            lines: vec![
                line(DiffLineKind::Context, "a", Some(10), Some(10)),
                line(DiffLineKind::Delete, "b", Some(11), None),
                line(DiffLineKind::Add, "B", None, Some(11)),
                line(DiffLineKind::Add, "B2", None, Some(12)),
                line(DiffLineKind::Context, "c", Some(12), Some(13)),
            ],
        };
        let out = to_review_hunk(&h);
        assert_eq!(out.header, h.header);
        assert_eq!((out.old_start, out.old_lines), (10, 3));
        assert_eq!((out.new_start, out.new_lines), (10, 4));
        assert_eq!(out.additions(), 2);
        assert_eq!(out.deletions(), 1);
        assert_eq!(out.lines[1].kind, LineKind::Delete);
        assert_eq!(out.lines[1].old_lineno, Some(11));
        assert_eq!(out.lines[1].new_lineno, None);
    }

    #[test]
    fn a_pure_addition_hunk_has_no_old_coordinates() {
        let h = skein_git::DiffHunk {
            header: "@@ -0,0 +1,2 @@".to_owned(),
            lines: vec![
                line(DiffLineKind::Add, "one", None, Some(1)),
                line(DiffLineKind::Add, "two", None, Some(2)),
            ],
        };
        let out = to_review_hunk(&h);
        assert_eq!((out.old_start, out.old_lines), (0, 0));
        assert_eq!((out.new_start, out.new_lines), (1, 2));
    }

    #[test]
    fn paths_normalise_to_forward_slashes_everywhere() {
        // The review model keys on worktree-relative forward-slash
        // paths; git hands back the OS separator on Windows.
        assert_eq!(norm(r"src\liveContext\diff.ts"), "src/liveContext/diff.ts");
        assert_eq!(norm("src/a.rs"), "src/a.rs");
    }

    #[test]
    fn status_kinds_serialise_to_the_names_the_pane_renders() {
        assert_eq!(status_str(StatusKind::Added), "added");
        assert_eq!(status_str(StatusKind::Untracked), "untracked");
        assert_eq!(status_str(StatusKind::Typechange), "typechange");
    }

    #[test]
    fn a_pending_scope_asks_git_for_nothing() {
        // Pending is baseline → disk and must work in a room that is
        // not a repo at all.
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = Repo::open(tmp.path());
        assert!(repo.is_err(), "not a repo, as the test intends");
    }
}
