use super::*;
use crate::db::test_support::{fresh_db, harness, room};
use tempfile::TempDir;

/// #433: a design harness's chosen entry survives a restart, and a blob without the key still loads.
#[test]
fn a_harness_design_entry_survives_the_round_trip() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    let mut with_entry = harness("h1");
    with_entry.design_entry = Some("mock ups/index.html".into());
    r.harnesses = vec![with_entry, harness("h2")];
    r.active_harness_id = "h1".into();
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    let hs = &outcome.rooms[0].harnesses;
    assert_eq!(hs[0].design_entry.as_deref(), Some("mock ups/index.html"));
    assert_eq!(hs[1].design_entry, None);

    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[{"id":"h1","kind":"design","name":"h1","status":"idle",
        "model":"","tokens":"0"}],"activeHarnessId":"h1"}"#;
    let old: Room = serde_json::from_str(json).unwrap();
    assert_eq!(old.harnesses[0].design_entry, None);
}

/// #528: a design harness's device setting round-trips verbatim, unknown keys included.
#[test]
fn a_harness_design_device_survives_the_round_trip() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    let device = serde_json::json!({"preset":"custom","width":500,"height":700,
        "landscape":true,"dpr":2,"futureFlag":true});
    let mut with_device = harness("h1");
    with_device.design_device = Some(device.clone());
    r.harnesses = vec![with_device, harness("h2")];
    r.active_harness_id = "h1".into();
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    let hs = &outcome.rooms[0].harnesses;
    assert_eq!(hs[0].design_device, Some(device));
    assert_eq!(hs[1].design_device, None);

    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[{"id":"h1","kind":"design","name":"h1","status":"idle",
        "model":"","tokens":"0"}],"activeHarnessId":"h1"}"#;
    let old: Room = serde_json::from_str(json).unwrap();
    assert_eq!(old.harnesses[0].design_device, None);
}

// ── room persistence (#167) ──────────────────────────────────

#[test]
fn rooms_round_trip_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r1 = room("r1");
    r1.branch = Some("skein/r1".into());
    r1.archived = Some(1_000);
    db.save_all(&[r1, room("r2")]).unwrap();
    let outcome = db.load_all().unwrap();
    assert!(outcome.skipped.is_empty());
    assert_eq!(outcome.rooms.len(), 2);
    // created_at preserves insertion order across the round-trip.
    assert_eq!(outcome.rooms[0].id, "r1");
    assert_eq!(outcome.rooms[0].branch.as_deref(), Some("skein/r1"));
    assert_eq!(outcome.rooms[0].archived, Some(1_000));
    assert_eq!(outcome.rooms[1].id, "r2");
}

/// #247: the selected agent survives a restart.
///
/// The interesting half is the *struct*, not the column. Rooms are
/// stored as one JSON blob, and this type is the shape they are
/// re-serialised through on the next autosave — so a field the
/// frontend writes and this struct does not name is dropped
/// silently, on a save the user never asked for, some minutes after
/// they picked the agent.
#[test]
fn a_harness_agent_survives_the_round_trip() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    let mut with_agent = harness("h1");
    with_agent.agent = Some("pr-review-toolkit:code-reviewer".into());
    // Absent is a choice — "the tool's own default" — and has to
    // come back absent rather than as an empty string.
    r.harnesses = vec![with_agent, harness("h2")];
    r.active_harness_id = "h1".into();
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    let hs = &outcome.rooms[0].harnesses;
    assert_eq!(
        hs[0].agent.as_deref(),
        Some("pr-review-toolkit:code-reviewer")
    );
    assert_eq!(hs[1].agent, None);
}

/// A blob written before #247 has no `agent` key at all. The field
/// policy says that must load, not quarantine the room.
#[test]
fn a_pre_247_blob_loads_without_an_agent_field() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[{"id":"h1","kind":"claude","name":"h1","status":"running",
        "model":"","tokens":"0"}],"activeHarnessId":"h1"}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.harnesses[0].agent, None);
}

