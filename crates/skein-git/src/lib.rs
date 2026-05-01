// All public methods that return `Result` collapse into a single
// `GitError` enum; documenting which variants each can produce is more
// noise than signal at this size.
#![allow(clippy::missing_errors_doc)]

//! `skein-git` — typed wrapper around the git operations Skein needs.
//!
//! Phase 4 covers the worktree/branch surface used by the new-session
//! flow. Phase 5 will extend this crate with `status` and `diff_workdir`
//! for the live-diff watcher; those land here rather than in
//! `app/src-tauri` so they stay testable in isolation, with no Tauri
//! runtime in the way.
//!
//! All operations are synchronous and local. We deliberately disable
//! git2's default `https`/`ssh` features at the workspace level — Skein
//! does not (yet) clone, fetch, or push from inside the app, so the
//! OpenSSL/libssh2 dependency tree is wasted weight.

use std::path::{Path, PathBuf};

use git2::{BranchType, DiffOptions, Patch, Repository, Status, StatusOptions, WorktreeAddOptions};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("path does not exist: {0}")]
    PathMissing(PathBuf),

    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    #[error("branch not found: {0}")]
    BranchNotFound(String),

    #[error("worktree {0} already exists")]
    WorktreeExists(String),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GitError>;

/// One local branch entry returned by [`Repo::branches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    /// `true` if HEAD currently points at this branch in the main worktree.
    pub is_head: bool,
}

/// One worktree entry returned by [`Repo::list_worktrees`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: PathBuf,
}

/// What kind of change a status entry represents. A single file can have
/// both staged and unstaged modifications — see [`StatusEntry::staged`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// New file, staged in the index. Untracked files are
    /// [`StatusKind::Untracked`].
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Brand new file that hasn't been added to the index.
    Untracked,
    Conflicted,
    /// Type changed (e.g. file → symlink). Rare.
    Typechange,
}

/// One entry in the worktree's status. We collapse libgit2's bitfield
/// of possible flags down to a single [`StatusKind`] + a `staged` flag —
/// good enough for a "what changed?" pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub kind: StatusKind,
    /// `true` if the change is in the index (staged), `false` if it's
    /// only in the working tree. A file with both an index change and a
    /// workdir change shows up twice — once with `staged: true` and
    /// once with `staged: false`.
    pub staged: bool,
}

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

/// One file's diff against HEAD (with index changes folded in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub kind: StatusKind,
    pub hunks: Vec<DiffHunk>,
    /// `true` if libgit2 marked this file as binary — in which case
    /// `hunks` will be empty and the UI should render a "binary file
    /// changed" placeholder rather than nothing.
    pub binary: bool,
}

/// A repository handle. Opens lazily and is cheap to construct — there's
/// no caching across calls, so callers can drop and reopen at will.
pub struct Repo {
    repo: Repository,
    /// The on-disk path the repo was opened with. Useful for derived
    /// paths (proposing a sibling worktree dir, etc.) — `Repository::path`
    /// returns the .git dir, which isn't what we want for that.
    workdir: PathBuf,
}

