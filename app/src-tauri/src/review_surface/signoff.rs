//! Has the reviewer approved this? (#214, epic #52 D9 as corrected.)
//!
//! The one fact about a review that exists nowhere but Skein. #213 let
//! the agent *read* the reviewer's comments; this is what lets it ask
//! whether the reviewer is **done** — the gate it checks before landing
//! the branch its own way, with the repo's own conventions.
//!
//! # A sign-off approves a commit, not a room
//!
//! The whole design is in the `head_sha` column. Approval is granted
//! against the code as it was, so anything reading it compares that sha
//! to HEAD now:
//!
//! * same sha → **approved**;
//! * different sha → **stale**, naming the sha that *was* approved;
//! * no row → not approved.
//!
//! An agent that commits after being approved therefore invalidates the
//! approval by doing so, which is the honest outcome — an approval that
//! silently stretched over commits nobody read would be worse than no
//! approval at all. `review_viewed` already plays the same trick one
//! level down with its content hash.
//!
//! # What it does not do
//!
//! Unresolved threads do **not** block a sign-off. The counts travel
//! with the status so the reviewer sees them in the confirmation and
//! the agent sees them alongside its clearance, but deciding that three
//! open threads are fine is exactly the kind of judgement this feature
//! exists to record rather than to second-guess.
//!
//! And nothing here can *grant* one on the agent's behalf. That
//! prohibition lives in `agent_api` beside the resolve prohibition it
//! copies, for the same reason: an agent that can sign off its own work
//! removes the only gate the loop has.

use std::path::Path;

use serde::Serialize;
use skein_git::Repo;

use crate::db::Database;

/// Where a room's review stands, for both the pane and the agent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignoffStatus {
    /// The reviewer has approved, and HEAD is still what they approved.
    /// The only field a caller deciding "may I land?" needs to read.
    pub approved: bool,
    /// A sign-off exists but HEAD has moved past it. Never `true` at
    /// the same time as `approved`.
    pub stale: bool,
    /// The sha that was approved, present whenever a sign-off exists —
    /// including a stale one, which is the case it matters for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_sha: Option<String>,
    /// HEAD right now. `None` in a room that is not a git repository,
    /// where a sign-off cannot be granted at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The branch the sign-off was granted against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// How many commits HEAD is past the approved sha. `0` unless
    /// stale; `None` when the two are not comparable (a rebase, a
    /// branch switch, an amended commit — the approved sha may not be
    /// reachable any more, and guessing a number would be a lie).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_since: Option<usize>,
    /// Threads the reviewer has not closed. Never a blocker.
    pub unresolved_count: usize,
    /// Unresolved threads the agent has not claimed to have handled —
    /// its own outstanding work, which is the more actionable number.
    pub unaddressed_count: usize,
    /// `false` for a room with no worktree or one that is not a repo.
    /// Nothing to sign off on, so the control says why.
    pub can_sign_off: bool,
}

/// Read the room's sign-off and place it against HEAD.
pub fn status_impl(db: &Database, room_id: &str, cwd: &str) -> Result<SignoffStatus, String> {
    let head_sha = Repo::open(Path::new(cwd))
        .ok()
        .and_then(|r| r.resolve_commit("HEAD").ok().flatten());
    let row = db.review_signoff(room_id)?;

    let threads = db.review_threads_for_room(room_id)?;
    let addressed: std::collections::BTreeSet<String> = db
        .addressed_for_room(room_id)?
        .into_iter()
        .map(|a| a.thread_id)
        .collect();
    let open: Vec<_> = threads.iter().filter(|t| t.resolved_ms.is_none()).collect();
    let unresolved_count = open.len();
    let unaddressed_count = open.iter().filter(|t| !addressed.contains(&t.id)).count();

    let (approved, stale, commits_since) = match (&row, &head_sha) {
        (Some(r), Some(head)) if &r.head_sha == head => (true, false, Some(0)),
        (Some(r), Some(head)) => (false, true, commits_between(cwd, &r.head_sha, head)),
        // A sign-off with no readable HEAD is not evidence of anything.
        // Reporting it as stale rather than approved keeps the failure
        // on the safe side.
        (Some(_), None) => (false, true, None),
        (None, _) => (false, false, None),
    };

    Ok(SignoffStatus {
        approved,
        stale,
        approved_sha: row.as_ref().map(|r| r.head_sha.clone()),
        head_sha,
        approved_ms: row.as_ref().map(|r| r.approved_ms),
        note: row.as_ref().and_then(|r| r.note.clone()),
        base_ref: row.as_ref().and_then(|r| r.base_ref.clone()),
        commits_since,
        unresolved_count,
        unaddressed_count,
        can_sign_off: Repo::is_repo(Path::new(cwd)),
    })
}