/// #76: a blob written before `repoRoot` existed has no such key at
/// all. The field policy says that must load, not quarantine the
/// room, with the group key simply absent.
#[test]
fn a_pre_76_blob_loads_without_a_repo_root_field() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":""}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.repo_root, None);
}

/// #411: a blob written before `closedBy`/`createdBy` (on a harness)
/// existed has neither key at all. The field policy says that must
/// load, not quarantine the room.
#[test]
fn a_pre_411_blob_loads_without_closed_by_or_harness_created_by_fields() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[{"id":"h1","kind":"claude","name":"h1","status":"running",
        "model":"","tokens":"0"}],"activeHarnessId":"h1"}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.closed_by, None);
    assert_eq!(room.harnesses[0].created_by, None);
}

/// #411: both new attribution fields round-trip like every other
/// optional field.
#[test]
fn closed_by_and_harness_created_by_round_trip_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    r.closed_by = Some(ClosedBy {
        room_id: "director".into(),
        harness_id: Some("d1".into()),
        at: 1_234,
    });
    let mut h = harness("h1");
    h.created_by = Some(HarnessCreatedBy {
        room_id: "director".into(),
        harness_id: Some("d1".into()),
    });
    r.harnesses = vec![h];
    r.active_harness_id = "h1".into();
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    let loaded = &outcome.rooms[0];
    assert_eq!(
        loaded.closed_by,
        Some(ClosedBy {
            room_id: "director".into(),
            harness_id: Some("d1".into()),
            at: 1_234,
        })
    );
    assert_eq!(
        loaded.harnesses[0].created_by,
        Some(HarnessCreatedBy {
            room_id: "director".into(),
            harness_id: Some("d1".into()),
        })
    );
}

/// #520: `shellClaim` round-trips with and without a port, and an old
/// blob without it still parses.
#[test]
fn shell_claim_round_trips_with_and_without_a_port() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    let mut a = harness("h1");
    a.shell_claim = Some(ShellClaim {
        session_id: "s1".into(),
        port: Some(4096),
    });
    let mut b = harness("h2");
    b.shell_claim = Some(ShellClaim {
        session_id: "s2".into(),
        port: None,
    });
    r.harnesses = vec![a, b];
    r.active_harness_id = "h1".into();
    db.save_all(&[r]).unwrap();
    let loaded = db.load_all().unwrap().rooms.remove(0);
    assert_eq!(
        loaded.harnesses[0].shell_claim,
        Some(ShellClaim {
            session_id: "s1".into(),
            port: Some(4096),
        })
    );
    assert_eq!(
        loaded.harnesses[1].shell_claim,
        Some(ShellClaim {
            session_id: "s2".into(),
            port: None,
        })
    );
    let json = serde_json::to_string(&loaded.harnesses[1]).unwrap();
    assert!(
        json.contains(r#""shellClaim":{"sessionId":"s2"}"#),
        "{json}"
    );
}

#[test]
fn a_pre_520_blob_loads_without_a_shell_claim() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[{"id":"h1","kind":"claude","name":"h1","status":"running",
        "model":"","tokens":"0"}],"activeHarnessId":"h1"}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.harnesses[0].shell_claim, None);
}

/// #417: `retired` round-trips through save and load.
#[test]
fn retired_round_trips_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    r.archived = Some(1_000);
    r.retired = Some(2_000);
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(outcome.rooms[0].retired, Some(2_000));
}

/// #417: a blob written before `retired` existed loads with `None`.
#[test]
fn a_pre_417_blob_loads_without_a_retired_field() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":"","archived":1000}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.retired, None);
}

/// #418: `repoIdentity` round-trips through save and load.
#[test]
fn repo_identity_round_trips_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    let ident = RepoIdentity {
        root_commits: vec!["abc".into()],
        origin_url: Some("https://example.com/x.git".into()),
    };
    r.repo_identity = Some(ident.clone());
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(outcome.rooms[0].repo_identity, Some(ident));
}

