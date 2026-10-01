//! Integration tests for `skein-git` — `head_blob` and `head_is_dir`.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use std::fs;
use std::path::Path;

use git2::{Repository, Signature};
use skein_git::Repo;
use tempfile::TempDir;
mod common;

use common::init_repo;

// ---------------------------------------------------------------------
// head_blob — the review baseline source (#211)
// ---------------------------------------------------------------------

#[test]
fn head_blob_returns_committed_content_not_the_working_tree() {
    let (_tmp, path) = init_repo();
    // The agent has already edited on disk by the time Skein sees the
    // patch row; the baseline must still be what was committed.
    fs::write(path.join("README.md"), b"edited by an agent\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    let blob = repo.head_blob("README.md").unwrap();
    assert_eq!(blob.as_deref(), Some(&b"hello\n"[..]));
}

#[test]
fn head_blob_is_none_for_a_path_git_never_tracked() {
    let (_tmp, path) = init_repo();
    fs::write(path.join("brand-new.rs"), b"fn main() {}\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.head_blob("brand-new.rs").unwrap(), None);
    assert_eq!(repo.head_blob("nested/deep/nope.rs").unwrap(), None);
}

#[test]
fn head_blob_is_none_on_an_unborn_head() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    Repository::init(&path).unwrap();
    fs::write(path.join("a.rs"), b"x\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.head_blob("a.rs").unwrap(), None);
}

/// Stage everything in the worktree and commit it onto HEAD.
fn commit_all(path: &Path, message: &str) {
    let repo = Repository::open(path).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
        .unwrap();
}

#[test]
fn head_blob_reads_a_nested_path() {
    let (_tmp, path) = init_repo();
    fs::create_dir_all(path.join("src/inner")).unwrap();
    fs::write(path.join("src/inner/lib.rs"), b"pub fn f() {}\n").unwrap();
    commit_all(&path, "add nested");

    let repo = Repo::open(&path).unwrap();
    assert_eq!(
        repo.head_blob("src/inner/lib.rs").unwrap().as_deref(),
        Some(&b"pub fn f() {}\n"[..])
    );
}

// head_is_dir — the directory-collapse discriminator for review
// discovery's catch-up fallback (#221)

#[test]
fn head_is_dir_true_for_a_committed_directory() {
    let (_tmp, path) = init_repo();
    fs::create_dir_all(path.join("src/inner")).unwrap();
    fs::write(path.join("src/inner/lib.rs"), b"pub fn f() {}\n").unwrap();
    commit_all(&path, "add nested");

    let repo = Repo::open(&path).unwrap();
    assert!(repo.head_is_dir("src/inner").unwrap());
    assert!(repo.head_is_dir("src").unwrap());
}

#[test]
fn head_is_dir_false_for_a_committed_file() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert!(!repo.head_is_dir("README.md").unwrap());
}

#[test]
fn head_is_dir_false_for_an_unknown_path() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert!(!repo.head_is_dir("nope").unwrap());
    assert!(!repo.head_is_dir("nested/deep/nope").unwrap());
}

#[test]
fn head_is_dir_false_on_an_unborn_head() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    Repository::init(&path).unwrap();

    let repo = Repo::open(&path).unwrap();
    assert!(!repo.head_is_dir("src").unwrap());
}
