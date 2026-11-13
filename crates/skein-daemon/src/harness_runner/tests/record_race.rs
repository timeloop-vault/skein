//! The spawn's write-back is field-level (#755): a client edit made while a
//! spawn is in flight, or after it, is not lost, and neither is the id the
//! spawn minted. Before #755 the write-back put the whole record it had read
//! before the spawn, which erased any client edit made in between.
//!
//! The rooms-changed event's origin is not asserted: the fixture's event sink
//! discards events.

use skein_proto::rooms::OpencodeVersion;

use super::super::record::{self, SESSION_REPORT_ORIGIN, SessionReport};
use super::*;
use crate::harness_kind::HarnessKind;
use crate::pty::command::PtySpawnResult;
use crate::spawn_env::lookups::OpencodeVersionDto;

fn claude_fx(session: Option<&str>) -> Fx {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness(
            "h1",
            "claude",
            Some(vec!["claude".to_owned()]),
            session,
        )],
    ));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner().set_session_probe(Arc::new(|_| Ok(true)));
    fx
}

fn stored(fx: &Fx) -> Harness {
    fx.daemon.rooms.harness_record("r1", "h1").unwrap()
}

/// Run `hook` while the spawn of `h1` is in flight (gate held, no entry yet).
fn mid_spawn(fx: &Fx, hook: impl Fn() + Send + Sync + 'static) {
    fx.runner()
        .pause_points
        .lock()
        .insert("before_spawn", Arc::new(hook));
}

fn put(store: &crate::room_store::RoomStore, h: Harness) {
    let out = store
        .apply(
            "client",
            vec![RoomMutation::PutHarness {
                room_id: "r1".to_owned(),
                harness: h,
            }],
        )
        .unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
}

#[test]
fn a_client_edit_during_a_spawn_survives_the_write_back() {
    let fx = claude_fx(None);
    let store = Arc::clone(&fx.daemon.rooms);
    mid_spawn(&fx, move || {
        let mut h = store.harness_record("r1", "h1").unwrap();
        h.name = "renamed".to_owned();
        h.agent = Some("reviewer".to_owned());
        put(&store, h);
    });
    fx.ensure(false, None).unwrap();
    let h = stored(&fx);
    // Failed before #755: the whole-record write-back restored the old name.
    assert_eq!(h.name, "renamed");
    assert_eq!(h.agent.as_deref(), Some("reviewer"));
    let argv = fx.runner().spawned_argv().pop().unwrap();
    assert_eq!(h.session_id.as_deref(), Some(argv[2].as_str()));
}

#[test]
fn a_stale_client_put_after_the_write_back_keeps_the_minted_id() {
    let fx = claude_fx(None);
    let stale = stored(&fx);
    assert_eq!(stale.session_id, None);
    fx.ensure(false, None).unwrap();
    let minted = stored(&fx).session_id.expect("an id was minted");

    let mut put_back = stale;
    put_back.name = "client name".to_owned();
    put(&fx.daemon.rooms, put_back);

    let h = stored(&fx);
    assert_eq!(h.session_id.as_deref(), Some(minted.as_str()));
    assert_eq!(h.name, "client name");
}

#[test]
fn a_session_recorded_during_the_spawn_wins_over_the_spawns_own() {
    let fx = claude_fx(None);
    let store = Arc::clone(&fx.daemon.rooms);
    mid_spawn(&fx, move || {
        let changed = record::report_session(
            &store,
            "r1",
            "h1",
            SessionReport::Replace {
                session_id: "newer".to_owned(),
            },
        )
        .unwrap();
        assert!(changed);
    });
    fx.ensure(false, None).unwrap();
    assert_eq!(stored(&fx).session_id.as_deref(), Some("newer"));
}

fn spawn_result(version: Option<OpencodeVersionDto>) -> PtySpawnResult {
    PtySpawnResult {
        id: "pty".to_owned(),
        injected: false,
        hermes_hooks: false,
        mouse_clicks_disabled: false,
        env_profile: None,
        sidecar_generation: None,
        opencode_version: version,
        session_id: None,
    }
}

#[test]
fn the_opencode_version_lands_and_a_mid_spawn_rename_survives() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "opencode", Some(shell_cmd()), Some("s"))],
    ));
    // What the spawn read, then a client rename before the write-back.
    let before = stored(&fx);
    let mut renamed = before.clone();
    renamed.name = "renamed".to_owned();
    put(&fx.daemon.rooms, renamed);

    record::write_back(
        &fx.daemon.rooms,
        "r1",
        "h1",
        HarnessKind::Opencode,
        &spawn_result(Some(OpencodeVersionDto {
            major: 2,
            version: "2.0.1".to_owned(),
        })),
        Some("s"),
        Some("s"),
        before.opencode_version.as_ref(),
    );
    let h = stored(&fx);
    assert_eq!(h.name, "renamed");
    assert_eq!(h.session_id.as_deref(), Some("s"));
    assert_eq!(
        h.opencode_version,
        Some(OpencodeVersion {
            major: 2,
            version: "2.0.1".to_owned()
        })
    );
}

fn report(fx: &Fx, r: SessionReport) -> Result<bool, String> {
    record::report_session(&fx.daemon.rooms, "r1", "h1", r)
}

fn id(s: &str) -> String {
    s.to_owned()
}

