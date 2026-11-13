//! Cross-room listing (#356): `list_rooms`, `get_room` and `list_harnesses`.

use std::sync::Arc;

use super::auth::Caller;
use super::mail_send_tests::send;
use super::mcp;
use super::phase_tap::PhaseTap;
use super::review_tests::{commit_file, git_repo_with_commit};
use super::state::AgentApiState;
use super::tests::{Fixture, agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};
use crate::db::CreatedBy;
use crate::event_sink::EventSink;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::PhaseService;
use crate::room_store::RoomStore;

// ── cross-room room and harness listing, issue #356 ─────────────────

async fn call_list_rooms(
    state: &AgentApiState,
    caller: &Caller,
    created_by: Option<&str>,
) -> Result<verbs::ListRoomsOut, VerbError> {
    verbs::list_rooms(
        state,
        caller,
        &verbs::ListRoomsArgs {
            created_by: created_by.map(str::to_owned),
            // These #356 tests are about attribution; the archived
            // child must still show (the #612 default hides it).
            include_archived: true,
            ..Default::default()
        },
        &MailContext::permissive(),
    )
    .await
}

pub(super) fn created_by(room_id: &str, harness_id: &str) -> CreatedBy {
    CreatedBy {
        room_id: room_id.to_owned(),
        harness_id: harness_id.to_owned(),
        prompt_first_line: None,
        base_sha: None,
    }
}

#[tokio::test]
async fn list_rooms_created_by_me_scopes_to_the_calling_director_and_carries_the_last_status() {
    // Two directors, each with a room `create_room` made for them —
    // proof that "me" scopes to the *caller's* room, one it created
    // stays visible after being archived, and `lastStatus` only ever
    // reports what was sent to the director asking, never a sibling's.
    let f = fixture();
    let mut child_a = room("child-a", vec![harness("ha", "claude", "main")]);
    child_a.created_by = Some(CreatedBy {
        prompt_first_line: Some("fix the flaky test".to_owned()),
        base_sha: Some("abc123".to_owned()),
        ..created_by("director-a", "da")
    });
    let mut child_a_done = room("child-a-done", vec![harness("ha2", "claude", "main")]);
    child_a_done.archived = Some(1_000);
    child_a_done.created_by = Some(created_by("director-a", "da"));
    let mut child_b = room("child-b", vec![harness("hb", "claude", "main")]);
    child_b.created_by = Some(created_by("director-b", "db"));
    save(
        &f.db,
        &[
            room("director-a", vec![harness("da", "claude", "main")]),
            room("director-b", vec![harness("db", "claude", "main")]),
            child_a,
            child_a_done,
            child_b,
        ],
    );
    let child_a_caller = caller_for(&f.db, "child-a", Some("ha"));
    send(
        &f.db,
        &child_a_caller,
        "director-a",
        "status: review — done",
    )
    .unwrap();
    let state = agent_api_state(&f);

    let director_a = caller_for(&f.db, "director-a", Some("da"));
    let out_a = call_list_rooms(&state, &director_a, Some("me"))
        .await
        .unwrap();
    let ids: Vec<&str> = out_a.rooms.iter().map(|r| r.room_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        2,
        "director-a must see only its own two rooms: {ids:?}"
    );
    assert!(ids.contains(&"child-a"));
    assert!(
        ids.contains(&"child-a-done"),
        "an archived child must still be listed, marked archived"
    );
    assert!(!ids.contains(&"child-b"), "not another director's room");

    let done = out_a
        .rooms
        .iter()
        .find(|r| r.room_id == "child-a-done")
        .unwrap();
    assert!(done.archived);
    assert_eq!(done.lifecycle, "archived");

    let open = out_a.rooms.iter().find(|r| r.room_id == "child-a").unwrap();
    assert!(!open.archived);
    assert_eq!(
        open.lifecycle, "unknown",
        "no webview is wired up in this test"
    );
    assert_eq!(open.lead_harness_id.as_deref(), Some("ha"));
    assert_eq!(
        open.prompt_first_line.as_deref(),
        Some("fix the flaky test")
    );
    assert_eq!(open.base_sha.as_deref(), Some("abc123"));
    let status = open
        .last_status
        .as_ref()
        .expect("child-a sent director-a a status");
    assert_eq!(status.first_line, "status: review — done");

    let director_b = caller_for(&f.db, "director-b", Some("db"));
    let out_b = call_list_rooms(&state, &director_b, Some("me"))
        .await
        .unwrap();
    assert_eq!(
        out_b
            .rooms
            .iter()
            .map(|r| r.room_id.as_str())
            .collect::<Vec<_>>(),
        vec!["child-b"],
        "director-b must see only its own room"
    );
    assert!(
        out_b.rooms[0].last_status.is_none(),
        "child-b never sent director-b anything"
    );
}

