//! `HubDriver` and `take_valid_utf8`, and the whole path from a channel to a
//! viewer through the scheduler.

#![allow(clippy::assert_is_empty)]

use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::HubDriver;
use super::driver::take_valid_utf8;
use super::scheduler::{FrameDriver, run_frame_scheduler};
use super::test_support::{Fx, Obs};

fn driver() -> (Fx, Arc<Obs>) {
    let obs = Arc::new(Obs::default());
    (Fx::observed(&obs), obs)
}

#[test]
fn a_cut_hands_the_observer_the_text_fed_since_the_last_cut_once() {
    let (fx, obs) = driver();
    let mut d = HubDriver::new(Arc::new(fx.hub));
    d.feed(b"hel");
    d.feed(b"lo");
    d.cut();
    assert_eq!(*obs.outputs.lock(), ["hello"]);
    d.cut();
    assert_eq!(obs.outputs.lock().len(), 1, "nothing new, nothing told");
    d.feed(b"!");
    d.cut();
    assert_eq!(*obs.outputs.lock(), ["hello", "!"]);
}

#[test]
fn a_character_split_across_two_feeds_arrives_intact_after_the_second() {
    let (fx, obs) = driver();
    let mut d = HubDriver::new(Arc::new(fx.hub));
    let euro = "\u{20ac}".as_bytes();
    d.feed(&euro[..2]);
    d.cut();
    assert!(obs.outputs.lock().is_empty(), "an incomplete tail waits");
    d.feed(&euro[2..]);
    d.cut();
    assert_eq!(*obs.outputs.lock(), ["\u{20ac}"]);
}

#[test]
fn invalid_bytes_are_dropped_before_the_observer_hears_the_text() {
    let (fx, obs) = driver();
    let mut d = HubDriver::new(Arc::new(fx.hub));
    d.feed(b"a\xffb");
    d.cut();
    assert_eq!(*obs.outputs.lock(), ["ab"]);
}

#[test]
fn the_driver_passes_the_sync_deadline_and_flush_to_the_hub() {
    let (fx, _) = driver();
    let hub = Arc::new(fx.hub);
    let mut d = HubDriver::new(Arc::clone(&hub));
    assert!(d.sync_deadline().is_none());
    d.feed(b"\x1b[?2026h");
    assert!(d.sync_deadline().is_some());
    d.flush_sync();
    assert!(d.sync_deadline().is_none());
}

#[test]
fn take_valid_utf8_takes_complete_text_and_empties_the_buffer() {
    let mut buf = "h\u{e9}llo".as_bytes().to_vec();
    assert_eq!(take_valid_utf8(&mut buf), "h\u{e9}llo");
    assert!(buf.is_empty());
}

#[test]
fn take_valid_utf8_keeps_an_incomplete_tail() {
    let mut buf = b"ok\xe2\x82".to_vec();
    assert_eq!(take_valid_utf8(&mut buf), "ok");
    assert_eq!(buf, b"\xe2\x82");
    buf.push(0xac);
    assert_eq!(take_valid_utf8(&mut buf), "\u{20ac}");
    assert!(buf.is_empty());
}

#[test]
fn take_valid_utf8_drops_certainly_invalid_bytes() {
    let mut buf = b"a\xffb".to_vec();
    assert_eq!(take_valid_utf8(&mut buf), "ab");
    assert!(buf.is_empty());
}

#[test]
fn take_valid_utf8_drops_several_invalid_runs_and_keeps_a_trailing_tail() {
    let mut buf = b"a\xff\xfeb\xc0c\xe2".to_vec();
    assert_eq!(take_valid_utf8(&mut buf), "abc");
    assert_eq!(buf, b"\xe2");
}

#[test]
fn take_valid_utf8_of_nothing_is_nothing() {
    let mut buf = Vec::new();
    assert_eq!(take_valid_utf8(&mut buf), "");
}

#[test]
fn chunks_through_the_scheduler_end_with_the_fed_text_on_the_viewers_screen() {
    let (fx, obs) = driver();
    let (_, viewer) = fx.viewer(None);
    let hub = Arc::new(fx.hub);
    let mut d = HubDriver::new(Arc::clone(&hub));
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    for chunk in ["hello ", "wor", "ld"] {
        tx.send(chunk.as_bytes().to_vec()).unwrap();
    }
    drop(tx);
    run_frame_scheduler(&rx, Duration::from_millis(20), &mut d);

    let frames = viewer.datas();
    assert!(
        frames.len() >= 2,
        "the attach redraw and at least one frame"
    );
    let text: String = frames.concat();
    assert!(
        text.contains("world"),
        "frames carried the output: {text:?}"
    );
    assert_eq!(obs.outputs.lock().concat(), "hello world");

    // The screen the engine now holds is what a late viewer is shown.
    let rec = super::test_support::Rec::default();
    hub.attach(Box::new(rec.clone()), None);
    assert!(rec.datas()[0].contains("hello world"));
}
