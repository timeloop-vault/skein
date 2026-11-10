//! The logic behind the git commands, around `skein-git`. Tauri-free: the
//! command wrappers live in `commands/git.rs`.
//!
//! This is thin glue: invoke the typed API and collapse `GitError` to
//! `String` for the Result. Any logic richer than that belongs in
//! `skein-git` so it stays testable on its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use parking_lot::Mutex;
use serde::Serialize;
use skein_git::{
    BranchInfo, DiffHunk, DiffLine, DiffLineKind, FileDiff, Repo, StatusEntry, StatusKind,
    WorktreeInfo, propose_worktree_path,
};

use crate::db::RepoIdentity;
use crate::stream_sink::StreamSink;
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
    /// Commits on the branch's upstream not reachable locally — a lower
    /// bound only as fresh as the last fetch (Skein never fetches).
    /// `None` when there's no upstream configured or it can't be
    /// resolved.
    #[serde(rename = "behindUpstream", skip_serializing_if = "Option::is_none")]
    pub behind_upstream: Option<usize>,
}

impl BranchDto {
    /// Builds a DTO from a name-only `BranchInfo`, filling
    /// `behind_upstream` with its own `graph_ahead_behind` walk. Only
    /// called from `git_inspect_folder`, on room open/create — never
    /// from the review surface's per-tick `Repo::branches` call, which
    /// only needs names and would pay this walk for nothing.
    fn from_info(repo: &Repo, b: BranchInfo) -> Self {
        let behind_upstream = repo.behind_upstream(&b.name).ok().flatten();
        Self {
            name: b.name,
            is_head: b.is_head,
            behind_upstream,
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
pub(crate) fn git_is_repo_impl(path: &str) -> bool {
    Repo::is_repo(Path::new(path))
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
    /// Shorthand remote-tracking branch names (e.g. "origin/main"), for
    /// offering a base that only exists upstream. Empty on error or for
    /// a non-repo/missing path — never fails the whole call.
    #[serde(rename = "remoteBranches")]
    pub remote_branches: Vec<String>,
}

/// The `repoRoot` a room created at `path` would get: `Some` when
/// `path` is inside a git checkout — the main checkout's path when
/// `path` is itself a linked worktree, else `path` unchanged — and
/// `None` when it is not a repo at all. Mirrors the frontend's
/// `worktreeRoom.ts` (`info.root` when `info.isRepo`, otherwise no
/// `repoRoot`) and is what [`git_inspect_folder_impl`] uses for that
/// same resolution below, so the two can't drift (issue #375's
/// per-repository-group room cap needs this outside the New Room
/// dialog's own round trip).
pub(crate) fn repo_root_for_path(path: &Path) -> Option<String> {
    let repo = Repo::open(path).ok()?;
    let root = repo.main_repo_root();
    if repo.is_worktree() && root != *repo.workdir() {
        Some(root.to_string_lossy().into_owned())
    } else {
        Some(path.to_string_lossy().into_owned())
    }
}

/// Inspect a folder for the New Room dialog: existence, repo-ness,
/// worktree resolution and the branch list, in a single call.
pub(crate) fn git_inspect_folder_impl(path: &str) -> Result<FolderInfoDto, String> {
    let picked = Path::new(path);
    let exists = picked.is_dir();
    let mut info = FolderInfoDto {
        exists,
        is_repo: false,
        root: path.to_owned(),
        resolved_from_worktree: false,
        branches: Vec::new(),
        head: None,
        remote_branches: Vec::new(),
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
    if let Some(root) = repo_root_for_path(picked)
        && root != path
    {
        info.resolved_from_worktree = true;
        info.root.clone_from(&root);
        // If the resolved root somehow will not open, keep the branches
        // of the worktree we did open rather than reporting none.
        if let Ok(main) = Repo::open(Path::new(&root)) {
            info.branches = main
                .branches()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|b| BranchDto::from_info(&main, b))
                .collect();
            info.head = main.head_branch();
            info.remote_branches = main.remote_branches().unwrap_or_default();
            return Ok(info);
        }
    }
    info.branches = repo
        .branches()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|b| BranchDto::from_info(&repo, b))
        .collect();
    info.head = repo.head_branch();
    info.remote_branches = repo.remote_branches().unwrap_or_default();
    Ok(info)
}

/// A repository's identity: what stays the same across clones and
/// worktrees (#418). `origin_url` is for display only.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RepoIdentityDto {
    #[serde(rename = "rootCommits")]
    pub root_commits: Vec<String>,
    #[serde(rename = "originUrl")]
    pub origin_url: Option<String>,
    /// Roots are a graft boundary, not real history: empty `rootCommits`
    /// then means "cannot tell", where for a non-shallow repo it means
    /// "no commits yet".
    #[serde(default)]
    pub shallow: bool,
}

/// `HEAD commit oid -> root commits`. The roots of a given commit never
/// change, and hydrate asks once per room, so remember them.
static ROOTS_MEMO: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const ROOTS_MEMO_MAX: usize = 256;

/// Root commits of `repo`'s HEAD, through the memo. Empty for an unborn
/// HEAD.
fn memoized_roots(repo: &Repo) -> Result<Vec<String>, String> {
    // A shallow clone's graft boundary reads as a root, and a later
    // deepen would change the answer for the same HEAD, so it is never
    // an identity and never memoized.
    if repo.is_shallow() {
        return Ok(Vec::new());
    }
    let Some(head) = repo.head_commit_id() else {
        return Ok(Vec::new());
    };
    if let Some(hit) = ROOTS_MEMO.lock().get(&head) {
        return Ok(hit.clone());
    }
    let roots = repo.root_commits().map_err(|e| e.to_string())?;
    let mut memo = ROOTS_MEMO.lock();
    if memo.len() > ROOTS_MEMO_MAX {
        memo.clear();
    }
    memo.insert(head, roots.clone());
    Ok(roots)
}

/// Whether the repository at a path is the one a room recorded (#418).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCheck {
    Same,
    Mismatch,
    /// Nothing to compare, or nothing there to compare against.
    Unknown,
}

