//! Boot resume: the pure core with injected probe/picker/minter, then the
//! load wiring (once per process, persisted, returned).

use std::sync::Arc;

use serde_json::json;
use skein_proto::rooms::{Harness, Room};

use super::{BootResume, opencode_port_from_cmd, prepare_with, session_survives};
use crate::db::test_support::fresh_db;
use crate::event_sink::EventSink;
use crate::room_store::RoomStore;

struct Quiet;

impl EventSink for Quiet {
    fn emit_json(&self, _: &str, _: String) -> Result<(), String> {
        Ok(())
    }
}

fn harness(
    id: &str,
    kind: &str,
    sid: Option<&str>,
    cmd: &[&str],
    extra: &serde_json::Value,
) -> Harness {
    let mut v = json!({
        "id": id, "kind": kind, "name": id, "status": "idle", "model": "", "tokens": "0",
        "cmd": cmd,
    });
    if let Some(s) = sid {
        v["sessionId"] = json!(s);
    }
    if let Some(o) = extra.as_object() {
        for (k, val) in o {
            v[k] = val.clone();
        }
    }
    serde_json::from_value(v).unwrap()
}

fn room(id: &str, archived: bool, harnesses: Vec<Harness>) -> Room {
    let mut v = json!({
        "id": id, "name": id, "task": "", "status": "idle", "badge": 0,
        "harnesses": [], "activeHarnessId": ""
    });
    if archived {
        v["archived"] = json!(1);
    }
    let mut r: Room = serde_json::from_value(v).unwrap();
    r.harnesses = harnesses;
    r
}

fn find<'a>(rooms: &'a [Room], id: &str) -> &'a Harness {
    rooms
        .iter()
        .flat_map(|r| &r.harnesses)
        .find(|h| h.id == id)
        .unwrap()
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn sample() -> Vec<Room> {
    vec![
        room(
            "r1",
            false,
            vec![
                harness(
                    "c",
                    "claude",
                    Some("c-sid"),
                    &["claude", "--session-id", "c-sid"],
                    &json!({}),
                ),
                harness("o", "opencode", Some("o-sid"), &["opencode"], &json!({})),
                harness(
                    "h",
                    "hermes",
                    Some("h-sid"),
                    &["hermes", "--tui"],
                    &json!({}),
                ),
                harness("s", "shell", Some("keep"), &["bash"], &json!({})),
            ],
        ),
        room(
            "r2",
            true,
            vec![harness(
                "c2",
                "claude",
                Some("gone"),
                &["claude", "--session-id", "gone"],
                &json!({}),
            )],
        ),
    ]
}

#[test]
fn stale_ids_are_dropped_for_each_probed_kind_and_the_cmd_follows_the_record() {
    let out = prepare_with(
        sample(),
        |h| Ok(h.id != "c" && h.id != "o" && h.id != "h" && h.id != "c2"),
        || Ok(4321),
        || "minted".to_owned(),
    );
    let c = find(&out.rooms, "c");
    assert_eq!(c.session_id.as_deref(), Some("minted"));
    assert_eq!(
        c.cmd.as_ref().unwrap(),
        &sv(&["claude", "--session-id", "minted"])
    );
    let o = find(&out.rooms, "o");
    assert_eq!(o.session_id, None);
    assert_eq!(opencode_port_from_cmd(o.cmd.as_ref().unwrap()), Some(4321));
    assert!(o.cmd.as_ref().unwrap().contains(&"--continue".to_owned()));
    let h = find(&out.rooms, "h");
    assert_eq!(h.session_id, None);
    assert_eq!(h.cmd.as_ref().unwrap(), &sv(&["hermes", "--tui"]));
    // Unprobed kind untouched; archived rooms are included.
    assert_eq!(find(&out.rooms, "s").session_id.as_deref(), Some("keep"));
    assert_eq!(find(&out.rooms, "c2").session_id.as_deref(), Some("minted"));
    assert_eq!(out.ports.get("o"), Some(&4321));
}

