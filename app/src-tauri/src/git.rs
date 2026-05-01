//! Tauri command wrappers around `skein-git`.
//!
//! These are thin glue: take the inputs from JS, invoke the typed API,
//! collapse `GitError` to `String` for the Result. Any logic richer
//! than that belongs in `skein-git` so it stays testable without Tauri.

use std::path::{Path, PathBuf};

use serde::Serialize;
use skein_git::{BranchInfo, Repo, StatusEntry, StatusKind, WorktreeInfo, propose_worktree_path};

#[derive(Debug, Serialize)]
pub struct BranchDto {
    pub name: String,
    #[serde(rename = "isHead")]
    pub is_head: bool,
}

impl From<BranchInfo> for BranchDto {
    fn from(b: BranchInfo) -> Self {
        Self {
            name: b.name,
            is_head: b.is_head,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorktreeDto {
    pub name: String,
    pub path: String,
}

impl From<WorktreeInfo> for WorktreeDto {
    fn from(w: WorktreeInfo) -> Self {
        Self {
            name: w.name,
            path: w.path.to_string_lossy().into_owned(),
        }
    }
}

/// Returns true iff `path` is a git repository (per libgit2).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_is_repo(path: String) -> bool {
    Repo::is_repo(Path::new(&path))
}

/// List local branches with a `isHead` marker on the current HEAD.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_branches(path: String) -> Result<Vec<BranchDto>, String> {
    let repo = Repo::open(Path::new(&path)).map_err(|e| e.to_string())?;
    let branches = repo.branches().map_err(|e| e.to_string())?;
    Ok(branches.into_iter().map(BranchDto::from).collect())
}

/// Default path proposal for a worktree under `repo_path` named after
/// `task_slug`. Pure function; no filesystem access.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_propose_worktree_path(repo_path: String, task_slug: String) -> String {
    propose_worktree_path(Path::new(&repo_path), &task_slug)
        .to_string_lossy()
        .into_owned()
}

/// Create a new worktree on a fresh branch. Returns the worktree path
/// (which the frontend uses as the new session's `cwd`).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_add_worktree(
    repo_path: String,
    branch: String,
    base_branch: String,
    worktree_path: String,
) -> Result<WorktreeDto, String> {
    let repo = Repo::open(Path::new(&repo_path)).map_err(|e| e.to_string())?;
    let info = repo
        .add_worktree(&branch, &base_branch, &PathBuf::from(&worktree_path))
        .map_err(|e| e.to_string())?;
    Ok(info.into())
}

#[derive(Debug, Serialize)]
pub struct StatusDto {
    pub path: String,
    /// Stringified `StatusKind`: "added" | "modified" | "deleted" |
    /// "renamed" | "untracked" | "conflicted" | "typechange". Wire format
    /// matches what the frontend expects in plain JSON.
    pub kind: &'static str,
    pub staged: bool,
}

impl From<StatusEntry> for StatusDto {
    fn from(s: StatusEntry) -> Self {
        let kind = match s.kind {
            StatusKind::Added => "added",
            StatusKind::Modified => "modified",
            StatusKind::Deleted => "deleted",
            StatusKind::Renamed => "renamed",
            StatusKind::Untracked => "untracked",
            StatusKind::Conflicted => "conflicted",
            StatusKind::Typechange => "typechange",
        };
        Self {
            path: s.path,
            kind,
            staged: s.staged,
        }
    }
}

/// Snapshot of the worktree's status — every changed file relative to
/// HEAD, sorted by path. The frontend re-fetches on demand (Phase 5a)
/// and via the file watcher (Phase 5b).
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_status(path: String) -> Result<Vec<StatusDto>, String> {
    let repo = Repo::open(Path::new(&path)).map_err(|e| e.to_string())?;
    let entries = repo.status().map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(StatusDto::from).collect())
}
