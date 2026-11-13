//! The daemon attaches the Hermes work poller itself (#732): no client
//! channel, the phase machine is its only consumer.

use rusqlite::{Connection, params};

use super::*;
use crate::harness_kind::HarnessKind;
use crate::harness_runner::adapters::AttachSpec;
use crate::spawn_settings::EnvVar;

const SID: &str = "20261021_140039_19c310";

/// A Hermes home whose `state.db` has the tables the poller reads and no work.
fn hermes_home(fx: &Fx) -> (PathBuf, Connection) {
    let home = fx.home.join("hermes-home");
    std::fs::create_dir_all(&home).unwrap();
    let c = Connection::open(home.join("state.db")).unwrap();
    c.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, \
           parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, title TEXT);
         CREATE TABLE async_delegations (delegation_id TEXT PRIMARY KEY, \
           origin_session TEXT NOT NULL, parent_session_id TEXT, state TEXT NOT NULL, \
           dispatched_at REAL NOT NULL, owner_pid INTEGER, task_json TEXT);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, \
           role TEXT NOT NULL, content TEXT);",
    )
    .unwrap();
    fx.runner()
        .spawn_env
        .settings
        .write()
        .extra_env
        .push(EnvVar {
            key: "HERMES_HOME".to_owned(),
            value: home.to_string_lossy().into_owned(),
        });
    (home, c)
}

fn hermes_spec(session: Option<&'static str>) -> AttachSpec<'static> {
    AttachSpec {
        harness_id: "h1",
        room_id: "r1",
        kind: HarnessKind::Hermes,
        cwd: ".",
        session_id: session,
        opencode_port: None,
        sidecar_generation: None,
        program: Some("hermes"),
        hermes_profile: None,
        hermes_hooks: true,
        fresh_process: false,
    }
}

fn subagents(fx: &Fx) -> usize {
    fx.daemon.phase.work_snapshot(["h1"])["h1"].subagents
}

#[test]
fn a_resumed_hermes_gets_its_work_poller_with_no_client_and_it_feeds_the_machine() {
    // Declared after the fixture, so the runtime is dropped first.
    let fx = Fx::with_shell("byoh");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _entered = rt.enter();
    let (_home, db) = hermes_home(&fx);
    fx.ensure(false, None).unwrap();

    let work = &fx.runner().adapters.hermes_work;
    fx.runner().adapters.attach(&hermes_spec(Some(SID)));
    assert!(work.is_attached("h1"), "attached at spawn from the record");

    // The first pass is the attach snapshot, which never counts as live work.
    assert!(
        wait_until(|| work.passes("h1") >= 1),
        "the poller never finished its first pass"
    );
    db.execute(
        "INSERT INTO async_delegations VALUES ('d1', ?1, ?1, 'running', 1000.0, ?2, ?3)",
        params![SID, std::process::id(), r#"{"goal":"ping it"}"#],
    )
    .unwrap();
    db.execute(
        "INSERT INTO sessions VALUES ('child1', 'tui', ?1, 1000.1, NULL, 'Subagent: ping it')",
        params![SID],
    )
    .unwrap();
    assert!(
        wait_until(|| subagents(&fx) == 1),
        "the poller's event never reached the phase machine"
    );

    // Stopping the harness stops the poller.
    assert!(fx.runner().stop_harness("h1"));
    assert!(!work.is_attached("h1"));
}

#[test]
fn an_unbound_hermes_has_no_poller_until_a_session_start_names_one() {
    let fx = Fx::with_shell("byoh");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _entered = rt.enter();
    let (_home, _db) = hermes_home(&fx);
    fx.ensure(false, None).unwrap();

    let work = &fx.runner().adapters.hermes_work;
    fx.runner().adapters.attach(&hermes_spec(None));
    assert!(!work.is_attached("h1"), "no session id to read yet");

    work.session_started(&fx.daemon.rooms, "r1", "h1", SID);
    assert!(work.is_attached("h1"));
    work.detach("h1");
    assert!(!work.is_attached("h1"));
}

/// A late `on_session_start` hook must not start a poller nothing would stop.
#[test]
fn a_late_session_start_attaches_no_poller_for_a_gone_or_exited_harness() {
    let fx = Fx::new();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _entered = rt.enter();
    let (_home, _db) = hermes_home(&fx);
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "byoh", Some(exit_3_cmd()), None)],
    ));
    let work = &fx.runner().adapters.hermes_work;

    // Never spawned: no phase record.
    work.session_started(&fx.daemon.rooms, "r1", "h1", SID);
    assert!(!work.is_attached("h1"), "no record");

    // Exited by itself.
    fx.ensure(false, None).unwrap();
    assert!(wait_until(|| fx.daemon.phase.get("h1").is_some_and(|a| a
        .phase
        == crate::harness_phase::ActivityPhase::Exited)));
    work.session_started(&fx.daemon.rooms, "r1", "h1", SID);
    assert!(!work.is_attached("h1"), "exited");
}

/// A stop followed by a late hook leaves no poller (the record is gone).
#[test]
fn a_session_start_after_a_stop_attaches_nothing() {
    let fx = Fx::with_shell("byoh");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _entered = rt.enter();
    let (_home, _db) = hermes_home(&fx);
    fx.ensure(false, None).unwrap();
    assert!(fx.runner().stop_harness("h1"));
    let work = &fx.runner().adapters.hermes_work;
    work.session_started(&fx.daemon.rooms, "r1", "h1", SID);
    assert!(!work.is_attached("h1"));
}

/// The re-check after an attach: a record forgotten between the guard and the
/// attach leaves the fresh poller detached again.
#[test]
fn the_post_attach_recheck_detaches_a_poller_whose_record_is_gone() {
    let fx = Fx::with_shell("byoh");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _entered = rt.enter();
    let (_home, _db) = hermes_home(&fx);
    fx.ensure(false, None).unwrap();
    let work = &fx.runner().adapters.hermes_work;

    // Record alive: the re-check keeps the poller.
    work.attach("h1", SID, None).unwrap();
    work.detach_if_gone("h1");
    assert!(work.is_attached("h1"));

    // The stop's forget lands after the guard: the poller is torn down.
    fx.daemon.phase.forget("h1");
    work.detach_if_gone("h1");
    assert!(!work.is_attached("h1"));
}

#[test]
fn the_poller_needs_a_runtime() {
    let fx = Fx::with_shell("byoh");
    let work = &fx.runner().adapters.hermes_work;
    assert!(work.attach("h1", SID, None).is_err());
    assert!(!work.is_attached("h1"));
}
