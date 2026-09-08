//! Integration tests for the commit-range API (#212, epic #52 D1).
//!
//! Real on-disk repositories, like `repo.rs` — the contract under test
//! is libgit2's, so mocking it would test nothing. The cases here are
//! the ones the review pane actually meets: a branch ahead of its base,
//! a base that has moved on since the branch left it (the merge-base
//! case, which is the whole reason the scope is not `base..HEAD`), a
//! root commit with no parent, and the several ways a revision fails to
//! resolve.

use std::fs;
use std::path::{Path, PathBuf};

use git2::{Repository, Signature};
use skein_git::{DiffLineKind, Repo, StatusKind};
use tempfile::TempDir;

/// A repo with one commit on `main`.
fn init_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    let repo = Repository::init(&path).unwrap();
    fs::write(path.join("README.md"), b"hello\n").unwrap();
    stage_and_commit(&repo, &["README.md"], "init");
    // libgit2's default initial branch varies by version.
    let head = repo.head().unwrap().shorthand().map(str::to_owned).unwrap();
    if head != "main" {
        repo.find_branch(&head, git2::BranchType::Local)
            .unwrap()
            .rename("main", false)
            .unwrap();
    }
    (tmp, path)
}

fn stage_and_commit(repo: &Repository, paths: &[&str], message: &str) -> String {
    let mut index = repo.index().unwrap();
    for p in paths {
        // `add_path` fails on a deleted file; `update_all` covers both.
        if repo.workdir().unwrap().join(p).exists() {
            index.add_path(Path::new(p)).unwrap();
        } else {
            index.remove_path(Path::new(p)).unwrap();
        }
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap()
        .to_string()
}

/// Write `content` to `name` and commit it, returning the new sha.
fn commit_file(path: &Path, name: &str, content: &str, message: &str) -> String {
    let repo = Repository::open(path).unwrap();
    fs::write(path.join(name), content).unwrap();
    stage_and_commit(&repo, &[name], message)
}

/// Create `name` from HEAD and check it out.
fn branch_from_head(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(name, &head, false).unwrap();
    repo.set_head(&format!("refs/heads/{name}")).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
}

// ── resolving revisions ───────────────────────────────────────────

#[test]
fn resolve_commit_handles_branches_head_and_shas() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();

    let head = repo.resolve_commit("HEAD").unwrap().unwrap();
    assert_eq!(
        repo.resolve_commit("main").unwrap().as_deref(),
        Some(&*head)
    );
    // A full sha resolves to itself.
    assert_eq!(repo.resolve_commit(&head).unwrap().as_deref(), Some(&*head));
}

#[test]
fn resolve_commit_returns_none_for_an_unknown_revision() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    // Not an error: the pane renders "no such base branch" as an answer.
    assert_eq!(repo.resolve_commit("no-such-branch").unwrap(), None);
    assert_eq!(repo.resolve_commit("").unwrap(), None);
}

// ── merge-base ────────────────────────────────────────────────────

#[test]
fn merge_base_is_where_the_branch_left_not_where_base_is_now() {
    // The case the whole scope decision rests on: base moves on after
    // the branch is cut, and a `base..HEAD` diff would report base's
    // own new work as this branch's.
    let (_tmp, path) = init_repo();
    let fork = commit_file(&path, "a.txt", "one\n", "base work");
    branch_from_head(&path, "feature");
    commit_file(&path, "b.txt", "feature\n", "feature work");

    // main moves on, independently.
    {
        let repo = Repository::open(&path).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
    }
    commit_file(&path, "c.txt", "later\n", "base moves on");

    let repo = Repo::open(&path).unwrap();
    assert_eq!(
        repo.merge_base("feature", "main").unwrap().as_deref(),
        Some(&*fork),
        "merge base is the fork point, not either tip"
    );

    // And the range built on it contains only the feature commit.
    let commits = repo.commits_between(Some(&fork), "feature").unwrap();
    let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["feature work"]);
}

