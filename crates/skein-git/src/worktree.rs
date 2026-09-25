use std::path::{Path, PathBuf};

use git2::{BranchType, Repository, WorktreeAddOptions};

use crate::{GitError, Repo, Result};

/// One worktree entry returned by [`Repo::list_worktrees`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: PathBuf,
}

impl Repo {
    /// Add a worktree at `path` on a fresh branch `branch_name` based on
    /// `base_branch`. `base_branch` is resolved as a local branch first;
    /// when no local branch by that name exists, a remote-tracking
    /// branch is tried instead (e.g. "origin/main") — the base picked in
    /// the New Room dialog can be one nobody has ever checked out
    /// locally, and Skein never fetches to fix that up first (D9: no git
    /// mutations, reads only). `GitError::BranchNotFound` when neither
    /// resolves. The worktree is named after the path's last component —
    /// git uses this name internally under `.git/worktrees/<name>/`.
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
        let base_commit = self.resolve_base_branch_commit(base_branch)?;

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

    /// Resolve `base_branch` to the commit a new branch should start
    /// from: a local branch by that name first, falling back to a
    /// remote-tracking branch (e.g. "origin/main") when no local branch
    /// exists. This is a pure ref lookup — no fetch, no ref write, no
    /// upstream configuration (D9).
    fn resolve_base_branch_commit(&self, base_branch: &str) -> Result<git2::Commit<'_>> {
        match self.repo.find_branch(base_branch, BranchType::Local) {
            Ok(branch) => Ok(branch.get().peel_to_commit()?),
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                match self.repo.find_branch(base_branch, BranchType::Remote) {
                    Ok(branch) => Ok(branch.get().peel_to_commit()?),
                    Err(e2) if e2.code() == git2::ErrorCode::NotFound => {
                        Err(GitError::BranchNotFound(base_branch.to_owned()))
                    }
                    Err(e2) => Err(GitError::Git(e2)),
                }
            }
            Err(e) => Err(GitError::Git(e)),
        }
    }

    /// Re-attach an *existing* local branch as a worktree at `path`
    /// (#164): the room's worktree directory was deleted out from under
    /// Skein but its branch survives, and the fix is a checkout, not a
    /// new branch. Unlike [`Repo::add_worktree`], this never creates or
    /// moves a branch.
    ///
    /// If `path`'s leaf name still has a `.git/worktrees/` entry (the
    /// original `add_worktree` call, or an earlier `restore_worktree`),
    /// a directory-gone entry is stale and gets pruned first — that's
    /// exactly the #164 case, metadata surviving a deleted directory.
    /// An entry whose directory is still there is left alone:
    /// [`GitError::WorktreeExists`]. `validate()` failing is not itself
    /// proof of "gone" — an unmounted drive or offline share fails it
    /// too — so pruning additionally requires `symlink_metadata` on the
    /// recorded path to return `NotFound` with its parent directory
    /// still reachable; anything less certain is refused as
    /// [`GitError::WorktreeUnreachable`] rather than pruned. A branch
    /// already checked out live elsewhere — the main checkout or
    /// another linked worktree — is also refused
    /// ([`GitError::BranchCheckedOut`]) rather than forced; two working
    /// directories on one branch is exactly what git's own worktree
    /// lock exists to prevent.
    pub fn restore_worktree(&self, branch_name: &str, path: &Path) -> Result<WorktreeInfo> {
        let branch = self
            .repo
            .find_branch(branch_name, BranchType::Local)
            .map_err(|e| {
                if e.code() == git2::ErrorCode::NotFound {
                    GitError::BranchNotFound(branch_name.to_owned())
                } else {
                    GitError::Git(e)
                }
            })?;
        let reference = branch.into_reference();

        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                GitError::Git(git2::Error::from_str(
                    "worktree path must end in a UTF-8 component",
                ))
            })?
            .to_owned();

        if self.repo.worktrees()?.iter().flatten().any(|n| n == name) {
            let existing = self.repo.find_worktree(&name)?;
            if existing.validate().is_ok() {
                return Err(GitError::WorktreeExists(name));
            }
            // `validate()` failing only means git2 couldn't confirm the
            // worktree directory is there — that's also what an
            // unmounted drive, an offline share or a permission error
            // looks like, and pruning on THAT would delete the admin
            // entry of a worktree that still exists, leaving its
            // `.git` file dangling (#164 review). Only prune when the
            // recorded path is provably gone: `symlink_metadata` on it
            // returns `NotFound`, AND its parent directory is itself
            // reachable, so a missing parent (the whole `<repo>-wt`
            // dir unmounted) doesn't get misread as "directory gone".
            // Anything else — path present, parent unreachable, any
            // other io error — is refused without touching the entry.
            let existing_path = existing.path();
            let path_confirmed_absent = match std::fs::symlink_metadata(existing_path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    existing_path.parent().is_some_and(Path::is_dir)
                }
                Ok(_) | Err(_) => false,
            };
            if !path_confirmed_absent {
                return Err(GitError::WorktreeUnreachable(
                    name,
                    existing_path.display().to_string(),
                ));
            }
            existing.prune(None)?;
        }

        if self.branch_checked_out_elsewhere(branch_name)? {
            return Err(GitError::BranchCheckedOut(branch_name.to_owned()));
        }

        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));

        // libgit2 won't create intermediate directories (see the same
        // note in `add_worktree`).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        self.repo.worktree(&name, path, Some(&opts))?;
        Ok(WorktreeInfo {
            name,
            path: path.to_path_buf(),
        })
    }

    /// `true` if `branch_name` is currently checked out — HEAD, not
    /// detached — in the main checkout or in any linked worktree whose
    /// directory still validates. A worktree whose directory is gone
    /// can't be "checked out" in any sense that matters here.
    fn branch_checked_out_elsewhere(&self, branch_name: &str) -> Result<bool> {
        let main_repo = Repository::open(self.main_repo_root())?;
        if Self::head_is_branch(&main_repo, branch_name) {
            return Ok(true);
        }
        for other_name in self.repo.worktrees()?.iter().flatten() {
            let other = self.repo.find_worktree(other_name)?;
            if other.validate().is_err() {
                continue;
            }
            let other_repo = Repository::open(other.path())?;
            if Self::head_is_branch(&other_repo, branch_name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn head_is_branch(repo: &Repository, branch_name: &str) -> bool {
        let Ok(head) = repo.head() else {
            return false;
        };
        head.is_branch() && head.shorthand() == Some(branch_name)
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
