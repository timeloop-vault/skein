//! `RoomStore::execute` (#756): what each intent command does to the record
//! and why it is refused. Patches are in `patches.rs`, revisions in
//! `revision.rs`.

use skein_proto::room_commands::{ClosedByRef, RoomCommand as C};
use skein_proto::rooms::{Harness, LaunchMode, Room, ShellClaim};

use super::{Fixture, harness, ids, refusals, room, room_with};

fn launch_shell() -> LaunchMode {
    LaunchMode::Shell {
        argv: vec!["pwsh".into()],
    }
}

fn claim() -> ShellClaim {
    serde_json::from_value(serde_json::json!({"sessionId":"s","port":1})).unwrap()
}

fn h_of(f: &Fixture, room_id: &str, harness_id: &str) -> Harness {
    f.stored_room(room_id)
        .harnesses
        .into_iter()
        .find(|h| h.id == harness_id)
        .unwrap()
}

fn harness_ids(r: &Room) -> Vec<&str> {
    r.harnesses.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn execute_before_the_store_is_loaded_is_an_error() {
    let f = Fixture::unfilled();
    let err = f
        .store
        .execute("a", vec![C::CreateRoom { room: room("r1") }])
        .unwrap_err();
    assert!(err.contains("not ready"), "{err}");
    assert!(f.events().is_empty());
    assert!(f.store.rooms().is_empty());
}

#[test]
fn create_room_appends_at_revision_one_whatever_the_payload_said() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let mut r2 = room("r2");
    r2.revision = 99;
    let out = f.execute("a", vec![C::CreateRoom { room: r2 }]);
    assert!(out.refused.is_empty());
    assert_eq!(f.store_ids(), ["r1", "r2"]);
    assert_eq!(f.stored_room("r2").revision, 1);
    assert_eq!(f.stored_room("r1").revision, 0);
    assert_eq!(f.disk_ids(), ["r1", "r2"], "structural: on disk at once");
}

#[test]
fn create_room_with_an_existing_id_is_refused_exists() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute("a", vec![C::CreateRoom { room: room("r1") }]);
    assert_eq!(refusals(&out), [(0, "exists")]);
    assert_eq!(out.revisions.len(), 0);
    assert_eq!(f.stored_room("r1").revision, 0);
    assert!(f.events().is_empty());
}

#[test]
fn rename_room_sets_the_name_and_refuses_an_unknown_room() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute(
        "a",
        vec![
            C::RenameRoom {
                room_id: "r1".into(),
                name: "new".into(),
            },
            C::RenameRoom {
                room_id: "nope".into(),
                name: "x".into(),
            },
        ],
    );
    assert_eq!(f.stored_room("r1").name, "new");
    assert_eq!(refusals(&out), [(1, "unknown_room")]);
}

#[test]
fn archive_room_sets_archived_and_closed_by_with_the_same_time() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute(
        "a",
        vec![C::ArchiveRoom {
            room_id: "r1".into(),
            closed_by: Some(ClosedByRef {
                room_id: "boss".into(),
                harness_id: Some("bh".into()),
            }),
        }],
    );
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    let at = r.archived.expect("archived is set");
    assert!(at > 0);
    let cb = r.closed_by.expect("closed_by is set");
    assert_eq!(cb.room_id, "boss");
    assert_eq!(cb.harness_id.as_deref(), Some("bh"));
    assert_eq!(cb.at, at);
}

#[test]
fn archive_room_with_no_closed_by_clears_it() {
    let mut r1 = room("r1");
    r1.closed_by = Some(skein_proto::rooms::ClosedBy {
        room_id: "old".into(),
        harness_id: None,
        at: 5,
    });
    let f = Fixture::with_rooms(&[r1]);
    f.execute(
        "a",
        vec![C::ArchiveRoom {
            room_id: "r1".into(),
            closed_by: None,
        }],
    );
    let r = f.stored_room("r1");
    assert!(r.archived.is_some());
    assert!(r.closed_by.is_none());
}

#[test]
fn archive_room_twice_is_refused_already_archived_and_keeps_the_first_time() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let archive = || C::ArchiveRoom {
        room_id: "r1".into(),
        closed_by: None,
    };
    f.execute("a", vec![archive()]);
    let first = f.stored_room("r1");
    let out = f.execute("a", vec![archive(), archive()]);
    assert_eq!(
        refusals(&out),
        [(0, "already_archived"), (1, "already_archived")]
    );
    assert_eq!(f.stored_room("r1").archived, first.archived);
    assert_eq!(f.stored_room("r1").revision, first.revision);
}

#[test]
fn unarchive_room_clears_archived_retired_and_closed_by() {
    let mut r1 = room("r1");
    r1.archived = Some(10);
    r1.retired = Some(20);
    r1.closed_by = Some(skein_proto::rooms::ClosedBy {
        room_id: "boss".into(),
        harness_id: None,
        at: 10,
    });
    let f = Fixture::with_rooms(&[r1]);
    let out = f.execute(
        "a",
        vec![C::UnarchiveRoom {
            room_id: "r1".into(),
        }],
    );
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert!(r.archived.is_none() && r.retired.is_none() && r.closed_by.is_none());
}

