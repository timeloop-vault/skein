//! Who sets the PTY's size, and what a size change tells the viewers.

#![allow(clippy::assert_is_empty)]

use std::sync::atomic::Ordering;

use super::test_support::{FULL_REDRAW, Fx};

#[test]
fn the_first_viewer_is_the_size_owner() {
    let fx = Fx::new();
    assert_eq!(fx.hub.size_owner(), None);
    let (a, _) = fx.viewer(None);
    let (_b, _) = fx.viewer(None);
    assert_eq!(fx.hub.size_owner(), Some(a));
}

#[test]
fn the_owner_resizing_reaches_the_pty_and_every_viewer() {
    let fx = Fx::new();
    let (a, rec_a) = fx.viewer(None);
    let (_, rec_b) = fx.viewer(None);
    assert!(fx.hub.resize(Some(a), 30, 100));
    assert_eq!(fx.resize_calls(), [(30, 100)]);
    assert_eq!(fx.hub.grid_size(), (30, 100));
    for rec in [&rec_a, &rec_b] {
        assert_eq!(rec.kinds(), ["grid", "data", "grid", "data"]);
        assert_eq!(rec.grids(), ["24x80", "30x100"]);
        assert!(rec.datas()[1].starts_with(FULL_REDRAW));
    }
}

#[test]
fn a_non_owner_resize_is_recorded_but_not_applied() {
    let fx = Fx::new();
    let (_a, _) = fx.viewer(None);
    let (b, rec_b) = fx.viewer(None);
    assert!(!fx.hub.resize(Some(b), 40, 120));
    assert!(fx.resize_calls().is_empty());
    assert_eq!(fx.hub.grid_size(), (24, 80));
    assert_eq!(rec_b.kinds(), ["grid", "data"]);
    // Its recorded size is what it gets when it becomes the owner.
    fx.hub.active(b);
    assert_eq!(fx.resize_calls(), [(40, 120)]);
    assert_eq!(fx.hub.grid_size(), (40, 120));
}

#[test]
fn a_resize_to_the_current_grid_does_not_touch_the_pty() {
    let fx = Fx::new();
    let (a, rec) = fx.viewer(None);
    assert!(!fx.hub.resize(Some(a), 24, 80));
    assert!(!fx.hub.resize(None, 24, 80));
    assert!(fx.resize_calls().is_empty());
    assert_eq!(rec.kinds(), ["grid", "data"]);
}

#[test]
fn a_resize_without_a_viewer_applies_when_different_and_keeps_the_owner() {
    let fx = Fx::new();
    let (a, rec) = fx.viewer(None);
    assert!(fx.hub.resize(None, 50, 132));
    assert_eq!(fx.hub.size_owner(), Some(a));
    assert_eq!(fx.resize_calls(), [(50, 132)]);
    assert_eq!(rec.grids(), ["24x80", "50x132"]);
}

#[test]
fn a_resize_for_an_unknown_viewer_changes_nothing() {
    let fx = Fx::new();
    let (_, rec) = fx.viewer(None);
    let before = rec.events();
    assert!(!fx.hub.resize(Some(77), 30, 100));
    assert!(fx.resize_calls().is_empty());
    assert_eq!(fx.hub.grid_size(), (24, 80));
    assert_eq!(rec.events(), before);
}

#[test]
fn a_zero_dimension_is_refused() {
    let fx = Fx::new();
    let (a, rec) = fx.viewer(None);
    assert!(!fx.hub.resize(Some(a), 0, 100));
    assert!(!fx.hub.resize(Some(a), 30, 0));
    assert!(
        !fx.hub.resize(Some(a), 30, 1),
        "below the engine's 2 columns"
    );
    assert!(!fx.hub.resize(None, 0, 0));
    assert!(fx.resize_calls().is_empty(), "the PTY is never asked");
    assert_eq!(fx.hub.grid_size(), (24, 80));
    assert_eq!(rec.kinds(), ["grid", "data"]);
}

#[test]
fn a_pty_that_refuses_the_size_leaves_everything_as_it_was() {
    let fx = Fx::new();
    let (a, rec) = fx.viewer(None);
    fx.refuse.store(true, Ordering::SeqCst);
    assert!(!fx.hub.resize(Some(a), 30, 100));
    assert_eq!(fx.resize_calls(), [(30, 100)], "it was asked once");
    assert_eq!(fx.hub.grid_size(), (24, 80));
    assert_eq!(rec.kinds(), ["grid", "data"], "no events");
    // The engine is unchanged: a later attach still gets the old grid.
    let (_, late) = fx.viewer(None);
    assert_eq!(late.grids(), ["24x80"]);
}

#[test]
fn attaching_with_a_different_size_and_no_owner_resizes_once() {
    let fx = Fx::new();
    let (a, rec) = fx.viewer(Some((30, 100)));
    assert_eq!(fx.hub.size_owner(), Some(a));
    assert_eq!(fx.resize_calls(), [(30, 100)]);
    assert_eq!(rec.kinds(), ["grid", "data"]);
    assert_eq!(rec.grids(), ["30x100"]);
}

