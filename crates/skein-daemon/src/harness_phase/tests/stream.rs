//! The authority's outputs (#732): the `harness_events` rows and the phase
//! stream, driven by a fake clock with a real database and a recording sink.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::harness_phase::SupervisorFacts;
use crate::harness_phase::constants::{IDLE_AFTER_MS, TICK_INTERVAL_MS};
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::stream::{HARNESS_PHASE_EVENT, PhaseUpdate, PhaseView};
use crate::harness_phase::test_support::RecordingSink;
use crate::harness_phase::types::ActivityPhase;

const T0: i64 = 1_000_000;

struct Rig {
    svc: Arc<PhaseService>,
    sink: Arc<RecordingSink>,
    now: Arc<AtomicI64>,
}

impl Rig {
    fn new() -> Self {
        let now = Arc::new(AtomicI64::new(T0));
        let c = Arc::clone(&now);
        let (svc, sink) =
            PhaseService::test_with_clock_and_sink(Arc::new(move || c.load(Ordering::SeqCst)));
        Self { svc, sink, now }
    }

    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn tick(&self) {
        self.svc.tick_once(&SupervisorFacts::default());
    }

    /// Every update emitted so far, in order, once the output thread caught up.
    fn updates(&self) -> Vec<PhaseUpdate> {
        self.svc.flush_outputs();
        self.sink
            .events()
            .into_iter()
            .map(|(event, payload)| {
                assert_eq!(event, HARNESS_PHASE_EVENT);
                parse(&payload)
            })
            .collect()
    }
}

/// `PhaseUpdate` is Serialize-only (it is a wire type); read it back through
/// JSON, which also proves the shape clients see.
fn parse(payload: &str) -> PhaseUpdate {
    let v: Value = serde_json::from_str(payload).unwrap();
    let view = match &v["view"] {
        Value::Null => None,
        a => Some(a.clone()),
    };
    PhaseUpdate {
        seq: v["seq"].as_u64().unwrap(),
        harness_id: v["harnessId"].as_str().unwrap().to_owned(),
        room_id: v["roomId"].as_str().unwrap().to_owned(),
        view: view.map(|a| {
            let mut rec = crate::harness_phase::types::HarnessActivity::fresh(
                serde_json::from_value(a["phase"].clone()).unwrap(),
                None,
                a["spawnedAt"].as_i64().unwrap(),
            );
            rec.last_output_at = a["lastOutputAt"].as_i64();
            PhaseView::from(&rec)
        }),
        subagents: Vec::new(),
        background_tasks: Vec::new(),
        transitions: v["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| crate::harness_phase::types::Transition {
                harness_id: t["harnessId"].as_str().unwrap().to_owned(),
                from: serde_json::from_value(t["from"].clone()).unwrap(),
                to: serde_json::from_value(t["to"].clone()).unwrap(),
                source: serde_json::from_value(t["source"].clone()).unwrap(),
                at_ms: t["atMs"].as_i64().unwrap(),
                has_user_input: t["hasUserInput"].as_bool().unwrap(),
            })
            .collect(),
        delegation_armed: v["delegationArmed"].as_bool().unwrap(),
    }
}

#[test]
fn no_client_phase_updates_and_daemon_writes_harness_events() {
    let r = Rig::new();
    r.svc.spawned("h", "room-7", false);
    r.svc.record_output("h", "hello");
    r.advance(IDLE_AFTER_MS + TICK_INTERVAL_MS);
    r.tick();

    // The phase is the daemon's, with no client having said anything.
    assert_eq!(r.svc.phase_snapshot()["h"], ActivityPhase::Idle);

    // The harness_events rows are the daemon's too.
    r.svc.flush_outputs();
    let mut rows = r
        .svc
        .test_db()
        .recent_harness_events_by_harness("h", 0, 100)
        .unwrap();
    rows.reverse();
    let got: Vec<_> = rows
        .iter()
        .map(|e| {
            (
                e.room_id.as_str(),
                e.from_phase.as_str(),
                e.to_phase.as_str(),
                e.source.as_deref(),
                e.has_user_input,
                e.timestamp_ms,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "room-7",
                "spawning",
                "running",
                Some("pty-output"),
                false,
                T0
            ),
            (
                "room-7",
                "running",
                "idle",
                Some("l2a-idle"),
                false,
                T0 + IDLE_AFTER_MS + TICK_INTERVAL_MS
            ),
        ]
    );

    // The stream carries the same transitions, in order, with rising seq.
    let updates = r.updates();
    assert!(updates.windows(2).all(|w| w[0].seq < w[1].seq));
    assert!(updates.iter().all(|u| u.room_id == "room-7"));
    let streamed: Vec<_> = updates
        .iter()
        .flat_map(|u| &u.transitions)
        .map(|t| (t.from.as_str(), t.to.as_str(), t.source.as_str(), t.at_ms))
        .collect();
    let stored: Vec<_> = rows
        .iter()
        .map(|e| {
            (
                e.from_phase.as_str(),
                e.to_phase.as_str(),
                e.source.as_deref().unwrap(),
                e.timestamp_ms,
            )
        })
        .collect();
    assert_eq!(streamed, stored);
    assert_eq!(
        updates.last().unwrap().view.as_ref().unwrap().phase,
        ActivityPhase::Idle
    );
}

#[test]
fn a_transition_for_an_unknown_id_writes_no_row_and_emits_nothing() {
    let r = Rig::new();
    r.svc.with(|s, now| s.machine.exited("ghost", Some(1), now));
    r.svc.flush_outputs();
    assert_eq!(r.sink.events().len(), 0);
    let rows = r
        .svc
        .test_db()
        .recent_harness_events_by_harness("ghost", 0, 10)
        .unwrap();
    assert_eq!(rows.len(), 0);
}

