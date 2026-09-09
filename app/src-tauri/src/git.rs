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

/// Everything the New Room dialog needs to know about a picked folder,
/// in one round-trip (#226).
///
/// It replaces the `git_is_repo` + `git_branches` pair the dialog used
/// to call in sequence. One command means one in-flight request to
/// cancel and one atomic answer to render, rather than a window where
/// the folder is known to be a repo but its branches are not yet
/// loaded — which is exactly the window a prefilled field opens into.
#[derive(Debug, Serialize)]
pub struct FolderInfoDto {
    /// Whether the path is a directory at all. Distinguishes "the
    /// remembered folder is gone" from "this folder is not a repo",
    /// which `git_is_repo` alone reports identically as `false` — and
    /// the latter is a legitimately submittable state.
    pub exists: bool,
    #[serde(rename = "isRepo")]
    pub is_repo: bool,
    /// The repo a worktree should be created from. Equal to the path
    /// handed in for a plain repo, a non-repo, or a missing path; the
    /// main checkout when the path is a linked worktree.
    pub root: String,
    /// Set when `root` differs from the requested path. The dialog uses
    /// it to explain why the folder it shows is not the one picked.
    #[serde(rename = "resolvedFromWorktree")]
    pub resolved_from_worktree: bool,
    pub branches: Vec<BranchDto>,
    pub head: Option<String>,
}

/// Inspect a folder for the New Room dialog: existence, repo-ness,
/// worktree resolution and the branch list, in a single call.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_inspect_folder(path: String) -> Result<FolderInfoDto, String> {
    let picked = Path::new(&path);
    let exists = picked.is_dir();
    let mut info = FolderInfoDto {
        exists,
        is_repo: false,
        root: path.clone(),
        resolved_from_worktree: false,
        branches: Vec::new(),
        head: None,
    };
    if !exists {
        return Ok(info);
    }
    // A non-repo folder is a valid choice (harnesses just run in it), so
    // failing to open is not an error to surface — it is an answer.
    let Ok(repo) = Repo::open(picked) else {
        return Ok(info);
    };
    info.is_repo = true;

    // Resolve a linked worktree back to its main checkout, then re-open
    // there: the branch list and HEAD the user picks from must describe
    // the repo the worktree will actually be created in.
    let root = repo.main_repo_root();
    if repo.is_worktree() && root != *repo.workdir() {
        info.resolved_from_worktree = true;
        info.root = root.to_string_lossy().into_owned();
        // If the resolved root somehow will not open, keep the branches
        // of the worktree we did open rather than reporting none.
        if let Ok(main) = Repo::open(&root) {
            info.branches = main
                .branches()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(BranchDto::from)
                .collect();
            info.head = main.head_branch();
            return Ok(info);
        }
    }
    info.branches = repo
        .branches()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(BranchDto::from)
        .collect();
    info.head = repo.head_branch();
    Ok(info)
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