#[test]
fn merge_base_is_none_for_an_unknown_revision() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.merge_base("main", "nope").unwrap(), None);
}

// ── the commit list ───────────────────────────────────────────────

#[test]
fn commits_between_is_newest_first_and_excludes_the_base() {
    let (_tmp, path) = init_repo();
    let base = commit_file(&path, "a.txt", "one\n", "base");
    commit_file(&path, "b.txt", "two\n", "first");
    commit_file(&path, "c.txt", "three\n", "second");

    let repo = Repo::open(&path).unwrap();
    let commits = repo.commits_between(Some(&base), "HEAD").unwrap();
    let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, vec!["second", "first"]);
    // Base itself is excluded, so no commit in the range is the base.
    assert!(commits.iter().all(|c| c.sha != base));
}

#[test]
fn commits_between_with_no_base_walks_the_whole_history() {
    let (_tmp, path) = init_repo();
    commit_file(&path, "a.txt", "one\n", "second");
    let repo = Repo::open(&path).unwrap();
    let commits = repo.commits_between(None, "HEAD").unwrap();
    assert_eq!(commits.len(), 2, "init + second");
}

#[test]
fn commits_between_survives_a_base_sha_that_no_longer_exists() {
    // A branch deleted (or a history rewritten) under a room must widen
    // the range, never empty the commit list.
    let (_tmp, path) = init_repo();
    commit_file(&path, "a.txt", "one\n", "work");
    let repo = Repo::open(&path).unwrap();
    let gone = "0".repeat(40);
    let commits = repo.commits_between(Some(&gone), "HEAD").unwrap();
    assert_eq!(commits.len(), 2);
}

#[test]
fn commit_info_splits_summary_from_body_and_records_parents() {
    let (_tmp, path) = init_repo();
    let first = {
        let repo = Repo::open(&path).unwrap();
        repo.resolve_commit("HEAD").unwrap().unwrap()
    };
    commit_file(
        &path,
        "a.txt",
        "one\n",
        "feat(#212): the subject\n\nA body paragraph.\nAnd a second line.\n",
    );

    let repo = Repo::open(&path).unwrap();
    let info = repo.commit_info("HEAD").unwrap().unwrap();
    assert_eq!(info.summary, "feat(#212): the subject");
    assert_eq!(info.body, "A body paragraph.\nAnd a second line.");
    assert_eq!(info.parents, vec![first]);
    assert_eq!(info.short_sha, info.sha.chars().take(8).collect::<String>());
    assert_eq!(info.author_name, "test");
    assert!(info.time_ms > 0);
}

#[test]
fn commit_info_on_a_root_commit_has_no_parents() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let info = repo.commit_info("HEAD").unwrap().unwrap();
    assert!(info.parents.is_empty());
    assert_eq!(info.body, "", "a one-line message has no body");
}

#[test]
fn commit_info_is_none_for_an_unknown_revision() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.commit_info("nope").unwrap(), None);
}

// ── range diffs ───────────────────────────────────────────────────

#[test]
fn diff_trees_reports_the_range_as_one_change_per_file() {
    let (_tmp, path) = init_repo();
    let base = commit_file(&path, "a.txt", "one\n", "base");
    commit_file(&path, "a.txt", "one\ntwo\n", "add a line");
    commit_file(&path, "b.txt", "new\n", "add a file");

    let repo = Repo::open(&path).unwrap();
    let files = repo.diff_trees(Some(&base), "HEAD").unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt", "b.txt"]);
    assert_eq!(files[0].kind, StatusKind::Modified);
    assert_eq!(files[1].kind, StatusKind::Added);

    // Two commits touched a.txt; the range diff shows the net change once.
    let added: Vec<&str> = files[0].hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == DiffLineKind::Add)
        .map(|l| l.content.as_str())
        .collect();
    assert_eq!(added, vec!["two"]);
}

#[test]
fn diff_trees_against_no_base_is_the_whole_tree_as_additions() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let files = repo.diff_trees(None, "HEAD").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "README.md");
    assert_eq!(files[0].kind, StatusKind::Added);
}