/// #418: a blob written before `repoIdentity` existed loads with
/// `None`, and a partial identity tolerates missing keys.
#[test]
fn a_pre_418_blob_loads_without_a_repo_identity() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":""}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.repo_identity, None);
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":"","repoIdentity":{}}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(
        room.repo_identity,
        Some(RepoIdentity {
            root_commits: Vec::new(),
            origin_url: None
        })
    );
}

/// #76: `repoRoot` round-trips through save and load like the other
/// optional room fields.
#[test]
fn repo_root_round_trips_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    r.repo_root = Some("/home/stefan/code/skein".into());
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(
        outcome.rooms[0].repo_root.as_deref(),
        Some("/home/stefan/code/skein")
    );
}

/// A blob written before #328 has no `attention` key at all. The
/// field policy says that must load, not quarantine the room.
#[test]
fn a_pre_328_blob_loads_without_an_attention_field() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":""}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    assert_eq!(room.attention, None);
}

/// #328: `attention` round-trips through save and load like the
/// other optional room fields.
#[test]
fn attention_round_trips_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    r.attention = Some(true);
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(outcome.rooms[0].attention, Some(true));
}

/// A blob written before #356 has a `createdBy` object with only
/// `roomId`/`harnessId` — no `promptFirstLine` or `baseSha` keys at
/// all. The field policy says that must load, not quarantine the
/// room, with the two new fields simply absent.
#[test]
fn a_pre_356_created_by_blob_loads_without_the_new_fields() {
    let json = r#"{"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":"",
        "createdBy":{"roomId":"r0","harnessId":"h0"}}"#;
    let room: Room = serde_json::from_str(json).unwrap();
    let created_by = room.created_by.expect("createdBy must still parse");
    assert_eq!(created_by.room_id, "r0");
    assert_eq!(created_by.harness_id, "h0");
    assert_eq!(created_by.prompt_first_line, None);
    assert_eq!(created_by.base_sha, None);
}

/// #356: `createdBy.promptFirstLine`/`baseSha` round-trip through
/// save and load like every other optional field.
#[test]
fn created_by_prompt_and_base_sha_round_trip_through_save_and_load() {
    let (_dir, db) = fresh_db();
    let mut r = room("r1");
    r.created_by = Some(CreatedBy {
        room_id: "r0".into(),
        harness_id: "h0".into(),
        prompt_first_line: Some("fix the flaky test".into()),
        base_sha: Some("deadbeef".into()),
    });
    db.save_all(&[r]).unwrap();
    let outcome = db.load_all().unwrap();
    let created_by = outcome.rooms[0].created_by.as_ref().unwrap();
    assert_eq!(
        created_by.prompt_first_line.as_deref(),
        Some("fix the flaky test")
    );
    assert_eq!(created_by.base_sha.as_deref(), Some("deadbeef"));
}

#[test]
fn load_all_quarantines_unparseable_rows_and_keeps_good_ones() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("good"), room("bad")]).unwrap();
    db.conn
        .lock()
        .execute("UPDATE sessions SET data = 'not json' WHERE id = 'bad'", [])
        .unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(outcome.rooms.len(), 1);
    assert_eq!(outcome.rooms[0].id, "good");
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(outcome.skipped[0].id, "bad");
    assert_ne!(outcome.skipped[0].error, "");
    // The blob is preserved in quarantine and gone from the live
    // table, so the next save_all wipe can't destroy it.
    let conn = db.conn.lock();
    let blob: String = conn
        .query_row(
            "SELECT data FROM sessions_quarantine WHERE id = 'bad'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(blob, "not json");
    let live: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(live, 1);
}

