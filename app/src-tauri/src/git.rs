//! Tauri command wrappers around `skein-git`.
//!
//! These are thin glue: take the inputs from JS, invoke the typed API,
//! collapse `GitError` to `String` for the Result. Any logic richer
//! than that belongs in `skein-git` so it stays testable without Tauri.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::Serialize;
use skein_git::{
    BranchInfo, DiffHunk, DiffLine, DiffLineKind, FileDiff, Repo, StatusEntry, StatusKind,
    WorktreeInfo, propose_worktree_path,
};
use tauri::ipc::Channel;

use crate::watcher::WatcherManager;

/// Serializes `git_add_worktree` (#171): nothing else stops two
/// concurrent calls against the same repo from colliding on the same
/// branch ref, and async scheduling is what makes that possible now
/// that the command runs on the blocking pool instead of inline on
/// the main thread.
static ADD_WORKTREE_LOCK: Mutex<()> = Mutex::new(());

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
///
/// Async: `is_repo` discovers `.git` by walking up from `path` — a
/// disk walk, and #171 wants that off the main thread.
#[tauri::command]
pub async fn git_is_repo(path: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || Repo::is_repo(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())
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

fn git_inspect_folder_impl(path: &str) -> Result<FolderInfoDto, String> {
    let picked = Path::new(path);
    let exists = picked.is_dir();
    let mut info = FolderInfoDto {
        exists,
        is_repo: false,
        root: path.to_owned(),
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
///
/// Async: opening the repo and reading HEAD are libgit2 calls that
/// touch disk, and this fires on every debounced watcher tick (#171).
#[tauri::command]
pub async fn git_head_branch(path: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || Repo::open(Path::new(&path)).ok()?.head_branch())
        .await
        .map_err(|e| e.to_string())
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

/// Guarded by `ADD_WORKTREE_LOCK` for its whole body (see
/// `git_add_worktree`'s doc comment) so the lock travels with the
/// open-then-add-worktree sequence it protects rather than living only
/// in the command closure, where an extracted-fn test could bypass it
/// unnoticed.
fn git_add_worktree_impl(
    repo_path: &str,
    branch: &str,
    base_branch: &str,
    worktree_path: &str,
) -> Result<WorktreeDto, String> {
    let _guard = ADD_WORKTREE_LOCK.lock();
    let repo = Repo::open(Path::new(repo_path)).map_err(|e| e.to_string())?;
    let info = repo
        .add_worktree(branch, base_branch, &PathBuf::from(worktree_path))
        .map_err(|e| e.to_string())?;
    Ok(info.into())
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

/// Guarded by `ADD_WORKTREE_LOCK` for its whole body — see
/// `git_add_worktree_impl`'s doc comment, same reasoning applies here.
fn git_restore_worktree_impl(
    repo_path: &str,
    branch: &str,
    worktree_path: &str,
) -> Result<WorktreeDto, String> {
    let _guard = ADD_WORKTREE_LOCK.lock();
    let repo = Repo::open(Path::new(repo_path)).map_err(|e| e.to_string())?;
    let info = repo
        .restore_worktree(branch, &PathBuf::from(worktree_path))
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
///
/// Async: `status` walks the whole worktree via libgit2 — #171 wants
/// that off the main thread, since this fires on every debounced
/// watcher tick.
#[tauri::command]
pub async fn git_status(path: String) -> Result<Vec<StatusDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let repo = Repo::open(Path::new(&path)).map_err(|e| e.to_string())?;
        let entries = repo.status().map_err(|e| e.to_string())?;
        Ok(entries.into_iter().map(StatusDto::from).collect())
    })
    .await
    .map_err(|e| e.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    /// A repo with one commit on its initial branch, whatever git2
    /// happens to name it (`master` unless `init.defaultBranch` says
    /// otherwise) — read back from HEAD rather than assumed.
    fn repo_with_commit() -> (TempDir, String) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let branch = repo.head().unwrap().shorthand().unwrap().to_owned();
        (dir, branch)
    }

    /// Two threads race `git_add_worktree_impl` for the same repo and
    /// the same new branch name (different worktree paths, so the only
    /// possible collision is the branch ref itself). A `Barrier` lines
    /// them up so both reach the guarded section at the same instant
    /// rather than merely running one after the other by scheduling
    /// luck — without `ADD_WORKTREE_LOCK` this is exactly the window
    /// where two libgit2 handles could both observe "branch absent"
    /// before either creates it.
    #[test]
    fn add_worktree_concurrent_same_branch_one_wins() {
        let (dir, base_branch) = repo_with_commit();
        // A dedicated parent for the candidate worktrees, separate from
        // the system temp root, so this test can't collide with another
        // test's own "wt-race-N" leaf names.
        let wt_parent = TempDir::new().unwrap();
        let repo_path = dir.path().to_string_lossy().into_owned();
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = Vec::new();
        for i in 0..2 {
            let repo_path = repo_path.clone();
            let base_branch = base_branch.clone();
            let barrier = Arc::clone(&barrier);
            let worktree_path = wt_parent
                .path()
                .join(format!("wt-race-{i}"))
                .to_string_lossy()
                .into_owned();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                git_add_worktree_impl(&repo_path, "feature/race", &base_branch, &worktree_path)
            }));
        }
        let results: Vec<Result<WorktreeDto, String>> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        let ok_count = results.iter().filter(|r| r.is_ok()).count();
        let err_count = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(ok_count, 1, "exactly one add_worktree call should win");
        assert_eq!(err_count, 1, "the other should fail, not silently no-op");

        // The losing call must not have left a half-created worktree:
        // the repo reports exactly one, and the loser's target
        // directory was never created.
        let raw = git2::Repository::open(dir.path()).unwrap();
        let worktrees = raw.worktrees().unwrap();
        assert_eq!(worktrees.len(), 1, "repo should have exactly one worktree");
        let existing: Vec<bool> = (0..2)
            .map(|i| wt_parent.path().join(format!("wt-race-{i}")).exists())
            .collect();
        assert_eq!(
            existing.iter().filter(|&&e| e).count(),
            1,
            "exactly one candidate worktree path should exist on disk"
        );
    }

    /// `git_restore_worktree_impl` is thin glue over
    /// `Repo::restore_worktree`, which has its own thorough coverage in
    /// `crates/skein-git/tests/repo.rs` — this just checks the wrapper
    /// wires it up correctly (#164).
    #[test]
    fn restore_worktree_impl_reattaches_after_directory_deleted() {
        let (dir, base_branch) = repo_with_commit();
        let wt_parent = TempDir::new().unwrap();
        let repo_path = dir.path().to_string_lossy().into_owned();
        let worktree_path = wt_parent
            .path()
            .join("wt-restore")
            .to_string_lossy()
            .into_owned();

        git_add_worktree_impl(&repo_path, "feature/restore", &base_branch, &worktree_path)
            .expect("add_worktree_impl");
        std::fs::remove_dir_all(&worktree_path).unwrap();

        let dto = git_restore_worktree_impl(&repo_path, "feature/restore", &worktree_path)
            .expect("restore_worktree_impl");
        assert_eq!(dto.path, worktree_path);
        assert!(Path::new(&worktree_path).exists());
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
///
/// Async: `diff_workdir` computes a real diff over the whole worktree —
/// libgit2 work that can be sizeable, and #171 wants it off the main
/// thread since this fires on every debounced watcher tick.
#[tauri::command]
pub async fn git_diff(path: String) -> Result<Vec<FileDiffDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let repo = Repo::open(Path::new(&path)).map_err(|e| e.to_string())?;
        let files = repo.diff_workdir().map_err(|e| e.to_string())?;
        Ok(files.into_iter().map(FileDiffDto::from).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}
