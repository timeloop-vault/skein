//! The hub's viewers, frames, replies and exit, on a fake PTY.

#![allow(clippy::assert_is_empty)]

use std::sync::Arc;

use super::hub::FrameHub;
use super::test_support::{Dead, ERASE_DISPLAY, FULL_REDRAW, Fx, Obs, Rec};
use crate::pty::sink_slot::{AttachOutcome, ExitRecord};

#[test]
fn two_viewers_get_a_grid_then_a_redraw_then_identical_frames() {
    let fx = Fx::new();
    let (_, a) = fx.viewer(None);
    let (_, b) = fx.viewer(None);
    assert_eq!(a.kinds(), ["grid", "data"]);
    assert_eq!(b.kinds(), ["grid", "data"]);
    assert_eq!(a.grids(), ["24x80"]);
    let (a0, b0) = (a.events().len(), b.events().len());

    fx.hub.feed(b"hello");
    fx.hub.cut();
    let (fa, fb) = (a.datas().split_off(1), b.datas().split_off(1));
    assert_eq!(fa.len(), 1);
    assert_eq!(fa, fb, "both viewers get byte-identical frames");
    assert!(fa[0].contains("hello"));

    let (id_a, id_b) = (1, 2);
    assert!(fx.hub.detach_if(id_a));
    fx.hub.feed(b" more");
    fx.hub.cut();
    assert_eq!(
        a.events().len(),
        a0 + 1,
        "the detached viewer hears nothing"
    );
    assert_eq!(b.events().len(), b0 + 2, "the other keeps being fed");
    assert_eq!(fx.hub.viewer_count(), 1);
    assert_eq!(fx.hub.latest_viewer(), Some(id_b));
}

#[test]
fn a_viewer_attached_after_output_sees_it_in_its_first_frame() {
    let fx = Fx::new();
    fx.hub.feed(b"already there");
    fx.hub.cut();
    let (_, late) = fx.viewer(None);
    assert_eq!(late.kinds(), ["grid", "data"]);
    assert!(late.datas()[0].contains("already there"));
}

#[test]
fn viewer_ids_start_at_one_and_increase() {
    let fx = Fx::new();
    let ids: Vec<u64> = (0..3).map(|_| fx.viewer(None).0).collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(fx.hub.latest_viewer(), Some(3));
    fx.hub.detach_if(3);
    assert_eq!(fx.viewer(None).0, 4, "ids are never reused");
}

#[test]
fn attach_is_a_full_redraw() {
    let fx = Fx::new();
    let (_, v) = fx.viewer(None);
    let first = &v.datas()[0];
    assert!(first.starts_with(FULL_REDRAW));
}

#[test]
fn a_cut_with_nothing_changed_sends_nothing() {
    let fx = Fx::new();
    let (_, v) = fx.viewer(None);
    let before = v.events();
    fx.hub.cut();
    assert_eq!(v.events(), before);
    fx.hub.feed(b"x");
    fx.hub.cut();
    let after = v.events().len();
    fx.hub.cut();
    assert_eq!(v.events().len(), after);
}

#[test]
fn an_open_synchronized_update_holds_the_frame_until_it_is_flushed() {
    let fx = Fx::new();
    let (_, v) = fx.viewer(None);
    let before = v.events().len();
    assert!(fx.hub.sync_deadline().is_none());
    fx.hub.feed(b"\x1b[?2026hsynced");
    assert!(fx.hub.sync_deadline().is_some());
    fx.hub.cut();
    assert_eq!(v.events().len(), before, "an open update sends nothing");
    fx.hub.flush_sync();
    assert!(fx.hub.sync_deadline().is_none());
    fx.hub.cut();
    let frames = v.datas();
    assert_eq!(frames.len(), 2);
    assert!(frames[1].contains("synced"));
}

#[test]
fn a_cursor_query_is_answered_with_the_real_cursor_position() {
    let fx = Fx::new();
    fx.hub.feed(b"ab\x1b[6n");
    assert_eq!(*fx.replies.lock(), [b"\x1b[1;3R".to_vec()]);
}

#[test]
fn a_query_is_answered_with_no_viewer_attached() {
    let fx = Fx::new();
    assert_eq!(fx.hub.viewer_count(), 0);
    fx.hub.feed(b"\x1b[6n");
    assert_eq!(*fx.replies.lock(), [b"\x1b[1;1R".to_vec()]);
    fx.hub.feed(b"plain text");
    assert_eq!(fx.replies.lock().len(), 1, "no query, no reply");
}