impl Repo {
    /// Open the repo rooted at `path`. Fails if `path` doesn't exist or
    /// isn't a git repo (use [`Repo::is_repo`] for a non-throwing check).
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(GitError::PathMissing(path.to_path_buf()));
        }
        // `Repository::open` also walks up parents looking for a .git
        // dir — fine for now, callers using New Session always pick a
        // root so this matches their intent.
        let repo = Repository::open(path).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                GitError::NotARepo(path.to_path_buf())
            } else {
                GitError::Git(e)
            }
        })?;
        let workdir = repo
            .workdir()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf);
        Ok(Self { repo, workdir })
    }

    /// Cheap "is this folder a git repo?" check used by the new-session
    /// dialog to validate a picked path before showing the branch list.
    pub fn is_repo(path: &Path) -> bool {
        Repository::open(path).is_ok()
    }

    /// Local branches, sorted alphabetically. Each entry knows whether
    /// it's HEAD so the UI can default to it.
    pub fn branches(&self) -> Result<Vec<BranchInfo>> {
        let head_name = self.head_branch_name();
        let mut out = Vec::new();
        for entry in self.repo.branches(Some(BranchType::Local))? {
            let (branch, _) = entry?;
            // `branch.name()` returns Result<Option<&str>> — None means
            // the branch name isn't valid UTF-8, which is rare enough
            // that we just skip those rather than surface them.
            let Some(name) = branch.name()? else { continue };
            let is_head = head_name.as_deref() == Some(name);
            out.push(BranchInfo {
                name: name.to_owned(),
                is_head,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Returns the current HEAD branch name (e.g. `main`), or `None` if
    /// HEAD is detached or otherwise unresolvable.
    pub fn head_branch(&self) -> Option<String> {
        self.head_branch_name()
    }

    fn head_branch_name(&self) -> Option<String> {
        let head = self.repo.head().ok()?;
        if !head.is_branch() {
            return None;
        }
        head.shorthand().map(str::to_owned)
    }

    /// Add a worktree at `path` on a fresh branch `branch_name` based on
    /// `base_branch`. The worktree is named after the path's last
    /// component — git uses this name internally under
    /// `.git/worktrees/<name>/`.
    ///
    /// The new branch lives in the main repo's branch namespace. The
    /// worktree's HEAD points at it on creation; switching branches
    /// inside the worktree later is fine.
    pub fn add_worktree(
        &self,
        branch_name: &str,
        base_branch: &str,
        path: &Path,
    ) -> Result<WorktreeInfo> {
        let base = self
            .repo
            .find_branch(base_branch, BranchType::Local)
            .map_err(|e| {
                if e.code() == git2::ErrorCode::NotFound {
                    GitError::BranchNotFound(base_branch.to_owned())
                } else {
                    GitError::Git(e)
                }
            })?;
        let base_commit = base.get().peel_to_commit()?;

        // `force = false` — fail if `branch_name` already exists rather
        // than silently overwriting. The new-session UI should already
        // be deduping but defense in depth doesn't hurt.
        let new_branch = self.repo.branch(branch_name, &base_commit, false)?;
        let new_ref = new_branch.into_reference();

        // Worktree name (used internally by git) is the leaf of the
        // path. Keep it filesystem-safe — callers compose the path via
        // `propose_worktree_path` so this is generally well-formed.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                GitError::Git(git2::Error::from_str(
                    "worktree path must end in a UTF-8 component",
                ))
            })?
            .to_owned();

        // Reject before calling git2 so we get a tidy error rather than
        // git2's slightly muddier "exists" message.
        if self.repo.worktrees()?.iter().flatten().any(|n| n == name) {
            return Err(GitError::WorktreeExists(name));
        }

        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&new_ref));

        // libgit2 won't create intermediate directories, so make sure
        // the parent of the worktree path exists before asking it to
        // populate `path` itself. (`path` must NOT exist — git2 creates
        // and populates it as part of `worktree`.)
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.repo.worktree(&name, path, Some(&opts))?;
        Ok(WorktreeInfo {
            name,
            path: path.to_path_buf(),
        })
    }

    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let mut out = Vec::new();
        let names = self.repo.worktrees()?;
        for name in names.iter().flatten() {
            let wt = self.repo.find_worktree(name)?;
            out.push(WorktreeInfo {
                name: name.to_owned(),
                path: wt.path().to_path_buf(),
            });
        }
        Ok(out)
    }

    /// Best-effort remove — calls `prune` so a worktree whose directory
    /// has already been deleted is also cleaned up from the metadata.
    pub fn remove_worktree(&self, name: &str) -> Result<()> {
        let wt = self.repo.find_worktree(name)?;
        // `prune` removes the .git/worktrees/<name> entry. Caller is
        // responsible for removing the worktree's working directory if
        // they want it gone — git's own `git worktree remove` does both,
        // but we prefer not to delete user files implicitly.
        wt.prune(None)?;
        Ok(())
    }

    /// The repo's working directory (i.e. the directory containing
    /// `.git/`), as opened.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Enumerate every changed path in the worktree relative to HEAD.
    ///
    /// Untracked files are included (recursing into untracked
    /// directories), but ignored files are not. A file with both staged
    /// and unstaged changes produces two entries — one for each — so
    /// the UI can render them distinctly without re-querying.
    ///
    /// Returned entries are sorted by path for stable rendering.
    pub fn status(&self) -> Result<Vec<StatusEntry>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);
        opts.include_ignored(false);
        opts.renames_head_to_index(true);
        opts.renames_index_to_workdir(true);

        let statuses = self.repo.statuses(Some(&mut opts))?;
        let mut out = Vec::new();
        for entry in statuses.iter() {
            let path = match entry.path() {
                Some(p) => p.to_owned(),
                None => continue, // non-UTF-8 path — skip rather than surface
            };
            let s = entry.status();

            // Index-side (staged) view. The order of these checks
            // matters: a single status flag can encode multiple bits,
            // so we prefer the most specific kind first.
            if let Some(kind) = classify_index(s) {
                out.push(StatusEntry {
                    path: path.clone(),
                    kind,
                    staged: true,
                });
            }
            // Workdir-side (unstaged) view, including untracked.
            if let Some(kind) = classify_workdir(s) {
                out.push(StatusEntry {
                    path,
                    kind,
                    staged: false,
                });
            }
        }
        out.sort_by(|a, b| (a.path.as_str(), a.staged).cmp(&(b.path.as_str(), b.staged)));
        Ok(out)
    }
}

