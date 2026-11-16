//! `open_harness` (#411): guards and happy paths.

use super::auth::Caller;
use super::close_room_tests::room_created_by;
use super::state::AgentApiState;
use super::tests::{
    Fixture, agent_api_state, agent_api_state_with_room_records, caller_for, fixture, harness,
    room, save,
};
use super::verbs::{self, MailContext, VerbError};
use crate::db::Harness;
use crate::room_store::RoomStore;
use skein_proto::rooms::Room;

// ── opening a harness (issue #411) ──────────────────────────────────────
//
// `open_harness` is exercised as a plain function (`verbs::open_harness`),
// the same way `create_room`'s and `close_room`'s own guards are —
// dispatch through MCP is already covered above
// (`open_harness_is_dispatched_through_mcp_not_refused_by_name`).

pub(super) fn open_harness_args(room: &str) -> verbs::OpenHarnessArgs {
    verbs::OpenHarnessArgs {
        room: room.to_owned(),
        kind: None,
        agent: None,
        prompt: None,
    }
}

#[tokio::test]
async fn open_harness_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("disabled:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_is_rate_limited_to_ten_attempts_a_minute() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // No target room exists at all — proof the rate counter is charged
    // before the target is even looked up, not after.
    for n in 0..10 {
        let err = verbs::open_harness(
            &state,
            &caller,
            &open_harness_args("missing"),
            &MailContext::permissive(),
            true,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::NotFound(_)),
            "attempt {n}: expected the guards to pass through to NotFound, got {err:?}"
        );
    }
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("missing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("rate_limited:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_unknown_room_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("nope"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_a_room_neither_its_own_nor_one_it_created() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![], "r-someone-else"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_in_scope:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_an_already_archived_room() {
    let f = fixture();
    let mut target = room_created_by("r2", vec![], "r1");
    target.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_a_room_whose_folder_holds_a_different_repo() {
    let f = fixture();
    // A plain folder that exists but is not a repo reads as a mismatch.
    let plain = tempfile::TempDir::new().unwrap();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(plain.path().to_string_lossy().into_owned());
    r1.repo_identity = Some(crate::db::RepoIdentity {
        root_commits: vec!["0".repeat(40)],
        origin_url: None,
    });
    save(&f.db, &[r1]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("repo_mismatch:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_once_the_harness_ceiling_is_hit() {
    let f = fixture();
    let harnesses: Vec<Harness> = (0..8)
        .map(|i| harness(&format!("h{i}"), "claude", "main"))
        .collect();
    save(&f.db, &[room("r1", harnesses)]);
    let caller = caller_for(&f.db, "r1", Some("h0"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("room_full:")),
        "{err:?}"
    );
}

/// An open room with a store behind it, and the caller that lives in `r1`.
fn with_store(f: &Fixture, rooms: Vec<Room>) -> (AgentApiState, std::sync::Arc<RoomStore>, Caller) {
    let (state, store) = agent_api_state_with_room_records(f, rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    (state, store, caller)
}

fn kind_args(room: &str, kind: &str) -> verbs::OpenHarnessArgs {
    verbs::OpenHarnessArgs {
        kind: Some(kind.to_owned()),
        ..open_harness_args(room)
    }
}

fn harnesses_of(store: &RoomStore, room: &str) -> Vec<Harness> {
    store
        .rooms()
        .into_iter()
        .find(|r| r.id == room)
        .unwrap()
        .harnesses
}

fn made_by(store: &RoomStore, room: &str, id: &str) -> Harness {
    harnesses_of(store, room)
        .into_iter()
        .find(|h| h.id == id)
        .expect("the harness is in the store")
}

#[tokio::test]
async fn open_harness_without_a_store_is_unavailable() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f); // no daemon handles
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(&err, VerbError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn open_harness_refuses_an_unknown_kind_naming_hermes_among_the_known() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let err = verbs::open_harness(
        &state,
        &caller,
        &kind_args("r1", "bogus"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("bogus") && m.contains("hermes")),
        "{err:?}"
    );
    assert_eq!(harnesses_of(&store, "r1").len(), 1);
}

#[tokio::test]
async fn open_harness_refuses_the_remote_kind() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let err = verbs::open_harness(
        &state,
        &caller,
        &kind_args("r1", "remote"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("only the user can enter")),
        "{err:?}"
    );
    assert_eq!(harnesses_of(&store, "r1").len(), 1);
}

#[tokio::test]
async fn open_harness_stores_the_harness_and_leaves_the_active_one_alone() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    assert!(!state.has_client(), "this test runs with no client");
    let out = verbs::open_harness(
        &state,
        &caller,
        &kind_args("r1", "opencode"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "r1");
    assert_eq!(out.kind, "opencode");
    assert_eq!(out.agent, None);
    assert_eq!(out.name, "oc-2");
    assert_eq!(out.message_id, None, "no prompt was given");

    let room = store.rooms().into_iter().find(|r| r.id == "r1").unwrap();
    assert_eq!(room.active_harness_id, "h1", "a verb never takes a screen");
    assert_eq!(room.harnesses.len(), 2);
    let made = made_by(&store, "r1", &out.harness_id);
    assert!(made.id.starts_with('h') && made.id != "h1");
    assert_eq!(made.kind, "opencode");
    assert_eq!(made.name, "oc-2");
    assert_eq!(made.status, "running");
    assert_eq!(made.live, Some(true));
    assert!(made.cmd.as_ref().is_some_and(|c| !c.is_empty()));
    let by = made.created_by.as_ref().expect("attributed to the caller");
    assert_eq!(by.room_id, "r1");
    assert_eq!(by.harness_id.as_deref(), Some("h1"));
}

#[tokio::test]
async fn open_harness_defaults_to_claude_with_a_conversation_id() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let out = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.kind, "claude");
    assert_eq!(out.name, "CC-2");
    let made = made_by(&store, "r1", &out.harness_id);
    let sid = made.session_id.expect("a minted Claude session id");
    assert!(made.cmd.unwrap().contains(&sid));
}

#[tokio::test]
async fn open_harness_with_an_empty_agent_runs_no_agent() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let args = verbs::OpenHarnessArgs {
        agent: Some(String::new()),
        ..open_harness_args("r1")
    };
    let out = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    assert_eq!(out.agent, None);
    let made = made_by(&store, "r1", &out.harness_id);
    assert_eq!(made.agent, None);
    assert!(!made.cmd.unwrap().iter().any(|a| a == "--agent"));
}

#[tokio::test]
async fn open_harness_accepts_hermes_with_the_skein_profile() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let out = verbs::open_harness(
        &state,
        &caller,
        &kind_args("r1", "hermes"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.kind, "hermes");
    assert_eq!(out.agent, None);
    assert!(
        made_by(&store, "r1", &out.harness_id)
            .hermes_profile
            .is_some()
    );
}

#[tokio::test]
async fn open_harness_files_is_a_surface_with_no_argv() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let out = verbs::open_harness(
        &state,
        &caller,
        &kind_args("r1", "files"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.name, "files-2");
    let made = made_by(&store, "r1", &out.harness_id);
    assert_eq!(made.status, "idle");
    assert!(made.cmd.is_none() && made.live.is_none());
}

#[tokio::test]
async fn open_harness_happy_path_in_a_room_it_created() {
    let f = fixture();
    let (state, store, caller) = with_store(
        &f,
        vec![
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![harness("h2a", "claude", "main")], "r1"),
        ],
    );
    let out = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(harnesses_of(&store, "r2").len(), 2);
    assert_eq!(
        harnesses_of(&store, "r1").len(),
        1,
        "the caller's room is untouched"
    );
    let made = made_by(&store, "r2", &out.harness_id);
    assert_eq!(made.created_by.as_ref().unwrap().room_id, "r1");
}

#[tokio::test]
async fn open_harness_with_a_prompt_refuses_an_unreachable_kind_before_opening_anything() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let args = verbs::OpenHarnessArgs {
        prompt: Some("go".to_owned()),
        ..kind_args("r1", "copilot")
    };
    let err = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("copilot")),
        "{err:?}"
    );
    assert_eq!(harnesses_of(&store, "r1").len(), 1, "nothing was stored");
}

#[tokio::test]
async fn open_harness_with_a_prompt_queues_the_first_message() {
    let f = fixture();
    let (state, store, caller) =
        with_store(&f, vec![room("r1", vec![harness("h1", "claude", "main")])]);
    let args = verbs::OpenHarnessArgs {
        prompt: Some("start working on the thing".to_owned()),
        ..open_harness_args("r1")
    };
    let out = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    let message_id = out
        .message_id
        .clone()
        .expect("a prompt should queue a message");
    made_by(&store, "r1", &out.harness_id);

    let messages = f.db.all_harness_messages("r1", &out.harness_id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, message_id);
    assert_eq!(messages[0].body, "start working on the thing");
    assert_eq!(messages[0].from_room_id, caller.room_id);
    assert!(messages[0].read_ms.is_none());
}