/// Grant or withdraw the sign-off.
///
/// Granting always reads HEAD fresh rather than taking a sha from the
/// caller: the pane's idea of HEAD is from its last refresh, and the
/// approval has to name the commit that exists when the button is
/// pressed, not the one that existed when the pane last looked.
pub fn set_impl(
    db: &Database,
    room_id: &str,
    cwd: &str,
    approved: bool,
    note: Option<&str>,
    now_ms: i64,
) -> Result<SignoffStatus, String> {
    if approved {
        let repo = Repo::open(Path::new(cwd)).map_err(|e| {
            format!("this room is not a git repository, so there is nothing to sign off on: {e}")
        })?;
        let head = repo
            .resolve_commit("HEAD")
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "this branch has no commits yet".to_owned())?;
        let base_ref = db.review_base_ref(room_id)?;
        db.set_review_signoff(room_id, &head, base_ref.as_deref(), note, now_ms)?;
    } else {
        db.clear_review_signoff(room_id)?;
    }
    status_impl(db, room_id, cwd)
}

/// How far HEAD has moved past the approved commit, when that question
/// has an answer.
///
/// `None` rather than a guess when the approved sha is no longer
/// reachable — a rebase, an amend, or a branch switch all produce that,
/// and "3 commits since" would be a fabrication in every one of them.
fn commits_between(cwd: &str, from: &str, to: &str) -> Option<usize> {
    let repo = Repo::open(Path::new(cwd)).ok()?;
    // Unreachable means `resolve_commit` still answers (the object may
    // exist) but the range walk is what tells us whether it is an
    // ancestor. `commits_between` walks `from..to`, which is empty both
    // when nothing changed and when the histories diverged — so check
    // ancestry first via the merge base.
    let base = repo.merge_base(from, to).ok().flatten()?;
    if base != from {
        return None;
    }
    repo.commits_between(Some(from), to)
        .ok()
        .map(|commits| commits.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_with_no_signoff_is_neither_approved_nor_stale() {
        let (db, tmp) = tests_support::db();
        let s = status_impl(&db, "r1", tmp.path().to_str().unwrap()).unwrap();
        assert!(!s.approved);
        assert!(!s.stale, "never approved is not the same as gone stale");
        assert_eq!(s.approved_sha, None);
    }

    #[test]
    fn approving_records_the_commit_and_reads_back_as_approved() {
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        let s = set_impl(&db, "r1", cwd, true, Some("looks good"), 1_000).unwrap();
        assert!(s.approved && !s.stale);
        assert_eq!(s.approved_sha, s.head_sha);
        assert_eq!(s.note.as_deref(), Some("looks good"));
        assert_eq!(s.commits_since, Some(0));
    }

    #[test]
    fn a_commit_after_the_signoff_makes_it_stale_and_says_what_was_approved() {
        // The whole point of the head_sha column: an approval must not
        // silently stretch over code the reviewer never saw.
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        let approved = set_impl(&db, "r1", cwd, true, None, 1_000)
            .unwrap()
            .approved_sha
            .unwrap();

        tests_support::commit(tmp.path(), "b.txt", "more\n", "feat: more");

        let s = status_impl(&db, "r1", cwd).unwrap();
        assert!(
            !s.approved,
            "HEAD moved, so the approval no longer covers it"
        );
        assert!(s.stale);
        assert_eq!(s.approved_sha.as_deref(), Some(approved.as_str()));
        assert_ne!(s.head_sha.as_deref(), Some(approved.as_str()));
        assert_eq!(s.commits_since, Some(1));
    }

    #[test]
    fn re_approving_at_the_new_head_clears_the_staleness() {
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        set_impl(&db, "r1", cwd, true, None, 1_000).unwrap();
        tests_support::commit(tmp.path(), "b.txt", "more\n", "feat: more");
        assert!(status_impl(&db, "r1", cwd).unwrap().stale);

        let s = set_impl(&db, "r1", cwd, true, None, 2_000).unwrap();
        assert!(s.approved && !s.stale);
        assert_eq!(s.approved_ms, Some(2_000));
    }

    #[test]
    fn withdrawing_leaves_no_trace_of_the_old_approval() {
        // Not a flag: "approved once, then withdrawn" is not a state
        // anything should be able to act on.
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        set_impl(&db, "r1", cwd, true, None, 1_000).unwrap();
        let s = set_impl(&db, "r1", cwd, false, None, 2_000).unwrap();
        assert!(!s.approved && !s.stale);
        assert_eq!(s.approved_sha, None);
        assert_eq!(db.review_signoff("r1").unwrap(), None);
    }

    #[test]
    fn a_signoff_survives_a_restart() {
        // It is persisted state the agent reads across restarts; an
        // approval that evaporated with the app would be worse than
        // none, because the agent would silently lose its clearance.
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        set_impl(&db, "r1", cwd, true, Some("ship it"), 1_000).unwrap();
        drop(db);

        let reopened = crate::db::Database::open(&tmp.path().join("skein.db")).unwrap();
        let s = status_impl(&reopened, "r1", cwd).unwrap();
        assert!(s.approved);
        assert_eq!(s.note.as_deref(), Some("ship it"));
    }

    #[test]
    fn a_room_that_is_not_a_repository_cannot_be_signed_off() {
        let (db, tmp) = tests_support::db();
        let cwd = tmp.path().to_str().unwrap();
        assert!(!status_impl(&db, "r1", cwd).unwrap().can_sign_off);
        let err = set_impl(&db, "r1", cwd, true, None, 1_000).unwrap_err();
        assert!(err.contains("not a git repository"), "{err}");
    }

    #[test]
    fn open_threads_are_counted_but_never_block() {
        let (db, tmp) = tests_support::repo_with_commit();
        let cwd = tmp.path().to_str().unwrap();
        tests_support::open_thread(&db, "t1", "r1");
        tests_support::open_thread(&db, "t2", "r1");
        tests_support::claim_addressed(&db, "t2", "r1");

        let s = set_impl(&db, "r1", cwd, true, None, 1_000).unwrap();
        assert!(s.approved, "open threads are the reviewer's call, not ours");
        assert_eq!(s.unresolved_count, 2);
        assert_eq!(s.unaddressed_count, 1, "t2 is claimed handled; t1 is not");
    }

    /// Fixtures. Kept beside the tests rather than in a shared helper:
    /// nothing else in the crate builds a repo *and* a database.
    mod tests_support {
        use std::fs;
        use std::path::Path;

        use tempfile::TempDir;

        use crate::db::{Database, ReviewAddressedRow, ReviewThreadRow};

        pub fn db() -> (Database, TempDir) {
            let tmp = TempDir::new().unwrap();
            let db = Database::open(&tmp.path().join("skein.db")).unwrap();
            (db, tmp)
        }

        /// A repository with one commit, and a database beside it.
        ///
        /// **git2, never a spawned `git`.** The pre-commit hook runs
        /// these tests with `GIT_DIR` exported, and a spawned git
        /// inherits it — `git init` then reinitialises Skein's own
        /// repository, `git config` writes to its config, and
        /// `git commit` commits into the branch under test. That is not
        /// hypothetical; it happened once while writing this file.
        /// git2 takes the path as an argument and has no such ambient
        /// state, which is also how the sibling `review.rs` fixtures
        /// build their repos.
        pub fn repo_with_commit() -> (Database, TempDir) {
            let tmp = TempDir::new().unwrap();
            git2::Repository::init(tmp.path()).unwrap();
            commit(tmp.path(), "a.txt", "one\n", "init");
            let db = Database::open(&tmp.path().join("skein.db")).unwrap();
            (db, tmp)
        }

        pub fn commit(cwd: &Path, file: &str, body: &str, msg: &str) {
            fs::write(cwd.join(file), body).unwrap();
            let repo = git2::Repository::open(cwd).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(file)).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("t", "t@example.com").unwrap();
            let parents = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .map(|c| vec![c])
                .unwrap_or_default();
            let refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
                .unwrap();
        }

        pub fn open_thread(db: &Database, id: &str, room_id: &str) {
            db.insert_review_thread(&ReviewThreadRow {
                id: id.to_owned(),
                room_id: room_id.to_owned(),
                scope: "review".to_owned(),
                file_path: None,
                commit_sha: None,
                side: None,
                line_start: None,
                line_end: None,
                anchor_hash: None,
                anchor_lines: None,
                resolved_ms: None,
                created_ms: 1,
                updated_ms: 1,
            })
            .unwrap();
        }

        pub fn claim_addressed(db: &Database, thread_id: &str, room_id: &str) {
            db.set_thread_addressed(
                room_id,
                &ReviewAddressedRow {
                    thread_id: thread_id.to_owned(),
                    commit_sha: None,
                    harness_id: "h1".to_owned(),
                    note: None,
                    addressed_ms: 500,
                },
            )
            .unwrap();
        }
    }
}
