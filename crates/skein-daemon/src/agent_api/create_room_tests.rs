//! `create_room` (#330): argument validation, kill switch and rate cap. Also hosts the `create_room` fixtures its siblings share.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::auth::Caller;
use super::review_tests::git_repo_with_commit;
use super::state::AgentApiState;
use super::tests::{
    Fixture, agent_api_state, agent_api_state_with_room_records, caller_for, fixture, harness,
    room, save,
};
use super::verbs::{self, MailContext, VerbError};
use crate::db::{CreatedBy, Room};
use crate::event_sink::EventSink;
use crate::room_store::RoomStore;

// ── opening a room (issue #330) ──────────────────────────────────────
//
// `create_room` is exercised as a plain function (`verbs::create_room`),
// not through `mcp::call_tool`: the properties under test are the verb's
// own guards and what it stores. Since #734 it needs no webview, so none of
// these tests attaches a client, and every repository is built with `git2`.

pub(super) fn create_room_args(task: &str) -> verbs::CreateRoomArgs {
    verbs::CreateRoomArgs {
        path: None,
        branch_mode: None,
        branch: None,
        base_branch: None,
        task: task.to_owned(),
        kind: None,
        agent: None,
        prompt: None,
    }
}

/// An open room `create_room`'s per-repository-group ceiling (#375)
/// counts: `created_by` set (as if an agent's own `create_room` call
/// had made it), not archived, grouped under `repo_root` (`None` for
/// the ungrouped bucket).
pub(super) fn agent_opened_room(id: &str, repo_root: Option<&str>) -> Room {
    let mut r = room(id, vec![]);
    r.repo_root = repo_root.map(str::to_owned);
    r.created_by = Some(CreatedBy {
        room_id: "creator".to_owned(),
        harness_id: "h".to_owned(),
        prompt_first_line: None,
        base_sha: None,
    });
    r
}

/// A repository with one commit at `<tmp>/repo`. Nested so the worktrees
/// `create_room` cuts (`<tmp>/repo-wt/<leaf>`) land inside the temp dir and
/// are removed with it.
pub(super) fn init_repo(tmp: &TempDir) -> String {
    let dir = tmp.path().join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git_repo_with_commit(&dir);
    dir.to_str().unwrap().to_owned()
}

/// Every event a state emitted, as `(name, payload)`. Each one also records
/// whether `watch` already held the room it names, which is what "after the
/// store applied it" means to a listener.
pub(super) struct Captured {
    pub(super) events: Mutex<Vec<(String, String)>>,
    watch: Mutex<Option<Arc<RoomStore>>>,
    pub(super) room_was_stored: Mutex<Vec<bool>>,
}

impl Captured {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            watch: Mutex::new(None),
            room_was_stored: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn named(&self, name: &str) -> Vec<serde_json::Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, p)| serde_json::from_str(p).unwrap())
            .collect()
    }
}

impl EventSink for Captured {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        if event == verbs::AGENT_ROOM_CREATED_EVENT
            && let Some(store) = self.watch.lock().unwrap().as_ref()
        {
            let id = serde_json::from_str::<serde_json::Value>(&payload)
                .ok()
                .and_then(|v| v["roomId"].as_str().map(str::to_owned));
            let stored = id.is_some_and(|id| store.rooms().iter().any(|r| r.id == id));
            self.room_was_stored.lock().unwrap().push(stored);
        }
        self.events
            .lock()
            .unwrap()
            .push((event.to_owned(), payload));
        Ok(())
    }
}

/// A caller room `r1` (harness `h1`) whose folder is a real git checkout, in
/// a state with a room store, no client and no runner.
pub(super) struct Env {
    pub(super) f: Fixture,
    pub(super) tmp: TempDir,
    pub(super) state: AgentApiState,
    pub(super) store: Arc<RoomStore>,
    pub(super) caller: Caller,
    pub(super) repo: String,
    pub(super) captured: Arc<Captured>,
}

impl Env {
    pub(super) fn repo_path(&self) -> PathBuf {
        PathBuf::from(&self.repo)
    }

    /// The rooms in the store other than `r1`, plus whatever was seeded.
    pub(super) fn room_ids(&self) -> Vec<String> {
        self.store.rooms().into_iter().map(|r| r.id).collect()
    }
}