#[test]
fn forget_emits_a_final_update_with_null_view() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.forget("h");
    let updates = r.updates();
    let last = updates.last().unwrap();
    assert_eq!(last.harness_id, "h");
    assert_eq!(last.room_id, "room-1");
    assert!(last.view.is_none());
    let (_, payload) = r.sink.events().pop().unwrap();
    let v: Value = serde_json::from_str(&payload).unwrap();
    assert!(v["view"].is_null());
    assert_eq!(r.svc.stream_snapshot().len(), 0);
}

#[test]
fn an_output_time_only_change_waits_for_the_tick() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.record_output("h", "a");
    let before = r.updates().len();

    r.advance(300);
    r.svc.record_output("h", "b");
    assert_eq!(r.updates().len(), before, "an ordinary call must not emit");

    r.tick();
    let updates = r.updates();
    assert_eq!(updates.len(), before + 1, "the tick emits it");
    let last = updates.last().unwrap();
    assert_eq!(last.transitions.len(), 0);
    assert_eq!(last.view.as_ref().unwrap().last_output_at, Some(T0 + 300));

    // Nothing changed since: the next tick is silent.
    r.advance(10);
    r.tick();
    assert_eq!(r.updates().len(), before + 1);
}

#[test]
fn stream_snapshot_lists_every_harness_at_the_current_seq() {
    let r = Rig::new();
    r.svc.spawned("b", "room-2", false);
    r.svc.spawned("a", "room-1", false);
    r.svc.record_output("a", "x");
    let last_seq = r.updates().last().unwrap().seq;

    let snap = r.svc.stream_snapshot();
    assert_eq!(
        snap.iter()
            .map(|u| u.harness_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(snap[0].room_id, "room-1");
    assert_eq!(snap[0].view.as_ref().unwrap().phase, ActivityPhase::Running);
    assert_eq!(
        snap[1].view.as_ref().unwrap().phase,
        ActivityPhase::Spawning
    );
    for u in &snap {
        assert_eq!(u.seq, last_seq);
        assert_eq!(u.transitions.len(), 0);
        assert!(!u.delegation_armed);
    }
}

#[test]
fn the_screen_tail_is_not_on_the_wire() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.record_output("h", "some screen text");
    assert_ne!(r.svc.get("h").unwrap().tail.len(), 0);

    let json = serde_json::to_string(&r.svc.stream_snapshot()[0]).unwrap();
    assert!(!json.contains("tail"), "{json}");
    assert!(!json.contains("some screen text"), "{json}");
    for (_, payload) in r.sink.events() {
        assert!(!payload.contains("\"tail\""));
    }
    r.svc.flush_outputs();
    for (_, payload) in r.sink.events() {
        assert!(!payload.contains("\"tail\""));
    }
}

#[test]
fn an_emitted_update_has_the_documented_shape() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.record_output("h", "x");
    r.svc.flush_outputs();
    let (event, payload) = r.sink.events().pop().unwrap();
    assert_eq!(event, "skein://harness-phase");
    let v: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(v["harnessId"], "h");
    assert_eq!(v["roomId"], "room-1");
    assert_eq!(v["view"]["phase"], "running");
    assert_eq!(v["transitions"][0]["from"], "spawning");
    assert_eq!(v["transitions"][0]["source"], "pty-output");
    assert_eq!(v["subagents"], Value::Array(Vec::new()));
    assert_eq!(v["delegationArmed"], false);
    println!("{payload}");
}

/// The emitted JSON's `view` is exactly this key set. Adding a field to
/// `PhaseView` means adding it here: a deliberate act, because the view is
/// the protocol and the machine's own record is not.
#[test]
fn the_emitted_view_has_exactly_the_wire_keys() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    r.svc.record_output("h", "x");
    r.svc.flush_outputs();
    let (_, payload) = r.sink.events().pop().unwrap();
    let v: Value = serde_json::from_str(&payload).unwrap();
    assert!(v.get("activity").is_none());
    let mut keys: Vec<&str> = v["view"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut expected = vec![
        // Display and notifications.
        "phase",
        "phaseSince",
        "spawnedAt",
        "hasUserInput",
        "permissionTool",
        "permissionAgentType",
        "delegatedCount",
        "lastTurnSignal",
        "lastSubmitAt",
        // The send gate (leaves with #733).
        "authoritative",
        "adapterHeard",
        "adapterSilent",
        "launchSignalAt",
        "lastOutputAt",
        "questionOpen",
        "delegationDeferredAt",
        "injected",
    ];
    expected.sort_unstable();
    assert_eq!(keys, expected);
    for internal in [
        "tail",
        "degradedBy",
        "silenceRecovered",
        "lastAdapterEvent",
        "authorityLostAt",
        "permissionAt",
        "promptSubmittedAt",
        "delegationActivityAt",
        "delegationEmptiedAt",
        "permissionAgentId",
        "exitCode",
    ] {
        assert!(
            v["view"].get(internal).is_none(),
            "{internal} must not be on the wire"
        );
    }
}

/// A field that is not on the wire does not make an update: the throttle
/// compares views.
#[test]
fn a_change_to_a_field_off_the_wire_emits_nothing() {
    let r = Rig::new();
    r.svc.spawned("h", "room-1", false);
    let before = r.updates().len();
    r.svc.with(|s, _| {
        s.machine.store.get_mut("h").unwrap().delegation_activity_at = 12345;
    });
    r.tick();
    assert_eq!(r.updates().len(), before);
}
