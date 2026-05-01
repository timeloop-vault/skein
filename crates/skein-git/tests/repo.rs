//! Integration tests for `skein-git`.
//!
//! Each test sets up a fresh on-disk repository with `tempfile::TempDir`
//! and exercises the public API end-to-end. We use real `git2`
//! repositories (not mocks) — the whole point of this crate is the
//! libgit2 contract, so mocking it would defeat the purpose.

use std::fs;
use std::path::Path;

use git2::{Repository, Signature};
use skein_git::{BranchInfo, DiffLineKind, Repo, StatusKind, propose_worktree_path};
use tempfile::TempDir;

/// Create a repo with one initial commit on `main` so `branches()` returns
/// at least one entry.
fn init_repo() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    let repo = Repository::init(&path).unwrap();
    {
        // Need a commit before HEAD points anywhere meaningful — write a
        // tiny README and commit it.
        fs::write(path.join("README.md"), b"hello\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        // Some libgit2 versions default the initial branch to `master`;
        // rename it to `main` so the tests don't depend on libgit2's
        // default.
        let head_branch_name = repo.head().unwrap().shorthand().map(str::to_owned).unwrap();
        if head_branch_name != "main" {
            let mut branch = repo
                .find_branch(&head_branch_name, git2::BranchType::Local)
                .unwrap();
            branch.rename("main", false).unwrap();
        }
    }
    (tmp, path)
}

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
    assert_eq!(worktrees[0].path, wt_path);
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

#[test]
fn propose_worktree_path_uses_sibling_dir() {
    let p = propose_worktree_path(Path::new("/tmp/code/skein"), "foo");
    assert_eq!(p, Path::new("/tmp/code/skein-wt/foo"));
}

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
fn diff_clean_repo_is_empty() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert!(repo.diff_workdir().unwrap().is_empty());
}

#[test]
fn diff_modified_file_has_add_and_delete_lines() {
    let (_tmp, path) = init_repo();
    // README starts as "hello\n"; rewrite it.
    fs::write(path.join("README.md"), b"goodbye\nworld\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let diff = repo.diff_workdir().unwrap();
    assert_eq!(diff.len(), 1);
    let f = &diff[0];
    assert_eq!(f.path, "README.md");
    assert_eq!(f.kind, StatusKind::Modified);
    assert!(!f.binary);
    assert!(!f.hunks.is_empty());

    // Collect the line kinds from the first hunk so we can assert
    // both `-hello` and `+goodbye` appear.
    let kinds_and_content: Vec<(DiffLineKind, &str)> = f.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind.clone(), l.content.as_str()))
        .collect();
    assert!(
        kinds_and_content.contains(&(DiffLineKind::Delete, "hello")),
        "no delete line, got: {kinds_and_content:?}"
    );
    assert!(
        kinds_and_content
            .iter()
            .any(|(k, c)| *k == DiffLineKind::Add && *c == "goodbye"),
        "no add line for 'goodbye', got: {kinds_and_content:?}"
    );
}

#[test]
fn diff_untracked_file_appears_as_all_add() {
    let (_tmp, path) = init_repo();
    fs::write(path.join("new.txt"), b"line one\nline two\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let diff = repo.diff_workdir().unwrap();
    let f = diff.iter().find(|f| f.path == "new.txt").expect("new.txt");
    assert_eq!(f.kind, StatusKind::Untracked);
    let add_count = f
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Add)
        .count();
    let delete_count = f
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Delete)
        .count();
    assert!(add_count >= 2, "expected ≥2 add lines, got {add_count}");
    assert_eq!(delete_count, 0, "untracked file should have no deletes");
}

#[test]
fn diff_deleted_file_appears_as_all_delete() {
    let (_tmp, path) = init_repo();
    fs::remove_file(path.join("README.md")).unwrap();
    let repo = Repo::open(&path).unwrap();
    let diff = repo.diff_workdir().unwrap();
    let f = diff
        .iter()
        .find(|f| f.path == "README.md")
        .expect("README.md");
    assert_eq!(f.kind, StatusKind::Deleted);
    let delete_count = f
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == DiffLineKind::Delete)
        .count();
    assert!(delete_count >= 1, "expected ≥1 delete line");
}
