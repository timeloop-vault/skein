//! Alternate screen, every field of `Modes`, damage and split input (#793,
//! design T3).
#![allow(clippy::assert_is_empty)]

use super::dump::ColorDump;
use super::tests::{cell, feed, history, row_str, screen};
use super::{
    AlacrittyEngine, CursorShape, Damage, LineDamage, Modes, MouseEncoding, MouseTracking,
    TerminalEngine,
};

fn modes_after(seq: &str) -> Modes {
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, seq);
    e.modes()
}

#[test]
fn the_alternate_screen_starts_blank_at_the_main_cursor_and_leaves_the_main_screen_intact() {
    let mut e = AlacrittyEngine::new(3, 12);
    feed(&mut e, "h1\r\nh2\r\nm1\r\nm2\r\nmain");
    let (hist, main) = (history(&e), screen(&e));
    assert_eq!(hist, ["h1", "h2"]);
    let cursor = e.cursor();
    assert!(!e.modes().alt_screen);

    feed(&mut e, "\x1b[?1049h");
    let d = e.dump();
    assert!(d.alt_screen && e.modes().alt_screen);
    assert!(
        d.screen.iter().all(|r| row_str(r).is_empty()),
        "the alternate screen starts blank"
    );
    assert_eq!(
        e.cursor(),
        cursor,
        "it opens at the main screen's cursor position"
    );
    // OK: the alternate screen has its own content and no history.
    feed(&mut e, "\x1b[Halt");
    assert_eq!(screen(&e), ["alt", "", ""]);
    // SHORTFALL: while it is active the main screen and its history cannot be
    // read. The main grid is private to `Term`; its only public route,
    // `swap_alt`, wipes the alternate grid when swapping back. `history` is
    // empty and `hidden_screen` None until the screen is left.
    assert!(e.dump().history.is_empty() && e.dump().hidden_screen.is_none());
    assert_eq!(e.history_len(), 0);

    feed(&mut e, "\x1b[?1049l");
    assert!(!e.modes().alt_screen);
    assert_eq!(history(&e), hist);
    assert_eq!(screen(&e), main);
    assert_eq!(e.cursor(), cursor, "the main cursor is where it was");
    // Entering again clears what the previous visit left.
    feed(&mut e, "\x1b[?1049h");
    assert!(screen(&e).iter().all(String::is_empty));
}

#[test]
fn only_1049_switches_screens() {
    // SHORTFALL against xterm: `?47` and `?1047` are not recognised and
    // ignored; `?1048` (save cursor) too. The harnesses Skein drives use 1049.
    for seq in ["\x1b[?47h", "\x1b[?1047h", "\x1b[?1048h"] {
        assert!(!modes_after(seq).alt_screen, "{seq:?}");
    }
    assert!(modes_after("\x1b[?1049h").alt_screen);
    assert!(!modes_after("\x1b[?1049h\x1b[?1049l").alt_screen);
}

#[test]
fn modes_default_to_a_fresh_terminal() {
    let e = AlacrittyEngine::new(4, 20);
    assert_eq!(e.modes(), Modes::default());
    assert!(e.modes().cursor_visible);
}

#[test]
fn simple_toggles_follow_their_sequences() {
    assert!(modes_after("\x1b[?1h").app_cursor_keys);
    assert!(!modes_after("\x1b[?1h\x1b[?1l").app_cursor_keys);
    assert!(modes_after("\x1b=").app_keypad);
    assert!(!modes_after("\x1b=\x1b>").app_keypad);
    assert!(modes_after("\x1b[?2004h").bracketed_paste);
    assert!(!modes_after("\x1b[?2004h\x1b[?2004l").bracketed_paste);
    assert!(modes_after("\x1b[?1004h").focus_reporting);
    assert!(!modes_after("\x1b[?1004h\x1b[?1004l").focus_reporting);
    assert!(!modes_after("\x1b[?25l").cursor_visible);
    assert!(modes_after("\x1b[?25l\x1b[?25h").cursor_visible);
    // Several modes in one sequence.
    let m = modes_after("\x1b[?1;2004;1004h");
    assert!(m.app_cursor_keys && m.bracketed_paste && m.focus_reporting);
    // Unknown modes are ignored.
    assert_eq!(
        modes_after("\x1b[?9999h\x1b[?9h\x1b[?1015h"),
        Modes::default()
    );
}

