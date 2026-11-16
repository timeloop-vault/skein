//! The folder half of `create_room` (#734): inspect the folder, name the
//! branch, cut the worktree. A port of `createRoomArgs` in
//! `app/src/worktreeRoom.ts`, which did this in the webview. Everything here
//! blocks (libgit2, the filesystem), so the verb runs [`prepare`] off the
//! executor.

use super::super::room_naming::{
    DEFAULT_BRANCH_TEMPLATE, apply_branch_template, branch_field_problem, task_slug, worktree_leaf,
};
use super::super::{VerbError, VerbResult};
use crate::db::Database;
use crate::git::{git_add_worktree_impl, git_inspect_folder_impl, git_propose_worktree_path_impl};
use crate::review_surface::{Scope, query::scope_impl};

/// Whether a new room works in the folder as it is or in a fresh worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BranchMode {
    Worktree,
    Current,
}

/// What the caller asked for, already trimmed and validated as far as the
/// verb's own guards go.
pub(super) struct CheckoutSpec<'a> {
    pub(super) path: &'a str,
    pub(super) mode: BranchMode,
    pub(super) task: &'a str,
    /// Worktree mode only; blank counts as omitted.
    pub(super) branch: Option<&'a str>,
    /// Worktree mode only; blank counts as omitted.
    pub(super) base_branch: Option<&'a str>,
}

/// Where the new room works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Checkout {
    pub(super) cwd: String,
    /// `None` for a folder that is not a repository.
    pub(super) branch: Option<String>,
    pub(super) repo_root: Option<String>,
    /// Worktree mode only, and only when the LOCAL base branch is behind its
    /// upstream (#367).
    pub(super) base_behind_upstream: Option<u32>,
    /// The commit the new worktree was cut from (#356). Worktree mode only,
    /// best effort.
    pub(super) base_sha: Option<String>,
    /// A worktree this call added, and so left behind if a later step fails.
    pub(super) created_worktree: bool,
}

fn refused(message: &str) -> VerbError {
    VerbError::Refused(message.to_owned())
}

/// Validate `spec` against the folder and, in worktree mode, create the
/// worktree.
pub(super) fn prepare(db: &Database, spec: &CheckoutSpec<'_>) -> VerbResult<Checkout> {
    let task = spec.task.trim();
    if task.is_empty() {
        return Err(refused("task can't be empty"));
    }
    let info = git_inspect_folder_impl(spec.path).map_err(VerbError::Internal)?;
    if !info.exists {
        return Err(refused("folder not found"));
    }
    if !info.is_repo {
        if spec.mode == BranchMode::Worktree {
            return Err(refused("not a git repo"));
        }
        return Ok(Checkout {
            cwd: spec.path.to_owned(),
            branch: None,
            repo_root: None,
            base_behind_upstream: None,
            base_sha: None,
            created_worktree: false,
        });
    }

    let default_base = info
        .head
        .clone()
        .filter(|h| !h.is_empty())
        .or_else(|| info.branches.first().map(|b| b.name.clone()))
        .unwrap_or_default();

    if spec.mode == BranchMode::Current {
        return Ok(Checkout {
            cwd: spec.path.to_owned(),
            branch: Some(info.head.clone().unwrap_or_else(|| "HEAD".to_owned())),
            repo_root: Some(info.root),
            base_behind_upstream: None,
            base_sha: None,
            created_worktree: false,
        });
    }

    let requested_base = spec.base_branch.filter(|b| !b.is_empty());
    let branch = spec
        .branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map_or_else(
            || apply_branch_template(DEFAULT_BRANCH_TEMPLATE, &task_slug(task)),
            str::to_owned,
        );
    let base_branch = requested_base.map_or(default_base, str::to_owned);
    if base_branch.is_empty() {
        return Err(refused("no base branch available"));
    }
    if let Some(requested) = requested_base
        && !info.branches.iter().any(|b| b.name == requested)
        && !info.remote_branches.iter().any(|b| b == requested)
    {
        return Err(refused("unknown base branch"));
    }

    let worktree_path = git_propose_worktree_path_impl(spec.path, &worktree_leaf(&branch));
    let worktree_exists = git_inspect_folder_impl(&worktree_path)
        .map_err(VerbError::Internal)?
        .exists;
    let existing: Vec<String> = info.branches.iter().map(|b| b.name.clone()).collect();
    if let Some(problem) = branch_field_problem(&branch, &existing, worktree_exists) {
        return Err(refused(problem));
    }

    let wt = git_add_worktree_impl(spec.path, &branch, &base_branch, &worktree_path)
        .map_err(VerbError::Refused)?;

    // Only a LOCAL base branch has a `behindUpstream` to report: a
    // remote-tracking base is the upstream (#367).
    let base_behind_upstream = info
        .branches
        .iter()
        .find(|b| b.name == base_branch)
        .and_then(|b| b.behind_upstream)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0);
    // The worktree's own fresh HEAD: nothing is committed onto it yet. The
    // room id plays no part in resolving it, so none is needed. A failed
    // read leaves it unset rather than failing the creation.
    let base_sha = match scope_impl(db, "", &wt.path, Scope::Branch, None) {
        Ok(scope) => scope.head_sha,
        Err(e) => {
            tracing::warn!(error = %e, "create_room: could not read the new worktree's HEAD sha");
            None
        }
    };

    Ok(Checkout {
        cwd: wt.path,
        branch: Some(branch),
        repo_root: Some(info.root),
        base_behind_upstream,
        base_sha,
        created_worktree: true,
    })
}

#[cfg(test)]
mod tests;
