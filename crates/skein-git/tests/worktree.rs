//! Integration tests for `skein-git` — worktrees: add, restore, path proposal, root resolution.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use std::fs;
use std::path::Path;

use git2::Repository;
use skein_git::{Repo, propose_worktree_path};
use tempfile::TempDir;
mod common;

use common::init_repo;

#[test]
fn add_worktree_creates_branch_and_directory() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();

    let wt_path = propose_worktree_path(&path, "feat-foo");
    let info = repo
        .add_worktree("feat/foo", "main", &wt_path)
        .expect("add_worktree");

    // Returned info matches what we asked for.
    assert_eq!(info.path, wt_path);
    assert!(wt_path.exists(), "worktree dir should exist: {wt_path:?}");

    // The new branch shows up in `branches()` (next to main).
    let names: Vec<String> = repo
        .branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"feat/foo".to_string()), "got: {names:?}");
    assert!(names.contains(&"main".to_string()));

    // And the worktree shows up in `list_worktrees`.
    let worktrees = repo.list_worktrees().unwrap();
    assert_eq!(worktrees.len(), 1);
    // libgit2 stores the canonical path; on macOS the tempdir lives under
    // `/var/...` which is a symlink to `/private/var/...`, so the strings
    // only match after canonicalising both sides.
    assert_eq!(
        worktrees[0].path.canonicalize().unwrap(),
        wt_path.canonicalize().unwrap()
    );
}

#[test]
fn add_worktree_rejects_unknown_base_branch() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    match repo.add_worktree("feat/foo", "does-not-exist", &wt_path) {
        Err(skein_git::GitError::BranchNotFound(_)) => {}
        Err(other) => panic!("expected BranchNotFound, got {other:?}"),
        Ok(_) => panic!("expected BranchNotFound, got Ok"),
    }
}

#[test]
fn restore_worktree_after_directory_deleted_metadata_left() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    repo.add_worktree("feat/foo", "main", &wt_path).unwrap();

    let raw_repo = Repository::open(&path).unwrap();
    let oid_before = raw_repo
        .find_branch("feat/foo", git2::BranchType::Local)
        .unwrap()
        .get()
        .target()
        .unwrap();

    // Directory gone, but `.git/worktrees/feat-foo` metadata is left
    // behind — exactly the #164 symptom.
    fs::remove_dir_all(&wt_path).unwrap();

    let info = repo
        .restore_worktree("feat/foo", &wt_path)
        .expect("restore_worktree");
    assert_eq!(info.path, wt_path);
    assert!(wt_path.exists(), "worktree dir should exist: {wt_path:?}");

    let restored = Repository::open(&wt_path).unwrap();
    assert_eq!(restored.head().unwrap().shorthand().unwrap(), "feat/foo");

    let oid_after = raw_repo
        .find_branch("feat/foo", git2::BranchType::Local)
        .unwrap()
        .get()
        .target()
        .unwrap();
    assert_eq!(
        oid_before, oid_after,
        "restore must not move the branch tip"
    );
}

#[test]
fn restore_worktree_after_metadata_also_pruned() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    repo.add_worktree("feat/foo", "main", &wt_path).unwrap();

    fs::remove_dir_all(&wt_path).unwrap();
    let name = wt_path.file_name().unwrap().to_str().unwrap();
    repo.remove_worktree(name).unwrap();
    assert!(
        repo.list_worktrees().unwrap().is_empty(),
        "metadata should be gone after prune"
    );

    let info = repo
        .restore_worktree("feat/foo", &wt_path)
        .expect("restore_worktree");
    assert_eq!(info.path, wt_path);
    assert!(wt_path.exists(), "worktree dir should exist: {wt_path:?}");

    let restored = Repository::open(&wt_path).unwrap();
    assert_eq!(restored.head().unwrap().shorthand().unwrap(), "feat/foo");
}

#[test]
fn restore_worktree_refuses_when_parent_dir_also_gone() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    repo.add_worktree("feat/foo", "main", &wt_path).unwrap();

    // Remove the whole `<repo>-wt` parent directory, not just the leaf
    // worktree dir — this is indistinguishable from an unmounted drive
    // or an offline share from `symlink_metadata`'s point of view, so
    // the entry must be refused, not pruned (#164 review).
    let parent = wt_path.parent().unwrap();
    fs::remove_dir_all(parent).unwrap();

    let name = wt_path.file_name().unwrap().to_str().unwrap().to_owned();
    match repo.restore_worktree("feat/foo", &wt_path) {
        Err(skein_git::GitError::WorktreeUnreachable(_, _)) => {}
        Err(other) => panic!("expected WorktreeUnreachable, got {other:?}"),
        Ok(_) => panic!("expected WorktreeUnreachable, got Ok"),
    }
    let names: Vec<String> = repo
        .list_worktrees()
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert!(
        names.contains(&name),
        "stale-looking entry must not be pruned when it can't be confirmed absent: {names:?}"
    );
}

#[test]
fn restore_worktree_rejects_unknown_branch() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    match repo.restore_worktree("does-not-exist", &wt_path) {
        Err(skein_git::GitError::BranchNotFound(_)) => {}
        Err(other) => panic!("expected BranchNotFound, got {other:?}"),
        Ok(_) => panic!("expected BranchNotFound, got Ok"),
    }
}