#[tokio::test]
async fn list_rooms_with_no_filter_lists_every_room() {
    let f = fixture();
    let mut child = room("child", vec![harness("h", "claude", "main")]);
    child.created_by = Some(created_by("director", "d"));
    save(
        &f.db,
        &[
            room("director", vec![harness("d", "claude", "main")]),
            child,
        ],
    );
    let caller = caller_for(&f.db, "director", Some("d"));
    let state = agent_api_state(&f);
    let out = call_list_rooms(&state, &caller, None).await.unwrap();
    let ids: Vec<&str> = out.rooms.iter().map(|r| r.room_id.as_str()).collect();
    assert_eq!(ids.len(), 2, "no filter must list every room: {ids:?}");
}

#[tokio::test]
async fn list_rooms_rejects_an_unknown_created_by_value() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = call_list_rooms(&state, &caller, Some("mine"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("mine")),
        "{err:?}"
    );
}

#[tokio::test]
async fn get_room_refuses_an_unknown_room_id_by_name() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let state = agent_api_state(&f);
    let err = verbs::get_room(
        &state,
        &verbs::GetRoomArgs {
            room: "ghost".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("ghost")),
        "{err:?}"
    );
}

#[tokio::test]
async fn get_room_refuses_an_archived_room_naming_it_and_never_answers_none() {
    let f = fixture();
    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.archived = Some(1_000);
    save(&f.db, &[r]);
    let state = agent_api_state(&f);
    let err = verbs::get_room(
        &state,
        &verbs::GetRoomArgs {
            room: "r1".to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("r1") && m.contains("archived")),
        "{err:?}"
    );
}

async fn assert_get_room_signoff_matches_review_status(
    f: &Fixture,
    state: &AgentApiState,
    caller: &Caller,
) {
    let status = verbs::review_status(&f.db, caller).unwrap();
    let room_out = verbs::get_room(
        state,
        &verbs::GetRoomArgs {
            room: caller.room_id.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&room_out.signoff).unwrap(),
        serde_json::to_value(Some(status)).unwrap()
    );
    assert!(room_out.signoff_unavailable.is_none());
}

#[tokio::test]
async fn get_room_signoff_block_matches_review_status_across_signoff_states() {
    // The shared `signoff_block` fn is the whole point: whatever
    // `review_status` says about a room's own sign-off, `get_room` must
    // say the identical thing when asked about that same room.
    let f = fixture();
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path().to_str().unwrap().to_owned();
    git_repo_with_commit(tmp.path());
    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.cwd = Some(cwd.clone());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    // none
    assert_get_room_signoff_matches_review_status(&f, &state, &caller).await;

    // approved
    crate::review_surface::signoff::set_impl(&f.db, "r1", &cwd, true, None, 1_000).unwrap();
    assert_get_room_signoff_matches_review_status(&f, &state, &caller).await;

    // stale
    commit_file(tmp.path(), "b.txt", "more\n", "feat: more");
    assert_get_room_signoff_matches_review_status(&f, &state, &caller).await;
}

#[tokio::test]
async fn get_room_reports_signoff_unavailable_when_the_room_has_no_worktree() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let state = agent_api_state(&f);
    let out = verbs::get_room(
        &state,
        &verbs::GetRoomArgs {
            room: "r1".to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(out.signoff.is_none());
    assert!(
        out.signoff_unavailable
            .as_deref()
            .unwrap()
            .contains("no worktree")
    );
}

struct NoEvents;

impl EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

/// A state whose phase tap holds a real, empty [`PhaseService`].
fn state_with_phases(f: &Fixture) -> (AgentApiState, Arc<PhaseService>) {
    let phase = PhaseService::test_new();
    let mut state = agent_api_state(f);
    state.phase_tap = Some(PhaseTap {
        service: Arc::clone(&phase),
        rooms: RoomStore::new(Arc::clone(&f.db), Arc::new(NoEvents)),
        hermes_work: None,
    });
    (state, phase)
}

fn subagent_start(id: &str) -> ClaudeEvent {
    ClaudeEvent::SubagentStart {
        agent_id: id.to_owned(),
        agent_type: None,
        description: None,
        initial: false,
    }
}

async fn summary_of(state: &AgentApiState) -> verbs::HarnessSummary {
    let mut out = verbs::get_room(
        state,
        &verbs::GetRoomArgs {
            room: "r1".to_owned(),
        },
    )
    .await
    .unwrap();
    out.harnesses.remove(0)
}

#[tokio::test]
async fn get_room_reports_the_phase_service_phase() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let (state, phase) = state_with_phases(&f);
    phase.spawned("h1", "r1", false);
    assert_eq!(summary_of(&state).await.phase, "spawning");
    phase.record_output("h1", "hello");
    assert_eq!(summary_of(&state).await.phase, "running");
    phase.exited("h1", Some(0));
    assert_eq!(summary_of(&state).await.phase, "exited");
}

