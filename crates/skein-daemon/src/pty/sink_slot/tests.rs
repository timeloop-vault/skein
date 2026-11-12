//! `SinkSlot` without a PTY (#731).

use std::sync::Arc;

use parking_lot::Mutex;

use super::{AttachOutcome, SinkSlot};
use crate::pty::PtyEvent;
use crate::stream_sink::StreamSink;

const QUERY: &str = "\x1b[6n";
const REPORT: &[u8] = b"\x1b[1;1R";

/// Records every event it is sent as a compact string.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.0.lock().clone()
    }
}

impl StreamSink<PtyEvent> for Recorder {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        self.0.lock().push(match item {
            PtyEvent::Data { chunk } => format!("data:{chunk}"),
            PtyEvent::Exit { code } => format!("exit:{code:?}"),
        });
        Ok(())
    }
}

/// A sink whose receiver is gone.
struct Dead;

impl StreamSink<PtyEvent> for Dead {
    fn send(&self, _: PtyEvent) -> Result<(), String> {
        Err("gone".into())
    }
}

/// A slot and the list of every reply it wrote to the PTY.
fn slot(sink: Option<Box<dyn StreamSink<PtyEvent>>>) -> (SinkSlot, Arc<Mutex<Vec<Vec<u8>>>>) {
    let replies = Arc::new(Mutex::new(Vec::new()));
    let r = Arc::clone(&replies);
    (SinkSlot::new(sink, move |b| r.lock().push(b)), replies)
}

#[test]
fn attach_replaces_the_previous_sink() {
    let first = Recorder::default();
    let second = Recorder::default();
    let (s, _) = slot(Some(Box::new(first.clone())));
    s.data("a".into());
    assert_eq!(s.attach(Box::new(second.clone())), AttachOutcome::Live);
    s.data("b".into());
    assert_eq!(first.events(), ["data:a"]);
    assert_eq!(second.events(), ["data:b"]);
}

#[test]
fn data_after_detach_is_dropped_without_error() {
    let rec = Recorder::default();
    let (s, replies) = slot(Some(Box::new(rec.clone())));
    assert!(s.detach());
    assert!(!s.detach(), "second detach finds nothing attached");
    s.data("lost".into());
    assert_eq!(rec.events(), Vec::<String>::new());
    assert!(replies.lock().is_empty());
    assert_eq!(s.exited(), None);
}

#[test]
fn a_failing_sink_is_not_fatal_and_stays_attached() {
    let (s, _) = slot(Some(Box::new(Dead)));
    s.data("x".into());
    s.exit(Some(1));
    assert!(s.detach(), "a failed send does not auto-detach");
}

#[test]
fn a_dead_sink_does_not_wedge_the_cursor_query() {
    let (s, replies) = slot(Some(Box::new(Dead)));
    s.data(format!("a{QUERY}b"));
    assert_eq!(*replies.lock(), [REPORT.to_vec()]);
    assert!(s.detach(), "the dead sink is still attached, not killed");
}

#[test]
fn a_query_split_over_two_failed_sends_is_answered_once() {
    let (s, replies) = slot(Some(Box::new(Dead)));
    s.data("text\x1b[6".into());
    assert!(replies.lock().is_empty());
    s.data("n".into());
    assert_eq!(*replies.lock(), [REPORT.to_vec()]);
    s.data("n".into());
    assert_eq!(replies.lock().len(), 1);
}

/// Succeeds or fails each send as scripted (true = ok), then fails.
struct Scripted(Mutex<Vec<bool>>);

impl StreamSink<PtyEvent> for Scripted {
    fn send(&self, _: PtyEvent) -> Result<(), String> {
        let mut script = self.0.lock();
        if !script.is_empty() && script.remove(0) {
            Ok(())
        } else {
            Err("gone".into())
        }
    }
}

#[test]
fn a_successful_send_drops_a_carry_left_by_a_failed_one() {
    let (s, replies) = slot(Some(Box::new(Scripted(Mutex::new(vec![false, true])))));
    s.data("[6".into()); // fails: carry kept
    s.data("ok".into()); // succeeds: carry cleared
    s.data("n".into()); // fails: must not complete the old carry
    assert!(replies.lock().is_empty());
}

#[test]
fn an_exit_while_detached_reaches_the_next_attach() {
    let (s, _) = slot(None);
    s.exit(Some(7));
    assert_eq!(s.exited().map(|e| e.code), Some(Some(7)));
    let rec = Recorder::default();
    assert_eq!(
        s.attach(Box::new(rec.clone())),
        AttachOutcome::Exited { code: Some(7) }
    );
    assert_eq!(rec.events(), ["exit:Some(7)"]);
}

#[test]
fn an_exit_with_a_sink_is_delivered_live_and_recorded() {
    let rec = Recorder::default();
    let (s, _) = slot(Some(Box::new(rec.clone())));
    s.exit(None);
    assert_eq!(rec.events(), ["exit:None"]);
    assert_eq!(s.exited().map(|e| e.code), Some(None));
}

#[test]
fn a_detached_cursor_query_is_answered_once_per_query() {
    let (s, replies) = slot(None);
    s.data(format!("hello {QUERY} world"));
    assert_eq!(*replies.lock(), [REPORT.to_vec()]);
    s.data(format!("{QUERY}{QUERY}"));
    assert_eq!(replies.lock().len(), 3);
}

#[test]
fn a_query_split_across_two_chunks_is_answered_once() {
    let (s, replies) = slot(None);
    s.data("text\x1b[".into());
    assert!(replies.lock().is_empty());
    s.data("6nmore".into());
    assert_eq!(*replies.lock(), [REPORT.to_vec()]);
    // The carry was consumed: a later unrelated chunk does not re-answer.
    s.data("6n".into());
    assert_eq!(replies.lock().len(), 1);
}

#[test]
fn a_split_query_is_not_answered_twice_by_the_overlap() {
    let (s, replies) = slot(None);
    for part in ["\x1b", "[", "6", "n"] {
        s.data(part.into());
    }
    assert_eq!(replies.lock().len(), 1);
}

#[test]
fn an_attached_sink_is_never_answered_for() {
    let rec = Recorder::default();
    let (s, replies) = slot(Some(Box::new(rec.clone())));
    s.data(format!("a{QUERY}b"));
    s.data("\x1b[".into());
    s.data("6n".into());
    assert!(replies.lock().is_empty());
    assert_eq!(rec.events().len(), 3, "the query still reaches the view");
}

#[test]
fn attaching_drops_a_half_seen_query() {
    let (s, replies) = slot(None);
    s.data("\x1b[".into());
    s.attach(Box::new(Recorder::default()));
    s.detach();
    s.data("6n".into());
    assert!(replies.lock().is_empty());
}