#[test]
fn mouse_tracking_levels_replace_each_other() {
    assert_eq!(
        modes_after("\x1b[?1000h").mouse_tracking,
        MouseTracking::Click
    );
    assert_eq!(
        modes_after("\x1b[?1002h").mouse_tracking,
        MouseTracking::Drag
    );
    assert_eq!(
        modes_after("\x1b[?1003h").mouse_tracking,
        MouseTracking::Motion
    );
    assert_eq!(
        modes_after("\x1b[?1000l").mouse_tracking,
        MouseTracking::Off
    );
    // The three are mutually exclusive: the latest wins, and turning the
    // latest off leaves tracking OFF, where xterm keeps the earlier levels.
    assert_eq!(
        modes_after("\x1b[?1000h\x1b[?1003h").mouse_tracking,
        MouseTracking::Motion
    );
    assert_eq!(
        modes_after("\x1b[?1000h\x1b[?1003h\x1b[?1003l").mouse_tracking,
        MouseTracking::Off
    );
    // Turning off a level that is not the current one changes nothing.
    assert_eq!(
        modes_after("\x1b[?1003h\x1b[?1000l").mouse_tracking,
        MouseTracking::Motion
    );
}

#[test]
fn mouse_encodings_replace_each_other() {
    assert_eq!(modes_after("").mouse_encoding, MouseEncoding::Default);
    assert_eq!(
        modes_after("\x1b[?1006h").mouse_encoding,
        MouseEncoding::Sgr
    );
    assert_eq!(
        modes_after("\x1b[?1005h").mouse_encoding,
        MouseEncoding::Utf8
    );
    assert_eq!(
        modes_after("\x1b[?1005h\x1b[?1006h").mouse_encoding,
        MouseEncoding::Sgr
    );
    assert_eq!(
        modes_after("\x1b[?1006h\x1b[?1005h").mouse_encoding,
        MouseEncoding::Utf8
    );
    assert_eq!(
        modes_after("\x1b[?1006h\x1b[?1006l").mouse_encoding,
        MouseEncoding::Default
    );
    // Encoding and tracking are independent.
    let m = modes_after("\x1b[?1002h\x1b[?1006h");
    assert_eq!(
        (m.mouse_tracking, m.mouse_encoding),
        (MouseTracking::Drag, MouseEncoding::Sgr)
    );
}

#[test]
fn cursor_shape_and_blink() {
    assert_eq!(modes_after("\x1b[5 q").cursor_shape, CursorShape::Bar);
    assert!(modes_after("\x1b[5 q").cursor_blinking);
    assert_eq!(modes_after("\x1b[6 q").cursor_shape, CursorShape::Bar);
    assert!(!modes_after("\x1b[6 q").cursor_blinking);
    assert_eq!(modes_after("\x1b[4 q").cursor_shape, CursorShape::Underline);
    assert_eq!(modes_after("\x1b[2 q").cursor_shape, CursorShape::Block);
    assert_eq!(
        modes_after("\x1b[5 q\x1b[0 q").cursor_shape,
        CursorShape::Block
    );
    assert!(modes_after("\x1b[?12h").cursor_blinking);
    assert!(!modes_after("\x1b[?12h\x1b[?12l").cursor_blinking);
}

#[test]
fn kitty_keyboard_flags_follow_push_pop_and_set() {
    let flags = |s: &str| modes_after(s).kitty_keyboard_flags;
    assert_eq!(flags(""), 0);
    assert_eq!(flags("\x1b[>1u"), 1, "push disambiguate");
    assert_eq!(flags("\x1b[>1u\x1b[>5u"), 5, "a push replaces the top");
    assert_eq!(flags("\x1b[>1u\x1b[>5u\x1b[<u"), 1, "pop one");
    assert_eq!(flags("\x1b[>1u\x1b[>5u\x1b[<2u"), 0, "pop two");
    assert_eq!(
        flags("\x1b[>1u\x1b[<9u"),
        0,
        "popping more than pushed is harmless"
    );
    assert_eq!(flags("\x1b[=3u"), 3, "set replaces");
    assert_eq!(flags("\x1b[=3u\x1b[=8;2u"), 11, "set with mode 2 adds bits");
    assert_eq!(
        flags("\x1b[=3u\x1b[=1;3u"),
        2,
        "set with mode 3 clears bits"
    );
    assert_eq!(flags("\x1b[>31u"), 31, "all five flags");
    // The stack is per screen: the alternate screen starts with none.
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, "\x1b[>1u\x1b[?1049h");
    assert_eq!(e.modes().kitty_keyboard_flags, 0);
    feed(&mut e, "\x1b[>8u");
    assert_eq!(e.modes().kitty_keyboard_flags, 8);
    feed(&mut e, "\x1b[?1049l");
    assert_eq!(e.modes().kitty_keyboard_flags, 1);
}

#[test]
fn a_reset_clears_every_mode() {
    let m = modes_after(
        "\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?1003h\x1b[?1006h\x1b[?25l\x1b[>5u\x1b[5 q\x1bc",
    );
    assert_eq!(m, Modes::default());
    let m = modes_after("\x1b[?1049h\x1b[?1h\x1bc");
    assert_eq!(m, Modes::default(), "RIS leaves the alternate screen too");
}

