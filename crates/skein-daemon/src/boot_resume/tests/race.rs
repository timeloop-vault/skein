//! #755: boot preparation plans from the snapshot the load returned and writes
//! field by field under the store lock. The injected probe (and `busy`) run
//! between the snapshot and the write, so they are where these tests make a
//! client, the runner or a spawn get in first. Every fixture holds a harness
//! "p" with a live #517 claim, which is what makes the probe run, and an
//! opencode "o" with a legacy cmd, so a plan exists for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::{Harness, LaunchMode, Room, ShellClaim};

use super::{find, harness, hooked_room, legacy_opencode_cmd, room, sv};
use crate::boot_resume::{BOOT_ORIGIN, BootResume};
use crate::db::test_support::fresh_db;
use crate::event_sink::EventSink;
use crate::room_store::RoomStore;

#[derive(Default)]
struct Rec(Mutex<Vec<(String, String)>>);

impl EventSink for Rec {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.0.lock().unwrap().push((event.to_owned(), payload));
        Ok(())
    }
}

impl Rec {
    fn origins(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, p)| {
                let v: Value = serde_json::from_str(p).ok()?;
                Some(v["origin"].as_str()?.to_owned())
            })
            .collect()
    }
}

fn setup() -> (
    tempfile::TempDir,
    Arc<crate::db::Database>,
    Arc<RoomStore>,
    Arc<Rec>,
) {
    let (dir, db) = fresh_db();
    let db = Arc::new(db);
    let sink = Arc::new(Rec::default());
    let store = RoomStore::new(Arc::clone(&db), Arc::clone(&sink) as Arc<dyn EventSink>);
    (dir, db, store, sink)
}

/// A probe that runs `f` (the interleaved write) and says the claim is live.
fn hook(f: impl FnMut()) -> impl Fn(&Harness) -> Result<bool, String> {
    let f = Mutex::new(f);
    move |_| {
        (f.lock().unwrap())();
        Ok(true)
    }
}

/// A whole-harness client put of the stored record after `f`.
fn client_put(store: &RoomStore, room_id: &str, id: &str, f: impl FnOnce(&mut Harness)) {
    let mut h = find(&store.rooms(), id).clone();
    f(&mut h);
    let res = store
        .apply(
            "client",
            vec![RoomMutation::PutHarness {
                room_id: room_id.to_owned(),
                harness: h,
            }],
        )
        .unwrap();
    assert!(res.skipped.is_empty(), "{:?}", res.skipped);
}

/// The preparation counted as done: a later load would not prepare again.
fn assert_done(boot: &BootResume) {
    assert!(*boot.done.lock().unwrap(), "the preparation was not done");
}

// Failed before #755: the whole-harness PutHarness wrote the snapshot's name
// and agent back over the client's.
#[test]
fn a_client_put_between_snapshot_and_apply_survives() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&hooked_room(), &[], None).unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| {
        client_put(&store, "r1", "o", |h| {
            h.name = "renamed".into();
            h.agent = Some("build".into());
        });
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, store.rooms(), db.load_all().unwrap().rooms] {
        let o = find(&rooms, "o");
        assert_eq!(o.name, "renamed");
        assert_eq!(o.agent.as_deref(), Some("build"));
        // The client's put carried the legacy cmd, so the plan still matched.
        assert_eq!(o.cmd, Some(sv(&["opencode"])));
    }
}

// Failed before #755: the put carried the snapshot's (absent) session id and
// the whole record, wiping what the runner had just minted.
#[test]
fn a_runner_session_write_makes_the_plan_stale_but_still_counts_as_done() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&hooked_room(), &[], None).unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| {
        let found = store
            .edit_harness("runner", "r1", "o", |h| {
                h.session_id = Some("minted".into());
            })
            .unwrap();
        assert!(found);
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    let o = find(&out.rooms, "o");
    assert_eq!(o.session_id.as_deref(), Some("minted"));
    assert_eq!(
        o.cmd,
        Some(legacy_opencode_cmd()),
        "the stale plan is skipped"
    );
    assert_eq!(
        find(&store.rooms(), "o").cmd,
        Some(legacy_opencode_cmd()),
        "the stale plan is skipped"
    );
    assert_done(&boot);
}

// Failed before #755: the planned cmd overwrote the client's.
#[test]
fn a_client_cmd_change_makes_the_plan_stale() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&hooked_room(), &[], None).unwrap();
    let boot = BootResume::detached();
    let mine = sv(&["opencode", "--port", "9"]);
    let probe = hook(|| client_put(&store, "r1", "o", |h| h.cmd = Some(mine.clone())));
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    assert_eq!(find(&out.rooms, "o").cmd, Some(mine.clone()));
    assert_eq!(find(&db.load_all().unwrap().rooms, "o").cmd, Some(mine));
    assert_done(&boot);
}

