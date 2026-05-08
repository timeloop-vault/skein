//! Tauri command wrappers around `skein-git`.
//!
//! These are thin glue: take the inputs from JS, invoke the typed API,
//! collapse `GitError` to `String` for the Result. Any logic richer
//! than that belongs in `skein-git` so it stays testable without Tauri.

use std::path::{Path, PathBuf};

use serde::Serialize;
use skein_git::{
    BranchInfo, DiffHunk, DiffLine, DiffLineKind, FileDiff, Repo, StatusEntry, StatusKind,
    WorktreeInfo, propose_worktree_path,
};
use tauri::ipc::Channel;

use crate::watcher::WatcherManager;

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

/// Current HEAD branch name, or `None` for detached HEAD / unborn branch /
/// non-repo path. Used by the bottom status bar to track checkouts that
/// happen inside a harness — `room.branch` is captured at room creation
/// and doesn't follow `git checkout`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_head_branch(path: String) -> Option<String> {
    Repo::open(Path::new(&path)).ok()?.head_branch()
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
/// (which the frontend uses as the new room's `cwd`).
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
        Self {
            path: s.path,
            kind: status_kind_str(s.kind),
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

/// Start a recursive filesystem watcher rooted at `path`. `on_change`
/// is fired (with no payload) every time a debounced quiet-window
/// passes after a real change — the frontend re-runs `git_status` in
/// response. Returns an opaque id; pass it to `git_watch_stop` to end
/// the watch.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_watch_start(
    path: String,
    on_change: Channel<()>,
    manager: tauri::State<'_, WatcherManager>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    manager
        .start(id.clone(), Path::new(&path), move || {
            // The channel send only fails if the frontend dropped its
            // half — nothing useful we can do at that point.
            let _ = on_change.send(());
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_watch_stop(id: String, manager: tauri::State<'_, WatcherManager>) {
    manager.stop(&id);
}

#[derive(Debug, Serialize)]
pub struct DiffLineDto {
    /// "context" | "add" | "delete".
    pub kind: &'static str,
    pub content: String,
    #[serde(rename = "oldLineno", skip_serializing_if = "Option::is_none")]
    pub old_lineno: Option<u32>,
    #[serde(rename = "newLineno", skip_serializing_if = "Option::is_none")]
    pub new_lineno: Option<u32>,
}

impl From<DiffLine> for DiffLineDto {
    fn from(l: DiffLine) -> Self {
        let kind = match l.kind {
            DiffLineKind::Context => "context",
            DiffLineKind::Add => "add",
            DiffLineKind::Delete => "delete",
        };
        Self {
            kind,
            content: l.content,
            old_lineno: l.old_lineno,
            new_lineno: l.new_lineno,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DiffHunkDto {
    pub header: String,
    pub lines: Vec<DiffLineDto>,
}

impl From<DiffHunk> for DiffHunkDto {
    fn from(h: DiffHunk) -> Self {
        Self {
            header: h.header,
            lines: h.lines.into_iter().map(DiffLineDto::from).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FileDiffDto {
    pub path: String,
    /// Same string set as `StatusDto::kind`.
    pub kind: &'static str,
    pub binary: bool,
    pub hunks: Vec<DiffHunkDto>,
}

impl From<FileDiff> for FileDiffDto {
    fn from(f: FileDiff) -> Self {
        let kind = status_kind_str(f.kind);
        Self {
            path: f.path,
            kind,
            binary: f.binary,
            hunks: f.hunks.into_iter().map(DiffHunkDto::from).collect(),
        }
    }
}

const fn status_kind_str(k: StatusKind) -> &'static str {
    match k {
        StatusKind::Added => "added",
        StatusKind::Modified => "modified",
        StatusKind::Deleted => "deleted",
        StatusKind::Renamed => "renamed",
        StatusKind::Untracked => "untracked",
        StatusKind::Conflicted => "conflicted",
        StatusKind::Typechange => "typechange",
    }
}

/// Structured diff of every changed file in the worktree against HEAD.
/// One entry per file; binary files have `binary: true` and empty hunks.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_diff(path: String) -> Result<Vec<FileDiffDto>, String> {
    let repo = Repo::open(Path::new(&path)).map_err(|e| e.to_string())?;
    let files = repo.diff_workdir().map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(FileDiffDto::from).collect())
}
