//! The deadline driver (#761): real threads and the real clock. Bounds are
//! generous (CI is slow) but each one still says something: a deadline fires
//! on time, nothing ticks on a timer, and an input wakes a sleeping driver.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use crate::harness_phase::constants::PATTERN_WAITING_AFTER_MS;
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::types::ActivityPhase;
use crate::harness_phase::{PhaseNotice, now_ms};

fn real_service() -> Arc<PhaseService> {
    PhaseService::test_with_clock(Arc::new(now_ms))
}

fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

/// Wait until the driver is asleep and has stopped scanning: a poke made while
/// it was starting can still bring one more planning pass after it first
/// sleeps. Returns the settled scan count.
fn settle(svc: &PhaseService) -> usize {
    let end = Instant::now() + Duration::from_secs(10);
    let mut seen = usize::MAX;
    let mut since = Instant::now();
    loop {
        let now = svc.deadline_scans.load(Ordering::Relaxed);
        if now != seen || !svc.driver_is_asleep() {
            seen = now;
            since = Instant::now();
        } else if since.elapsed() >= Duration::from_millis(150) {
            return now;
        }
        assert!(Instant::now() < end, "driver never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A prompt-looking tail goes `waiting` after `PATTERN_WAITING_AFTER_MS` of
/// silence: not before its due instant, and promptly after it. The driver
/// was asleep with nothing pending when the output arrived, so this also
/// covers the wake from "no deadline".
#[test]
fn a_deadline_wakes_the_driver_on_time() {
    let svc = real_service();
    let (tx, rx) = channel();
    svc.set_listener(Arc::new(move |n| {
        if let PhaseNotice::Transition {
            to: ActivityPhase::Waiting,
            ..
        } = n
        {
            tx.send(now_ms()).ok();
        }
    }));
    let _driver = svc.start_deadline_driver().expect("thread");
    svc.spawned("h", "room-1", false);
    assert!(
        wait_until(Duration::from_secs(5), || svc.driver_is_asleep()),
        "driver asleep"
    );
    svc.record_output("h", "Password:");
    let due = svc.get("h").unwrap().last_output_at.unwrap() + PATTERN_WAITING_AFTER_MS;
    let at = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the driver ticked at the deadline");
    let late = at - due;
    eprintln!("L2b deadline: observed {late} ms after its due instant");
    assert!(late >= 0, "fired {} ms before its deadline", -late);
    assert!(late <= 1_000, "fired {late} ms late");
}

/// With harnesses present and no deadline pending, nothing runs the machine
/// tick, and the driver does not even wake to look.
#[test]
fn no_periodic_tick_while_no_deadline_is_pending() {
    let svc = real_service();
    let _driver = svc.start_deadline_driver().expect("thread");
    svc.spawned("a", "room-1", false);
    svc.spawned("b", "room-1", false);
    // Capture only once the driver has planned and gone to sleep: a thread
    // that starts later would add its first scan after the capture.
    let scans = settle(&svc);
    assert_eq!(svc.driver_ticks.load(Ordering::Relaxed), 0);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(svc.driver_ticks.load(Ordering::Relaxed), 0, "a tick ran");
    assert_eq!(
        svc.deadline_scans.load(Ordering::Relaxed),
        scans,
        "the driver woke with nothing due"
    );
}

/// The driver sleeps on a later deadline (8 s idle); an input that creates a
/// sooner one (a prompt tail, 500 ms) wakes it.
#[test]
fn an_input_with_an_earlier_deadline_wakes_a_sleeping_driver() {
    let svc = real_service();
    let _driver = svc.start_deadline_driver().expect("thread");
    svc.spawned("slow", "room-1", false);
    svc.record_output("slow", "working...");
    assert!(
        wait_until(Duration::from_secs(2), || svc.driver_is_asleep()),
        "driver armed on the idle deadline"
    );
    svc.spawned("fast", "room-1", false);
    svc.record_output("fast", "Password:");
    assert!(
        wait_until(Duration::from_secs(3), || svc.get("fast").unwrap().phase
            == ActivityPhase::Waiting),
        "the earlier deadline was missed by a sleeping driver"
    );
    assert_eq!(svc.get("slow").unwrap().phase, ActivityPhase::Running);
}

/// A later deadline (each chunk pushes the idle instant out) wakes nothing,
/// and the output path never scans the whole machine.
#[test]
fn output_chunks_do_not_scan_the_whole_machine() {
    let svc = real_service();
    let _driver = svc.start_deadline_driver().expect("thread");
    for id in ["a", "b", "c"] {
        svc.spawned(id, "room-1", false);
    }
    svc.record_output("a", "first");
    let scans = settle(&svc);
    for _ in 0..2_000 {
        svc.record_output("a", "x");
    }
    assert_eq!(
        svc.deadline_scans.load(Ordering::Relaxed),
        scans,
        "an output chunk scanned the whole machine"
    );
}
