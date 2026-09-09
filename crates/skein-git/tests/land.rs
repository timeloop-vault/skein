//! Integration tests for the land actions (#214, epic #52 D9).
//!
//! These drive the real `git` binary against real repositories in
//! `tempfile` directories, because that is the whole point: D9 chose
//! the git CLI over libgit2 precisely so that the user's config, hooks
//! and credential helpers apply, and a test that mocked the process
//! would be testing the opposite of what ships.
//!
//! The shapes worth building here are the ones a room actually has: a
//! branch in a linked worktree while the base branch is checked out in
//! the main one, and a branch whose base is checked out nowhere at all.

use std::fs;
use std::path::{Path, PathBuf};

use skein_git::cli;
use skein_git::{GitError, MergeKind, MergeTarget, land};
use tempfile::TempDir;

// ── fixtures ──────────────────────────────────────────────────────

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = cli::git(cwd, args).unwrap();
    assert!(out.ok(), "git {args:?} failed: {}", out.failure());
    out.line().to_owned()
}

/// A repo on `main` with one commit, at `<tmp>/repo`. Worktrees go in
/// siblings under the same `TempDir`, the way Skein lays rooms out.
fn init_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    write_commit(&repo, "README.md", "hello\n", "init");
    (tmp, repo)
}

fn write_commit(cwd: &Path, file: &str, body: &str, msg: &str) -> String {
    fs::write(cwd.join(file), body).unwrap();
    git(cwd, &["add", file]);
    git(cwd, &["commit", "-m", msg]);
    git(cwd, &["rev-parse", "HEAD"])
}

/// A linked worktree on a new branch — a Skein room.
fn add_room(tmp: &TempDir, repo: &Path, name: &str, branch: &str) -> PathBuf {
    let path = tmp.path().join(name);
    git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            path.to_str().unwrap(),
            "main",
        ],
    );
    path
}

fn head(cwd: &Path) -> String {
    git(cwd, &["rev-parse", "HEAD"])
}

fn rev(cwd: &Path, refname: &str) -> String {
    git(cwd, &["rev-parse", refname])
}

// ── preflight ─────────────────────────────────────────────────────

#[test]
fn preflight_counts_what_would_land_and_finds_nothing_in_the_way() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    write_commit(&room, "b.txt", "two\n", "feat: b");

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert!(pre.is_repo);
    assert_eq!(pre.branch.as_deref(), Some("feat/x"));
    assert_eq!(pre.base.as_deref(), Some("main"));
    assert!(pre.base_resolved && pre.base_is_local);
    assert_eq!((pre.ahead, pre.behind), (2, 0));
    assert!(pre.can_fast_forward);
    assert_eq!(pre.commits, vec!["feat: a", "feat: b"]);
    assert!(pre.merge_blockers.is_empty(), "{:?}", pre.merge_blockers);
}

#[test]
fn preflight_without_a_base_falls_back_to_the_repo_guess() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");

    let pre = land::preflight(&room, None).unwrap();
    assert_eq!(pre.base.as_deref(), Some("main"));
    assert_eq!(pre.ahead, 1);
}

#[test]
fn preflight_sees_the_worktree_that_holds_the_base_branch() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");

    let pre = land::preflight(&room, Some("main")).unwrap();
    match &pre.merge_target {
        MergeTarget::Worktree { path, dirty } => {
            assert!(path.ends_with("repo"), "{path}");
            assert_eq!(*dirty, 0);
        }
        other => panic!("expected the main checkout as the merge target, got {other:?}"),
    }
}

#[test]
fn a_base_checked_out_nowhere_is_reported_as_unchecked() {
    // Switching the only worktree onto the branch leaves `main` with no
    // checkout at all — the shape where only a fast-forward is possible.
    let (_tmp, repo) = init_repo();
    git(&repo, &["checkout", "-b", "feat/x"]);
    write_commit(&repo, "a.txt", "one\n", "feat: a");

    let pre = land::preflight(&repo, Some("main")).unwrap();
    assert_eq!(pre.merge_target, MergeTarget::Unchecked);
    assert!(pre.can_fast_forward);
    assert!(pre.merge_blockers.is_empty(), "{:?}", pre.merge_blockers);
}

