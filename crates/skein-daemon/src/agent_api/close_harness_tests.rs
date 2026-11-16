//! `close_harness` (#411): guards, and the daemon-side removal (#734).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use skein_proto::room_mutations::RoomMutation;

use super::close_room_tests::room_created_by;
use super::open_harness_tests::open_harness_args;
use super::phase_tap::PhaseTap;
use super::state::AgentApiState;
use super::tests::{
    Fixture, agent_api_state, agent_api_state_with_room_records, caller_for, fixture, harness,
    room, save,
};
use super::verbs::{self, MailContext, VerbError};
use crate::db::{Harness, Room};
use crate::event_sink::EventSink;
use crate::harness_phase::PhaseService;
use crate::room_store::RoomStore;

struct NoEvents;

impl EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

// ── closing a harness (issue #411) ──────────────────────────────────────
//
// `close_harness` is exercised as a plain function (`verbs::close_harness`),
// the same way `close_room`'s and `open_harness`'s own guards are —
// dispatch through MCP is already covered above
// (`close_harness_is_dispatched_through_mcp`). It shares `open_harness`'s
// `harness_control` rate bucket, so the two verbs' rate-limit tests
// together prove the cap is combined rather than per-verb.

fn close_harness_args(harness: &str) -> verbs::CloseHarnessArgs {
    verbs::CloseHarnessArgs {
        harness: harness.to_owned(),
    }
}

#[tokio::test]
async fn close_harness_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), false)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("disabled:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_shares_open_harness_rate_bucket() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // Spend nine of the ten combined attempts through `open_harness`
    // (proof against a missing room, same trick `open_harness`'s own
    // rate test uses), then the tenth through `close_harness` against an
    // unknown harness — both must reach their own NotFound, and the
    // eleventh, on either verb, must be rate-limited.
    for n in 0..9 {
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
            "attempt {n}: expected NotFound, got {err:?}"
        );
    }
    let err = verbs::close_harness(&state, &caller, &close_harness_args("missing"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(_)),
        "tenth combined attempt: expected NotFound, got {err:?}"
    );
    let err = verbs::close_harness(&state, &caller, &close_harness_args("missing"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("rate_limited:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_unknown_harness_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("nope"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_a_harness_neither_in_its_own_room_nor_one_it_created() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "main"),
                    harness("h2b", "claude", "two"),
                ],
                "r-someone-else",
            ),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2a"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_in_scope:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_in_its_own_room_refuses_an_unidentified_caller() {
    // No `X-Skein-Harness` at all: there is no way to prove the target
    // isn't the caller itself, so this must refuse rather than guess.
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", None);
    assert_eq!(caller.harness_id, None, "test setup: no caller identity");
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("caller_unknown:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_cannot_close_itself() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h1"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("self:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_a_harness_in_an_archived_room_it_created() {
    let f = fixture();
    let mut target = room_created_by(
        "r2",
        vec![
            harness("h2a", "claude", "main"),
            harness("h2b", "claude", "two"),
        ],
        "r1",
    );
    target.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2a"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_the_last_harness_in_a_room_it_created() {
    // A room's only harness can never be `self` from another room's
    // caller — only `close_harness_cannot_close_itself` above exercises
    // that overlap, where the caller and the room's last harness are the
    // same id. Here the caller is a *different* room, so this is the one
    // path that actually reaches the `last_harness` guard.
    // Unlike the test above, the caller here is a *different* room, so
    // `self` never applies and `last_harness` is the guard that fires.
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![harness("h2", "claude", "main")], "r1"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("last_harness:")),
        "{err:?}"
    );
}

// ── the removal itself runs in the daemon (#734) ────────────────────────
//
// No client is attached in any of these unless a test says so.

fn stored(store: &RoomStore, id: &str) -> Room {
    store.rooms().into_iter().find(|r| r.id == id).unwrap()
}

/// `r1` holding `h1` (the caller) and `harnesses` in the db and the store.
fn own_room(f: &Fixture, harnesses: Vec<Harness>) -> (AgentApiState, Arc<RoomStore>) {
    let mut all = vec![harness("h1", "claude", "main")];
    all.extend(harnesses);
    agent_api_state_with_room_records(f, vec![room("r1", all)])
}

fn files_client(state: &AgentApiState, dirty: Value) -> Arc<Mutex<Vec<(String, Value)>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_clone = Arc::clone(&seen);
    state.set_test_frontend(move |kind, args| {
        seen_clone
            .lock()
            .unwrap()
            .push((kind.to_owned(), args.clone()));
        match kind {
            "files.dirty" => Ok(dirty.clone()),
            other => Err(format!("unexpected request_frontend kind {other:?}")),
        }
    });
    seen
}

#[tokio::test]
async fn close_harness_without_a_room_store_is_unavailable() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(matches!(&err, VerbError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn close_harness_removes_it_in_its_own_room() {
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("h2", "claude", "second")]);
    let caller = caller_for(&f.db, "r1", Some("h1"));

    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r1");
    assert_eq!(out.harness_id, "h2");
    assert_eq!(
        out.phase, "unknown",
        "no phase service, so nothing is known"
    );
    assert_eq!(out.closed_by.room_id, "r1");
    assert_eq!(out.closed_by.harness_id.as_deref(), Some("h1"));

    let r1 = stored(&store, "r1");
    let ids: Vec<&str> = r1.harnesses.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["h1"]);
    assert_eq!(
        r1.active_harness_id, "h1",
        "the active harness was not touched"
    );
}