/// Compare a room's stored identity with what `path` holds now. A missing
/// folder is `Unknown` (the #164 missing-folder flow owns it); an
/// existing folder that is not a repo is a `Mismatch`.
pub fn check_identity(stored: Option<&RepoIdentity>, path: &Path) -> IdentityCheck {
    let Some(stored) = stored.filter(|s| !s.root_commits.is_empty()) else {
        return IdentityCheck::Unknown;
    };
    if !path.exists() {
        return IdentityCheck::Unknown;
    }
    let Ok(repo) = Repo::open(path) else {
        return IdentityCheck::Mismatch;
    };
    // A shallow repo cannot vouch either way; an empty one (unborn HEAD)
    // cannot be the repo whose history was recorded.
    if repo.is_shallow() {
        return IdentityCheck::Unknown;
    }
    let Ok(roots) = memoized_roots(&repo) else {
        return IdentityCheck::Unknown;
    };
    if roots.is_empty() {
        IdentityCheck::Mismatch
    } else if roots.iter().any(|r| stored.root_commits.contains(r)) {
        IdentityCheck::Same
    } else {
        IdentityCheck::Mismatch
    }
}

pub(crate) fn repo_identity_impl(path: &str) -> Result<Option<RepoIdentityDto>, String> {
    let Ok(repo) = Repo::open(Path::new(path)) else {
        return Ok(None);
    };
    let root_commits = memoized_roots(&repo)?;
    Ok(Some(RepoIdentityDto {
        root_commits,
        origin_url: repo.origin_url(),
        shallow: repo.is_shallow(),
    }))
}

/// Current HEAD branch name, or `None` for detached HEAD / unborn branch /
/// non-repo path. Used by the bottom status bar to track checkouts that
/// happen inside a harness — `room.branch` is captured at room creation
/// and doesn't follow `git checkout`.
pub(crate) fn git_head_branch_impl(path: &str) -> Option<String> {
    Repo::open(Path::new(path)).ok()?.head_branch()
}

