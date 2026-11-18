//! The frame as a whole: full redraw, diffs, SGR, links, title, bell.
//!
//! Frames are compared as text: ESC is `^[`, BEL is `^G`, and the string
//! terminator `ESC \` is `^[\`.

use crate::pty::engine::dump::ColorDump;

use super::test_grid::{CLOSE, OPEN, TestGrid, body, show, synced};
use super::{ViewerState, encode_frame};

/// Every mode mirror at its default, as a full redraw sends them.
const ALL_MODES: &str = "^[[?1l^[>^[[?1000l^[[?1002l^[[?1003l^[[?1005l^[[?1006l\
                         ^[[?2004l^[[?1004l^[[=0;1u";
const TAIL: &str = "^[[2 q^[[?25h";

#[test]
fn fresh_viewer_gets_a_full_redraw_with_exact_bytes() {
    let mut g = TestGrid::new(3, 10).text(0, "hi");
    g.cursor = (0, 2);
    let mut v = ViewerState::new();
    assert!(v.is_fresh());
    let f = show(&encode_frame(&g, &mut v).unwrap());
    let expected = format!(
        "{OPEN}^[[?1049l^[[H^[[2J^[[3J^[[1;1H^[[Khi^[]2;^[\\{ALL_MODES}^[[1;3H{TAIL}{CLOSE}"
    );
    assert_eq!(f, expected);
    assert!(!v.is_fresh());
}

