//! Synchronized output, `?2026` (#793, design T3: "handled in the vte
//! processor by buffering, up to 150 ms or 2 MiB").
#![allow(clippy::assert_is_empty)]

use std::time::Duration;

use super::tests::{ManualClock, feed, screen};
use super::{AlacrittyEngine, TerminalEngine};

const BSU: &str = "\x1b[?2026h";
const ESU: &str = "\x1b[?2026l";

fn blank(e: &AlacrittyEngine) -> bool {
    e.screen_text().iter().all(String::is_empty)
}

#[test]
fn nothing_shows_between_begin_and_end_of_a_synchronized_update() {
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, "before\r\n");
    feed(&mut e, &format!("{BSU}\x1b[2J\x1b[Hhalf drawn"));
    // OK: the bytes are held back; the grid still shows the old frame.
    assert_eq!(screen(&e)[0], "before");
    assert!(!screen(&e).concat().contains("half"));
    assert_eq!(e.cursor(), (1, 0), "the cursor has not moved either");
    assert!(e.modes().sync_pending, "Modes reports the open update");
    assert!(e.sync_deadline().is_some());

    feed(&mut e, &format!(" and done{ESU}"));
    assert_eq!(screen(&e)[0], "half drawn and done");
    assert!(!e.modes().sync_pending);
    assert_eq!(e.sync_deadline(), None);
}

#[test]
fn the_deadline_is_150_ms_after_the_begin() {
    let clock = ManualClock::new();
    let mut e = clock.engine(4, 20);
    let t0 = clock.now();
    feed(&mut e, BSU);
    assert_eq!(e.sync_deadline(), Some(t0 + Duration::from_millis(150)));
}

#[test]
fn an_update_is_pending_at_149_ms_and_ended_at_150_ms() {
    let clock = ManualClock::new();
    // By `feed`.
    let mut e = clock.engine(4, 20);
    feed(&mut e, &format!("{BSU}held"));
    clock.advance(Duration::from_millis(149));
    assert!(e.modes().sync_pending);
    feed(&mut e, "!");
    assert!(
        blank(&e),
        "149 ms: still held, the new byte joins the frame"
    );
    assert!(e.modes().sync_pending);
    clock.advance(Duration::from_millis(1));
    feed(&mut e, "\r\nnew");
    assert_eq!(
        screen(&e)[..2],
        ["held!", "new"],
        "150 ms: ended before the new bytes"
    );
    assert!(!e.modes().sync_pending);
    // By `flush_sync`, which the daemon's timer calls at the deadline.
    let mut e = clock.engine(4, 20);
    feed(&mut e, &format!("{BSU}late"));
    let deadline = e.sync_deadline().expect("open");
    assert_eq!(deadline, clock.now() + Duration::from_millis(150));
    clock.advance(Duration::from_millis(149));
    assert!(e.modes().sync_pending && blank(&e));
    clock.advance(Duration::from_millis(1));
    assert!(clock.now() >= deadline);
    assert!(e.flush_sync().is_empty());
    assert_eq!(screen(&e)[0], "late");
    assert!(!e.modes().sync_pending);
}

#[test]
fn the_timeout_is_not_self_driven_the_caller_must_flush() {
    let clock = ManualClock::new();
    let mut e = clock.engine(4, 20);
    feed(&mut e, &format!("{BSU}late"));
    clock.advance(Duration::from_millis(500));
    // SHORTFALL against "buffering for up to 150 ms": the engine runs no
    // timer. Past the deadline the bytes are still held and the update is
    // still reported pending, until something acts.
    assert!(blank(&e));
    assert!(e.modes().sync_pending);
    assert!(e.sync_deadline().is_some_and(|d| d <= clock.now()));
    assert!(e.flush_sync().is_empty());
    assert_eq!(screen(&e)[0], "late");
    assert!(!e.modes().sync_pending);
    // Flushing with nothing open does nothing.
    assert!(e.flush_sync().is_empty());
    assert_eq!(screen(&e)[0], "late");
}

