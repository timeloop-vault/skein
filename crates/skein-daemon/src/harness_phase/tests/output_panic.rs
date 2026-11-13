//! A panic in the event sink must not end the `skein-phase-out` thread (#732).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::event_sink::{EventSink, SharedEventSink};
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::test_support::RecordingSink;

/// Panics on its first emit, then records like [`RecordingSink`].
struct PanicsOnce {
    armed: AtomicBool,
    inner: Arc<RecordingSink>,
}

impl EventSink for PanicsOnce {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        assert!(!self.armed.swap(false, Ordering::SeqCst), "sink panic");
        self.inner.emit_json(event, payload)
    }
}

#[test]
fn a_panicking_sink_does_not_stop_the_next_row_and_event() {
    let (_dir, db) = crate::db::test_support::fresh_db();
    let db = Arc::new(db);
    let recorded = Arc::new(RecordingSink::default());
    let events: SharedEventSink = Arc::new(PanicsOnce {
        armed: AtomicBool::new(true),
        inner: Arc::clone(&recorded),
    });
    let svc = PhaseService::with_clock(Arc::new(|| 1_000_000), Arc::clone(&db), events);

    // The first update's emit panics.
    svc.spawned("h", "r1", false);
    svc.flush_outputs();
    assert!(recorded.events().is_empty(), "the first emit panicked");

    // The next transition still gets its row and its event.
    svc.record_output("h", "hello");
    svc.flush_outputs();
    let rows = db.recent_harness_events_by_harness("h", 0, 100).unwrap();
    assert!(
        rows.iter().any(|e| e.to_phase == "running"),
        "rows: {rows:?}"
    );
    let events = recorded.events();
    assert_eq!(events.len(), 1, "events: {events:?}");
    assert!(events[0].1.contains("\"running\""));
}