/// "l": a pre-#752 shell record with a live #517 claim, so a full apply puts
/// it back on `claude` while a busy one may only take the launch migration.
fn legacy_claimed() -> Harness {
    let mut l = harness("l", "claude", Some("sid"), &["pwsh.exe"], &json!({}));
    l.shell_claim = Some(ShellClaim {
        session_id: "sid".into(),
        port: None,
    });
    l
}

/// "o" (legacy cmd) is planned before "l", whose probe is the hook.
fn busy_fixture() -> Vec<Room> {
    let mut o = harness("o", "opencode", Some("o-sid"), &["opencode"], &json!({}));
    o.cmd = Some(legacy_opencode_cmd());
    vec![room("r1", false, vec![o, legacy_claimed()])]
}

// Failed before #755: a harness that became busy after the plan was made
// still had its cmd and claim rewritten.
#[test]
fn a_harness_busy_at_write_time_takes_only_the_launch_migration() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&busy_fixture(), &[], None).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let mut boot = BootResume::detached();
    let f = Arc::clone(&flag);
    boot.busy = Arc::new(move |_| f.load(Ordering::SeqCst));
    // Both look idle at plan time and busy when the write happens.
    let probe = hook(|| flag.store(true, Ordering::SeqCst));
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    let o = find(&out.rooms, "o");
    assert_eq!(o.cmd, Some(legacy_opencode_cmd()));
    let l = find(&out.rooms, "l");
    assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])));
    assert!(l.shell_claim.is_some(), "the claim is left as stored");
    assert_eq!(
        l.launch,
        Some(LaunchMode::Shell {
            argv: sv(&["pwsh.exe"])
        }),
        "the launch migration is still written"
    );
    assert_eq!(
        serde_json::to_value(&out.rooms).unwrap(),
        serde_json::to_value(db.load_all().unwrap().rooms).unwrap()
    );
    assert_done(&boot);
}

#[test]
fn a_harness_busy_at_plan_time_but_idle_at_write_time_gets_nothing_extra() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&busy_fixture(), &[], None).unwrap();
    let mut boot = BootResume::detached();
    // "l" is busy for the plan only: the first answer is true, then idle.
    let first = AtomicBool::new(true);
    boot.busy = Arc::new(move |id| id == "l" && first.swap(false, Ordering::SeqCst));
    let out = boot
        .load_rooms_with(&db, &store, |_| panic!("a busy harness is not probed"))
        .unwrap();
    let l = find(&out.rooms, "l");
    // Planned while busy: only the migrated launch was planned, so that is
    // all that is written even though the harness is idle by then.
    assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])));
    assert!(l.shell_claim.is_some());
    assert_eq!(
        l.launch,
        Some(LaunchMode::Shell {
            argv: sv(&["pwsh.exe"])
        })
    );
    assert_eq!(find(&out.rooms, "o").cmd, Some(sv(&["opencode"])));
}

// Failed before #755: one harness's put failing or being stale was not
// isolated from the others (and the whole record was written).
#[test]
fn a_skipped_harness_does_not_block_the_others_and_one_boot_event_is_emitted() {
    let (_dir, db, store, sink) = setup();
    let legacy = |id: &str| {
        let mut h = harness(id, "opencode", Some("sid"), &["opencode"], &json!({}));
        h.cmd = Some(legacy_opencode_cmd());
        h
    };
    let mut seed = hooked_room();
    seed.push(room("r2", false, vec![legacy("o2")]));
    db.write_room_rows(&seed, &[], None).unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| client_put(&store, "r1", "o", |h| h.cmd = Some(sv(&["opencode", "-x"]))));
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    assert_eq!(
        find(&out.rooms, "o").cmd,
        Some(sv(&["opencode", "-x"])),
        "stale: the client's cmd stays"
    );
    assert_eq!(find(&out.rooms, "o2").cmd, Some(sv(&["opencode"])));
    let boot_events = sink
        .origins()
        .into_iter()
        .filter(|o| o == BOOT_ORIGIN)
        .count();
    assert_eq!(boot_events, 1, "one rooms-changed for the whole load");
}