/// Default path proposal for a worktree under `repo_path` named after
/// `task_slug`. Pure function; no filesystem access.
pub(crate) fn git_propose_worktree_path_impl(repo_path: &str, task_slug: &str) -> String {
    propose_worktree_path(Path::new(repo_path), task_slug)
        .to_string_lossy()
        .into_owned()
}

/// Create a new worktree on a fresh branch. Returns the worktree path
/// (which the frontend uses as the new room's `cwd`).
///
/// Guarded by `ADD_WORKTREE_LOCK` for its whole body, which serializes it
/// against any other concurrent call (nothing else stops two of them from
/// colliding on the same repo), so the lock travels with the
/// open-then-add-worktree sequence it protects rather than living only
/// in the command closure, where an extracted-fn test could bypass it
/// unnoticed.
pub(crate) fn git_add_worktree_impl(
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
/// Guarded by `ADD_WORKTREE_LOCK` for its whole body — see
/// `git_add_worktree_impl`'s doc comment, same reasoning applies here.
pub(crate) fn git_restore_worktree_impl(
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
pub(crate) fn git_status_impl(path: &str) -> Result<Vec<StatusDto>, String> {
    let repo = Repo::open(Path::new(path)).map_err(|e| e.to_string())?;
    let entries = repo.status().map_err(|e| e.to_string())?;
    Ok(entries.into_iter().map(StatusDto::from).collect())
}

/// Start a recursive filesystem watcher rooted at `path`. `sink` is fired
/// (with no payload) every time a debounced quiet-window passes after a
/// real change — the frontend re-runs `git_status` in response. Returns
/// an opaque id; pass it to `git_watch_stop` to end the watch.
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
pub(crate) fn git_watch_start_impl(
    manager: &WatcherManager,
    path: &str,
    sink: impl StreamSink<()>,
    skip_ignored: Option<bool>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let extra: Vec<std::path::PathBuf> = Repo::worktree_admin_dir(Path::new(path))
        .into_iter()
        .collect();
    manager
        .start_worktree_watch(
            id.clone(),
            Path::new(path),
            &extra,
            skip_ignored.unwrap_or(false),
            move || {
                // The send only fails if the frontend dropped its half —
                // nothing useful we can do at that point.
                let _ = sink.send(());
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Watch only the git dir that holds `path`'s `HEAD` (non-recursive) and
/// fire `sink` when `HEAD` itself is touched — a branch switch,
/// detached commit or rebase. A build in the worktree never ticks.
/// `None` when `path` is not a repo (nothing watched). Stop it with
/// `git_watch_stop`.
pub(crate) fn git_head_watch_start_impl(
    manager: &WatcherManager,
    path: &str,
    sink: impl StreamSink<()>,
) -> Result<Option<String>, String> {
    let Some(dir) = Repo::head_dir(Path::new(path)) else {
        return Ok(None);
    };
    let id = uuid::Uuid::new_v4().to_string();
    manager
        .start_head_watch(id.clone(), &dir, move || {
            let _ = sink.send(());
        })
        .map_err(|e| e.to_string())?;
    Ok(Some(id))
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
pub(crate) fn git_diff_impl(path: &str) -> Result<Vec<FileDiffDto>, String> {
    let repo = Repo::open(Path::new(path)).map_err(|e| e.to_string())?;
    let files = repo.diff_workdir().map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(FileDiffDto::from).collect())
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

    fn stored_of(path: &Path) -> RepoIdentity {
        let dto = repo_identity_impl(&path.to_string_lossy())
            .unwrap()
            .unwrap();
        RepoIdentity {
            root_commits: dto.root_commits,
            origin_url: dto.origin_url,
        }
    }

    #[test]
    fn check_identity_covers_every_branch() {
        let (dir, base) = repo_with_commit();
        let stored = stored_of(dir.path());

        // Same repo, and a linked worktree of it.
        assert_eq!(
            check_identity(Some(&stored), dir.path()),
            IdentityCheck::Same
        );
        let wt_parent = TempDir::new().unwrap();
        let wt = wt_parent.path().join("wt");
        git_add_worktree_impl(
            &dir.path().to_string_lossy(),
            "feature/id",
            &base,
            &wt.to_string_lossy(),
        )
        .unwrap();
        assert_eq!(check_identity(Some(&stored), &wt), IdentityCheck::Same);

        // A different repo at the path (its own root commit).
        let (other, _) = repo_with_commit();
        {
            let repo = git2::Repository::open(other.path()).unwrap();
            let sig = git2::Signature::now("t", "t@example.com").unwrap();
            let tree = repo
                .find_tree(repo.index().unwrap().write_tree().unwrap())
                .unwrap();
            // Orphan root: distinct history, so a distinct root id.
            let c = repo
                .commit(None, &sig, &sig, "other root", &tree, &[])
                .unwrap();
            repo.reference("refs/heads/other", c, true, "t").unwrap();
            repo.set_head("refs/heads/other").unwrap();
        }
        assert_eq!(
            check_identity(Some(&stored), other.path()),
            IdentityCheck::Mismatch
        );

        // Plain directory: exists, not a repo.
        let plain = TempDir::new().unwrap();
        assert_eq!(
            check_identity(Some(&stored), plain.path()),
            IdentityCheck::Mismatch
        );

        // Missing folder.
        assert_eq!(
            check_identity(Some(&stored), &plain.path().join("nope")),
            IdentityCheck::Unknown
        );

        // Unborn HEAD: no commits cannot be the recorded history.
        let unborn = TempDir::new().unwrap();
        git2::Repository::init(unborn.path()).unwrap();
        assert_eq!(
            check_identity(Some(&stored), unborn.path()),
            IdentityCheck::Mismatch
        );

        // Nothing stored / empty roots.
        assert_eq!(check_identity(None, dir.path()), IdentityCheck::Unknown);
        let empty = RepoIdentity {
            root_commits: Vec::new(),
            origin_url: None,
        };
        assert_eq!(
            check_identity(Some(&empty), plain.path()),
            IdentityCheck::Unknown
        );
    }

    #[test]
    fn repo_identity_paths() {
        let (dir, _) = repo_with_commit();
        let p = dir.path().to_string_lossy().into_owned();
        let id = repo_identity_impl(&p).unwrap().unwrap();
        assert_eq!(id.root_commits.len(), 1);
        assert_eq!(id.origin_url, None);
        // Second call is served from the memo and agrees.
        assert_eq!(repo_identity_impl(&p).unwrap().unwrap(), id);

        let empty = TempDir::new().unwrap();
        git2::Repository::init(empty.path()).unwrap();
        let e = repo_identity_impl(&empty.path().to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(e.root_commits, Vec::<String>::new());
        assert!(!e.shallow);

        let plain = TempDir::new().unwrap();
        assert_eq!(
            repo_identity_impl(&plain.path().to_string_lossy()).unwrap(),
            None
        );
        assert_eq!(repo_identity_impl("Z:/does/not/exist").unwrap(), None);
    }

    #[test]
    fn a_shallow_repo_has_no_identity_and_is_never_memoized() {
        let (dir, _) = repo_with_commit();
        let p = dir.path().to_string_lossy().into_owned();
        let stored = stored_of(dir.path());
        assert_eq!(stored.root_commits.len(), 1);
        let head = git2::Repository::open(dir.path())
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap();
        // libgit2 honours `.git/shallow`: the named commit becomes the
        // graft boundary.
        std::fs::write(dir.path().join(".git").join("shallow"), format!("{head}\n")).unwrap();
        let id = repo_identity_impl(&p).unwrap().unwrap();
        assert_eq!(id.root_commits, Vec::<String>::new());
        assert!(id.shallow);
        assert_eq!(
            check_identity(Some(&stored), dir.path()),
            IdentityCheck::Unknown
        );
        // Unshallowing brings the real answer back, not a cached one.
        std::fs::remove_file(dir.path().join(".git").join("shallow")).unwrap();
        assert_eq!(
            repo_identity_impl(&p).unwrap().unwrap().root_commits.len(),
            1
        );
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
