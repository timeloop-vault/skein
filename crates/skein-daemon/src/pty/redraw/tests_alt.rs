//! The alternate screen: what entering, living on and leaving it send, on a
//! tiny grid (a `TestGrid`, which can change the hidden main screen where a
//! real engine cannot).

use crate::pty::engine::dump::RowDump;

use super::ViewerState;
use super::encode_frame;
use super::test_grid::{TestGrid, body, synced};

fn hrow(text: &str) -> RowDump {
    let mut row = RowDump::blank(3);
    for (c, ch) in text.chars().enumerate() {
        row.cells[c].text = ch.to_string();
    }
    row
}

/// A main screen of two rows, `aa` and `bb`, with `h1` and `h2` in history.
fn main_grid() -> TestGrid {
    let mut g = TestGrid::new(2, 3).text(0, "aa").text(1, "bb");
    g.history = vec![hrow("h1"), hrow("h2")];
    g.entered = 2;
    g.cursor = (1, 2);
    g
}

/// `g` with the alternate screen showing `rows`.
fn on_alt(g: &TestGrid, rows: [&str; 2]) -> TestGrid {
    let mut a = g.clone();
    a.modes.alt_screen = true;
    a.set_text(0, rows[0]);
    a.set_text(1, rows[1]);
    a.cursor = (0, 1);
    a
}

#[test]
fn entering_sends_the_mode_every_row_and_the_cursor_and_no_clear() {
    let g = main_grid();
    let mut v = synced(&g);
    let a = on_alt(&g, ["xx", ""]);
    assert_eq!(
        body(&a, &mut v).unwrap(),
        "^[[?1049h^[[1;1H^[[Kxx^[[2;1H^[[K^[[1;2H"
    );
    // The viewer keeps its main rows and its history counters.
    assert_eq!(v.main_screen.as_ref(), Some(&g.screen));
    assert_eq!((v.history_entered, v.history_len), (2, 2));
    assert_eq!(v.screen, a.screen);
}

#[test]
fn entering_never_clears_the_scrollback() {
    let g = main_grid();
    let mut v = synced(&g);
    let f = body(&on_alt(&g, ["xx", "yy"]), &mut v).unwrap();
    assert!(!f.contains("^[[3J"), "{f}");
    assert!(!f.contains("^[[2J"), "{f}");
}

