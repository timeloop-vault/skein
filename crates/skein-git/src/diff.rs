use std::path::Path;

use git2::{DiffOptions, Patch};

use crate::{GitError, Repo, Result, StatusKind};

/// One line in a diff hunk. `old_lineno`/`new_lineno` mirror git's
/// gutter — `None` on the side that doesn't apply (an added line has
/// no old line number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

/// A hunk inside a file diff. `header` is the verbatim
/// `@@ -10,5 +10,7 @@ fn foo() {` line git emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// Per-file diff cost cap, in bytes (#171 slice c). A file at or above
/// this size is diffed with libgit2's `max_size` in effect, which reads
/// it as binary — with no hunk text ever built — rather than spending a
/// full text diff on a file the review pane discards past this size
/// anyway. Matters most for [`Repo::diff_tree_to_workdir`], which the
/// review pane calls on every debounced watcher tick: without the cap,
/// one large generated file becomes per-line hunk text on every tick.
///
/// `skein-git` must not depend on `skein-review` (the crate graph goes
/// the other way), so this is its own constant rather than a re-export.
/// Keep it equal to `skein_review::content::MAX_FILE_BYTES` — if one
/// changes, change the other.
pub const MAX_DIFF_FILE_BYTES: u64 = 1024 * 1024;

/// One file's diff against HEAD (with index changes folded in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub kind: StatusKind,
    pub hunks: Vec<DiffHunk>,
    /// `true` if libgit2 marked this file as binary — in which case
    /// `hunks` will be empty and the UI should render a "binary file
    /// changed" placeholder rather than nothing. A file over
    /// [`MAX_DIFF_FILE_BYTES`] is *also* reported binary by libgit2 (see
    /// `too_large`), since `max_size` works by making the delta look
    /// binary before any content is read.
    pub binary: bool,
    /// `true` when the file was at or above [`MAX_DIFF_FILE_BYTES`] and
    /// was therefore never diffed — distinguishes a genuinely oversized
    /// file from a real binary, both of which come back with
    /// `binary: true` and empty `hunks` and are otherwise
    /// indistinguishable from the delta alone. Always `false` from
    /// [`Repo::diff_workdir`], which sets no cap.
    pub too_large: bool,
}

impl Repo {
    /// The content of `relpath` (repo-relative, forward slashes) as of
    /// HEAD, or `None` when HEAD has no such entry.
    ///
    /// This is where a review baseline comes from (#211): the first
    /// time a harness touches a file we snapshot what was committed, so
    /// the change stays reviewable *after* the agent commits it — which
    /// is exactly the workflow #52 is built around. It reads HEAD rather
    /// than the working tree on purpose: by the time Skein sees the
    /// harness's patch row the edit is already on disk.
    ///
    /// `None` covers every "git cannot tell us" case uniformly — an
    /// unborn HEAD (no commits yet), a path git has never tracked, and
    /// a tree entry that is not a blob (a submodule, a directory). The
    /// caller treats all of them as "the file did not exist at
    /// baseline", which over-reports rather than silently absorbing a
    /// change.
    pub fn head_blob(&self, relpath: &str) -> Result<Option<Vec<u8>>> {
        let Ok(head) = self.repo.head() else {
            return Ok(None); // unborn HEAD — a repo with no commits
        };
        let tree = head.peel_to_tree()?;
        let Ok(entry) = tree.get_path(Path::new(relpath)) else {
            return Ok(None); // not tracked at HEAD
        };
        let object = entry.to_object(&self.repo)?;
        Ok(object.as_blob().map(|b| b.content().to_vec()))
    }

    /// Is `relpath` (repo-relative, forward slashes) covered by a
    /// `.gitignore` rule (or a `core.excludesfile` / `.git/info/exclude`
    /// one)? Used by watcher-driven review discovery (#221) so an
    /// ignored file the filesystem watcher notices — `target/`,
    /// `node_modules/` — never grows a baseline just because it changed
    /// on disk.
    pub fn is_path_ignored(&self, relpath: &str) -> Result<bool> {
        Ok(self.repo.is_path_ignored(relpath)?)
    }