#[test]
fn damage_reports_changed_lines_with_column_bounds_and_resets_on_read() {
    let mut e = AlacrittyEngine::new(4, 20);
    assert_eq!(
        e.take_damage(),
        Damage::Full,
        "everything is damaged at creation"
    );
    // After a read the next one lists only the cursor's own cell: reading
    // always damages the cursor.
    assert_eq!(
        e.take_damage(),
        Damage::Lines(vec![LineDamage {
            line: 0,
            left: 0,
            right: 0
        }])
    );
    feed(&mut e, "\x1b[3;5Hab");
    assert_eq!(
        e.take_damage(),
        Damage::Lines(vec![
            LineDamage {
                line: 0,
                left: 0,
                right: 0
            },
            LineDamage {
                line: 2,
                left: 4,
                right: 6
            }
        ]),
        "per line, inclusive columns; the old and the new cursor cell are included"
    );
    feed(&mut e, "\x1b[1;1H\x1b[2K");
    // An erased line is damaged across its whole width.
    let Damage::Lines(lines) = e.take_damage() else {
        panic!("expected lines")
    };
    assert!(
        lines.contains(&LineDamage {
            line: 0,
            left: 0,
            right: 19
        }),
        "{lines:?}"
    );
    // Nothing is carried over between reads.
    let Damage::Lines(lines) = e.take_damage() else {
        panic!("expected lines")
    };
    assert_eq!(lines.len(), 1);
}

#[test]
fn damage_goes_full_on_scroll_resize_and_screen_switch() {
    let mut e = AlacrittyEngine::new(3, 10);
    let _ = e.take_damage();
    feed(&mut e, "\x1b[3;1H\n");
    assert_eq!(e.take_damage(), Damage::Full, "a scroll");
    let _ = e.take_damage();
    e.resize(3, 12);
    assert_eq!(e.take_damage(), Damage::Full, "a resize");
    let _ = e.take_damage();
    feed(&mut e, "\x1b[?1049h");
    assert_eq!(e.take_damage(), Damage::Full, "a screen switch");
}

#[test]
fn damage_is_not_a_sync_signal() {
    // While a synchronized update is open nothing reaches the grid, so there
    // is no damage either: damage can be read as is, with no extra gate.
    let mut e = AlacrittyEngine::new(3, 10);
    let _ = e.take_damage();
    let _ = e.take_damage();
    feed(&mut e, "\x1b[?2026hxyz");
    let Damage::Lines(l) = e.take_damage() else {
        panic!("expected lines")
    };
    assert_eq!(
        l,
        [LineDamage {
            line: 0,
            left: 0,
            right: 0
        }]
    );
}

#[test]
fn an_escape_sequence_split_across_feeds_parses_and_reads_in_between_are_harmless() {
    let mut e = AlacrittyEngine::new(3, 20);
    for split in 1.."\x1b[1;38;2;10;20;30mX".len() {
        let seq = "\x1b[1;38;2;10;20;30mX".as_bytes();
        let mut e = AlacrittyEngine::new(3, 20);
        feed(&mut e, "a");
        e.feed(&seq[..split]);
        // Every kind of read, mid-sequence.
        let _ = (
            e.dump(),
            e.screen_text(),
            e.modes(),
            e.cursor(),
            e.take_damage(),
        );
        e.feed(&seq[split..]);
        let x = cell(&e, 0, 1);
        assert_eq!(x.text, "X", "split at {split}");
        assert!(x.bold);
        assert_eq!(
            x.fg,
            ColorDump::Rgb {
                r: 10,
                g: 20,
                b: 30
            },
            "split at {split}"
        );
    }
    // An OSC 8 and a mode switch split the same way.
    e.feed(b"\x1b]8;;http://sp");
    let _ = e.dump();
    e.feed(b"lit\x07L\x1b[?20");
    let _ = e.modes();
    e.feed(b"04h");
    assert_eq!(cell(&e, 0, 0).link.as_deref(), Some("http://split"));
    assert!(e.modes().bracketed_paste);
}

#[test]
fn a_utf8_character_split_across_feeds_is_not_corrupted() {
    for ch in ["\u{e9}", "\u{4e2d}", "\u{1f600}"] {
        let bytes = ch.as_bytes();
        for split in 1..bytes.len() {
            let mut e = AlacrittyEngine::new(2, 10);
            e.feed(b"a");
            e.feed(&bytes[..split]);
            let mid = (e.dump(), e.screen_text(), e.modes(), e.cursor());
            assert_eq!(mid.1[0], "a", "an unfinished character shows nothing yet");
            e.feed(&bytes[split..]);
            e.feed(b"b");
            assert_eq!(
                e.screen_text()[0],
                format!("a{ch}b"),
                "{ch:?} split at {split}"
            );
        }
    }
    // Invalid UTF-8 never panics.
    let mut e = AlacrittyEngine::new(2, 10);
    e.feed(&[0xff, 0xfe, b'o', b'k', 0xc3]);
    assert!(e.screen_text()[0].ends_with("ok"));
    assert_eq!(e.rebuilds(), 0);
}
