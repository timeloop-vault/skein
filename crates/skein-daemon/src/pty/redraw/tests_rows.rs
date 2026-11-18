//! Writing rows: wide characters, ambiguous widths and wrapped rows.

use crate::pty::engine::dump::ColorDump;

use super::ViewerState;
use super::test_grid::{TestGrid, body, synced};

#[test]
fn a_wide_character_is_followed_by_a_cup() {
    let mut g = TestGrid::new(1, 6).text(0, "a");
    g.wide(0, 1, '日');
    g.cell(0, 3).text = "b".into();
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Ka日^[[1;4Hb"), "{f}");
}

#[test]
fn a_wide_character_in_the_last_two_columns_ends_the_row_without_a_cup() {
    let mut g = TestGrid::new(1, 4).text(0, "ab");
    g.wide(0, 2, '日');
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kab日^[]2;"), "{f}");
}

#[test]
fn an_ambiguous_width_character_is_followed_by_a_cup() {
    // U+00B1 is East Asian Ambiguous: one column here, two in a CJK locale.
    let g = TestGrid::new(1, 6).text(0, "a\u{b1}b");
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Ka\u{b1}^[[1;3Hb"), "{f}");
}

#[test]
fn a_character_with_a_combining_mark_is_followed_by_a_cup() {
    let mut g = TestGrid::new(1, 6).text(0, "eb");
    g.cell(0, 0).text = "e\u{301}".into();
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Ke\u{301}^[[1;2Hb"), "{f}");
}

#[test]
fn plain_ascii_gets_no_extra_cup() {
    let g = TestGrid::new(1, 6).text(0, "abc");
    let mut v = ViewerState::new();
    assert!(body(&g, &mut v).unwrap().contains("^[[1;1H^[[Kabc"));
}

fn wrapped_grid() -> TestGrid {
    TestGrid::new(3, 4)
        .text(0, "abcd")
        .text(1, "efgh")
        .text(2, "ij")
        .wrapped(1)
}

#[test]
fn a_wrapped_row_is_written_through_the_margin() {
    let g = wrapped_grid();
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabcdefgh^[[3;1H^[[Kij"), "{f}");
}

#[test]
fn a_changed_first_row_of_a_wrapped_pair_stops_at_the_margin() {
    let g = wrapped_grid();
    let mut v = synced(&g);
    let g2 = g.clone().text(0, "abcX");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;1H^[[KabcX^[[1;1H");
}

#[test]
fn only_the_continuation_row_changed_re_emits_the_last_cell_above() {
    let g = wrapped_grid();
    let mut v = synced(&g);
    let g2 = g.clone().text(1, "efgX");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;4HdefgX^[[1;1H");
}

#[test]
fn a_row_that_stopped_wrapping_is_positioned_again() {
    let g = wrapped_grid();
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.screen[1].wrapped = false;
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[2;1H^[[Kefgh^[[1;1H");
}

#[test]
fn both_rows_of_a_wrapped_pair_changed_flow_without_a_cup() {
    let g = wrapped_grid();
    let mut v = synced(&g);
    let g2 = g.clone().text(0, "ABCD").text(1, "EFGH");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;1H^[[KABCDEFGH^[[1;1H");
}

#[test]
fn a_chain_of_three_wrapped_rows_flows_once() {
    let g = TestGrid::new(3, 3)
        .text(0, "abc")
        .text(1, "def")
        .text(2, "gh")
        .wrapped(1)
        .wrapped(2);
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabcdefgh^[[K"), "{f}");
}

#[test]
fn the_prefix_uses_the_wide_character_when_it_ends_the_row_above() {
    let mut g = TestGrid::new(2, 4).text(0, "ab");
    g.wide(0, 2, '日');
    let g = g.text(1, "z").wrapped(1);
    let mut v = synced(&g);
    let g2 = g.clone().text(1, "y");
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;3H日y^[[K^[[1;1H");
}

fn did_not_fit_grid() -> TestGrid {
    // Row 0 is "abc" and a padding cell, because the wide character did not
    // fit in the last column. Row 1 starts with it and is flagged wrapped.
    let mut g = TestGrid::new(2, 4).text(0, "abc").wrapped(1);
    g.cell(0, 3).leading_spacer = true;
    g.wide(1, 0, '日');
    g.cell(1, 2).text = "x".into();
    g
}

#[test]
fn a_wide_character_that_did_not_fit_wraps_by_itself_without_the_spacer() {
    let g = did_not_fit_grid();
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabc日^[[2;3Hx^[[K"), "{f}");
}

#[test]
fn only_the_row_after_a_did_not_fit_padding_changed_re_emits_the_last_real_cell_above() {
    let g = did_not_fit_grid();
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.cell(1, 2).text = "y".into();
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[1;3Hc日^[[2;3Hy^[[K^[[1;1H");
}

#[test]
fn a_blank_continuation_row_is_wrapped_into_with_a_blank_not_erased_from_the_margin() {
    let g = TestGrid::new(2, 4).text(0, "abcd").wrapped(1);
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabcd ^[[K"), "{f}");
}

/// What a reflow can leave: width 2 at the last column, no spacer after it.
fn wide_in_last_column(g: &mut TestGrid, r: usize) {
    let last = g.screen[r].cells.len() - 1;
    g.cell(r, last).text = "日".into();
    g.cell(r, last).width = 2;
}

#[test]
fn a_wide_character_starting_in_the_last_column_is_one_blank_in_its_style() {
    // The engine can hold one after a reflow; the client would wrap it.
    let mut g = TestGrid::new(1, 4).text(0, "ab");
    wide_in_last_column(&mut g, 0);
    g.cell(0, 3).bg = ColorDump::Palette { index: 1 };
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kab ^[[0;41m ^[]2;"), "{f}");
    assert!(!f.contains('日'), "{f}");
    // The stored row stays the engine's: the same grid is no change.
    assert!(body(&g, &mut v).is_none());
}

#[test]
fn a_wide_character_in_the_last_column_is_not_the_margin_cell_either() {
    let mut g = TestGrid::new(2, 4).text(0, "ab").text(1, "z").wrapped(1);
    wide_in_last_column(&mut g, 0);
    let mut v = synced(&g);
    let g2 = g.clone().text(1, "y");
    let f = body(&g2, &mut v).unwrap();
    assert!(!f.contains('日'), "{f}");
}

#[test]
fn a_wrapped_row_that_follows_a_did_not_fit_padding_is_a_continuation() {
    // Both rows changed: row 0's runs without the spacer, then row 1's text
    // flows on with no CUP, so the client wraps the wide character itself.
    let g = did_not_fit_grid();
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.cell(0, 0).text = "x".into();
    g2.cell(1, 2).text = "y".into();
    let b = body(&g2, &mut v).unwrap();
    assert_eq!(b, "^[[1;1H^[[Kxbc日^[[2;3Hy^[[K^[[1;1H");
}