    /// Was `relpath` (repo-relative, forward slashes) a **directory** at
    /// HEAD?
    ///
    /// Watcher-driven review discovery (#221) uses this to tell a
    /// deleted directory — which the OS folds into a single event, so
    /// discovery falls back to a full `git status` sweep to find the
    /// individual files inside it — from ordinary file churn (a
    /// lockfile, an editor swap file) that is merely gone by the time
    /// discovery runs and must NOT trigger that sweep.
    ///
    /// `false` covers every "not a tracked directory" case uniformly:
    /// an unborn HEAD, a path git never tracked, and a tracked path
    /// that is a blob rather than a tree. Read-only, like every other
    /// method here.
    pub fn head_is_dir(&self, relpath: &str) -> Result<bool> {
        let Ok(head) = self.repo.head() else {
            return Ok(false); // unborn HEAD — a repo with no commits
        };
        let tree = head.peel_to_tree()?;
        let Ok(entry) = tree.get_path(Path::new(relpath)) else {
            return Ok(false); // not tracked at HEAD
        };
        Ok(entry.kind() == Some(git2::ObjectType::Tree))
    }

    /// Compute a structured diff of the working tree against HEAD, with
    /// index changes folded in. Untracked files appear as all-add diffs.
    /// Binary files appear with `binary: true` and an empty `hunks`.
    ///
    /// Returns one entry per changed file, sorted by path. A repo on an
    /// unborn branch (no commits yet) returns the worktree as all-adds.
    pub fn diff_workdir(&self) -> Result<Vec<FileDiff>> {
        let mut opts = DiffOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.show_untracked_content(true);
        opts.context_lines(3);

        // `Repository::head()` errors on an unborn branch (fresh repo,
        // no commits). In that case there's no tree to diff against,
        // so we pass None — git2 treats it as the empty tree.
        let head_tree = self.repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        let diff = self
            .repo
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
        self.collect_file_diffs(&diff)
    }