#[tokio::test]
async fn get_room_reports_unknown_for_a_harness_the_service_has_no_record_of() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let (state, _phase) = state_with_phases(&f);
    let h = summary_of(&state).await;
    assert_eq!(h.phase, "unknown");
    assert_eq!(h.outstanding_subagents, None);
    assert_eq!(h.outstanding_background_tasks, None);
}

#[tokio::test]
async fn get_room_reports_unknown_when_the_state_has_no_phase_service() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let state = agent_api_state(&f);
    assert_eq!(summary_of(&state).await.phase, "unknown");
}

#[tokio::test]
async fn get_room_maps_outstanding_work_counts() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let (state, phase) = state_with_phases(&f);
    phase.spawned("h1", "r1", false);
    phase.claude_attached("h1");
    phase.claude_event("h1", &subagent_start("a"));
    phase.claude_event("h1", &subagent_start("b"));
    let h = summary_of(&state).await;
    assert_eq!(h.outstanding_subagents, Some(2));
    assert_eq!(h.outstanding_background_tasks, Some(0));
}

#[tokio::test]
async fn work_counts_are_null_and_serialized_for_an_unknown_harness() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let (state, _phase) = state_with_phases(&f);
    let out = verbs::list_harnesses(&state, &verbs::ListHarnessesArgs::default())
        .await
        .unwrap();
    assert_eq!(out.harnesses[0].outstanding_subagents, None);
    let v = serde_json::to_value(&out.harnesses[0]).unwrap();
    assert!(v["outstanding_subagents"].is_null());
    assert!(v["outstanding_background_tasks"].is_null());
}

#[tokio::test]
async fn list_harnesses_maps_outstanding_work_counts() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let (state, phase) = state_with_phases(&f);
    phase.spawned("h1", "r1", false);
    let out = verbs::list_harnesses(&state, &verbs::ListHarnessesArgs::default())
        .await
        .unwrap();
    assert_eq!(out.harnesses[0].outstanding_subagents, Some(0));
    assert_eq!(out.harnesses[0].outstanding_background_tasks, Some(0));
}

#[tokio::test]
async fn list_harnesses_refuses_an_unknown_room_id_by_name() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let state = agent_api_state(&f);
    let err = verbs::list_harnesses(
        &state,
        &verbs::ListHarnessesArgs {
            room: Some("ghost".to_owned()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("ghost")),
        "{err:?}"
    );
}

#[tokio::test]
async fn list_harnesses_refuses_an_archived_room_id_by_name() {
    let f = fixture();
    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.archived = Some(1_000);
    save(&f.db, &[r]);
    let state = agent_api_state(&f);
    let err = verbs::list_harnesses(
        &state,
        &verbs::ListHarnessesArgs {
            room: Some("r1".to_owned()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("r1") && m.contains("archived")),
        "{err:?}"
    );
}

#[tokio::test]
async fn list_harnesses_with_no_room_filter_skips_archived_rooms() {
    let f = fixture();
    let mut archived = room("r1", vec![harness("h1", "claude", "main")]);
    archived.archived = Some(1_000);
    save(
        &f.db,
        &[archived, room("r2", vec![harness("h2", "claude", "main")])],
    );
    let state = agent_api_state(&f);
    let out = verbs::list_harnesses(&state, &verbs::ListHarnessesArgs { room: None })
        .await
        .unwrap();
    let ids: Vec<&str> = out
        .harnesses
        .iter()
        .map(|h| h.harness_id.as_str())
        .collect();
    assert_eq!(ids, vec!["h2"]);
}

#[tokio::test]
async fn list_rooms_get_room_and_list_harnesses_are_in_the_mcp_tool_list() {
    let names: Vec<String> = mcp::tool_specs()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    for name in ["list_rooms", "get_room", "list_harnesses"] {
        assert!(names.contains(&name.to_owned()), "{names:?}");
    }
}
