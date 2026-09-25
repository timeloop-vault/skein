use std::path::{Path, PathBuf};

use git2::{BranchType, Repository};

use crate::{GitError, Result};

/// One local branch entry returned by [`Repo::branches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    /// `true` if HEAD currently points at this branch in the main worktree.
    pub is_head: bool,
}

/// A repository handle. Opens lazily and is cheap to construct — there's
/// no caching across calls, so callers can drop and reopen at will.
pub struct Repo {
    pub(crate) repo: Repository,
    /// The on-disk path the repo was opened with. Useful for derived
    /// paths (proposing a sibling worktree dir, etc.) — `Repository::path`
    /// returns the .git dir, which isn't what we want for that.
    pub(crate) workdir: PathBuf,
}

impl Repo {
    /// Open the repo rooted at `path`. Fails if `path` doesn't exist or
    /// isn't a git repo (use [`Repo::is_repo`] for a non-throwing check).
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(GitError::PathMissing(path.to_path_buf()));
        }
        // `Repository::open` does NOT walk up parents looking for a
        // .git dir (only `discover`/`open_ext` do) — `path` must be the
        // repo root or its `.git`, which matches every caller's intent.
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

    /// How many commits `local_branch`'s configured upstream has that
    /// `local_branch` itself doesn't — how far behind a `git fetch` would
    /// find it. An up-to-date branch returns `Some(0)`; `None` means
    /// there is no upstream configured, or it can't be resolved. This is
    /// a lower bound only as fresh as the last fetch — Skein never
    /// fetches (D9), so a long-idle room can under-report. Any lookup
    /// error here is swallowed to `None` rather than propagated.
    pub fn behind_upstream(&self, local_branch: &str) -> Result<Option<usize>> {
        let Ok(branch) = self.repo.find_branch(local_branch, BranchType::Local) else {
            return Ok(None);
        };
        let Ok(upstream) = branch.upstream() else {
            return Ok(None);
        };
        let (Some(local_oid), Some(upstream_oid)) =
            (branch.get().target(), upstream.get().target())
        else {
            return Ok(None);
        };
        Ok(self
            .repo
            .graph_ahead_behind(local_oid, upstream_oid)
            .map(|(_, behind)| behind)
            .ok())
    }

    /// Shorthand names of remote-tracking branches (e.g. `"origin/main"`),
    /// sorted, excluding symbolic refs such as `"origin/HEAD"`. Read-only
    /// — reflects whatever was fetched last, never triggers a fetch.
    pub fn remote_branches(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for entry in self.repo.branches(Some(BranchType::Remote))? {
            let (branch, _) = entry?;
            // `origin/HEAD` is a symbolic ref pointing at another
            // remote-tracking branch, not a branch of its own.
            if branch.get().kind() != Some(git2::ReferenceType::Direct) {
                continue;
            }
            let Some(name) = branch.name()? else { continue };
            out.push(name.to_owned());
        }
        out.sort();
        Ok(out)
    }

    /// Returns the current HEAD branch name (e.g. `main`), or `None` if
    /// HEAD is detached or otherwise unresolvable.
    pub fn head_branch(&self) -> Option<String> {
        self.head_branch_name()
    }

    pub(crate) fn head_branch_name(&self) -> Option<String> {
        let head = self.repo.head().ok()?;
        if !head.is_branch() {
            return None;
        }
        head.shorthand().map(str::to_owned)
    }

    /// The repo's working directory (i.e. the directory containing
    /// `.git/`), as opened.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The working directory of the *main* checkout this repo belongs
    /// to — i.e. the folder a new worktree should be created from.
    ///
    /// For an ordinary repo this is just [`Repo::workdir`]. For a linked
    /// worktree it is the repo the worktree was added from, which
    /// [`Repo::workdir`] cannot tell you: opening `<repo>-wt/<slug>`
    /// yields that worktree's own directory, so stacking a worktree on a
    /// worktree looks perfectly valid (#226).
    ///
    /// Resolution goes through libgit2's *common dir*, which for a linked
    /// worktree points at the main repo's git dir (`<main>/.git/`, not
    /// `<main>/.git/worktrees/<name>/`). We take its parent and re-open
    /// it to confirm, rather than trusting the path shape: with
    /// `--separate-git-dir`, or a `.git` file pointing elsewhere, that
    /// parent need not be a checkout at all.
    ///
    /// Best-effort by construction — every branch that cannot prove a
    /// better answer returns [`Repo::workdir`], which is exactly today's
    /// behaviour. Resolution can improve the result, never worsen it.
    pub fn main_repo_root(&self) -> PathBuf {
        if !self.repo.is_worktree() {
            return self.workdir.clone();
        }
        let Some(parent) = self.repo.commondir().parent() else {
            return self.workdir.clone();
        };
        match Repository::open(parent) {
            // A bare main repo has no checkout to hand back, so the
            // worktree we were opened with stays the best answer.
            Ok(main) => main
                .workdir()
                .map_or_else(|| self.workdir.clone(), Path::to_path_buf),
            Err(_) => self.workdir.clone(),
        }
    }

    /// Whether this handle was opened on a linked worktree rather than
    /// the main checkout. The New Room dialog uses it to say *why* the
    /// folder it shows is not the folder that was picked.
    pub fn is_worktree(&self) -> bool {
        self.repo.is_worktree()
    }

    /// Is this reported path actually a directory on disk?
    ///
    /// libgit2 sees a Windows junction (and a symlink to a directory)
    /// as a file-like entry, so a directory-only ignore rule —
    /// `node_modules/`, the one every Skein worktree leans on — never
    /// matches it and the whole tree surfaces as a single untracked
    /// entry whose "content" cannot be read. git itself ignores these,
    /// so we drop them too. Everything git legitimately reports is a
    /// file, so this never hides a real change.
    pub(crate) fn is_dir_entry(&self, path: &str) -> bool {
        !path.is_empty() && self.workdir.join(path).is_dir()
    }
}
