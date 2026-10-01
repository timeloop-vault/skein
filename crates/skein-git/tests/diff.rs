//! Integration tests for `skein-git` — `diff_workdir`.
//!
//! Real on-disk `git2` repositories (not mocks): the point of this crate is
//! the libgit2 contract.

use std::fs;

use skein_git::{DiffHunk, DiffLineKind, FileDiff, Repo, StatusKind, propose_worktree_path};
mod common;

use common::init_repo;

#[test]
fn diff_clean_repo_is_empty() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.diff_workdir().unwrap(), Vec::<FileDiff>::new());
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
    assert_ne!(f.hunks, Vec::<DiffHunk>::new());

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
fn diff_strips_crlf_line_endings() {
    // #147: libgit2 returns CRLF-terminated lines for files stored with
    // CRLF (common on Windows). We trim the trailing `\r` along with the
    // `\n` so a stray carriage return never leaks into the diff content
    // the UI renders. Before the fix the content was "alpha\r".
    let (_tmp, path) = init_repo();
    fs::write(path.join("crlf.txt"), b"alpha\r\nbeta\r\n").unwrap();
    let repo = Repo::open(&path).unwrap();
    let diff = repo.diff_workdir().unwrap();
    let f = diff
        .iter()
        .find(|f| f.path == "crlf.txt")
        .expect("crlf.txt in diff");
    let contents: Vec<&str> = f
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .map(|l| l.content.as_str())
        .collect();
    assert!(
        contents.iter().all(|c| !c.ends_with('\r')),
        "diff line content must not retain a trailing CR, got: {contents:?}"
    );
    // The text itself is intact — only the CR/LF terminator is gone.
    assert!(contents.contains(&"alpha"), "got: {contents:?}");
    assert!(contents.contains(&"beta"), "got: {contents:?}");
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

#[test]
fn diff_ignores_directory_link_and_still_reports_real_changes() {
    // Regression (#217): libgit2 reads a junction / dir symlink
    // as a file-like entry, so `node_modules/` doesn't match it, it
    // lands in the diff as untracked, and building its patch fails with
    // "requested file is a directory" — which used to abort the whole
    // call and leave the Diff card empty.
    let Some((_tmp, path)) = common::repo_with_node_modules_link() else {
        return;
    };
    fs::write(path.join("new.txt"), b"line one\nline two\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    let diff = repo
        .diff_workdir()
        .expect("a directory link must not fail the diff");

    assert!(
        diff.iter().any(|f| f.path == "new.txt"),
        "the real new file must still be reported, got: {:?}",
        diff.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert!(
        !diff.iter().any(|f| f.path == "node_modules"),
        "the directory link must not appear as a changed file"
    );
}

#[test]
fn diff_inside_linked_worktree_reports_untracked_file() {
    // Every Skein room is a linked worktree, but until now nothing
    // exercised the diff from inside one.
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let wt = propose_worktree_path(&path, "diff-in-worktree");
    repo.add_worktree("diff-in-worktree", "main", &wt).unwrap();

    fs::write(wt.join("inside.txt"), b"alpha\nbeta\n").unwrap();
    let wt_repo = Repo::open(&wt).unwrap();
    let diff = wt_repo.diff_workdir().unwrap();
    let f = diff
        .iter()
        .find(|f| f.path == "inside.txt")
        .expect("inside.txt");
    assert_eq!(f.kind, StatusKind::Untracked);
    assert!(!f.hunks.is_empty(), "expected hunk content for a new file");
}
