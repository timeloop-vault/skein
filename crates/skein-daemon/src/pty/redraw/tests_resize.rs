//! A size change. The rule: any change of rows or columns is a full redraw
//! (clear screen and scrollback, history as flowing lines, screen, modes), on
//! the main screen and, with the main marked unknown, on the alternate one.
//!
//! A height-only change is not cheaper because the client cannot be trusted
//! to land on the engine's scrollback by its own adjustment: it resizes the
//! rows it holds, the engine resizes the rows it holds now, and the two
//! differ whenever output (or just the cursor) moved after the last frame.
//! `a_height_only_change_with_unsent_output_*` pins that down.

use crate::pty::engine::dump::RowDump;
use crate::pty::engine::{AlacrittyEngine, TerminalEngine};

use super::ViewerState;
use super::test_grid::{TestGrid, body, engine, synced};

fn hrow(text: &str) -> RowDump {
    let mut row = RowDump::blank(3);
    for (c, ch) in text.chars().enumerate() {
        row.cells[c].text = ch.to_string();
    }
    row
}

/// Two rows, `aa` and `bb`, with `h1` and `h2` in history.
fn main_grid() -> TestGrid {
    let mut g = TestGrid::new(2, 3).text(0, "aa").text(1, "bb");
    g.history = vec![hrow("h1"), hrow("h2")];
    g.entered = 2;
    g.cursor = (1, 2);
    g
}

/// `g` at a different width: every row cut or padded to `cols`.
fn with_cols(g: &TestGrid, cols: u16) -> TestGrid {
    let fit = |r: &RowDump| {
        let mut row = RowDump::blank(usize::from(cols));
        for (c, cell) in r.cells.iter().take(usize::from(cols)).enumerate() {
            row.cells[c] = cell.clone();
        }
        row
    };
    let mut n = g.clone();
    n.cols = cols;
    n.history = g.history.iter().map(fit).collect();
    n.screen = g.screen.iter().map(fit).collect();
    n
}

#[test]
fn a_width_change_clears_and_resends_history_and_screen_exactly() {
    let g = main_grid();
    let mut v = synced(&g);
    let wide = with_cols(&g, 4);
    assert_eq!(
        body(&wide, &mut v).unwrap(),
        "^[[?1049l^[[H^[[2J^[[3J^[[1;1H^[[Kh1^[[2;1H^[[Kh2\n^[[2;1H^[[Kaa\n^[[2;1H^[[Kbb\
         ^[]2;^[\\^[[?1l^[>^[[?1000l^[[?1002l^[[?1003l^[[?1005l^[[?1006l^[[?2004l\
         ^[[?1004l^[[=0;1u^[[2;3H^[[2 q^[[?25h"
    );
    assert_eq!((v.rows, v.cols), (2, 4));
    assert_eq!(v.screen, wide.screen);
}

#[test]
fn a_width_change_rebaselines_the_history_counters() {
    let g = main_grid();
    let mut v = synced(&g);
    // The resize itself moved rows across the boundary: the counter and the
    // history length no longer relate to what the viewer was sent.
    let mut narrow = with_cols(&g, 5);
    narrow.history = vec![hrow("h3"), hrow("h4")];
    narrow.entered = 2;
    body(&narrow, &mut v).unwrap();
    assert_eq!((v.history_entered, v.history_len), (2, 2));
    // The next scroll is an ordinary push of one row.
    let mut next = narrow.clone();
    next.entered = 3;
    next.history.push(hrow("h5"));
    let f = body(&next, &mut v).unwrap();
    assert!(!f.contains("^[[3J"), "{f}");
    assert_eq!((v.history_entered, v.history_len), (3, 3));
}