#[test]
fn uncommitted_work_in_the_room_is_reported_but_never_blocks() {
    // A merge takes commits. Uncommitted work simply does not travel,
    // and the dialog has to say so rather than the action deciding for
    // the user either way.
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    fs::write(room.join("scratch.txt"), "not committed\n").unwrap();

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert_eq!(pre.dirty_total, 1);
    assert_eq!(pre.dirty, vec!["scratch.txt"]);
    assert!(pre.merge_blockers.is_empty(), "{:?}", pre.merge_blockers);
}

#[test]
fn a_folder_that_is_not_a_repository_blocks_both_actions() {
    let tmp = TempDir::new().unwrap();
    let pre = land::preflight(tmp.path(), None).unwrap();
    assert!(!pre.is_repo);
    assert!(pre.merge_blockers[0].contains("not a git repository"));
    assert!(pre.pr_blockers[0].contains("not a git repository"));
}

#[test]
fn a_remote_tracking_base_merges_into_the_local_branch_it_follows() {
    // `origin/main` is the *ordinary* base: the review pane guesses
    // `origin/HEAD` first. Nothing can merge into a remote-tracking
    // ref, so landing substitutes the local branch beside it — and
    // says so, which is why `merge_base` is a field of its own rather
    // than a quiet rewrite of `base`.
    let (tmp, repo) = init_repo();
    let bare = tmp.path().join("origin.git");
    git(&repo, &["init", "--bare", bare.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "origin", "main"]);
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");

    let pre = land::preflight(&room, Some("origin/main")).unwrap();
    assert!(pre.base_resolved, "the remote-tracking ref does resolve");
    assert!(!pre.base_is_local);
    assert_eq!(
        pre.base.as_deref(),
        Some("origin/main"),
        "the review's base"
    );
    assert_eq!(pre.merge_base.as_deref(), Some("main"), "what a merge hits");
    assert!(pre.merge_blockers.is_empty(), "{:?}", pre.merge_blockers);
    // …and it is a perfectly good PR base once the remote name is off.
    assert_eq!(pre.pr_base.as_deref(), Some("main"));

    let out = land::merge_to_base(&room, Some("origin/main")).unwrap();
    assert_eq!(out.base, "main");
    assert_eq!(rev(&repo, "main"), head(&room));
}

#[test]
fn a_remote_tracking_base_with_no_local_branch_beside_it_is_refused() {
    let (tmp, repo) = init_repo();
    let bare = tmp.path().join("origin.git");
    git(&repo, &["init", "--bare", bare.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "origin", "main"]);
    git(&repo, &["checkout", "-b", "feat/x"]);
    write_commit(&repo, "a.txt", "one\n", "feat: a");
    // Now there is an `origin/release` with nothing local behind it.
    git(&repo, &["push", "origin", "main:release"]);
    git(&repo, &["fetch", "origin"]);

    let pre = land::preflight(&repo, Some("origin/release")).unwrap();
    assert_eq!(pre.merge_base, None);
    assert!(
        pre.merge_blockers[0].contains("remote-tracking"),
        "{:?}",
        pre.merge_blockers
    );

    let err = land::merge_to_base(&repo, Some("origin/release")).unwrap_err();
    assert!(matches!(err, GitError::Refused(_)), "got {err:?}");
}

#[test]
fn a_branch_with_nothing_on_it_suggests_no_body_at_all() {
    // The base's own HEAD message is not this change's description, and
    // offering it would put a stranger's commit in the user's PR.
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert_eq!(pre.commits.len(), 0);
    assert_eq!(pre.suggested_body, "");
    assert_eq!(pre.suggested_title, "feat/x");
}

