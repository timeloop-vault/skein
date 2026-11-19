//! The scheduler against a recording driver and a bare channel, in real
//! time with generous margins, like the coalescer tests.
#![allow(clippy::assert_is_empty)]

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{FrameDriver, run_frame_scheduler};

const WINDOW: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Feed(Vec<u8>),
    Cut,
    Flush,
}

#[derive(Clone, Default)]
struct Shared {
    log: Arc<Mutex<Vec<(Instant, Ev)>>>,
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Shared {
    fn events(&self) -> Vec<Ev> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }
    fn cuts(&self) -> Vec<Instant> {
        let log = self.log.lock().unwrap();
        log.iter()
            .filter(|(_, e)| *e == Ev::Cut)
            .map(|(t, _)| *t)
            .collect()
    }
    fn count(&self, ev: &Ev) -> usize {
        self.events().iter().filter(|e| *e == ev).count()
    }
}

struct Fake(Shared);

impl FrameDriver for Fake {
    fn feed(&mut self, bytes: &[u8]) {
        self.0
            .log
            .lock()
            .unwrap()
            .push((Instant::now(), Ev::Feed(bytes.to_vec())));
    }
    fn cut(&mut self) {
        self.0.log.lock().unwrap().push((Instant::now(), Ev::Cut));
    }
    fn sync_deadline(&self) -> Option<Instant> {
        *self.0.deadline.lock().unwrap()
    }
    fn flush_sync(&mut self) {
        *self.0.deadline.lock().unwrap() = None;
        self.0.log.lock().unwrap().push((Instant::now(), Ev::Flush));
    }
}

fn start() -> (mpsc::Sender<Vec<u8>>, Shared, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let shared = Shared::default();
    let mut fake = Fake(shared.clone());
    let handle = thread::spawn(move || run_frame_scheduler(&rx, WINDOW, &mut fake));
    (tx, shared, handle)
}

fn pause(d: Duration) {
    thread::sleep(d);
}

#[test]
fn the_first_chunk_is_fed_and_cut_at_once() {
    let (tx, shared, handle) = start();
    tx.send(b"a".to_vec()).unwrap();
    pause(WINDOW / 4);
    assert_eq!(shared.events(), vec![Ev::Feed(b"a".to_vec()), Ev::Cut]);
    drop(tx);
    handle.join().unwrap();
    assert_eq!(shared.count(&Ev::Cut), 1, "nothing new, no final cut");
}

#[test]
fn a_burst_is_fed_chunk_by_chunk_and_its_tail_cut_once() {
    let (tx, shared, handle) = start();
    tx.send(b"1".to_vec()).unwrap();
    pause(WINDOW / 5);
    for c in [b"2", b"3", b"4"] {
        tx.send(c.to_vec()).unwrap();
        pause(WINDOW / 10);
    }
    // Everything inside the window is fed already, with only the leading cut.
    assert_eq!(shared.count(&Ev::Cut), 1);
    assert_eq!(
        shared
            .events()
            .iter()
            .filter(|e| matches!(e, Ev::Feed(_)))
            .count(),
        4
    );
    pause(WINDOW * 2);
    assert_eq!(
        shared.count(&Ev::Cut),
        2,
        "the tail is cut after the window"
    );
    assert_eq!(shared.events().last(), Some(&Ev::Cut));
    drop(tx);
    handle.join().unwrap();
    assert_eq!(shared.count(&Ev::Cut), 2);
}

#[test]
fn cuts_are_never_closer_than_the_window() {
    let (tx, shared, handle) = start();
    let started = Instant::now();
    while started.elapsed() < WINDOW * 5 {
        tx.send(b"x".to_vec()).unwrap();
        pause(Duration::from_millis(5));
    }
    drop(tx);
    handle.join().unwrap();
    let cuts = shared.cuts();
    assert!(cuts.len() >= 3, "{} cuts", cuts.len());
    // At most one per window, plus the final cut on disconnect.
    assert!(cuts.len() <= 7, "{} cuts", cuts.len());
    let timed = &cuts[..cuts.len() - 1];
    for pair in timed.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(gap + Duration::from_millis(10) >= WINDOW, "{gap:?}");
    }
}

#[test]
fn a_sync_deadline_flushes_and_cuts_without_further_output() {
    let (tx, shared, handle) = start();
    tx.send(b"a".to_vec()).unwrap();
    pause(WINDOW / 4);
    assert_eq!(shared.count(&Ev::Cut), 1);
    // An update opened by the feed; the scheduler learns of it on its next
    // loop, so wake it with a chunk, then leave it quiet.
    *shared.deadline.lock().unwrap() = Some(Instant::now() + WINDOW * 3);
    tx.send(b"b".to_vec()).unwrap();
    pause(WINDOW * 2);
    assert_eq!(shared.count(&Ev::Flush), 0, "not before the deadline");
    pause(WINDOW * 3);
    let events = shared.events();
    assert_eq!(shared.count(&Ev::Flush), 1);
    let flush = events.iter().position(|e| *e == Ev::Flush).unwrap();
    assert_eq!(events.get(flush + 1), Some(&Ev::Cut), "{events:?}");
    drop(tx);
    handle.join().unwrap();
}

#[test]
fn disconnect_makes_the_final_cut() {
    let (tx, shared, handle) = start();
    tx.send(b"a".to_vec()).unwrap();
    pause(WINDOW / 4);
    tx.send(b"b".to_vec()).unwrap();
    pause(WINDOW / 4);
    assert_eq!(shared.count(&Ev::Cut), 1);
    drop(tx);
    handle.join().unwrap();
    let events = shared.events();
    assert_eq!(events.last(), Some(&Ev::Cut));
    assert_eq!(shared.count(&Ev::Cut), 2);
}

#[test]
fn disconnect_with_an_open_sync_flushes_then_cuts() {
    let (tx, shared, handle) = start();
    *shared.deadline.lock().unwrap() = Some(Instant::now() + Duration::from_secs(60));
    pause(WINDOW / 4);
    drop(tx);
    handle.join().unwrap();
    assert_eq!(shared.events(), vec![Ev::Flush, Ev::Cut]);
}

#[test]
fn idle_never_cuts() {
    let (tx, shared, handle) = start();
    pause(WINDOW * 3);
    assert!(shared.events().is_empty());
    drop(tx);
    handle.join().unwrap();
    assert!(shared.events().is_empty(), "nothing fed, no final cut");
}
