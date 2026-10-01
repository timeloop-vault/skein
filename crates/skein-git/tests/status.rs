//! Integration tests for `skein-git` — worktree status.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use std::fs;
use std::path::Path;

use git2::Repository;
use skein_git::{Repo, StatusKind};
mod common;

use common::init_repo;

#[test]
fn status_clean_repo_is_empty() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let status = repo.status().unwrap();
    assert!(status.is_empty(), "expected empty status, got {status:?}");
}

#[test]
fn status_reports_untracked_file() {
    let (_tmp, path) = init_repo();
    fs::write(path.join("new.txt"), b"hi\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, "new.txt");
    assert_eq!(status[0].kind, StatusKind::Untracked);
    assert!(!status[0].staged);
}

#[test]
fn status_reports_modified_file() {
    let (_tmp, path) = init_repo();
    fs::write(path.join("README.md"), b"changed\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, "README.md");
    assert_eq!(status[0].kind, StatusKind::Modified);
    assert!(!status[0].staged);
}

#[test]
fn status_distinguishes_staged_from_unstaged() {
    let (_tmp, path) = init_repo();
    // Stage one new file.
    fs::write(path.join("staged.txt"), b"a\n").unwrap();
    let g2 = Repository::open(&path).unwrap();
    {
        let mut idx = g2.index().unwrap();
        idx.add_path(Path::new("staged.txt")).unwrap();
        idx.write().unwrap();
    }
    // Modify a tracked file in the workdir but don't stage it.
    fs::write(path.join("README.md"), b"changed\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    let status = repo.status().unwrap();
    let names: Vec<(&str, StatusKind, bool)> = status
        .iter()
        .map(|s| (s.path.as_str(), s.kind, s.staged))
        .collect();
    assert!(
        names.contains(&("README.md", StatusKind::Modified, false)),
        "got: {names:?}"
    );
    assert!(
        names.contains(&("staged.txt", StatusKind::Added, true)),
        "got: {names:?}"
    );
}

#[test]
fn status_results_are_sorted_by_path() {
    let (_tmp, path) = init_repo();
    fs::write(path.join("z.txt"), b"z\n").unwrap();
    fs::write(path.join("a.txt"), b"a\n").unwrap();
    fs::write(path.join("m.txt"), b"m\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let paths: Vec<String> = repo.status().unwrap().into_iter().map(|s| s.path).collect();
    assert_eq!(paths, vec!["a.txt", "m.txt", "z.txt"]);
}

#[test]
fn status_ignores_directory_link() {
    let Some((_tmp, path)) = common::repo_with_node_modules_link() else {
        return;
    };
    let repo = Repo::open(&path).unwrap();
    let status = repo.status().unwrap();
    assert!(
        !status.iter().any(|e| e.path == "node_modules"),
        "got: {:?}",
        status.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
}
