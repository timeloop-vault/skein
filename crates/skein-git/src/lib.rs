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

use git2::{BranchType, Repository, WorktreeAddOptions};
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