/// [`env_with`] with no extra rooms.
pub(super) fn git_env() -> Env {
    env_with(|_| Vec::new())
}

/// `r1` in a fresh repository, plus the rooms `extra` makes from the
/// repository's path.
pub(super) fn env_with(extra: impl FnOnce(&str) -> Vec<Room>) -> Env {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    let repo = init_repo(&tmp);
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo.clone());
    let mut rooms = vec![r1];
    rooms.extend(extra(&repo));
    let (mut state, store) = agent_api_state_with_room_records(&f, rooms);
    let captured = Captured::new();
    *captured.watch.lock().unwrap() = Some(Arc::clone(&store));
    state.events = Some(captured.clone());
    let caller = caller_for(&f.db, "r1", Some("h1"));
    Env {
        f,
        tmp,
        state,
        store,
        caller,
        repo,
        captured,
    }
}

#[tokio::test]
async fn create_room_rejects_an_empty_task() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("   "),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("task")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_an_unknown_branch_mode() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        branch_mode: Some("yolo".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("branchMode")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_an_unknown_kind() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        kind: Some("frobnicator".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("kind")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_a_relative_path() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        path: Some("relative/path".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("absolute")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_a_path_that_does_not_exist() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let bogus = std::env::temp_dir().join("skein-330-does-not-exist");
    let args = verbs::CreateRoomArgs {
        path: Some(bogus.to_str().unwrap().to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("does not exist")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_worktree_mode_refuses_a_non_checkout_folder() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let tmp = TempDir::new().unwrap(); // exists, but never `git init`-ed
    let args = verbs::CreateRoomArgs {
        path: Some(tmp.path().to_str().unwrap().to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("git checkout")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_current_mode_is_allowed_on_a_non_git_folder() {
    let env = git_env();
    let plain = env.tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let path = plain.to_str().unwrap().to_owned();
    let args = verbs::CreateRoomArgs {
        path: Some(path.clone()),
        branch_mode: Some("current".to_owned()),
        kind: Some("byoh".to_owned()),
        ..create_room_args("hi")
    };
    let out = verbs::create_room(
        &env.state,
        &env.caller,
        &args,
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    let stored = env
        .store
        .rooms()
        .into_iter()
        .find(|r| r.id == out.room_id)
        .expect("the room is in the store");
    assert_eq!(stored.cwd.as_deref(), Some(path.as_str()));
    assert_eq!(
        (stored.branch, stored.repo, stored.repo_root),
        (None, None, None)
    );
    assert_eq!(out.branch, None);
}

#[tokio::test]
async fn create_room_is_refused_when_the_kill_switch_is_off() {
    let env = git_env();
    let err = verbs::create_room(
        &env.state,
        &env.caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("Settings")),
        "{err:?}"
    );
    assert_eq!(env.room_ids(), vec!["r1".to_owned()]);
}

#[tokio::test]
async fn create_room_is_rate_limited_to_five_attempts_a_minute() {
    let env = git_env();
    // Five creations pass (each its own task, so its own branch), and the
    // sixth meets the wall before anything is created.
    for n in 0..5 {
        verbs::create_room(
            &env.state,
            &env.caller,
            &create_room_args(&format!("task {n}")),
            &MailContext::permissive(),
            true,
        )
        .await
        .unwrap_or_else(|e| panic!("attempt {n}: {e:?}"));
    }
    let err = verbs::create_room(
        &env.state,
        &env.caller,
        &create_room_args("one too many"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("rate limit")),
        "{err:?}"
    );
    assert_eq!(env.room_ids().len(), 6);
}

#[tokio::test]
async fn a_refused_attempt_still_counts_against_the_rate_cap() {
    let env = git_env();
    for n in 0..5 {
        let args = verbs::CreateRoomArgs {
            base_branch: Some("no-such-branch".to_owned()),
            ..create_room_args(&format!("task {n}"))
        };
        let err = verbs::create_room(
            &env.state,
            &env.caller,
            &args,
            &MailContext::permissive(),
            true,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m == "unknown base branch"),
            "attempt {n}: {err:?}"
        );
    }
    let err = verbs::create_room(
        &env.state,
        &env.caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("rate limit")),
        "{err:?}"
    );
}