#[test]
fn a_failing_sink_is_dropped_and_the_others_keep_being_fed() {
    let fx = Fx::new();
    let (_, healthy) = fx.viewer(None);
    let (_, doomed) = fx.viewer(None);
    doomed.fail_from_now();
    fx.hub.feed(b"one");
    fx.hub.cut();
    assert_eq!(fx.hub.viewer_count(), 1);
    fx.hub.feed(b"two");
    fx.hub.cut();
    assert!(healthy.datas().last().is_some_and(|d| d.contains("two")));
    assert_eq!(
        doomed.datas().len(),
        1,
        "the dropped sink gets nothing more"
    );
}

#[test]
fn a_sink_that_fails_at_attach_is_dropped_without_a_panic() {
    let fx = Fx::new();
    let (outcome, id) = fx.hub.attach(Box::new(Dead), None);
    assert_eq!(outcome, AttachOutcome::Live);
    assert_eq!(id, 1);
    assert_eq!(fx.hub.viewer_count(), 0);
    assert_eq!(fx.hub.size_owner(), None);
    let (_, ok) = fx.viewer(None);
    assert_eq!(ok.kinds(), ["grid", "data"]);
}

#[test]
fn a_redraw_request_gives_only_that_viewer_a_full_redraw() {
    let fx = Fx::new();
    let (a, rec_a) = fx.viewer(None);
    let (_, rec_b) = fx.viewer(None);
    fx.hub.feed(b"text");
    fx.hub.cut();
    let (na, nb) = (rec_a.events().len(), rec_b.events().len());
    assert!(fx.hub.request_redraw(a));
    assert_eq!(rec_b.events().len(), nb, "the other viewer hears nothing");
    let events = rec_a.events();
    assert_eq!(events.len(), na + 1);
    let redraw = rec_a.datas().pop().unwrap_or_default();
    assert!(redraw.starts_with(FULL_REDRAW));
    assert!(redraw.contains(ERASE_DISPLAY));
    assert!(redraw.contains("text"));
}

#[test]
fn a_redraw_request_for_an_unknown_viewer_is_refused() {
    let fx = Fx::new();
    let (_, v) = fx.viewer(None);
    let before = v.events();
    assert!(!fx.hub.request_redraw(99));
    assert_eq!(v.events(), before);
}

#[test]
fn exit_sends_pending_output_then_exit_and_tells_the_observer_once() {
    let obs = Arc::new(Obs::default());
    let fx = Fx::observed(&obs);
    let (_, v) = fx.viewer(None);
    fx.hub.feed(b"last words");
    fx.hub.exit(Some(3));
    fx.hub.exit(Some(3));
    let events = v.events();
    assert_eq!(v.kinds()[..3], ["grid", "data", "data"]);
    assert!(events[2].contains("last words"));
    assert_eq!(events[3], "exit:Some(3)");
    assert_eq!(*obs.exits.lock(), [Some(3)], "the observer hears it once");
    assert_eq!(fx.hub.exited(), Some(ExitRecord { code: Some(3) }));
}

#[test]
fn exited_is_none_while_the_child_runs() {
    let fx = Fx::new();
    assert_eq!(fx.hub.exited(), None);
}

#[test]
fn attaching_after_exit_gets_grid_redraw_exit_in_that_order() {
    let fx = Fx::new();
    fx.hub.feed(b"final screen");
    fx.hub.exit(None);
    let rec = Rec::default();
    let (outcome, _) = fx.hub.attach(Box::new(rec.clone()), None);
    assert_eq!(outcome, AttachOutcome::Exited { code: None });
    assert_eq!(rec.kinds(), ["grid", "data", "exit"]);
    assert!(rec.datas()[0].contains("final screen"));
}

#[test]
fn a_hub_built_without_an_observer_still_exits() {
    let hub = FrameHub::new(
        24,
        80,
        super::HubIo {
            reply: Box::new(|_| {}),
            resize_pty: Box::new(|_, _| Ok(())),
        },
    );
    hub.exit(Some(0));
    assert_eq!(hub.exited(), Some(ExitRecord { code: Some(0) }));
}