#[test]
fn unarchive_room_on_an_active_room_is_refused_not_archived() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute(
        "a",
        vec![
            C::UnarchiveRoom {
                room_id: "r1".into(),
            },
            C::UnarchiveRoom {
                room_id: "nope".into(),
            },
        ],
    );
    assert_eq!(refusals(&out), [(0, "not_archived"), (1, "unknown_room")]);
}

#[test]
fn reorder_rooms_changes_the_order_and_emits_it() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2"), room("r3")]);
    let out = f.execute(
        "a",
        vec![C::ReorderRooms {
            room_ids: ids(&["r3", "r1", "r2"]),
        }],
    );
    assert!(out.refused.is_empty());
    assert_eq!(out.revisions.len(), 0, "no room record changed");
    assert_eq!(f.store_ids(), ["r3", "r1", "r2"]);
    assert_eq!(f.only_event().1, ["order:r3,r1,r2"]);
    assert_eq!(f.stored_room("r1").revision, 0);
}

#[test]
fn reorder_rooms_to_the_same_order_changes_nothing() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);
    f.execute(
        "a",
        vec![C::ReorderRooms {
            room_ids: ids(&["r1", "r2"]),
        }],
    );
    assert!(f.events().is_empty());
}

#[test]
fn add_harness_appends_and_activate_decides_the_active_harness() {
    let mut r1 = room_with("r1", &["h1"]);
    r1.active_harness_id = "h1".into();
    let f = Fixture::with_rooms(&[r1]);
    let add = |id: &str, activate: bool| C::AddHarness {
        room_id: "r1".into(),
        harness: harness(id),
        activate,
    };
    let out = f.execute("a", vec![add("h2", false)]);
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(harness_ids(&r), ["h1", "h2"]);
    assert_eq!(r.active_harness_id, "h1", "not activated");

    f.execute("a", vec![add("h3", true)]);
    let r = f.stored_room("r1");
    assert_eq!(harness_ids(&r), ["h1", "h2", "h3"]);
    assert_eq!(r.active_harness_id, "h3");
}

#[test]
fn add_harness_refuses_an_existing_id_and_an_unknown_room() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![
            C::AddHarness {
                room_id: "r1".into(),
                harness: harness("h1"),
                activate: true,
            },
            C::AddHarness {
                room_id: "nope".into(),
                harness: harness("h9"),
                activate: false,
            },
        ],
    );
    assert_eq!(refusals(&out), [(0, "exists"), (1, "unknown_room")]);
    let r = f.stored_room("r1");
    assert_eq!(harness_ids(&r), ["h1"]);
    assert_eq!(r.active_harness_id, "", "a refused add does not activate");
}

#[test]
fn add_harness_refuses_an_id_used_in_another_room() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"]), room("r2")]);
    let out = f.execute(
        "a",
        vec![C::AddHarness {
            room_id: "r2".into(),
            harness: harness("h1"),
            activate: true,
        }],
    );
    assert_eq!(refusals(&out), [(0, "exists")]);
    assert!(f.stored_room("r2").harnesses.is_empty());
    assert_eq!(f.stored_room("r2").active_harness_id, "");
    assert_eq!(f.stored_room("r2").revision, 0);
    assert!(f.events().is_empty());
}

#[test]
fn close_harness_removes_it_and_moves_the_active_one_to_the_first_remaining() {
    let mut r1 = room_with("r1", &["h1", "h2", "h3"]);
    r1.active_harness_id = "h2".into();
    let f = Fixture::with_rooms(&[r1]);
    let close = |id: &str| C::CloseHarness {
        room_id: "r1".into(),
        harness_id: id.into(),
    };
    f.execute("a", vec![close("h2")]);
    let r = f.stored_room("r1");
    assert_eq!(harness_ids(&r), ["h1", "h3"]);
    assert_eq!(r.active_harness_id, "h1");
}

#[test]
fn close_harness_leaves_the_active_one_when_another_was_active() {
    let mut r1 = room_with("r1", &["h1", "h2"]);
    r1.active_harness_id = "h2".into();
    let f = Fixture::with_rooms(&[r1]);
    f.execute(
        "a",
        vec![C::CloseHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
        }],
    );
    let r = f.stored_room("r1");
    assert_eq!(harness_ids(&r), ["h2"]);
    assert_eq!(r.active_harness_id, "h2");
}

#[test]
fn closing_the_last_harness_empties_the_active_id() {
    let mut r1 = room_with("r1", &["h1"]);
    r1.active_harness_id = "h1".into();
    let f = Fixture::with_rooms(&[r1]);
    f.execute(
        "a",
        vec![C::CloseHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
        }],
    );
    let r = f.stored_room("r1");
    assert!(r.harnesses.is_empty());
    assert_eq!(r.active_harness_id, "");
}

