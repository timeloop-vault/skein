//! Integration tests for `skein-git` — remote-tracking bases, `behind_upstream`, `remote_branches`.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use git2::{Repository, Signature};
use skein_git::{Repo, propose_worktree_path};
mod common;

use common::init_repo;

// ---------------------------------------------------------------------
// Stale local base (#367): resolving `base_branch` against a remote-
// tracking branch, and `behind_upstream` / `remote_branches`. Fixtures
// simulate "the remote" entirely with git2 ref/object writes — never a
// real fetch or a spawned `git` process, per D9 and the GIT_DIR trap.
// ---------------------------------------------------------------------

/// Create a commit layered on top of `parent`, writing straight to the
/// object database (no working-directory checkout, no ref moved) — used
/// to simulate a remote-tracking branch that is ahead of local history
/// without a real fetch.
fn commit_dangling(
    repo: &Repository,
    parent: &git2::Commit,
    filename: &str,
    contents: &[u8],
) -> git2::Oid {
    let blob_oid = repo.blob(contents).unwrap();
    let parent_tree = parent.tree().unwrap();
    let mut builder = repo.treebuilder(Some(&parent_tree)).unwrap();
    builder.insert(filename, blob_oid, 0o100_644).unwrap();
    let tree_oid = builder.write().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    repo.commit(None, &sig, &sig, "dangling", &tree, &[parent])
        .unwrap()
}

#[test]
fn add_worktree_bases_on_remote_tracking_branch_when_no_local_branch() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let local_main = raw.head().unwrap().peel_to_commit().unwrap();
    let remote_oid = commit_dangling(&raw, &local_main, "upstream-only.txt", b"x\n");
    raw.reference("refs/remotes/origin/feature", remote_oid, true, "test")
        .unwrap();

    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-remote");
    repo.add_worktree("feat/remote", "origin/feature", &wt_path)
        .expect("add_worktree from a remote-tracking base");

    let new_branch = raw
        .find_branch("feat/remote", git2::BranchType::Local)
        .unwrap();
    assert_eq!(new_branch.get().target(), Some(remote_oid));

    // No local branch named "feature" was ever created — only the
    // requested "feat/remote".
    let names: Vec<String> = repo
        .branches()
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(!names.contains(&"feature".to_string()), "got: {names:?}");
}

#[test]
fn add_worktree_prefers_local_branch_over_same_named_remote() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let local_main = raw.head().unwrap().peel_to_commit().unwrap();
    let local_oid = local_main.id();
    // A same-named remote-tracking branch, deliberately ahead — local
    // must still win.
    let remote_oid = commit_dangling(&raw, &local_main, "ahead.txt", b"x\n");
    raw.reference("refs/remotes/origin/main", remote_oid, true, "test")
        .unwrap();

    let repo = Repo::open(&path).unwrap();
    let wt_path = propose_worktree_path(&path, "feat-local-wins");
    repo.add_worktree("feat/local-wins", "main", &wt_path)
        .expect("add_worktree");

    let new_branch = raw
        .find_branch("feat/local-wins", git2::BranchType::Local)
        .unwrap();
    assert_eq!(new_branch.get().target(), Some(local_oid));
}

#[test]
fn behind_upstream_counts_commits_only_on_the_remote() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let local_main = raw.head().unwrap().peel_to_commit().unwrap();
    let ahead1 = commit_dangling(&raw, &local_main, "a.txt", b"a\n");
    let ahead1_commit = raw.find_commit(ahead1).unwrap();
    let ahead2 = commit_dangling(&raw, &ahead1_commit, "b.txt", b"b\n");
    raw.reference("refs/remotes/origin/main", ahead2, true, "test")
        .unwrap();
    // `set_upstream` needs a `remote.origin.*` config entry to resolve
    // "origin/main" against — the URL is never dialed, since nothing
    // here fetches.
    raw.remote("origin", "https://example.invalid/origin.git")
        .unwrap();
    raw.find_branch("main", git2::BranchType::Local)
        .unwrap()
        .set_upstream(Some("origin/main"))
        .unwrap();

    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.behind_upstream("main").unwrap(), Some(2));
}

#[test]
fn behind_upstream_is_zero_when_up_to_date() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let local_oid = raw.head().unwrap().target().unwrap();
    raw.reference("refs/remotes/origin/main", local_oid, true, "test")
        .unwrap();
    raw.remote("origin", "https://example.invalid/origin.git")
        .unwrap();
    raw.find_branch("main", git2::BranchType::Local)
        .unwrap()
        .set_upstream(Some("origin/main"))
        .unwrap();

    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.behind_upstream("main").unwrap(), Some(0));
}

#[test]
fn behind_upstream_is_none_without_an_upstream_configured() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.behind_upstream("main").unwrap(), None);
}

#[test]
fn remote_branches_lists_names_and_excludes_symbolic_head() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let local_oid = raw.head().unwrap().target().unwrap();
    raw.reference("refs/remotes/origin/main", local_oid, true, "test")
        .unwrap();
    raw.reference_symbolic(
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
        true,
        "test",
    )
    .unwrap();

    let repo = Repo::open(&path).unwrap();
    assert_eq!(
        repo.remote_branches().unwrap(),
        vec!["origin/main".to_string()]
    );
}