#[test]
fn restore_worktree_rejects_branch_checked_out_live() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-foo");
    repo.add_worktree("feat/foo", "main", &wt_path).unwrap();

    // The branch is still checked out live at `wt_path` — restoring it
    // at a second, different path must be refused, not forced.
    let other_path = propose_worktree_path(&path, "feat-foo-2");
    match repo.restore_worktree("feat/foo", &other_path) {
        Err(skein_git::GitError::BranchCheckedOut(_)) => {}
        Err(other) => panic!("expected BranchCheckedOut, got {other:?}"),
        Ok(_) => panic!("expected BranchCheckedOut, got Ok"),
    }
    assert!(!other_path.exists());
}

#[test]
fn enclosing_workdir_walks_up_from_a_nested_folder() {
    let (_tmp, path) = init_repo();
    let nested = path.join("src").join("deep");
    fs::create_dir_all(&nested).unwrap();

    let found = Repo::enclosing_workdir(&nested).expect("inside a checkout");
    // Same `/var` → `/private/var` symlink caveat as the worktree test.
    assert_eq!(found.canonicalize().unwrap(), path.canonicalize().unwrap());
}

#[test]
fn enclosing_workdir_names_the_linked_worktree_not_the_main_checkout() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "enclosing");
    repo.add_worktree("feat/enclosing", "main", &wt_path)
        .unwrap();
    let nested = wt_path.join("sub");
    fs::create_dir_all(&nested).unwrap();

    let found = Repo::enclosing_workdir(&nested).expect("inside a worktree");
    assert_eq!(
        found.canonicalize().unwrap(),
        wt_path.canonicalize().unwrap()
    );
}

#[test]
fn worktree_admin_dir_is_some_for_a_linked_worktree_and_none_for_main() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "admin");
    repo.add_worktree("feat/admin", "main", &wt_path).unwrap();

    let admin = Repo::worktree_admin_dir(&wt_path).expect("linked worktree has an admin dir");
    let admin = admin.canonicalize().unwrap();
    assert!(admin.join("HEAD").is_file());
    assert!(!admin.starts_with(wt_path.canonicalize().unwrap()));
    assert_eq!(
        admin.parent().unwrap().parent().unwrap(),
        path.join(".git").canonicalize().unwrap()
    );

    assert_eq!(Repo::worktree_admin_dir(&path), None);
    let tmp = TempDir::new().unwrap();
    assert_eq!(Repo::worktree_admin_dir(tmp.path()), None);
}

#[test]
fn enclosing_workdir_is_none_outside_any_repo() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(Repo::enclosing_workdir(tmp.path()), None);
}

#[test]
fn propose_worktree_path_uses_sibling_dir() {
    let p = propose_worktree_path(Path::new("/tmp/code/skein"), "foo");
    assert_eq!(p, Path::new("/tmp/code/skein-wt/foo"));
}

// ── main_repo_root / is_worktree (#226) ────────────────────────────
//
// The New Room dialog resolves whatever folder it is handed back to the
// repo a worktree should be created *from*. Browsing into a Skein
// worktree is one misclick away — the `-wt` sibling dir sits right next
// to the repo — and without this it would stack a worktree on a
// worktree.

#[test]
fn main_repo_root_is_identity_for_a_plain_repo() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();

    assert!(!repo.is_worktree());
    assert_eq!(
        repo.main_repo_root().canonicalize().unwrap(),
        path.canonicalize().unwrap()
    );
}

#[test]
fn main_repo_root_resolves_a_linked_worktree_to_its_main_checkout() {
    let (_tmp, path) = init_repo();
    let main = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "resolve-me");
    main.add_worktree("feat/resolve-me", "main", &wt_path)
        .expect("add_worktree");

    // Opening the worktree directly: `workdir` is the worktree itself,
    // which is exactly the trap — only `main_repo_root` climbs out.
    let wt = Repo::open(&wt_path).unwrap();
    assert!(wt.is_worktree());
    assert_eq!(
        wt.workdir().canonicalize().unwrap(),
        wt_path.canonicalize().unwrap()
    );
    assert_eq!(
        wt.main_repo_root().canonicalize().unwrap(),
        path.canonicalize().unwrap()
    );
}

#[test]
fn main_repo_root_of_a_resolved_root_is_stable() {
    // Resolution has to be idempotent: the dialog re-validates whatever
    // it put in the field, and a second pass must not move the answer.
    let (_tmp, path) = init_repo();
    let main = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "stable");
    main.add_worktree("feat/stable", "main", &wt_path).unwrap();

    let once = Repo::open(&wt_path).unwrap().main_repo_root();
    let twice = Repo::open(&once).unwrap().main_repo_root();
    assert_eq!(
        once.canonicalize().unwrap(),
        twice.canonicalize().unwrap(),
        "resolving an already-resolved root should be a no-op"
    );
}

#[test]
fn main_repo_root_falls_back_to_workdir_for_a_bare_repo() {
    // A bare repo has no checkout at all. `main_repo_root` must not
    // invent one — it hands back what it was opened with, which is the
    // same thing every other caller already sees.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    Repository::init_bare(&path).unwrap();

    let repo = Repo::open(&path).unwrap();
    assert!(!repo.is_worktree());
    assert_eq!(
        repo.main_repo_root().canonicalize().unwrap(),
        path.canonicalize().unwrap()
    );
}