#[test]
fn a_width_change_during_the_alternate_screen_leaves_it_as_a_full_redraw() {
    let g = main_grid();
    let mut v = synced(&g);
    let mut a = g.clone();
    a.modes.alt_screen = true;
    a.set_text(0, "xx");
    body(&a, &mut v).unwrap();
    assert!(v.main_screen.is_some());

    let mut a2 = with_cols(&a, 4);
    a2.history.clear();
    let f = body(&a2, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049h^[[H^[[2J^[[3J"), "{f}");
    assert!(v.main_screen.is_none(), "the main rows are unknown now");

    // Leaving: the main screen is redrawn whole, history included.
    let back = with_cols(&g, 4);
    let f = body(&back, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049l^[[H^[[2J^[[3J"), "{f}");
    assert!(f.contains("h1") && f.contains("h2"), "{f}");
    assert_eq!((v.history_entered, v.history_len), (2, 2));
}

#[test]
fn a_height_change_during_the_alternate_screen_marks_the_main_unknown() {
    let g = main_grid();
    let mut v = synced(&g);
    let mut a = g.clone();
    a.modes.alt_screen = true;
    body(&a, &mut v).unwrap();
    let mut taller = TestGrid::new(3, 3);
    taller.modes.alt_screen = true;
    taller.entered = 2;
    let f = body(&taller, &mut v).unwrap();
    assert!(f.contains("^[[3J"), "{f}");
    assert!(v.main_screen.is_none());
}

// --- a real engine, and a client that resizes before the next frame ---------

fn fill(e: &mut AlacrittyEngine, n: usize) {
    for i in 0..n {
        e.feed(format!("line {i} with some text\r\n").as_bytes());
    }
}

/// The client resizes first, then gets the frame, as the gate's client does.
fn resize_both(
    server: &mut AlacrittyEngine,
    client: &mut AlacrittyEngine,
    v: &mut ViewerState,
    size: (u16, u16),
) -> Option<String> {
    server.resize(size.0, size.1);
    client.resize(size.0, size.1);
    let f = server.frame(v)?;
    let shown = super::test_grid::show(&f);
    client.feed(&f);
    Some(shown)
}

fn assert_same(server: &AlacrittyEngine, client: &AlacrittyEngine, at: &str) {
    let (s, c) = (server.dump(), client.dump());
    let text = |rows: &[RowDump]| -> Vec<String> {
        rows.iter()
            .map(|r| r.cells.iter().map(|c| c.text.as_str()).collect())
            .collect()
    };
    assert_eq!(text(&s.history), text(&c.history), "history {at}");
    assert_eq!(text(&s.screen), text(&c.screen), "screen {at}");
    assert_eq!(s.cursor, c.cursor, "cursor {at}");
}

fn synced_pair(rows: u16, cols: u16, n: usize) -> (AlacrittyEngine, AlacrittyEngine, ViewerState) {
    let mut server = AlacrittyEngine::new(rows, cols);
    let mut client = AlacrittyEngine::new(rows, cols);
    let mut v = ViewerState::new();
    fill(&mut server, n);
    client.feed(&server.frame(&mut v).unwrap());
    (server, client, v)
}

#[test]
fn a_height_shrink_and_a_grow_are_full_redraws_that_agree() {
    for cursor_row in [None, Some(2)] {
        let (mut server, mut client, mut v) = synced_pair(6, 20, 14);
        if let Some(r) = cursor_row {
            server.feed(format!("\x1b[{r};1H").as_bytes());
            client.feed(&server.frame(&mut v).unwrap());
        }
        for size in [(3, 20), (9, 20), (6, 20), (2, 20), (4, 20)] {
            let f = resize_both(&mut server, &mut client, &mut v, size).unwrap();
            assert!(f.contains("^[[3J"), "{size:?}: {f}");
            assert_same(&server, &client, &format!("{cursor_row:?} -> {size:?}"));
            let mut fresh = ViewerState::new();
            let mut attached = AlacrittyEngine::new(size.0, size.1);
            attached.feed(&server.frame(&mut fresh).unwrap());
            assert_same(&server, &attached, &format!("fresh {size:?}"));
            assert!(server.frame(&mut v).is_none(), "{size:?}");
        }
    }
}

#[test]
fn a_height_only_change_with_unsent_output_leaves_the_clients_own_history_different() {
    // Why a height change cannot send the screen alone: output that the
    // viewer has not been sent scrolls the engine before it resizes, so the
    // client's own resize (of the older screen) pulls or pushes other rows.
    let (mut server, mut client, mut v) = synced_pair(6, 20, 14);
    fill(&mut server, 5);
    server.resize(3, 20);
    client.resize(3, 20);
    let (s, c) = (server.dump(), client.dump());
    assert_ne!(s.history.len(), c.history.len());
    // The full redraw puts it right.
    client.feed(&server.frame(&mut v).unwrap());
    assert_same(&server, &client, "after the redraw");
}

#[test]
fn a_resize_to_the_clamped_minimum_matches_the_engine() {
    let (mut server, mut client, mut v) = synced_pair(4, 12, 10);
    resize_both(&mut server, &mut client, &mut v, (0, 0)).unwrap();
    assert_eq!(server.dump().cols, 2);
    assert_eq!(server.dump().rows, 1);
    assert_same(&server, &client, "at the minimum");
    // Asking for the minimum again changes nothing.
    server.resize(0, 1);
    client.resize(0, 1);
    assert!(server.frame(&mut v).is_none());
    resize_both(&mut server, &mut client, &mut v, (4, 12)).unwrap();
    assert_same(&server, &client, "grown back");
}

#[test]
fn a_resize_to_the_same_size_or_back_without_output_sends_nothing() {
    let mut e = engine(3, 10, "a\r\nb\r\nc\r\nd\r\n");
    let mut v = ViewerState::new();
    e.frame(&mut v).unwrap();
    e.resize(3, 10);
    assert!(e.frame(&mut v).is_none());
    // Resized and resized back before the next frame: the viewer sees no change.
    e.resize(5, 10);
    e.resize(3, 10);
    let f = e.frame(&mut v);
    if let Some(f) = f {
        // Whatever the round trip did to the history is repaired whole.
        let mut client = AlacrittyEngine::new(3, 10);
        client.feed(&f);
        assert_same(&e, &client, "after a there-and-back");
    }
}

#[test]
fn a_tall_resize_of_a_long_session_round_trips_across_modes() {
    let (mut server, mut client, mut v) = synced_pair(5, 16, 40);
    let steps: &[(u16, u16, &str)] = &[
        (5, 9, "wrapped wrapped wrapped wrapped\r\n"),
        (8, 9, "\x1b[?1049halt\x1b[?1049l"),
        (8, 12, "more\r\nlines\r\n"),
        (3, 12, "\x1b[?1049h"),
        (3, 7, "\x1b[2Jalt narrow"),
        (6, 7, "\x1b[?1049lback\r\n"),
        (6, 16, ""),
    ];
    for (rows, cols, out) in steps {
        server.feed(out.as_bytes());
        // Output reaches the viewer before the resize, as it would.
        if let Some(f) = server.frame(&mut v) {
            client.feed(&f);
        }
        resize_both(&mut server, &mut client, &mut v, (*rows, *cols));
        assert_same(&server, &client, &format!("{rows}x{cols} after {out:?}"));
        assert_eq!(server.dump().alt_screen, client.dump().alt_screen);
    }
}

#[test]
fn resizing_away_and_back_between_frames_is_a_full_redraw_that_agrees() {
    let (mut server, mut client, mut v) = synced_pair(4, 12, 12);
    assert_eq!(server.resizes(), 0);
    server.resize(7, 12);
    // Output while larger rewrites what the history will hold on the way back.
    fill(&mut server, 3);
    server.resize(4, 12);
    assert_eq!(server.resizes(), 2);
    client.resize(4, 12);
    let f = server.frame(&mut v).expect("a resize since the last frame");
    assert!(super::test_grid::show(&f).contains("^[[3J"));
    client.feed(&f);
    assert_same(&server, &client, "after away and back");
    assert!(server.frame(&mut v).is_none());
}

#[test]
fn a_changed_resize_count_forces_a_full_redraw_at_the_same_size() {
    let g = main_grid();
    let mut v = synced(&g);
    let mut same = g.clone();
    assert!(body(&same, &mut v).is_none());
    same.resizes = 2;
    let f = body(&same, &mut v).unwrap();
    assert!(f.starts_with("^[[?1049l^[[H^[[2J^[[3J"), "{f}");
    assert_eq!(v.resizes, 2);
    assert!(body(&same, &mut v).is_none());

    // On the alternate screen the main becomes unknown.
    let mut alt = same.clone();
    alt.modes.alt_screen = true;
    body(&alt, &mut v).unwrap();
    assert!(v.main_screen.is_some());
    alt.resizes = 3;
    body(&alt, &mut v).unwrap();
    assert!(v.main_screen.is_none());
}