#[test]
fn unreadable_room_count_counts_unparsed_and_quarantined_rows() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("good"), room("quarantined")]).unwrap();
    assert_eq!(db.unreadable_room_count().unwrap(), 0);

    // Corrupt one row and run the same load_all pass that moves it
    // into sessions_quarantine (#167) — the row is no longer
    // unparseable-in-place, but it is still a room Skein can't
    // produce.
    db.conn
        .lock()
        .execute(
            "UPDATE sessions SET data = 'not json' WHERE id = 'quarantined'",
            [],
        )
        .unwrap();
    db.load_all().unwrap();
    assert_eq!(db.unreadable_room_count().unwrap(), 1);

    // A second row that is corrupt but has NOT gone through a
    // load_all pass yet must count too — all_rooms() itself never
    // quarantines, so a bad row can sit in `sessions` indefinitely.
    db.conn
        .lock()
        .execute(
            "INSERT INTO sessions (id, data, created_at) VALUES ('still_bad', \
             'also not json', 0)",
            [],
        )
        .unwrap();
    assert_eq!(db.unreadable_room_count().unwrap(), 2);
}

#[test]
fn save_all_empty_before_load_is_refused_when_rooms_exist() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    {
        let db = Database::open(&path).unwrap();
        db.save_all(&[room("r1")]).unwrap();
    }
    // Fresh open = fresh process: loaded_ok is false. This call is
    // the exact #167 boot-wipe chain and must be refused.
    let db = Database::open(&path).unwrap();
    let err = db.save_all(&[]).unwrap_err();
    assert!(err.contains("#167"), "unexpected error: {err}");
    assert_eq!(db.load_all().unwrap().rooms.len(), 1);
}

#[test]
fn save_all_empty_after_successful_load_is_allowed() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    let _ = db.load_all().unwrap();
    // "User deleted the last room" — legitimate empty save.
    db.save_all(&[]).unwrap();
    assert!(db.load_all().unwrap().rooms.is_empty());
}

// ── save_all_seq ordering (#171) ─────────────────────────────

#[test]
fn save_all_seq_applies_in_order_tickets() {
    let (_dir, db) = fresh_db();
    let _ = db.load_all().unwrap();
    db.save_all_seq(&[room("r1")], db.next_save_seq()).unwrap();
    db.save_all_seq(&[room("r1"), room("r2")], db.next_save_seq())
        .unwrap();
    assert_eq!(db.load_all().unwrap().rooms.len(), 2);
}

#[test]
fn save_all_seq_drops_a_save_whose_ticket_already_lost() {
    let (_dir, db) = fresh_db();
    let _ = db.load_all().unwrap();
    // Mint tickets in order, but apply them out of order — the
    // shape of an async db_save_rooms(N) that reaches the
    // connection lock after db_save_rooms(N+1) already committed.
    let older = db.next_save_seq();
    let newer = db.next_save_seq();
    db.save_all_seq(&[room("r1"), room("r2")], newer).unwrap();
    db.save_all_seq(&[room("r1")], older).unwrap();
    // The stale save must not have reverted the newer room list.
    assert_eq!(db.load_all().unwrap().rooms.len(), 2);
}

#[test]
fn save_all_seq_still_refuses_the_167_empty_save() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    {
        let db = Database::open(&path).unwrap();
        db.save_all(&[room("r1")]).unwrap();
    }
    // Fresh process: loaded_ok is false, same #167 boot-wipe guard
    // save_all_seq must not bypass.
    let db = Database::open(&path).unwrap();
    let err = db.save_all_seq(&[], db.next_save_seq()).unwrap_err();
    assert!(err.contains("#167"), "unexpected error: {err}");
    assert_eq!(db.load_all().unwrap().rooms.len(), 1);
}

#[test]
fn first_load_is_flagged_only_once_per_process() {
    let (_dir, db) = fresh_db();
    assert!(db.load_all().unwrap().first_load);
    assert!(!db.load_all().unwrap().first_load);
}