fn classify_index(s: Status) -> Option<StatusKind> {
    if s.contains(Status::INDEX_NEW) {
        Some(StatusKind::Added)
    } else if s.contains(Status::INDEX_MODIFIED) {
        Some(StatusKind::Modified)
    } else if s.contains(Status::INDEX_DELETED) {
        Some(StatusKind::Deleted)
    } else if s.contains(Status::INDEX_RENAMED) {
        Some(StatusKind::Renamed)
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        Some(StatusKind::Typechange)
    } else if s.contains(Status::CONFLICTED) {
        // Conflicted is reported as an index-side thing — not strictly
        // staged, but it lives in the index half of the status.
        Some(StatusKind::Conflicted)
    } else {
        None
    }
}

fn classify_workdir(s: Status) -> Option<StatusKind> {
    if s.contains(Status::WT_NEW) {
        Some(StatusKind::Untracked)
    } else if s.contains(Status::WT_MODIFIED) {
        Some(StatusKind::Modified)
    } else if s.contains(Status::WT_DELETED) {
        Some(StatusKind::Deleted)
    } else if s.contains(Status::WT_RENAMED) {
        Some(StatusKind::Renamed)
    } else if s.contains(Status::WT_TYPECHANGE) {
        Some(StatusKind::Typechange)
    } else {
        None
    }
}

impl Repo {
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

        let mut files: Vec<FileDiff> = Vec::new();
        let n_deltas = diff.deltas().len();
        for i in 0..n_deltas {
            // `Patch::from_diff` returns None for binary deltas — we
            // still want to surface those, just without hunk content.
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

            let patch_opt = Patch::from_diff(&diff, i)?;
            let Some(file_patch) = patch_opt else {
                files.push(FileDiff {
                    path,
                    kind,
                    hunks: Vec::new(),
                    binary: true,
                });
                continue;
            };

            let mut hunks = Vec::new();
            for h_idx in 0..file_patch.num_hunks() {
                let (hunk, line_count) = file_patch.hunk(h_idx)?;
                let header = std::str::from_utf8(hunk.header())
                    .unwrap_or("")
                    .trim_end_matches('\n')
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
                        .trim_end_matches('\n')
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
            });
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
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

/// Compose a default worktree path for a new task: a sibling directory
/// of the repo named `<repo>-wt`, with the slugified task as the leaf.
///
/// `D:\code\skein` + `wire-up-the-migration-runner` →
/// `D:\code\skein-wt\wire-up-the-migration-runner`
///
/// This is just a default — callers can override. We chose sibling-dir
/// over under-repo (`./.skein/worktrees/foo`) so editor indexing,
/// `.gitignore`, and tooling that walks up from a child dir don't trip
/// on a worktree inside their own repo.
pub fn propose_worktree_path(repo_path: &Path, task_slug: &str) -> PathBuf {
    let repo_name = repo_path
        .file_name()
        .map_or_else(|| "repo".into(), |s| s.to_string_lossy().into_owned());
    let parent = repo_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{repo_name}-wt")).join(task_slug)
}