#[test]
fn diff_commit_shows_only_that_commit_not_the_range() {
    let (_tmp, path) = init_repo();
    commit_file(&path, "a.txt", "one\n", "first");
    commit_file(&path, "b.txt", "two\n", "second");

    let repo = Repo::open(&path).unwrap();
    let files = repo.diff_commit("HEAD").unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["b.txt"],
        "the previous commit's file is not here"
    );
}

#[test]
fn diff_commit_on_a_root_commit_diffs_against_the_empty_tree() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    let files = repo.diff_commit("HEAD").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, StatusKind::Added);
}

#[test]
fn diff_tree_to_workdir_counts_committed_and_uncommitted_together() {
    // The review's default view. A branch mid-task has both, and showing
    // only one of them is the omission #211 set out to stop.
    let (_tmp, path) = init_repo();
    let base = commit_file(&path, "a.txt", "one\n", "base");
    commit_file(&path, "a.txt", "one\ntwo\n", "committed work");
    fs::write(path.join("b.txt"), "uncommitted\n").unwrap();

    let repo = Repo::open(&path).unwrap();
    let files = repo.diff_tree_to_workdir(Some(&base)).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt", "b.txt"]);
    assert_eq!(files[1].kind, StatusKind::Untracked);
}

#[test]
fn diff_trees_errors_on_an_unknown_revision() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    // Unlike resolve_commit, asking for a *diff* of something that does
    // not exist is a real error — the caller resolved it first or should
    // have.
    assert!(repo.diff_trees(Some("nope"), "HEAD").is_err());
}

// ── blob at a revision ────────────────────────────────────────────

#[test]
fn blob_at_reads_content_from_an_older_commit() {
    let (_tmp, path) = init_repo();
    let first = commit_file(&path, "a.txt", "original\n", "first");
    commit_file(&path, "a.txt", "rewritten\n", "second");

    let repo = Repo::open(&path).unwrap();
    assert_eq!(
        repo.blob_at(&first, "a.txt").unwrap().as_deref(),
        Some(&b"original\n"[..]),
        "an anchor recomputes against the text it was written on"
    );
    assert_eq!(
        repo.blob_at("HEAD", "a.txt").unwrap().as_deref(),
        Some(&b"rewritten\n"[..])
    );
}

#[test]
fn blob_at_is_none_for_unknown_paths_and_revisions() {
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.blob_at("HEAD", "no-such-file").unwrap(), None);
    assert_eq!(repo.blob_at("no-such-rev", "README.md").unwrap(), None);
    // A directory is a tree, not a blob.
    fs::create_dir(path.join("dir")).unwrap();
    commit_file(&path, "dir/x.txt", "x\n", "add dir");
    assert_eq!(repo.blob_at("HEAD", "dir").unwrap(), None);
}

// ── base-branch guessing ──────────────────────────────────────────

#[test]
fn default_base_branch_picks_main_when_not_on_it() {
    let (_tmp, path) = init_repo();
    branch_from_head(&path, "feature");
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.default_base_branch().as_deref(), Some("main"));
}

#[test]
fn default_base_branch_never_returns_the_current_branch() {
    // On main itself there is no candidate left — a review of main
    // against main is empty, and an empty pane teaches the user nothing.
    let (_tmp, path) = init_repo();
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.head_branch().as_deref(), Some("main"));
    assert_eq!(repo.default_base_branch(), None);
}

#[test]
fn default_base_branch_falls_back_to_master() {
    let (_tmp, path) = init_repo();
    {
        let repo = Repository::open(&path).unwrap();
        repo.find_branch("main", git2::BranchType::Local)
            .unwrap()
            .rename("master", false)
            .unwrap();
    }
    branch_from_head(&path, "feature");
    let repo = Repo::open(&path).unwrap();
    assert_eq!(repo.default_base_branch().as_deref(), Some("master"));
}
