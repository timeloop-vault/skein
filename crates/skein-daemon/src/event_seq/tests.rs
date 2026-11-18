use std::sync::Mutex;

use serde_json::{Value, json};
use skein_proto::Sequenced;

use super::*;

#[derive(Default)]
struct Rec {
    got: Mutex<Vec<(String, String)>>,
    fail: bool,
}

impl EventSink for Rec {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.got.lock().unwrap().push((event.to_owned(), payload));
        if self.fail {
            Err("boom".into())
        } else {
            Ok(())
        }
    }
}

fn make(caps: Option<(usize, usize)>, fail: bool) -> (Arc<SeqEventSink>, Arc<Rec>) {
    let rec = Arc::new(Rec {
        got: Mutex::default(),
        fail,
    });
    let sink = match caps {
        Some((n, b)) => SeqEventSink::with_caps(rec.clone(), n, b),
        None => SeqEventSink::new(rec.clone()),
    };
    (sink, rec)
}

fn seqs(r: &Replay) -> Vec<u64> {
    match r {
        Replay::Events { events, .. } => events.iter().map(|e| e.seq).collect(),
        Replay::Gap { .. } => panic!("gap"),
    }
}

#[test]
fn seq_increases_across_events_and_threads() {
    let (sink, rec) = make(None, false);
    assert_eq!(sink.current(), 0);
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let s = sink.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    let name = if (t + i) % 2 == 0 { "a" } else { "b" };
                    s.emit_json(name, "{}".into()).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(sink.current(), 400);
    let Replay::Events { events, seq } = sink.subscribe(sink.instance_id(), 0) else {
        panic!("gap")
    };
    assert_eq!(seq, 400);
    let got: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(got, (1..=400).collect::<Vec<_>>());
    assert_eq!(rec.got.lock().unwrap().len(), 400);
}

#[test]
fn seq_is_written_and_overwrites_existing_key() {
    let (sink, rec) = make(None, false);
    sink.emit_json("a", r#"{"x":1,"seq":99}"#.into()).unwrap();
    let got = rec.got.lock().unwrap();
    let text = &got[0].1;
    assert_eq!(text.matches("\"seq\"").count(), 1);
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(v, json!({"x":1,"seq":1}));
}

#[test]
fn sequenced_decodes_a_stamped_payload() {
    #[derive(serde::Deserialize)]
    struct P {
        origin: String,
    }
    let (sink, rec) = make(None, false);
    sink.emit_json("a", r#"{"origin":"c1"}"#.into()).unwrap();
    let text = rec.got.lock().unwrap()[0].1.clone();
    let s: Sequenced<P> = serde_json::from_str(&text).unwrap();
    assert_eq!((s.seq, s.payload.origin.as_str()), (1, "c1"));
}

#[test]
fn harness_action_is_stamped_and_delivered_but_not_logged() {
    let (sink, rec) = make(None, false);
    sink.emit_json("harness-action", "{}".into()).unwrap();
    sink.emit_json("b", "{}".into()).unwrap();
    let got = rec.got.lock().unwrap();
    assert_eq!(got[0].1, r#"{"seq":1}"#);
    drop(got);
    assert_eq!(seqs(&sink.subscribe(sink.instance_id(), 0)), vec![2]);
}

#[test]
fn non_object_payload_passes_through_consumes_seq_unlogged() {
    let (sink, rec) = make(None, false);
    sink.emit_json("a", "[1]".into()).unwrap();
    sink.emit_json("a", "not json".into()).unwrap();
    sink.emit_json("a", "{}".into()).unwrap();
    let got = rec.got.lock().unwrap();
    assert_eq!(got[0].1, "[1]");
    assert_eq!(got[1].1, "not json");
    drop(got);
    assert_eq!(sink.current(), 3);
    assert_eq!(seqs(&sink.subscribe(sink.instance_id(), 0)), vec![3]);
}

#[test]
fn subscribe_returns_exactly_the_tail() {
    let (sink, _) = make(None, false);
    for _ in 0..5 {
        sink.emit_json("a", "{}".into()).unwrap();
    }
    let id = sink.instance_id().to_owned();
    assert_eq!(seqs(&sink.subscribe(&id, 2)), vec![3, 4, 5]);
    assert_eq!(
        sink.subscribe(&id, 5),
        Replay::Events {
            events: Vec::new(),
            seq: 5
        }
    );
    let Replay::Events { events, .. } = sink.subscribe(&id, 4) else {
        panic!("gap")
    };
    assert_eq!(events[0].event, "a");
    assert_eq!(events[0].payload, r#"{"seq":5}"#);
}

#[test]
fn gap_on_count_cap_eviction() {
    let (sink, _) = make(Some((3, usize::MAX)), false);
    for _ in 0..5 {
        sink.emit_json("a", "{}".into()).unwrap();
    }
    let id = sink.instance_id().to_owned();
    // Log holds 3..=5, evicted 2.
    assert_eq!(sink.subscribe(&id, 1), Replay::Gap { seq: 5 });
    assert_eq!(seqs(&sink.subscribe(&id, 2)), vec![3, 4, 5]);
}

#[test]
fn gap_on_byte_cap_eviction() {
    // `{"seq":1}` is 9 bytes; cap 20 keeps two.
    let (sink, _) = make(Some((100, 20)), false);
    for _ in 0..4 {
        sink.emit_json("a", "{}".into()).unwrap();
    }
    let id = sink.instance_id().to_owned();
    assert_eq!(sink.subscribe(&id, 1), Replay::Gap { seq: 4 });
    assert_eq!(seqs(&sink.subscribe(&id, 2)), vec![3, 4]);
}

#[test]
fn gap_on_instance_mismatch_and_future_since() {
    let (sink, _) = make(None, false);
    sink.emit_json("a", "{}".into()).unwrap();
    let id = sink.instance_id().to_owned();
    assert_eq!(sink.subscribe("other", 0), Replay::Gap { seq: 1 });
    assert_eq!(sink.subscribe(&id, 2), Replay::Gap { seq: 1 });
    let (other, _) = make(None, false);
    assert_ne!(other.instance_id(), id);
}

#[test]
fn inner_error_is_returned_and_event_stays_logged() {
    let (sink, _) = make(None, true);
    assert_eq!(sink.emit_json("a", "{}".into()), Err("boom".to_owned()));
    assert_eq!(seqs(&sink.subscribe(sink.instance_id(), 0)), vec![1]);
    let st = sink.stamp("a", "{}".into());
    assert_eq!(sink.deliver("a", st), Err("boom".to_owned()));
}