#[test]
fn attaching_with_the_same_size_does_not_resize() {
    let fx = Fx::new();
    let (_, rec) = fx.viewer(Some((24, 80)));
    assert!(fx.resize_calls().is_empty());
    assert_eq!(rec.kinds(), ["grid", "data"]);
}

#[test]
fn attaching_with_a_size_while_there_is_an_owner_only_records_it() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    let (_, rec_b) = fx.viewer(Some((40, 120)));
    assert_eq!(fx.hub.size_owner(), Some(a));
    assert!(fx.resize_calls().is_empty());
    assert_eq!(rec_b.grids(), ["24x80"]);
}

#[test]
fn becoming_active_makes_the_viewer_the_owner_and_applies_its_size() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    let (b, rec_b) = fx.viewer(Some((40, 120)));
    fx.hub.active(b);
    assert_eq!(fx.hub.size_owner(), Some(b));
    assert_eq!(fx.resize_calls(), [(40, 120)]);
    assert_eq!(rec_b.grids(), ["24x80", "40x120"]);
    // The old owner no longer drives the PTY.
    assert!(!fx.hub.resize(Some(a), 10, 20));
    assert_eq!(fx.resize_calls().len(), 1);
}

#[test]
fn becoming_active_with_an_equal_size_does_not_resize() {
    let fx = Fx::new();
    let (_a, _) = fx.viewer(None);
    let (b, _) = fx.viewer(Some((24, 80)));
    fx.hub.active(b);
    assert_eq!(fx.hub.size_owner(), Some(b));
    assert!(fx.resize_calls().is_empty());
}

#[test]
fn an_unknown_viewer_becoming_active_changes_nothing() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    fx.hub.active(55);
    assert_eq!(fx.hub.size_owner(), Some(a));
}

#[test]
fn when_the_owner_detaches_the_most_recently_active_viewer_takes_over() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    let (b, rec_b) = fx.viewer(Some((30, 100)));
    let (c, _) = fx.viewer(Some((40, 120)));
    fx.hub.active(c);
    fx.hub.active(b);
    fx.hub.active(a);
    assert!(fx.hub.resize(None, 50, 132));
    assert!(fx.hub.detach_if(a));
    assert_eq!(fx.hub.size_owner(), Some(b), "b was active after c");
    assert_eq!(
        fx.hub.grid_size(),
        (30, 100),
        "b's recorded size is applied"
    );
    assert_eq!(rec_b.grids().last().map(String::as_str), Some("30x100"));
}

#[test]
fn a_dead_owner_hands_over_to_the_remaining_viewer() {
    let fx = Fx::new();
    let (a, rec_a) = fx.viewer(None);
    let (b, rec_b) = fx.viewer(Some((30, 100)));
    assert_eq!(fx.hub.size_owner(), Some(a));
    rec_a.fail_from_now();
    fx.hub.feed(b"x");
    fx.hub.cut();
    assert_eq!(fx.hub.viewer_count(), 1);
    assert_eq!(fx.hub.size_owner(), Some(b));
    assert_eq!(fx.hub.grid_size(), (30, 100));
    assert_eq!(rec_b.grids(), ["24x80", "30x100"]);
}

#[test]
fn detaching_every_viewer_leaves_no_owner_and_the_grid_alone() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    fx.hub.resize(Some(a), 30, 100);
    let (_b, _) = fx.viewer(None);
    assert!(fx.hub.detach_all());
    assert!(!fx.hub.detach_all(), "nothing left to detach");
    assert_eq!(fx.hub.size_owner(), None);
    assert_eq!(fx.hub.viewer_count(), 0);
    assert_eq!(fx.hub.latest_viewer(), None);
    assert_eq!(fx.hub.grid_size(), (30, 100));
    assert_eq!(fx.resize_calls(), [(30, 100)]);
}

#[test]
fn detaching_an_unknown_viewer_is_a_no_op() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    assert!(!fx.hub.detach_if(9));
    assert_eq!(fx.hub.size_owner(), Some(a));
}

#[test]
fn after_exit_sizes_are_recorded_but_never_applied() {
    let fx = Fx::new();
    let (a, _) = fx.viewer(None);
    fx.hub.exit(Some(0));
    let (b, rec_b) = fx.viewer(Some((40, 120)));
    assert_eq!(fx.hub.size_owner(), Some(a));
    assert!(!fx.hub.resize(Some(a), 30, 100));
    fx.hub.active(b);
    assert!(fx.resize_calls().is_empty(), "no resize of a dead child");
    assert_eq!(fx.hub.grid_size(), (24, 80));
    assert_eq!(rec_b.grids(), ["24x80"]);
}
