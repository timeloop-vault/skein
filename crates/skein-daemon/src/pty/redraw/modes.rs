//! Mode mirrors: set and reset sequences that make the client encode input
//! the way the application expects, and the cursor's look.
//!
//! Each function sends what differs between `prev` (what the viewer was
//! last sent) and `now`, or everything with `all` (a full redraw: the client
//! may hold any state). Modes are state, not events, so a viewer that skipped
//! frames still ends right.

use std::io::Write;

use crate::pty::engine::{CursorShape, Modes, MouseEncoding, MouseTracking};

fn private(out: &mut Vec<u8>, mode: u16, on: bool) {
    let _ = write!(out, "\x1b[?{mode}{}", if on { 'h' } else { 'l' });
}

/// `?1049`, the alternate screen. Its full semantics (what is repainted on
/// entry and exit) are the redraw's, see `encode.rs`.
pub(super) fn alt_screen(out: &mut Vec<u8>, on: bool) {
    private(out, 1049, on);
}

fn tracking_mode(t: MouseTracking) -> Option<u16> {
    match t {
        MouseTracking::Off => None,
        MouseTracking::Click => Some(1000),
        MouseTracking::Drag => Some(1002),
        MouseTracking::Motion => Some(1003),
    }
}

fn reset_tracking(out: &mut Vec<u8>) {
    for m in [1000, 1002, 1003] {
        private(out, m, false);
    }
}

fn encoding_mode(e: MouseEncoding) -> Option<u16> {
    match e {
        MouseEncoding::Default => None,
        MouseEncoding::Utf8 => Some(1005),
        MouseEncoding::Sgr => Some(1006),
    }
}

/// Everything except the alternate screen, the cursor and the title.
pub(super) fn mirror(out: &mut Vec<u8>, prev: &Modes, now: &Modes, all: bool) {
    if all || prev.app_cursor_keys != now.app_cursor_keys {
        private(out, 1, now.app_cursor_keys);
    }
    if all || prev.app_keypad != now.app_keypad {
        out.extend_from_slice(if now.app_keypad { b"\x1b=" } else { b"\x1b>" });
    }
    if all || prev.mouse_tracking != now.mouse_tracking {
        // The client keeps the three levels independent, the engine keeps one.
        match tracking_mode(now.mouse_tracking) {
            None => reset_tracking(out),
            Some(level) => {
                if all {
                    reset_tracking(out);
                } else if let Some(old) = tracking_mode(prev.mouse_tracking) {
                    private(out, old, false);
                }
                private(out, level, true);
            }
        }
    }
    if all || prev.mouse_encoding != now.mouse_encoding {
        if all {
            for m in [1005, 1006] {
                private(out, m, false);
            }
        } else if let Some(old) = encoding_mode(prev.mouse_encoding) {
            private(out, old, false);
        }
        if let Some(new) = encoding_mode(now.mouse_encoding) {
            private(out, new, true);
        }
    }
    if all || prev.bracketed_paste != now.bracketed_paste {
        private(out, 2004, now.bracketed_paste);
    }
    if all || prev.focus_reporting != now.focus_reporting {
        private(out, 1004, now.focus_reporting);
    }
    if all || prev.kitty_keyboard_flags != now.kitty_keyboard_flags {
        // `= flags ; 1` sets exactly these flags.
        let _ = write!(out, "\x1b[={};1u", now.kitty_keyboard_flags);
    }
}

/// Cursor shape (`DECSCUSR`, `CSI n SP q`) and visibility (`?25`).
pub(super) fn cursor_look(out: &mut Vec<u8>, prev: &Modes, now: &Modes, all: bool) {
    if all || prev.cursor_shape != now.cursor_shape || prev.cursor_blinking != now.cursor_blinking {
        let base = match now.cursor_shape {
            CursorShape::Block => 1,
            CursorShape::Underline => 3,
            CursorShape::Bar => 5,
        };
        let n = if now.cursor_blinking { base } else { base + 1 };
        let _ = write!(out, "\x1b[{n} q");
    }
    if all || prev.cursor_visible != now.cursor_visible {
        private(out, 25, now.cursor_visible);
    }
}