#[tokio::test]
async fn close_harness_moves_the_active_harness_to_the_first_remaining() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![room("r1", vec![harness("h1", "claude", "main")]), {
            let mut r = room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "main"),
                    harness("h2b", "claude", "two"),
                    harness("h2c", "claude", "three"),
                ],
                "r1",
            );
            r.active_harness_id = "h2a".to_owned();
            r
        }],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2a"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
    let r2 = stored(&store, "r2");
    assert_eq!(r2.active_harness_id, "h2b");
    assert_eq!(r2.harnesses.len(), 2);
}

#[tokio::test]
async fn close_harness_refuses_a_permission_dialog_the_phase_service_reports() {
    let f = fixture();
    let (mut state, store) = own_room(&f, vec![harness("h2", "claude", "second")]);
    let phase = PhaseService::test_new();
    state.phase_tap = Some(PhaseTap {
        service: Arc::clone(&phase),
        rooms: RoomStore::new(Arc::clone(&f.db), Arc::new(NoEvents)),
        hermes_work: None,
    });
    phase.spawned("h2", "r1", false);
    phase.claude_permission_hook("h2", Some("Bash"), None, None);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("permission_open:")),
        "{err:?}"
    );
    assert_eq!(stored(&store, "r1").harnesses.len(), 2);
}

#[tokio::test]
async fn close_harness_reports_the_phase_it_interrupted() {
    let f = fixture();
    let (mut state, _store) = own_room(&f, vec![harness("h2", "claude", "second")]);
    let phase = PhaseService::test_new();
    state.phase_tap = Some(PhaseTap {
        service: Arc::clone(&phase),
        rooms: RoomStore::new(Arc::clone(&f.db), Arc::new(NoEvents)),
        hermes_work: None,
    });
    phase.spawned("h2", "r1", false);
    phase.record_output("h2", "hello");
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap();
    assert_eq!(out.phase, "running");
}

#[tokio::test]
async fn close_harness_in_a_room_it_created_removes_it() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "main"),
                    harness("h2b", "claude", "two"),
                ],
                "r1",
            ),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2b"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(out.harness_id, "h2b");
    assert_eq!(stored(&store, "r2").harnesses.len(), 1);
}

#[tokio::test]
async fn close_harness_refuses_when_the_store_no_longer_has_it() {
    // The guards read the db; the store is the authority and dropped the
    // harness in the meantime.
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("h2", "claude", "second")]);
    store
        .apply(
            "test",
            vec![RoomMutation::RemoveHarness {
                room_id: "r1".to_owned(),
                harness_id: "h2".to_owned(),
            }],
        )
        .unwrap();
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.starts_with("not_found:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_unsaved_files_a_client_reports() {
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("hf", "files", "files")]);
    let seen = files_client(
        &state,
        json!({ "dirty": [{ "harnessId": "hf", "names": ["notes.md"] }] }),
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_harness(&state, &caller, &close_harness_args("hf"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m)
            if m.starts_with("unsaved_files:") && m.contains("notes.md")),
        "{err:?}"
    );
    assert_eq!(stored(&store, "r1").harnesses.len(), 2);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "files.dirty");
    assert_eq!(seen[0].1, json!({ "harnessIds": ["hf"] }));
}

#[tokio::test]
async fn close_harness_proceeds_when_the_client_reports_the_buffers_clean() {
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("hf", "files", "files")]);
    let _seen = files_client(&state, json!({ "dirty": [] }));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_harness(&state, &caller, &close_harness_args("hf"), true)
        .await
        .unwrap();
    assert_eq!(stored(&store, "r1").harnesses.len(), 1);
}

#[tokio::test]
async fn close_harness_of_a_files_harness_with_no_client_proceeds() {
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("hf", "files", "files")]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_harness(&state, &caller, &close_harness_args("hf"), true)
        .await
        .unwrap();
    assert_eq!(stored(&store, "r1").harnesses.len(), 1);
}

#[tokio::test]
async fn close_harness_never_asks_the_client_for_a_non_files_harness() {
    let f = fixture();
    let (state, store) = own_room(
        &f,
        vec![
            harness("h2", "claude", "second"),
            harness("hf", "files", "files"),
        ],
    );
    let seen = files_client(&state, json!({ "dirty": [] }));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap();
    assert_eq!(stored(&store, "r1").harnesses.len(), 2);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn close_harness_refuses_when_the_client_cannot_answer_files_dirty() {
    let f = fixture();
    let (state, store) = own_room(&f, vec![harness("hf", "files", "files")]);
    state.set_test_frontend(|_, _| Err("agent request 1 (files.dirty) timed out after 5s".into()));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_harness(&state, &caller, &close_harness_args("hf"), true)
        .await
        .unwrap_err();
    assert!(matches!(&err, VerbError::Unavailable(_)), "{err:?}");
    assert_eq!(stored(&store, "r1").harnesses.len(), 2);
}
