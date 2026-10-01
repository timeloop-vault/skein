//! Integration tests for `skein-git` — opening, branches, ignore checks, root commits, origin.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use std::fs;
use std::path::Path;

use git2::{Repository, Signature};
use skein_git::{BranchInfo, Repo};
use tempfile::TempDir;
mod common;

use common::init_repo;

#[test]
fn is_repo_detects_non_repo() {
    let tmp = TempDir::new().unwrap();
    assert!(!Repo::is_repo(tmp.path()));
}

#[test]
fn is_repo_detects_real_repo() {
    let (_tmp, path) = init_repo();
    assert!(Repo::is_repo(&path));
}

#[test]
fn branches_lists_main_with_head_marker() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let branches = repo.branches().unwrap();
    assert_eq!(
        branches,
        vec![BranchInfo {
            name: "main".into(),
            is_head: true,
        }]
    );
    assert_eq!(repo.head_branch().as_deref(), Some("main"));
}

#[test]
fn open_rejects_non_repo() {
    let tmp = TempDir::new().unwrap();
    match Repo::open(tmp.path()) {
        Err(skein_git::GitError::NotARepo(_)) => {}
        Err(other) => panic!("expected NotARepo, got {other:?}"),
        Ok(_) => panic!("expected NotARepo, got Ok"),
    }
}

#[test]
fn open_rejects_missing_path() {
    match Repo::open(Path::new("/nope/this/does/not/exist")) {
        Err(skein_git::GitError::PathMissing(_)) => {}
        Err(other) => panic!("expected PathMissing, got {other:?}"),
        Ok(_) => panic!("expected PathMissing, got Ok"),
    }
}

// is_path_ignored — the watcher-discovery gate (#221)

#[test]
fn is_path_ignored_honors_gitignore() {
    let (_tmp, path) = init_repo();
    fs::write(path.join(".gitignore"), "target/\n*.log\n").unwrap();
    fs::create_dir_all(path.join("target")).unwrap();
    fs::write(path.join("target/out.bin"), b"bin").unwrap();
    fs::write(path.join("debug.log"), b"log").unwrap();
    fs::write(path.join("src.rs"), b"fn main() {}").unwrap();

    let repo = Repo::open(&path).unwrap();
    assert!(repo.is_path_ignored("target/out.bin").unwrap());
    assert!(repo.is_path_ignored("debug.log").unwrap());
    assert!(!repo.is_path_ignored("src.rs").unwrap());
    // The .gitignore file itself is not ignored.
    assert!(!repo.is_path_ignored(".gitignore").unwrap());
}

fn commit_file(repo: &Repository, name: &str) -> git2::Oid {
    let workdir = repo.workdir().unwrap().to_path_buf();
    fs::write(workdir.join(name), name.as_bytes()).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(name)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    let parents: Vec<git2::Commit> = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect();
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, name, &tree, &refs)
        .unwrap()
}

#[test]
fn root_commits_single_commit_is_itself() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let head = repo.head_commit_id().unwrap();
    assert_eq!(repo.root_commits().unwrap(), vec![head]);
}

#[test]
fn root_commits_linear_history_is_the_first() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    let first = raw.head().unwrap().peel_to_commit().unwrap().id();
    commit_file(&raw, "b.txt");
    commit_file(&raw, "c.txt");
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.root_commits().unwrap(), vec![first.to_string()]);
    assert_ne!(repo.head_commit_id().unwrap(), first.to_string());
}

#[test]
fn root_commits_of_independent_repos_are_disjoint() {
    // Distinct commit messages, so the ids differ even when both repos
    // are created within the same second.
    let mk = |name: &str| {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, name);
        tmp
    };
    let (a, b) = (mk("a.txt"), mk("b.txt"));
    let ra = Repo::open(a.path()).unwrap().root_commits().unwrap();
    let rb = Repo::open(b.path()).unwrap().root_commits().unwrap();
    assert_eq!((ra.len(), rb.len()), (1, 1));
    assert_ne!(ra, rb);
}

#[test]
fn root_commits_match_across_clone_and_worktree() {
    let (_tmp, path) = init_repo();
    let raw = Repository::open(&path).unwrap();
    commit_file(&raw, "b.txt");
    let want = Repo::open(&path).unwrap().root_commits().unwrap();

    let clone_dir = TempDir::new().unwrap();
    Repository::clone(path.to_str().unwrap(), clone_dir.path()).unwrap();
    assert_eq!(
        Repo::open(clone_dir.path())
            .unwrap()
            .root_commits()
            .unwrap(),
        want
    );

    let wt_parent = TempDir::new().unwrap();
    let wt_path = wt_parent.path().join("wt");
    let wt = Repo::open(&path)
        .unwrap()
        .add_worktree("feat/x", "main", &wt_path)
        .unwrap();
    assert_eq!(Repo::open(&wt.path).unwrap().root_commits().unwrap(), want);
}

#[test]
fn root_commits_empty_repo_is_empty() {
    let tmp = TempDir::new().unwrap();
    Repository::init(tmp.path()).unwrap();
    let repo = Repo::open(tmp.path()).unwrap();
    assert!(repo.root_commits().unwrap().is_empty());
    assert!(repo.head_commit_id().is_none());
}

#[test]
fn origin_url_set_and_unset() {
    let (_tmp, path) = init_repo();
    assert_eq!(Repo::open(&path).unwrap().origin_url(), None);
    Repository::open(&path)
        .unwrap()
        .remote("origin", "https://example.com/a/b.git")
        .unwrap();
    assert_eq!(
        Repo::open(&path).unwrap().origin_url().as_deref(),
        Some("https://example.com/a/b.git")
    );
}