    /// Walk a prepared `git2::Diff` into [`FileDiff`]s.
    ///
    /// Split out of [`Repo::diff_workdir`] for #212: the review surface
    /// diffs tree-against-tree (one commit) and tree-against-worktree
    /// (the whole branch), and all three want identical hunk shapes so
    /// one renderer serves them. Every quirk handled here — the
    /// directory-masquerading-as-a-file skip, the per-delta patch
    /// failure that must not sink the whole diff (#217), binary deltas
    /// surfaced without hunks — was learned on the worktree path and
    /// applies just as much to a commit range.
    pub(crate) fn collect_file_diffs(&self, diff: &git2::Diff<'_>) -> Result<Vec<FileDiff>> {
        let mut files: Vec<FileDiff> = Vec::new();
        let n_deltas = diff.deltas().len();
        for i in 0..n_deltas {
            // `Patch::from_diff` returns `Ok(None)` for a delta libgit2
            // never marked binary in the first place (rare — a content
            // error mid-diff). A delta *that is* binary — real NUL
            // content, or one pushed over `max_size` — still comes back
            // `Ok(Some(patch))`, just with zero hunks, so the binary
            // flag on the delta's file entries is the one source of
            // truth, checked before trusting the patch's hunks at all.
            let delta = diff.get_delta(i).ok_or_else(|| {
                GitError::Git(git2::Error::from_str("diff delta index out of range"))
            })?;

            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let kind = delta_to_status_kind(delta.status());

            // A directory masquerading as a file-like entry (see
            // `is_dir_entry`) has no readable content — libgit2 fails
            // the patch with "requested file is a directory".
            if self.is_dir_entry(&path) {
                continue;
            }

            // One delta libgit2 can't build a patch for must not take
            // the whole worktree diff down with it: surface the file
            // without hunks and keep going. A card showing every other
            // change beats an empty card (#217).
            let Ok(patch_opt) = Patch::from_diff(diff, i) else {
                files.push(FileDiff {
                    path,
                    kind,
                    hunks: Vec::new(),
                    binary: false,
                    too_large: false,
                });
                continue;
            };
            // The delta's binary flag is only populated once content
            // generation actually runs — i.e. after `Patch::from_diff`
            // above, never before it — which is also why it must be
            // checked here rather than trusted from `patch_opt` alone:
            // a delta pushed over `max_size` still comes back
            // `Ok(Some(patch))`, just with zero hunks.
            let is_binary = delta.new_file().is_binary() || delta.old_file().is_binary();
            if is_binary || patch_opt.is_none() {
                // A delta over `max_size` is reported binary the same
                // way a real binary file is — the size check is what
                // tells the two apart.
                let too_large = self.delta_too_large(&delta, &path);
                files.push(FileDiff {
                    path,
                    kind,
                    hunks: Vec::new(),
                    binary: true,
                    too_large,
                });
                continue;
            }
            let file_patch = patch_opt.expect("checked Some above");

            let mut hunks = Vec::new();
            for h_idx in 0..file_patch.num_hunks() {
                let (hunk, line_count) = file_patch.hunk(h_idx)?;
                let header = std::str::from_utf8(hunk.header())
                    .unwrap_or("")
                    .trim_end_matches(['\r', '\n'])
                    .to_owned();
                let mut lines = Vec::with_capacity(line_count);
                for l_idx in 0..line_count {
                    let line = file_patch.line_in_hunk(h_idx, l_idx)?;
                    let line_kind = match line.origin() {
                        '+' | '>' => DiffLineKind::Add,
                        '-' | '<' => DiffLineKind::Delete,
                        // Includes ' ', '=', '\\' (no newline at eof),
                        // 'F', 'H', 'B' — the latter three shouldn't
                        // appear inside a hunk but are harmless in the
                        // Context bucket.
                        _ => DiffLineKind::Context,
                    };
                    let content = std::str::from_utf8(line.content())
                        .unwrap_or("")
                        .trim_end_matches(['\r', '\n'])
                        .to_owned();
                    lines.push(DiffLine {
                        kind: line_kind,
                        content,
                        old_lineno: line.old_lineno(),
                        new_lineno: line.new_lineno(),
                    });
                }
                hunks.push(DiffHunk { header, lines });
            }
            files.push(FileDiff {
                path,
                kind,
                hunks,
                binary: false,
                too_large: false,
            });
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Was this delta forced binary by [`MAX_DIFF_FILE_BYTES`] rather
    /// than genuinely being one?
    ///
    /// Reads the larger of the two sides' reported sizes. For a tree
    /// side that is always the real git-object size, but an *untracked*
    /// workdir file can come back reporting `0` — libgit2 only stats it
    /// as part of reading the content that `max_size` exists to skip —
    /// so a `0` falls back to statting the file ourselves, relative to
    /// the worktree.
    fn delta_too_large(&self, delta: &git2::DiffDelta<'_>, path: &str) -> bool {
        let reported = delta.new_file().size().max(delta.old_file().size());
        let size = if reported > 0 {
            reported
        } else {
            std::fs::metadata(self.workdir.join(path)).map_or(0, |m| m.len())
        };
        size >= MAX_DIFF_FILE_BYTES
    }
}

fn delta_to_status_kind(s: git2::Delta) -> StatusKind {
    use git2::Delta;
    match s {
        Delta::Added => StatusKind::Added,
        Delta::Deleted => StatusKind::Deleted,
        Delta::Renamed => StatusKind::Renamed,
        Delta::Untracked => StatusKind::Untracked,
        Delta::Conflicted => StatusKind::Conflicted,
        Delta::Typechange => StatusKind::Typechange,
        // Modified plus the rare Copied/Ignored/Unmodified/Unreadable
        // bucket — none of those should show up in our diff in practice
        // but Modified is the harmless fallback.
        _ => StatusKind::Modified,
    }
}