#[test]
fn full_redraw_writes_nonblank_rows_only() {
    let mut g = TestGrid::new(3, 4).text(1, "ab");
    g.cell(2, 0).text = "z".into();
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[2;1H^[[Kab^[[3;1H^[[Kz"), "{f}");
    assert!(!f.contains("^[[1;1H^[[K"), "{f}");
}

#[test]
fn no_change_is_none_and_leaves_the_state() {
    let g = TestGrid::new(3, 10).text(0, "hi");
    let mut v = synced(&g);
    let before = v.clone();
    assert!(encode_frame(&g, &mut v).is_none());
    assert_eq!(v, before);
}

#[test]
fn sync_pending_gives_no_frame_and_keeps_the_state() {
    let g = TestGrid::new(3, 10).text(0, "hi");
    let mut v = synced(&g);
    let mut held = g.clone().text(1, "new");
    held.modes.sync_pending = true;
    let before = v.clone();
    assert!(encode_frame(&held, &mut v).is_none());
    assert_eq!(v, before);
    // A fresh viewer gets nothing either, until the update ends.
    let mut fresh = ViewerState::new();
    assert!(encode_frame(&held, &mut fresh).is_none());
    assert!(fresh.is_fresh());
    held.modes.sync_pending = false;
    assert!(encode_frame(&held, &mut v).is_some());
}

#[test]
fn one_changed_row_is_the_only_row_written() {
    let g = TestGrid::new(3, 10).text(0, "hi").text(1, "there");
    let mut v = synced(&g);
    let g2 = g.clone().text(1, "where");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[2;1H^[[Kwhere^[[1;1H");
    assert!(body(&g2, &mut v).is_none());
}

#[test]
fn a_row_reaching_the_last_column_is_not_erased() {
    let g = TestGrid::new(2, 3).text(0, "abc");
    let mut v = synced(&g);
    let g2 = g.clone().text(0, "abd");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;1H^[[Kabd^[[1;1H");
}

#[test]
fn cursor_move_alone_is_a_frame() {
    let g = TestGrid::new(2, 5);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.cursor = (1, 3);
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[2;4H");
}

#[test]
fn trailing_blanks_with_a_background_are_written_not_erased() {
    let mut g = TestGrid::new(2, 4).text(0, "a");
    g.cell(0, 1).bg = ColorDump::Palette { index: 1 };
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    // The coloured blank is written; the row was erased up front.
    assert!(f.contains("^[[1;1H^[[Ka^[[0;41m ^[]2;"), "{f}");
}

#[test]
fn sgr_covers_every_attribute_and_colour_kind() {
    let mut g = TestGrid::new(1, 12);
    for (i, t) in "abcdefghijk".chars().enumerate() {
        g.cell(0, i).text = t.to_string();
    }
    g.cell(0, 0).bold = true;
    g.cell(0, 1).italic = true;
    g.cell(0, 2).dim = true;
    g.cell(0, 3).underline = true;
    g.cell(0, 4).inverse = true;
    g.cell(0, 5).invisible = true;
    g.cell(0, 6).strikethrough = true;
    g.cell(0, 7).fg = ColorDump::Palette { index: 3 };
    g.cell(0, 8).fg = ColorDump::Palette { index: 12 };
    g.cell(0, 9).fg = ColorDump::Palette { index: 200 };
    g.cell(0, 10).bg = ColorDump::Rgb { r: 1, g: 2, b: 3 };
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.contains(
            "^[[1;1H^[[K^[[0;1ma^[[0;3mb^[[0;2mc^[[0;4md^[[0;7me^[[0;8mf^[[0;9mg\
             ^[[0;33mh^[[0;94mi^[[0;38;5;200mj^[[0;48;2;1;2;3mk"
        ),
        "{f}"
    );
}

#[test]
fn attributes_combine_and_colours_follow_them() {
    let mut g = TestGrid::new(1, 3).text(0, "ab");
    g.cell(0, 0).bold = true;
    g.cell(0, 0).underline = true;
    g.cell(0, 0).fg = ColorDump::Rgb { r: 9, g: 8, b: 7 };
    g.cell(0, 0).bg = ColorDump::Palette { index: 15 };
    g.cell(0, 1).bold = true; // a new run: reset, then the full style again
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[0;1;4;38;2;9;8;7;107ma^[[0;1mb"), "{f}");
}

#[test]
fn a_run_of_one_style_is_one_sequence() {
    let mut g = TestGrid::new(1, 4).text(0, "abc");
    for c in 0..3 {
        g.cell(0, c).bold = true;
    }
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[0;1mabc"), "{f}");
}

#[test]
fn osc8_wraps_a_linked_run_and_survives_a_style_change_inside_it() {
    let mut g = TestGrid::new(1, 8).text(0, "abcd");
    for c in 0..3 {
        g.cell(0, c).link = Some("https://x.test/a?b#c".into());
    }
    g.cell(0, 1).bold = true;
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.contains("^[[1;1H^[[K^[]8;;https://x.test/a?b#c^[\\a^[[0;1mb^[[0mc^[]8;;^[\\d"),
        "{f}"
    );
}

#[test]
fn a_link_is_closed_before_the_next_row_and_never_left_open() {
    let mut g = TestGrid::new(2, 3).text(0, "abc").text(1, "xyz");
    g.cell(0, 2).link = Some("u".into());
    g.cell(1, 0).link = Some("u".into());
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.contains("ab^[]8;;u^[\\c^[]8;;^[\\^[[2;1H^[[K^[]8;;u^[\\x^[]8;;^[\\yz"),
        "{f}"
    );
}

#[test]
fn a_link_with_a_control_character_is_dropped() {
    let mut g = TestGrid::new(1, 3).text(0, "ab");
    g.cell(0, 0).link = Some("bad\x07uri".into());
    let mut v = ViewerState::new();
    assert!(!body(&g, &mut v).unwrap().contains("]8"));
}

#[test]
fn title_changes_and_resets() {
    let g = TestGrid::new(1, 3);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.title = Some("build".into());
    assert_eq!(body(&g2, &mut v).unwrap(), "^[]2;build^[\\^[[1;1H");
    assert!(body(&g2, &mut v).is_none());
    g2.title = None;
    assert_eq!(body(&g2, &mut v).unwrap(), "^[]2;^[\\^[[1;1H");
}

#[test]
fn a_title_with_control_characters_is_cleaned() {
    let mut g = TestGrid::new(1, 3);
    g.title = Some("a\x1b]x\x07b".into());
    let mut v = ViewerState::new();
    assert!(body(&g, &mut v).unwrap().contains("^[]2;a]xb^[\\"));
}

#[test]
fn several_bells_collapse_into_one() {
    let g = TestGrid::new(1, 3);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.bells = 3;
    assert_eq!(body(&g2, &mut v).unwrap(), "^G^[[1;1H");
    assert!(body(&g2, &mut v).is_none());
}

#[test]
fn a_first_frame_does_not_ring_for_old_bells() {
    let mut g = TestGrid::new(1, 3);
    g.bells = 7;
    let mut v = ViewerState::new();
    assert!(!body(&g, &mut v).unwrap().contains("^G"));
}

#[test]
fn invalidate_makes_the_next_frame_a_full_redraw() {
    let g = TestGrid::new(2, 4).text(0, "ab");
    let mut v = synced(&g);
    assert!(body(&g, &mut v).is_none());
    v.invalidate();
    assert!(v.is_fresh());
    assert!(body(&g, &mut v).unwrap().contains("^[[2J"));
}

#[test]
fn a_rebuild_forces_a_full_redraw() {
    let g = TestGrid::new(2, 4).text(0, "ab");
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.rebuilds = 1;
    let f = body(&g2, &mut v).unwrap();
    assert!(f.contains("^[[H^[[2J^[[3J^[[1;1H^[[Kab"), "{f}");
}

#[test]
fn a_size_change_is_a_full_redraw() {
    let g = TestGrid::new(2, 4).text(0, "ab");
    let mut v = synced(&g);
    let g2 = TestGrid::new(3, 6).text(0, "ab");
    let f = body(&g2, &mut v).unwrap();
    assert!(f.contains("^[[H^[[2J^[[3J"), "{f}");
    assert!(body(&g2, &mut v).is_none());
}

#[test]
fn a_skipped_frame_still_ends_in_the_same_state() {
    let g = TestGrid::new(3, 6).text(0, "one");
    let mut seen = synced(&g);
    let mut skipped = synced(&g);
    let mut g2 = g.clone().text(1, "two");
    g2.title = Some("t".into());
    g2.modes.bracketed_paste = true;
    encode_frame(&g2, &mut seen).unwrap();
    let g3 = g2.clone().text(0, "uno");
    encode_frame(&g3, &mut seen);
    encode_frame(&g3, &mut skipped).unwrap();
    assert_eq!(seen, skipped);
}

#[test]
fn entering_the_alternate_screen_repaints_every_row_without_clearing() {
    let g = TestGrid::new(2, 3).text(0, "abc");
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.modes.alt_screen = true;
    let f = body(&g2, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049h^[[1;1H^[[Kabc^[[2;1H^[[K"), "{f}");
    assert!(!f.contains("^[[2J"), "{f}");
}