// Failed before #755: the whole-harness put overwrote the client's tokens
// (and model) with the snapshot's.
#[test]
fn boot_never_writes_fields_outside_the_plan() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&hooked_room(), &[], None).unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| {
        client_put(&store, "r1", "o", |h| {
            h.tokens = "999".into();
            h.model = "fast".into();
            h.status = "running".into();
        });
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, db.load_all().unwrap().rooms] {
        let o = find(&rooms, "o");
        assert_eq!(o.tokens, "999");
        assert_eq!(o.model, "fast");
        assert_eq!(o.status, "running");
        assert_eq!(o.cmd, Some(sv(&["opencode"])), "the plan still applies");
    }
}

/// "l": a pre-#752 shell-mode record (no `launch`, cmd not the kind's program)
/// with a live claim, next to an opencode harness.
fn legacy_shell_fixture() -> Vec<Room> {
    busy_fixture()
}

fn shell_launch() -> LaunchMode {
    LaunchMode::Shell {
        argv: sv(&["pwsh.exe"]),
    }
}

#[test]
fn a_stale_legacy_shell_record_still_gets_its_launch_migrated() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&legacy_shell_fixture(), &[], None)
        .unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| {
        assert!(
            store
                .edit_harness("runner", "r1", "l", |h| h.session_id = Some("other".into()))
                .unwrap()
        );
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, store.rooms(), db.load_all().unwrap().rooms] {
        let l = find(&rooms, "l");
        assert_eq!(l.session_id.as_deref(), Some("other"));
        assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])), "stale: cmd untouched");
        assert!(l.shell_claim.is_some(), "stale: claim untouched");
        assert_eq!(
            l.launch,
            Some(shell_launch()),
            "the migration is independent"
        );
    }
    assert_done(&boot);
}

#[test]
fn a_record_both_stale_and_busy_takes_only_the_launch_migration() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&legacy_shell_fixture(), &[], None)
        .unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let mut boot = BootResume::detached();
    let f = Arc::clone(&flag);
    boot.busy = Arc::new(move |id| id == "l" && f.load(Ordering::SeqCst));
    // The plan sees "l" idle (the probe is only called for an idle one); by
    // the write it is busy and stale.
    let probe = hook(|| {
        flag.store(true, Ordering::SeqCst);
        store
            .edit_harness("runner", "r1", "l", |h| h.session_id = Some("other".into()))
            .unwrap();
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, db.load_all().unwrap().rooms] {
        let l = find(&rooms, "l");
        assert_eq!(l.session_id.as_deref(), Some("other"));
        assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])));
        assert!(l.shell_claim.is_some());
        assert_eq!(l.launch, Some(shell_launch()));
    }
    assert_done(&boot);
}

#[test]
fn a_client_restart_to_the_tool_is_not_migrated_to_a_shell() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&legacy_shell_fixture(), &[], None)
        .unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| {
        client_put(&store, "r1", "l", |h| {
            h.cmd = Some(sv(&["claude"]));
            h.shell_claim = None;
        });
    });
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, store.rooms(), db.load_all().unwrap().rooms] {
        let l = find(&rooms, "l");
        assert_eq!(l.launch, None, "boot wrote nothing to the restarted record");
        assert_eq!(l.cmd, Some(sv(&["claude"])), "the client's cmd stays");
        assert!(l.shell_claim.is_none());
    }
    assert_done(&boot);
}

#[test]
fn a_client_claim_removal_alone_blocks_the_migration() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&legacy_shell_fixture(), &[], None)
        .unwrap();
    let boot = BootResume::detached();
    let probe = hook(|| client_put(&store, "r1", "l", |h| h.shell_claim = None));
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, store.rooms(), db.load_all().unwrap().rooms] {
        let l = find(&rooms, "l");
        assert_eq!(l.launch, None, "the claim was an input of the migration");
        assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])));
        assert!(l.shell_claim.is_none());
    }
    assert_done(&boot);
}

#[test]
fn a_client_launch_change_is_not_migrated_over() {
    let (_dir, db, store, _sink) = setup();
    db.write_room_rows(&legacy_shell_fixture(), &[], None)
        .unwrap();
    let boot = BootResume::detached();
    let mine = Some(LaunchMode::Shell { argv: sv(&["zsh"]) });
    let probe = hook(|| client_put(&store, "r1", "l", |h| h.launch = mine.clone()));
    let out = boot.load_rooms_with(&db, &store, probe).unwrap();
    for rooms in [out.rooms, store.rooms(), db.load_all().unwrap().rooms] {
        let l = find(&rooms, "l");
        assert_eq!(l.launch, mine, "the client's launch wins");
        assert_eq!(l.cmd, Some(sv(&["pwsh.exe"])), "stale: cmd untouched");
        assert!(l.shell_claim.is_some());
    }
    assert_done(&boot);
}