#[test]
fn the_suggested_pull_request_is_built_from_commits_and_nothing_else() {
    // D9: the PR starts clean. There is no path from a Skein review
    // comment into this body, and the test pins the body's whole
    // contents so that stays true.
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: first");
    write_commit(&room, "b.txt", "two\n", "feat: second");

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert_eq!(pre.suggested_title, "feat: first");
    assert_eq!(pre.suggested_body, "- feat: first\n- feat: second");
}

#[test]
fn a_single_commit_suggests_its_own_message() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    fs::write(room.join("a.txt"), "one\n").unwrap();
    git(&room, &["add", "a.txt"]);
    git(
        &room,
        &["commit", "-m", "feat: only", "-m", "why it exists"],
    );

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert_eq!(pre.suggested_title, "feat: only");
    assert_eq!(pre.suggested_body, "why it exists");
}

// ── merge ─────────────────────────────────────────────────────────

#[test]
fn merging_into_a_base_nobody_has_checked_out_moves_the_ref() {
    let (_tmp, repo) = init_repo();
    git(&repo, &["checkout", "-b", "feat/x"]);
    let tip = write_commit(&repo, "a.txt", "one\n", "feat: a");

    let out = land::merge_to_base(&repo, Some("main")).unwrap();
    assert_eq!(out.kind, MergeKind::FastForwarded);
    assert_eq!(out.worktree, None);
    assert_eq!(rev(&repo, "main"), tip);
}

#[test]
fn merging_runs_in_the_worktree_that_holds_the_base() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    let tip = write_commit(&room, "a.txt", "one\n", "feat: a");

    let out = land::merge_to_base(&room, Some("main")).unwrap();
    assert_eq!(out.kind, MergeKind::FastForwarded);
    assert!(out.worktree.is_some_and(|w| w.ends_with("repo")));
    assert_eq!(rev(&repo, "main"), tip);
    // The merge really happened in that checkout, not just on the ref.
    assert_eq!(head(&repo), tip);
    assert!(repo.join("a.txt").exists());
}

#[test]
fn a_diverged_base_gets_a_real_merge_commit() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    // main moves on independently, in a file the branch never touches.
    write_commit(&repo, "c.txt", "three\n", "chore: c");

    let out = land::merge_to_base(&room, Some("main")).unwrap();
    assert_eq!(out.kind, MergeKind::Merged);
    assert_eq!(
        git(&repo, &["rev-list", "--count", "--merges", "main"]),
        "1"
    );
    assert!(repo.join("a.txt").exists() && repo.join("c.txt").exists());
}

#[test]
fn a_conflicting_merge_is_aborted_and_names_the_files() {
    // Out of scope for the epic is *resolving* conflicts; leaving a
    // worktree the user is not even looking at stuck mid-merge would be
    // a trap they did not set. So: abort, then report.
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "README.md", "from the branch\n", "feat: edit");
    write_commit(&repo, "README.md", "from main\n", "chore: edit");

    let err = land::merge_to_base(&room, Some("main")).unwrap_err();
    match &err {
        GitError::MergeConflict(files) => assert!(files.contains("README.md"), "{files}"),
        other => panic!("expected a conflict, got {other:?}"),
    }

    // Nothing left behind: no MERGE_HEAD, no conflict markers, main
    // still where it was.
    assert!(!repo.join(".git/MERGE_HEAD").exists());
    let status = cli::git(&repo, &["status", "--porcelain"]).unwrap();
    assert_eq!(status.stdout.trim(), "", "the base worktree must be clean");
    assert_eq!(
        fs::read_to_string(repo.join("README.md")).unwrap(),
        "from main\n"
    );
}

#[test]
fn a_dirty_base_worktree_refuses_the_merge_before_it_starts() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    fs::write(repo.join("README.md"), "edited by hand\n").unwrap();

    let err = land::merge_to_base(&room, Some("main")).unwrap_err();
    assert!(
        matches!(&err, GitError::Refused(m) if m.contains("uncommitted")),
        "got {err:?}"
    );
    // The refusal has to be total: main did not move.
    assert_eq!(rev(&repo, "main"), head(&repo));
}