#[test]
fn live_ids_resume_and_probe_errors_keep_the_id() {
    let out = prepare_with(
        sample(),
        |h| {
            if h.id == "c" {
                Ok(true)
            } else {
                Err("boom".to_owned())
            }
        },
        || Ok(4000),
        || "minted".to_owned(),
    );
    assert_eq!(
        find(&out.rooms, "c").cmd.as_ref().unwrap(),
        &sv(&["claude", "--resume", "c-sid"])
    );
    let o = find(&out.rooms, "o");
    assert_eq!(o.session_id.as_deref(), Some("o-sid"));
    assert_eq!(
        o.cmd.as_ref().unwrap(),
        &sv(&[
            "opencode",
            "--port",
            "4000",
            "--hostname",
            "127.0.0.1",
            "--session",
            "o-sid"
        ])
    );
    assert_eq!(find(&out.rooms, "c2").session_id.as_deref(), Some("gone"));
}

#[test]
fn opencode_v2_skips_the_probe() {
    let h = harness(
        "o",
        "opencode",
        Some("v2-sid"),
        &["opencode"],
        &json!({"opencodeVersion": {"major": 2, "version": "2.0.1"}}),
    );
    assert!(session_survives(&h, &|_| panic!("v2 must not be probed")));
    let blank = Harness {
        session_id: Some("  ".into()),
        ..h
    };
    assert!(!session_survives(&blank, &|_| panic!(
        "v2 must not be probed"
    )));
}

#[test]
fn port_is_recovered_from_a_resume_cmd() {
    assert_eq!(
        opencode_port_from_cmd(&sv(&["opencode", "--port", "99"])),
        Some(99)
    );
    assert_eq!(
        opencode_port_from_cmd(&sv(&["opencode", "--continue"])),
        None
    );
    assert_eq!(opencode_port_from_cmd(&sv(&["opencode", "--port"])), None);
}

#[test]
fn a_failed_flush_still_returns_the_rooms_the_store_holds() {
    let (_dir, db) = fresh_db();
    let db = Arc::new(db);
    let store = RoomStore::new(Arc::clone(&db), Arc::new(Quiet));
    db.write_room_rows(&sample(), &[], None).unwrap();
    let boot = BootResume::detached();
    // Every session is live, so only the cmds change (a debounced write);
    // the probe breaks the table after the rooms were read.
    let probe = |_: &Harness| {
        crate::db::test_support::break_room_writes(&db);
        Ok(true)
    };

    let out = boot
        .load_rooms_with(&db, &store, probe, || Ok(5000), || "m".to_owned())
        .unwrap();
    assert_eq!(
        find(&out.rooms, "c").cmd.as_ref().unwrap(),
        &sv(&["claude", "--resume", "c-sid"])
    );
    assert_eq!(
        serde_json::to_value(&out.rooms).unwrap(),
        serde_json::to_value(store.rooms()).unwrap(),
        "the client baseline must be what the store holds"
    );
}

#[test]
fn first_load_prepares_stores_and_returns_later_loads_do_not() {
    let (_dir, db) = fresh_db();
    let db = Arc::new(db);
    let store = RoomStore::new(Arc::clone(&db), Arc::new(Quiet));
    db.write_room_rows(&sample(), &[], None).unwrap();
    let boot = BootResume::detached();
    let probe = |h: &Harness| Ok(h.id != "c2");

    let first = boot
        .load_rooms_with(&db, &store, probe, || Ok(5000), || "m".to_owned())
        .unwrap();
    assert_eq!(
        find(&first.rooms, "c").cmd.as_ref().unwrap(),
        &sv(&["claude", "--resume", "c-sid"])
    );
    assert_eq!(find(&first.rooms, "c2").session_id.as_deref(), Some("m"));
    assert_eq!(store.rooms().len(), 2);
    // Persisted through the per-row path.
    let reread = db.load_all().unwrap();
    assert_eq!(
        find(&reread.rooms, "o").cmd.as_ref().unwrap(),
        find(&first.rooms, "o").cmd.as_ref().unwrap()
    );

    // A second load must not re-prepare (no new port, same cmds).
    let second = boot
        .load_rooms_with(&db, &store, probe, || Ok(6000), || "m2".to_owned())
        .unwrap();
    assert_eq!(
        serde_json::to_value(&second.rooms).unwrap(),
        serde_json::to_value(&first.rooms).unwrap()
    );
}
