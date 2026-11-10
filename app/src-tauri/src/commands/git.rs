//! The git commands (Phases 5a/5b, #164, #171, #226, #418, #563, #637).
//! The logic and DTOs live in `git`; this file only moves each call onto
//! the blocking pool, resolves Tauri state and turns the frontend's
//! channel into a sink.

use tauri::ipc::Channel;

use crate::git::{
    FileDiffDto, FolderInfoDto, RepoIdentityDto, StatusDto, WorktreeDto, git_add_worktree_impl,
    git_diff_impl, git_head_branch_impl, git_head_watch_start_impl, git_inspect_folder_impl,
    git_is_repo_impl, git_propose_worktree_path_impl, git_restore_worktree_impl, git_status_impl,
    git_watch_start_impl, repo_identity_impl,
};
use crate::tauri_stream_sink::TauriChannelSink;
use crate::watcher::WatcherManager;

/// Returns true iff `path` is a git repository (per libgit2).
///
/// Async: `is_repo` discovers `.git` by walking up from `path` — a
/// disk walk, and #171 wants that off the main thread.
#[tauri::command]
pub async fn git_is_repo(path: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || git_is_repo_impl(&path))
        .await
        .map_err(|e| e.to_string())
}

/// Inspect a folder for the New Room dialog: existence, repo-ness,
/// worktree resolution and the branch list, in a single call.
///
/// Async: opens the repo, resolves the worktree root and walks the
/// branch list — several libgit2 calls, each of which touches disk
/// (#171).
#[tauri::command]
pub async fn git_inspect_folder(path: String) -> Result<FolderInfoDto, String> {
    tauri::async_runtime::spawn_blocking(move || git_inspect_folder_impl(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// The repository identity of `path` (root commit ids + origin URL);
/// `None` when the path is missing or not a repo. Async for the same
/// reason as [`git_inspect_folder`] (#171).
#[tauri::command]
pub async fn git_repo_identity(path: String) -> Result<Option<RepoIdentityDto>, String> {
    tauri::async_runtime::spawn_blocking(move || repo_identity_impl(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Current HEAD branch name, or `None` for detached HEAD / unborn branch /
/// non-repo path. Used by the bottom status bar to track checkouts that
/// happen inside a harness — `room.branch` is captured at room creation
/// and doesn't follow `git checkout`.
///
/// Async: opening the repo and reading HEAD are libgit2 calls that
/// touch disk, and this fires on every debounced watcher tick (#171).
#[tauri::command]
pub async fn git_head_branch(path: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || git_head_branch_impl(&path))
        .await
        .map_err(|e| e.to_string())
}

/// Default path proposal for a worktree under `repo_path` named after
/// `task_slug`. Pure function; no filesystem access.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_propose_worktree_path(repo_path: String, task_slug: String) -> String {
    git_propose_worktree_path_impl(&repo_path, &task_slug)
}

/// Create a new worktree on a fresh branch. Returns the worktree path
/// (which the frontend uses as the new room's `cwd`).
///
/// Async: `add_worktree` creates a branch ref and a new checkout on
/// disk — real libgit2 + filesystem work (#171). `ADD_WORKTREE_LOCK`
/// serializes it against any other concurrent call, since nothing else
/// stops two of them from colliding on the same repo.
#[tauri::command]
pub async fn git_add_worktree(
    repo_path: String,
    branch: String,
    base_branch: String,
    worktree_path: String,
) -> Result<WorktreeDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_add_worktree_impl(&repo_path, &branch, &base_branch, &worktree_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Re-attach an existing branch as a worktree at `worktree_path` (#164):
/// the room's worktree directory was deleted but its branch survives.
/// Never creates or moves a branch — see `Repo::restore_worktree`.
///
/// Async + `ADD_WORKTREE_LOCK`: same reasoning as `git_add_worktree` —
/// this also does real libgit2 + filesystem work, and shares the same
/// `.git/worktrees/` namespace it would race on.
#[tauri::command]
pub async fn git_restore_worktree(
    repo_path: String,
    branch: String,
    worktree_path: String,
) -> Result<WorktreeDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git_restore_worktree_impl(&repo_path, &branch, &worktree_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Snapshot of the worktree's status — every changed file relative to
/// HEAD, sorted by path. The frontend re-fetches on demand (Phase 5a)
/// and via the file watcher (Phase 5b).
///
/// Async: `status` walks the whole worktree via libgit2 — #171 wants
/// that off the main thread, since this fires on every debounced
/// watcher tick.
#[tauri::command]
pub async fn git_status(path: String) -> Result<Vec<StatusDto>, String> {
    tauri::async_runtime::spawn_blocking(move || git_status_impl(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Start a recursive filesystem watcher rooted at `path`. `on_change`
/// is fired (with no payload) every time a debounced quiet-window
/// passes after a real change — the frontend re-runs `git_status` in
/// response. Returns an opaque id; pass it to `git_watch_stop` to end
/// the watch.
///
/// For a linked worktree (every Skein room) HEAD, the index and
/// `logs/HEAD` live in the worktree's admin dir under the main repo's
/// `.git/worktrees/`, outside `path`, so that dir is watched too or a
/// commit of already-on-disk files would never tick (#563). The shared
/// common dir is not watched: sibling rooms' commits would tick us.
///
/// `skip_ignored` (default false) drops batches made only of gitignored
/// worktree paths (#637), for the review pane. `.git/` stays unfiltered.
/// The file tree watches arbitrary subdirectories, possibly inside ignored
/// ones, so it leaves this off.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_watch_start(
    path: String,
    on_change: Channel<()>,
    skip_ignored: Option<bool>,
    manager: tauri::State<'_, WatcherManager>,
) -> Result<String, String> {
    git_watch_start_impl(
        &manager,
        &path,
        TauriChannelSink::new(on_change),
        skip_ignored,
    )
}

/// Watch only the git dir that holds `path`'s `HEAD` (non-recursive) and
/// fire `on_change` when `HEAD` itself is touched — a branch switch,
/// detached commit or rebase. A build in the worktree never ticks.
/// `None` when `path` is not a repo (nothing watched). Stop it with
/// `git_watch_stop`.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_head_watch_start(
    path: String,
    on_change: Channel<()>,
    manager: tauri::State<'_, WatcherManager>,
) -> Result<Option<String>, String> {
    git_head_watch_start_impl(&manager, &path, TauriChannelSink::new(on_change))
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn git_watch_stop(id: String, manager: tauri::State<'_, WatcherManager>) {
    manager.stop(&id);
}

/// Structured diff of every changed file in the worktree against HEAD.
/// One entry per file; binary files have `binary: true` and empty hunks.
///
/// Async: `diff_workdir` computes a real diff over the whole worktree —
/// libgit2 work that can be sizeable, and #171 wants it off the main
/// thread since this fires on every debounced watcher tick.
#[tauri::command]
pub async fn git_diff(path: String) -> Result<Vec<FileDiffDto>, String> {
    tauri::async_runtime::spawn_blocking(move || git_diff_impl(&path))
        .await
        .map_err(|e| e.to_string())?
}