#[test]
fn nothing_to_merge_reports_rather_than_failing() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");

    let out = land::merge_to_base(&room, Some("main")).unwrap();
    assert_eq!(out.kind, MergeKind::AlreadyUpToDate);
}

#[test]
fn nothing_to_merge_still_reports_when_the_base_is_checked_out_nowhere() {
    // The same answer in the shape where a fast-forward is the only
    // possible merge: an empty branch is up to date, not un-mergeable.
    let (_tmp, repo) = init_repo();
    git(&repo, &["checkout", "-b", "feat/x"]);

    let out = land::merge_to_base(&repo, Some("main")).unwrap();
    assert_eq!(out.kind, MergeKind::AlreadyUpToDate);
}

#[test]
fn merging_leaves_uncommitted_work_in_the_room_alone() {
    let (tmp, repo) = init_repo();
    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    fs::write(room.join("scratch.txt"), "still mine\n").unwrap();

    land::merge_to_base(&room, Some("main")).unwrap();
    assert_eq!(
        fs::read_to_string(room.join("scratch.txt")).unwrap(),
        "still mine\n"
    );
}

// ── push ──────────────────────────────────────────────────────────

#[test]
fn pushing_sends_the_branch_and_sets_its_upstream() {
    let (tmp, repo) = init_repo();
    let bare = tmp.path().join("origin.git");
    git(&repo, &["init", "--bare", bare.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "origin", "main"]);

    let room = add_room(&tmp, &repo, "room", "feat/x");
    let tip = write_commit(&room, "a.txt", "one\n", "feat: a");

    let out = land::push_branch(&room, "origin", "feat/x").unwrap();
    assert_eq!(out.remote, "origin");
    assert_eq!(rev(&bare, "refs/heads/feat/x"), tip);

    let pre = land::preflight(&room, Some("main")).unwrap();
    assert_eq!(pre.upstream.as_deref(), Some("origin/feat/x"));
    assert_eq!(pre.remote.as_deref(), Some("origin"));
}

#[test]
fn a_rejected_push_reports_the_rejection_rather_than_forcing_it() {
    // Push is the one action other people can see, and grok-build's
    // permission layer hard-codes it as never auto-approvable for that
    // reason. There is no force path here at all.
    let (tmp, repo) = init_repo();
    let bare = tmp.path().join("origin.git");
    git(&repo, &["init", "--bare", bare.to_str().unwrap()]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "origin", "main"]);

    let room = add_room(&tmp, &repo, "room", "feat/x");
    write_commit(&room, "a.txt", "one\n", "feat: a");
    land::push_branch(&room, "origin", "feat/x").unwrap();

    // Rewrite history so the next push is not a fast-forward.
    git(&room, &["reset", "--hard", "HEAD~1"]);
    write_commit(&room, "a.txt", "different\n", "feat: rewritten");

    let err = land::push_branch(&room, "origin", "feat/x").unwrap_err();
    assert!(
        matches!(&err, GitError::CommandFailed(what, _) if what.starts_with("push")),
        "got {err:?}"
    );
}

#[test]
fn a_push_to_a_remote_that_does_not_exist_fails_instead_of_waiting() {
    // The auth-suppression env exists so a prompt becomes an error; the
    // point here is that the error arrives at all, promptly, rather
    // than the action hanging on something the user cannot see.
    let (tmp, repo) = init_repo();
    let missing = tmp.path().join("nowhere.git");
    git(
        &repo,
        &["remote", "add", "origin", missing.to_str().unwrap()],
    );

    let started = std::time::Instant::now();
    let err = land::push_branch(&repo, "origin", "main").unwrap_err();
    assert!(matches!(err, GitError::CommandFailed(..)), "got {err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