#[test]
fn rows_that_equal_the_main_screen_are_still_repainted_on_entry() {
    let g = main_grid();
    let mut v = synced(&g);
    let mut a = g.clone();
    a.modes.alt_screen = true;
    let f = body(&a, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kaa^[[2;1H^[[Kbb"), "{f}");
}

#[test]
fn while_active_a_frame_is_a_row_diff_with_no_history() {
    let g = main_grid();
    let mut v = synced(&g);
    let a = on_alt(&g, ["xx", "yy"]);
    encode_frame(&a, &mut v).unwrap();
    let mut a2 = a.clone().text(1, "yz");
    a2.cursor = (1, 2);
    assert_eq!(body(&a2, &mut v).unwrap(), "^[[2;1H^[[Kyz^[[2;3H");
    // The alternate screen scrolls: rows enter a "history" the engine would
    // not count, and nothing is pushed or remembered.
    let mut a3 = a2.clone().text(0, "yz").text(1, "qq");
    a3.entered += 1;
    let f = body(&a3, &mut v).unwrap();
    assert!(!f.contains('\n'), "{f}");
    assert_eq!((v.history_entered, v.history_len), (2, 2));
    assert_eq!(v.main_screen.as_ref(), Some(&g.screen));
    assert!(encode_frame(&a3, &mut v).is_none());
}

#[test]
fn leaving_with_the_main_screen_unchanged_repaints_nothing() {
    let g = main_grid();
    let mut v = synced(&g);
    encode_frame(&on_alt(&g, ["xx", "yy"]), &mut v).unwrap();
    let mut back = g.clone();
    back.cursor = (0, 0);
    assert_eq!(body(&back, &mut v).unwrap(), "^[[?1049l^[[1;1H");
    assert!(v.main_screen.is_none());
    assert_eq!(v.screen, g.screen);
}

#[test]
fn leaving_after_the_main_screen_changed_repaints_only_what_differs() {
    let g = main_grid();
    let mut v = synced(&g);
    encode_frame(&on_alt(&g, ["xx", "yy"]), &mut v).unwrap();
    let back = g.clone().text(1, "bc");
    assert_eq!(
        body(&back, &mut v).unwrap(),
        "^[[?1049l^[[2;1H^[[Kbc^[[2;3H"
    );
}

#[test]
fn leaving_after_the_main_screen_scrolled_pushes_the_rows() {
    let g = main_grid();
    let mut v = synced(&g);
    encode_frame(&on_alt(&g, ["xx", "yy"]), &mut v).unwrap();
    // `aa` scrolled into history, `cc` came in at the bottom.
    let mut back = g.clone();
    back.history.push(hrow("aa"));
    back.entered += 1;
    back.set_text(0, "bb");
    back.set_text(1, "cc");
    let f = body(&back, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049l^[[2;1H\n"), "{f}");
    assert!(!f.contains("^[[3J"), "{f}");
    assert_eq!((v.history_entered, v.history_len), (3, 3));
}

#[test]
fn a_viewer_that_attached_on_the_alternate_screen_gets_only_it() {
    let g = main_grid();
    let a = on_alt(&g, ["xx", "yy"]);
    let mut v = ViewerState::new();
    let f = body(&a, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049h^[[H^[[2J^[[3J^[[1;1H^[[Kxx"), "{f}");
    assert!(!f.contains("h1"), "no history on the alternate screen: {f}");
    assert!(v.main_screen.is_none(), "its main is unknown");
}

#[test]
fn leaving_for_a_viewer_that_attached_on_it_is_a_full_redraw_with_history() {
    let g = main_grid();
    let mut v = ViewerState::new();
    encode_frame(&on_alt(&g, ["xx", "yy"]), &mut v).unwrap();
    let f = body(&g, &mut v).unwrap();
    // The mode first, then the clear, then history and screen as one
    // flowing run: the scroll files `h1` and `h2` into the scrollback.
    assert!(
        f.starts_with(
            "^[[?1049l^[[H^[[2J^[[3J^[[1;1H^[[Kh1^[[2;1H^[[Kh2
^[[2;1H^[[Kaa
^[[2;1H^[[Kbb"
        ),
        "{f}"
    );
    assert_eq!((v.history_entered, v.history_len), (2, 2));
}

#[test]
fn entering_and_leaving_between_two_frames_is_a_main_diff() {
    let g = main_grid();
    let mut v = synced(&g);
    // The engine went alt and back: the viewer never saw alt.
    let back = g.clone().text(0, "ab");
    assert_eq!(body(&back, &mut v).unwrap(), "^[[1;1H^[[Kab^[[2;3H");
    assert!(!v.modes.alt_screen);
    assert!(v.main_screen.is_none());
}

#[test]
fn a_resize_while_active_marks_the_main_unknown() {
    let g = main_grid();
    let mut v = synced(&g);
    let a = on_alt(&g, ["xx", "yy"]);
    encode_frame(&a, &mut v).unwrap();
    let mut wide = TestGrid::new(2, 4);
    wide.modes.alt_screen = true;
    let f = body(&wide, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049h^[[H^[[2J^[[3J"), "{f}");
    assert!(v.main_screen.is_none());
}

#[test]
fn a_rebuild_while_active_is_a_full_redraw_of_the_alternate_screen() {
    let main = main_grid();
    let mut v = synced(&main);
    let a = on_alt(&main, ["xx", "yy"]);
    encode_frame(&a, &mut v).unwrap();
    let mut rebuilt = a.clone();
    rebuilt.rebuilds = 1;
    let f = body(&rebuilt, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049h^[[H^[[2J^[[3J"), "{f}");
    assert!(v.main_screen.is_none());
}
