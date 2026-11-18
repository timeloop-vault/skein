//! Mode mirrors: each on and off, and the transitions between levels.

use crate::pty::engine::{CursorShape, Modes, MouseEncoding, MouseTracking};

use super::ViewerState;
use super::test_grid::{TestGrid, body, synced};

/// The frame after `from` became `to`.
fn change(from: impl Fn(&mut Modes), to: impl Fn(&mut Modes)) -> String {
    let mut g = TestGrid::new(1, 3);
    from(&mut g.modes);
    let mut v = synced(&g);
    to(&mut g.modes);
    body(&g, &mut v).unwrap()
}

#[test]
fn application_cursor_keys() {
    assert_eq!(
        change(|_| {}, |m| m.app_cursor_keys = true),
        "^[[?1h^[[1;1H"
    );
    assert_eq!(
        change(|m| m.app_cursor_keys = true, |m| m.app_cursor_keys = false),
        "^[[?1l^[[1;1H"
    );
}

#[test]
fn application_keypad() {
    assert_eq!(change(|_| {}, |m| m.app_keypad = true), "^[=^[[1;1H");
    assert_eq!(
        change(|m| m.app_keypad = true, |m| m.app_keypad = false),
        "^[>^[[1;1H"
    );
}

#[test]
fn mouse_tracking_levels_replace_each_other() {
    let set = |t| move |m: &mut Modes| m.mouse_tracking = t;
    assert_eq!(
        change(|_| {}, set(MouseTracking::Click)),
        "^[[?1000h^[[1;1H"
    );
    assert_eq!(
        change(|_| {}, set(MouseTracking::Motion)),
        "^[[?1003h^[[1;1H"
    );
    assert_eq!(
        change(set(MouseTracking::Click), set(MouseTracking::Drag)),
        "^[[?1000l^[[?1002h^[[1;1H"
    );
    assert_eq!(
        change(set(MouseTracking::Drag), set(MouseTracking::Off)),
        "^[[?1000l^[[?1002l^[[?1003l^[[1;1H"
    );
}

#[test]
fn mouse_encoding() {
    let set = |e| move |m: &mut Modes| m.mouse_encoding = e;
    assert_eq!(change(|_| {}, set(MouseEncoding::Sgr)), "^[[?1006h^[[1;1H");
    assert_eq!(change(|_| {}, set(MouseEncoding::Utf8)), "^[[?1005h^[[1;1H");
    assert_eq!(
        change(set(MouseEncoding::Sgr), set(MouseEncoding::Utf8)),
        "^[[?1006l^[[?1005h^[[1;1H"
    );
    assert_eq!(
        change(set(MouseEncoding::Utf8), set(MouseEncoding::Default)),
        "^[[?1005l^[[1;1H"
    );
}

#[test]
fn bracketed_paste_and_focus_reporting() {
    assert_eq!(
        change(|_| {}, |m| m.bracketed_paste = true),
        "^[[?2004h^[[1;1H"
    );
    assert_eq!(
        change(|m| m.bracketed_paste = true, |m| m.bracketed_paste = false),
        "^[[?2004l^[[1;1H"
    );
    assert_eq!(
        change(|_| {}, |m| m.focus_reporting = true),
        "^[[?1004h^[[1;1H"
    );
    assert_eq!(
        change(|m| m.focus_reporting = true, |m| m.focus_reporting = false),
        "^[[?1004l^[[1;1H"
    );
}

#[test]
fn kitty_keyboard_flags() {
    assert_eq!(
        change(|_| {}, |m| m.kitty_keyboard_flags = 5),
        "^[[=5;1u^[[1;1H"
    );
    assert_eq!(
        change(
            |m| m.kitty_keyboard_flags = 5,
            |m| m.kitty_keyboard_flags = 0
        ),
        "^[[=0;1u^[[1;1H"
    );
}

#[test]
fn the_alternate_screen_switches_the_clients_buffer() {
    assert!(change(|_| {}, |m| m.alt_screen = true).starts_with("^[[?1049h"));
    assert!(change(|m| m.alt_screen = true, |m| m.alt_screen = false).starts_with("^[[?1049l"));
}

#[test]
fn cursor_visibility_and_shape_follow_the_cursor_position() {
    assert_eq!(
        change(|_| {}, |m| m.cursor_visible = false),
        "^[[1;1H^[[?25l"
    );
    assert_eq!(
        change(|m| m.cursor_visible = false, |m| m.cursor_visible = true),
        "^[[1;1H^[[?25h"
    );
    let shape = |s, blink| {
        move |m: &mut Modes| {
            m.cursor_shape = s;
            m.cursor_blinking = blink;
        }
    };
    assert_eq!(
        change(|_| {}, shape(CursorShape::Bar, true)),
        "^[[1;1H^[[5 q"
    );
    assert_eq!(
        change(|_| {}, shape(CursorShape::Underline, false)),
        "^[[1;1H^[[4 q"
    );
    assert_eq!(
        change(|_| {}, shape(CursorShape::Block, true)),
        "^[[1;1H^[[1 q"
    );
}

#[test]
fn a_viewer_that_missed_frames_gets_every_difference_at_once() {
    let mut g = TestGrid::new(1, 3);
    let mut v = synced(&g);
    g.modes.bracketed_paste = true;
    g.modes.bracketed_paste = false; // on and off again: nothing to send
    assert!(body(&g, &mut v).is_none());
    g.modes.app_cursor_keys = true;
    g.modes.focus_reporting = true;
    assert_eq!(body(&g, &mut v).unwrap(), "^[[?1h^[[?1004h^[[1;1H");
}

#[test]
fn a_full_redraw_resets_what_the_client_may_still_hold() {
    let g = TestGrid::new(1, 3);
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    for reset in [
        "^[[?1l",
        "^[>",
        "^[[?1000l",
        "^[[?1002l",
        "^[[?1003l",
        "^[[?1005l",
        "^[[?1006l",
        "^[[?2004l",
        "^[[?1004l",
        "^[[=0;1u",
        "^[[?1049l",
        "^[[?25h",
    ] {
        assert!(f.contains(reset), "{reset} missing in {f}");
    }
}