#[test]
fn a_second_begin_extends_the_deadline() {
    let clock = ManualClock::new();
    let mut e = clock.engine(4, 20);
    feed(&mut e, BSU);
    let first = e.sync_deadline().expect("open");
    clock.advance(Duration::from_millis(20));
    feed(&mut e, &format!("a{BSU}b"));
    let second = e.sync_deadline().expect("still open");
    assert_eq!(
        second,
        first + Duration::from_millis(20),
        "a nested begin restarts the 150 ms"
    );
    assert!(blank(&e), "and does not end the update");
    feed(&mut e, ESU);
    assert_eq!(screen(&e)[0], "ab");
}

#[test]
fn the_end_may_arrive_split_across_chunks() {
    for split in 1..ESU.len() {
        let mut e = AlacrittyEngine::new(4, 20);
        feed(&mut e, &format!("{BSU}frame{}", &ESU[..split]));
        assert!(blank(&e), "split at {split}: the end is not complete yet");
        assert!(e.modes().sync_pending);
        feed(&mut e, &format!("{}tail", &ESU[split..]));
        assert_eq!(screen(&e)[0], "frametail", "split at {split}");
        assert!(!e.modes().sync_pending);
    }
}

#[test]
fn the_begin_may_arrive_split_across_chunks() {
    for split in 1..BSU.len() {
        let mut e = AlacrittyEngine::new(4, 20);
        feed(&mut e, &format!("pre{}", &BSU[..split]));
        feed(&mut e, &format!("{}held{ESU}", &BSU[split..]));
        assert_eq!(screen(&e)[0], "preheld", "split at {split}");
        assert!(!e.modes().sync_pending);
    }
    // A frame boundary inside a chunk: bytes before the begin show at once.
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, &format!("seen{BSU}hidden"));
    assert_eq!(screen(&e)[0], "seen");
}

#[test]
fn an_update_is_ended_by_two_mebibytes() {
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, &format!("{BSU}start"));
    let chunk = vec![b'x'; 64 * 1024];
    let mut n = 0;
    while e.modes().sync_pending {
        e.feed(&chunk);
        n += 1;
        assert!(n < 100, "never ended");
    }
    // 2 MiB (0x200000) of held bytes, minus the one the engine keeps free.
    assert_eq!(n, 32);
    // The held bytes were applied (they scrolled the screen full of x).
    assert!(screen(&e)[0].starts_with('x'));
    assert!(e.history_rows_entered() > 1000);
    assert_eq!(e.sync_deadline(), None);
    // One chunk bigger than the limit ends it at once too.
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, &format!("{BSU}start"));
    e.feed(&vec![b'y'; 3 * 1024 * 1024]);
    assert!(!e.modes().sync_pending);
    assert!(screen(&e)[0].starts_with('y'));
}

#[test]
fn replies_to_queries_inside_an_update_wait_for_the_end() {
    let mut e = AlacrittyEngine::new(4, 20);
    assert!(feed(&mut e, &format!("{BSU}\x1b[3;4H\x1b[6n")).is_empty());
    let reply = feed(&mut e, ESU);
    assert_eq!(reply, b"\x1b[3;4R", "the position as it is after the frame");
    // And a reply held by a timed-out update comes out of `flush_sync`.
    feed(&mut e, &format!("{BSU}\x1b[6n"));
    assert_eq!(e.flush_sync(), b"\x1b[3;4R");
}

#[test]
fn rows_scrolled_inside_an_update_are_counted_when_it_ends() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, &format!("{BSU}{}", "\r\n".repeat(30)));
    assert_eq!(e.history_rows_entered(), 0);
    assert_eq!(e.history_len(), 0);
    feed(&mut e, ESU);
    assert_eq!(
        e.history_rows_entered(),
        26,
        "30 line feeds on 5 rows scroll 26"
    );
    assert_eq!(e.history_len(), 26);
}
