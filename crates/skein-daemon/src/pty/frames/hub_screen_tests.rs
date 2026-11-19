//! The hub hands the observer the screen at a cut (#809): only when the
//! observer asks, never mid synchronized update, with the lock released.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::test_support::{Fx, Obs};

const READY: &str = "\x1b[24;1H ─ ready │ haiku 4.5 │ 1k/2k │ x";

fn wanting() -> (Arc<Obs>, Fx) {
    let obs = Arc::new(Obs::default());
    obs.wants.store(true, Ordering::SeqCst);
    let fx = Fx::observed(&obs);
    (obs, fx)
}

fn has_ready_row(rows: &[String]) -> bool {
    rows.iter().any(|r| r.contains("ready │ haiku"))
}

#[test]
fn a_cut_hands_a_wanting_observer_the_fed_screen() {
    let (obs, fx) = wanting();
    fx.hub.feed(READY.as_bytes());
    fx.hub.cut();
    let screens = obs.screens.lock();
    assert_eq!(screens.len(), 1);
    assert_eq!(screens[0].len(), 24);
    assert!(has_ready_row(&screens[0]), "{:?}", screens[0]);
}

#[test]
fn an_observer_that_does_not_ask_is_never_handed_a_screen() {
    let (obs, fx) = wanting();
    obs.wants.store(false, Ordering::SeqCst);
    fx.hub.feed(READY.as_bytes());
    fx.hub.cut();
    assert!(obs.screens.lock().is_empty());
}

#[test]
fn nothing_is_delivered_while_a_synchronized_update_is_open() {
    let (obs, fx) = wanting();
    fx.hub.feed(b"\x1b[?2026h");
    fx.hub.feed(READY.as_bytes());
    fx.hub.cut();
    assert!(obs.screens.lock().is_empty(), "half-drawn screen");
    fx.hub.flush_sync();
    fx.hub.cut();
    let screens = obs.screens.lock();
    assert_eq!(screens.len(), 1);
    assert!(has_ready_row(&screens[0]));
}

#[test]
fn a_hub_without_an_observer_cuts_as_before() {
    let fx = Fx::new();
    fx.hub.feed(READY.as_bytes());
    fx.hub.cut();
}