#[test]
fn close_harness_refuses_an_absent_harness_and_an_unknown_room() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![
            C::CloseHarness {
                room_id: "r1".into(),
                harness_id: "ghost".into(),
            },
            C::CloseHarness {
                room_id: "nope".into(),
                harness_id: "h1".into(),
            },
        ],
    );
    assert_eq!(refusals(&out), [(0, "absent"), (1, "unknown_room")]);
    assert_eq!(harness_ids(&f.stored_room("r1")), ["h1"]);
}

#[test]
fn restart_harness_clears_launch_and_shell_claim_only() {
    let mut r1 = room_with("r1", &["h1"]);
    {
        let h = &mut r1.harnesses[0];
        h.launch = Some(launch_shell());
        h.shell_claim = Some(claim());
        h.cmd = Some(vec!["claude".into()]);
        h.session_id = Some("sid".into());
    }
    let f = Fixture::with_rooms(&[r1]);
    let out = f.execute(
        "a",
        vec![C::RestartHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
        }],
    );
    assert!(out.refused.is_empty());
    let h = h_of(&f, "r1", "h1");
    assert!(h.launch.is_none() && h.shell_claim.is_none());
    assert_eq!(h.cmd, Some(vec!["claude".to_owned()]));
    assert_eq!(h.session_id.as_deref(), Some("sid"));
}

#[test]
fn restart_harness_on_a_clean_record_changes_nothing() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![C::RestartHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
        }],
    );
    assert_eq!(out.refused.len() + out.revisions.len(), 0);
    assert!(f.events().is_empty());
}

#[test]
fn restart_harness_refuses_an_unknown_harness() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![C::RestartHarness {
            room_id: "r1".into(),
            harness_id: "ghost".into(),
        }],
    );
    assert_eq!(refusals(&out), [(0, "unknown_harness")]);
}

#[test]
fn set_harness_agent_sets_and_clears_it() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let set = |agent: Option<&str>| C::SetHarnessAgent {
        room_id: "r1".into(),
        harness_id: "h1".into(),
        agent: agent.map(str::to_owned),
    };
    f.execute("a", vec![set(Some("reviewer"))]);
    assert_eq!(h_of(&f, "r1", "h1").agent.as_deref(), Some("reviewer"));
    f.execute("a", vec![set(None)]);
    assert_eq!(h_of(&f, "r1", "h1").agent, None);
}

#[test]
fn set_harness_agent_refuses_an_unknown_harness_and_an_unknown_room() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![
            C::SetHarnessAgent {
                room_id: "r1".into(),
                harness_id: "ghost".into(),
                agent: Some("x".into()),
            },
            C::SetHarnessAgent {
                room_id: "nope".into(),
                harness_id: "h1".into(),
                agent: Some("x".into()),
            },
        ],
    );
    assert_eq!(
        refusals(&out),
        [(0, "unknown_harness"), (1, "unknown_room")]
    );
}

#[test]
fn reorder_harnesses_changes_the_order() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2", "h3"])]);
    let out = f.execute(
        "a",
        vec![
            C::ReorderHarnesses {
                room_id: "r1".into(),
                harness_ids: ids(&["h3", "h1", "h2"]),
            },
            C::ReorderHarnesses {
                room_id: "nope".into(),
                harness_ids: ids(&["h1"]),
            },
        ],
    );
    assert_eq!(refusals(&out), [(1, "unknown_room")]);
    assert_eq!(harness_ids(&f.stored_room("r1")), ["h3", "h1", "h2"]);
    assert_eq!(f.stored_room("r1").revision, 1);
}

#[test]
fn a_refused_command_does_not_stop_the_later_ones() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute(
        "a",
        vec![
            C::RenameRoom {
                room_id: "nope".into(),
                name: "x".into(),
            },
            C::RenameRoom {
                room_id: "r1".into(),
                name: "ok".into(),
            },
            C::CreateRoom { room: room("r1") },
            C::CreateRoom { room: room("r2") },
        ],
    );
    assert_eq!(refusals(&out), [(0, "unknown_room"), (2, "exists")]);
    assert_eq!(f.stored_room("r1").name, "ok");
    assert_eq!(f.store_ids(), ["r1", "r2"]);
}

#[test]
fn a_batch_emits_one_event_with_the_given_origin_and_reports_revisions() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2"), room("r3")]);
    let out = f.execute(
        "client-7",
        vec![
            C::RenameRoom {
                room_id: "r1".into(),
                name: "a".into(),
            },
            C::RenameRoom {
                room_id: "r1".into(),
                name: "b".into(),
            },
            C::RenameRoom {
                room_id: "r3".into(),
                name: "c".into(),
            },
        ],
    );
    let (origin, changes) = f.only_event();
    assert_eq!(origin, "client-7");
    assert_eq!(changes, ["put:r1", "put:r3"]);
    let revs: Vec<(&str, u64)> = out
        .revisions
        .iter()
        .map(|r| (r.room_id.as_str(), r.revision))
        .collect();
    assert_eq!(revs, [("r1", 1), ("r3", 1)], "r2 untouched, r1 bumped once");
}
