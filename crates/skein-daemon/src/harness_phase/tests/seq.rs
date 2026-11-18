//! The phase stream shares the per-daemon event seq (#757): stamped under the
//! service lock, so `stream_snapshot` is exactly consistent with the stream.

use std::sync::Arc;
use std::sync::PoisonError;

use serde_json::Value;
use skein_proto::Replay;

use crate::event_sink::EventSink;
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::stream::HARNESS_PHASE_EVENT;
use crate::harness_phase::types::ActivityPhase;

fn rig() -> (
    Arc<PhaseService>,
    Arc<crate::harness_phase::test_support::RecordingSink>,
    Arc<crate::event_seq::SeqEventSink>,
) {
    PhaseService::test_with_clock_sink_and_seq(Arc::new(|| 1_000_000))
}

fn seq_of(payload: &str) -> u64 {
    serde_json::from_str::<Value>(payload).unwrap()["seq"]
        .as_u64()
        .unwrap()
}

#[test]
fn phase_updates_and_other_events_share_one_rising_sequence() {
    let (svc, sink, seq) = rig();
    svc.spawned("h", "r1", false);
    seq.emit_json("other", r#"{"x":1}"#.to_owned()).unwrap();
    svc.record_output("h", "hello");
    seq.emit_json("other", r#"{"x":2}"#.to_owned()).unwrap();
    svc.flush_outputs();

    let events = sink.events();
    let phase: Vec<u64> = events
        .iter()
        .filter(|(e, _)| e == HARNESS_PHASE_EVENT)
        .map(|(_, p)| seq_of(p))
        .collect();
    assert_eq!(phase.len(), 2);
    assert!(phase[0] < phase[1], "{phase:?}");
    assert!(
        phase[1] - phase[0] >= 2,
        "the other event took a number in between: {phase:?}"
    );
    let mut all: Vec<u64> = events.iter().map(|(_, p)| seq_of(p)).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 4, "every event has its own seq");
}

#[test]
fn the_snapshot_matches_the_stamped_stream_before_delivery() {
    let (svc, sink, seq) = rig();
    // Holding the recording sink's lock parks the output thread in its
    // emit, so nothing is delivered while we read.
    let held = sink.0.lock().unwrap_or_else(PoisonError::into_inner);

    svc.spawned("h", "r1", false);
    let first = svc.stream_snapshot();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].seq, seq.current());
    assert_eq!(
        first[0].view.as_ref().unwrap().phase,
        ActivityPhase::Spawning
    );

    svc.record_output("h", "hello");
    let second = svc.stream_snapshot();
    assert!(second[0].seq > first[0].seq);
    assert_eq!(second[0].seq, seq.current());
    assert_eq!(
        second[0].view.as_ref().unwrap().phase,
        ActivityPhase::Running
    );

    drop(held);
    svc.flush_outputs();

    // Every delivered update at or below a snapshot's seq is the state that
    // snapshot holds; every later one is higher.
    let updates: Vec<(u64, String)> = sink
        .events()
        .iter()
        .map(|(_, p)| {
            let v: Value = serde_json::from_str(p).unwrap();
            (
                v["seq"].as_u64().unwrap(),
                v["view"]["phase"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0], (first[0].seq, "spawning".to_owned()));
    assert_eq!(updates[1], (second[0].seq, "running".to_owned()));
}

#[test]
fn the_delivered_json_seq_is_the_typed_seq_and_replay_is_in_seq_order() {
    let (svc, sink, seq) = rig();
    svc.spawned("a", "r1", false);
    svc.spawned("b", "r2", false);
    svc.record_output("a", "x");
    svc.flush_outputs();

    let delivered: Vec<u64> = sink.events().iter().map(|(_, p)| seq_of(p)).collect();
    assert_eq!(delivered.len(), 3);
    let snap_seq = svc.stream_snapshot()[0].seq;
    assert_eq!(*delivered.last().unwrap(), snap_seq);

    let Replay::Events { events, seq: head } = seq.subscribe(seq.instance_id(), 0) else {
        panic!("a fresh subscription replays");
    };
    assert_eq!(head, snap_seq);
    let replayed: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(replayed, delivered);
    assert!(events.iter().all(|e| e.event == HARNESS_PHASE_EVENT));
}