#[test]
fn a_capture_sets_an_empty_record_but_never_overwrites() {
    let fx = claude_fx(None);
    assert_eq!(
        report(
            &fx,
            SessionReport::Capture {
                session_id: id("a")
            }
        ),
        Ok(true)
    );
    assert_eq!(stored(&fx).session_id.as_deref(), Some("a"));
    assert_eq!(
        report(
            &fx,
            SessionReport::Capture {
                session_id: id("b")
            }
        ),
        Ok(false)
    );
    assert_eq!(stored(&fx).session_id.as_deref(), Some("a"));
}

#[test]
fn a_blank_id_is_ignored() {
    let fx = claude_fx(Some("keep"));
    assert_eq!(
        report(
            &fx,
            SessionReport::Replace {
                session_id: id("  ")
            }
        ),
        Ok(false)
    );
    assert_eq!(
        report(&fx, SessionReport::Capture { session_id: id("") }),
        Ok(false)
    );
    assert_eq!(stored(&fx).session_id.as_deref(), Some("keep"));
}

#[test]
fn a_replace_overwrites_and_a_clear_removes() {
    let fx = claude_fx(Some("old"));
    assert_eq!(
        report(
            &fx,
            SessionReport::Replace {
                session_id: id("new")
            }
        ),
        Ok(true)
    );
    assert_eq!(stored(&fx).session_id.as_deref(), Some("new"));
    assert_eq!(report(&fx, SessionReport::Clear), Ok(true));
    assert_eq!(stored(&fx).session_id, None);
}

#[test]
fn a_report_for_an_unknown_harness_changes_nothing() {
    let fx = claude_fx(None);
    let r = record::report_session(
        &fx.daemon.rooms,
        "r1",
        "nope",
        SessionReport::Replace {
            session_id: id("x"),
        },
    );
    assert_eq!(r, Ok(false));
    assert_eq!(SESSION_REPORT_ORIGIN, "daemon-session");
}

#[test]
fn is_busy_tracks_the_gate_and_the_entry() {
    let fx = claude_fx(None);
    assert!(!fx.runner().is_busy("h1"));

    let seen = Arc::new(Mutex::new(None));
    let (runner, slot) = (Arc::clone(fx.runner()), Arc::clone(&seen));
    mid_spawn(&fx, move || {
        *slot.lock() = Some((runner.is_busy("h1"), runner.pty_id("h1").is_some()));
    });
    fx.ensure(false, None).unwrap();
    assert_eq!(*seen.lock(), Some((true, false)), "gate held, no entry yet");
    assert!(fx.runner().is_busy("h1"), "the entry exists");

    assert!(fx.runner().stop_harness("h1"));
    assert!(!fx.runner().is_busy("h1"));
}

// The lock-order claim in harness_runner/mod.rs's module doc: boot's busy
// check runs under the store lock and the spawn's write-back takes the store
// lock after the gate, so `is_busy` must never wait on the gate. This is the
// real wiring: `edit_harnesses` + `is_busy` while a spawn holds the gate.
#[test]
fn boot_busy_check_under_the_store_lock_does_not_block_a_spawn() {
    let fx = claude_fx(None);
    fx.create(room(
        "r2",
        &fx.work,
        vec![harness(
            "h2",
            "claude",
            Some(vec!["claude".to_owned()]),
            None,
        )],
    ));
    let (store, runner) = (Arc::clone(&fx.daemon.rooms), Arc::clone(fx.runner()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let slot = Arc::clone(&seen);
    mid_spawn(&fx, move || {
        let mut out = Vec::new();
        store
            .edit_harnesses("test", |_, h| {
                out.push((h.id.clone(), runner.is_busy(&h.id)));
            })
            .unwrap();
        *slot.lock() = out;
    });
    fx.ensure(false, None).unwrap();
    let seen = seen.lock().clone();
    assert!(seen.contains(&("h1".to_owned(), true)), "{seen:?}");
    assert!(seen.contains(&("h2".to_owned(), false)), "{seen:?}");
    assert!(
        stored(&fx).session_id.is_some(),
        "the minted id was written"
    );
}

#[test]
fn write_back_keeps_a_newer_id_over_a_non_blank_stored_one() {
    let fx = claude_fx(Some("old"));
    assert_eq!(
        report(
            &fx,
            SessionReport::Replace {
                session_id: id("newer")
            }
        ),
        Ok(true)
    );
    record::write_back(
        &fx.daemon.rooms,
        "r1",
        "h1",
        HarnessKind::Claude,
        &spawn_result(None),
        Some("spawned"),
        Some("old"),
        None,
    );
    assert_eq!(stored(&fx).session_id.as_deref(), Some("newer"));
}

#[test]
fn a_mid_spawn_client_put_cannot_set_the_session_id_or_version() {
    let fx = claude_fx(None);
    let store = Arc::clone(&fx.daemon.rooms);
    mid_spawn(&fx, move || {
        let mut h = store.harness_record("r1", "h1").unwrap();
        h.session_id = Some("client-sid".to_owned());
        h.opencode_version = Some(OpencodeVersion {
            major: 9,
            version: "9.9.9".to_owned(),
        });
        put(&store, h);
    });
    fx.ensure(false, None).unwrap();
    let h = stored(&fx);
    let argv = fx.runner().spawned_argv().pop().unwrap();
    assert_eq!(h.session_id.as_deref(), Some(argv[2].as_str()));
    assert_ne!(h.session_id.as_deref(), Some("client-sid"));
    assert_eq!(h.opencode_version, None, "not the client's");
}
