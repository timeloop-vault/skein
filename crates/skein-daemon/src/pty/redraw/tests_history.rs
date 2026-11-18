//! History: the push in diff frames and the history of a full redraw.
//! Exact bytes on tiny grids (a `TestGrid`), then the client's scrollback
//! against the engine's history (a second `AlacrittyEngine` as the client).

#![allow(clippy::assert_is_empty)]

use crate::pty::engine::dump::RowDump;
use crate::pty::engine::{AlacrittyEngine, HISTORY_ROWS, TerminalEngine};

use super::test_grid::{TestGrid, body, engine, show, synced};
use super::{ViewerState, encode_frame};

/// A history row of `text`, all width 1.
fn hrow(text: &str, wrapped: bool) -> RowDump {
    let mut row = RowDump::blank(4);
    for (c, ch) in text.chars().enumerate() {
        row.cells[c].text = ch.to_string();
    }
    row.wrapped = wrapped;
    row
}

/// A 4-column grid of `rows` rows showing `texts`.
fn grid(rows: u16, texts: &[&str]) -> TestGrid {
    let mut g = TestGrid::new(rows, 4);
    for (r, t) in texts.iter().enumerate() {
        g.set_text(r, t);
    }
    g
}

/// `g` after its top `k` rows scrolled into history and `new` came in below.
fn scrolled(g: &TestGrid, k: usize, new: &[&str]) -> TestGrid {
    let mut n = g.clone();
    n.history.extend(g.screen[..k].iter().cloned());
    n.entered += k as u64;
    n.screen.drain(..k);
    n.screen.extend((0..k).map(|_| RowDump::blank(4)));
    let rows = usize::from(g.rows);
    for (i, t) in new.iter().enumerate() {
        n.set_text(rows - k + i, t);
    }
    n
}

// --- the push ---------------------------------------------------------------

#[test]
fn one_row_that_left_the_screen_is_a_line_feed_at_the_bottom() {
    let g = grid(3, &["aa", "bb", "cc"]);
    let mut v = synced(&g);
    let g2 = scrolled(&g, 1, &["dd"]);
    // The top row already holds what enters history: nothing is repainted.
    assert_eq!(body(&g2, &mut v).unwrap(), "^[[3;1H\n^[[3;1H^[[Kdd^[[1;1H");
    assert_eq!((v.history_entered, v.history_len), (1, 1));
    assert_eq!(v.screen, g2.screen);
    assert!(encode_frame(&g2, &mut v).is_none());
}

#[test]
fn a_top_row_that_changed_before_it_left_is_repainted_first() {
    let g = grid(3, &["aa", "bb", "cc"]);
    let mut v = synced(&g);
    let mut g2 = scrolled(&g, 1, &["dd"]);
    g2.history[0] = hrow("aX", false);
    assert_eq!(
        body(&g2, &mut v).unwrap(),
        "^[[1;1H^[[KaX^[[3;1H\n^[[3;1H^[[Kdd^[[1;1H"
    );
}

#[test]
fn several_rows_push_as_one_batch_of_line_feeds() {
    let g = grid(4, &["aa", "bb", "cc", "dd"]);
    let mut v = synced(&g);
    let g2 = scrolled(&g, 3, &["ee", "ff", "gg"]);
    assert_eq!(
        body(&g2, &mut v).unwrap(),
        "^[[4;1H\n\n\n^[[2;1H^[[Kee^[[3;1H^[[Kff^[[4;1H^[[Kgg^[[1;1H"
    );
}

#[test]
fn more_than_a_screenful_goes_in_batches_of_at_most_the_screen() {
    let g = grid(2, &["aa", "bb"]);
    let mut v = synced(&g);
    // Five rows entered: aa, bb (the viewer's) and cc, dd, ee it never saw.
    let mut g2 = g.clone();
    g2.history = ["aa", "bb", "cc", "dd", "ee"]
        .map(|t| hrow(t, false))
        .to_vec();
    g2.entered = 5;
    g2.set_text(0, "ff");
    g2.set_text(1, "");
    assert_eq!(
        body(&g2, &mut v).unwrap(),
        // batch 1 (aa, bb: held), 2 (cc, dd), 3 (ee), then the screen
        "^[[2;1H\n\n^[[1;1H^[[Kcc^[[2;1H^[[Kdd^[[2;1H\n\n\
         ^[[1;1H^[[Kee^[[2;1H\n^[[1;1H^[[Kff^[[1;1H"
    );
    assert_eq!((v.history_entered, v.history_len), (5, 5));
}

#[test]
fn a_batch_ends_before_a_continuation_when_it_can() {
    // Three rows pushed through a 2-row screen: aa, bb (continues aa), cc.
    // Cutting after aa would leave bb, a continuation, to start batch 2.
    let g = grid(2, &["xx", "yy"]);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.history = vec![hrow("aa", false), hrow("bb", true), hrow("cc", false)];
    g2.entered = 3;
    let f = body(&g2, &mut v).unwrap();
    // aa and bb in one batch (written as a pair, so bb is flagged), cc alone.
    assert!(f.starts_with("^[[1;1H^[[Kaa  bb^[[K^[[2;1H\n\n"), "{f}");
    assert!(f.contains("^[[1;1H^[[Kcc^[[2;1H\n"), "{f}");
}

#[test]
fn a_pushed_row_continuing_the_one_above_is_written_through_the_margin() {
    let g = grid(3, &["xx", "yy", "zz"]);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.history = vec![hrow("abcd", false), hrow("ef", true)];
    g2.entered = 2;
    let f = body(&g2, &mut v).unwrap();
    assert!(
        f.starts_with("^[[1;1H^[[Kabcdef^[[K^[[3;1H\n\n"),
        "the second row flows from the first: {f}"
    );
}

// --- the history of a full redraw -------------------------------------------

#[test]
fn a_full_redraw_flows_the_history_and_the_screen_from_the_top() {
    let mut g = grid(2, &["cc", "dd"]);
    g.history = vec![hrow("aa", false), hrow("bb", false)];
    g.entered = 2;
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.starts_with(
            "^[[?1049l^[[H^[[2J^[[3J^[[1;1H^[[Kaa^[[2;1H^[[Kbb\
             \n^[[2;1H^[[Kcc\n^[[2;1H^[[Kdd^[]2;"
        ),
        "{f}"
    );
    assert_eq!((v.history_entered, v.history_len), (2, 2));
}

#[test]
fn blank_rows_in_the_flow_are_only_line_feeds() {
    let mut g = grid(2, &["", "dd"]);
    g.history = vec![hrow("", false)];
    g.entered = 1;
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.starts_with("^[[?1049l^[[H^[[2J^[[3J^[[2;1H\n^[[2;1H^[[Kdd^[]2;"),
        "{f}"
    );
}

#[test]
fn a_screen_row_continuing_the_last_history_row_flows_through_the_margin() {
    let mut g = grid(2, &["ii", ""]);
    g.screen[0].wrapped = true;
    g.history = vec![hrow("hhhh", false)];
    g.entered = 1;
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.contains("^[[3J^[[1;1H^[[Khhhhii^[[K\n^[]2;"),
        "no CUP between the two: {f}"
    );
}

#[test]
fn history_with_a_wide_character_and_a_link_is_written_like_a_screen_row() {
    let mut g = grid(2, &["", ""]);
    let mut h = hrow("", false);
    h.cells[0].text = "\u{65e5}".into();
    h.cells[0].width = 2;
    h.cells[1].text = String::new();
    h.cells[1].width = 0;
    h.cells[2].text = "l".into();
    h.cells[2].link = Some("https://a.test".into());
    g.history = vec![h];
    g.entered = 1;
    let mut v = ViewerState::new();
    let f = body(&g, &mut v).unwrap();
    assert!(
        f.contains("^[[1;1H^[[K\u{65e5}^[[1;3H^[]8;;https://a.test^[\\l^[[2;1H^[]8;;^[\\\n"),
        "{f}"
    );
}

// --- when a push is not possible --------------------------------------------

fn assert_full(g: &TestGrid, v: &mut ViewerState) {
    let f = body(g, v).unwrap();
    assert!(f.contains("^[[H^[[2J^[[3J"), "a full redraw: {f}");
}

#[test]
fn history_at_its_bound_still_pushes() {
    let mut g = grid(2, &["aa", "bb"]);
    g.history = vec![hrow("", false); HISTORY_ROWS];
    g.entered = 7000;
    let mut v = synced(&g);
    assert_eq!(v.history_len, HISTORY_ROWS);
    // One row in at the bottom of history, the oldest out.
    let mut g2 = scrolled(&g, 1, &["cc"]);
    g2.history.remove(0);
    let f = body(&g2, &mut v).unwrap();
    assert!(!f.contains("^[[2J"), "a push, not a redraw: {f}");
    assert!(f.starts_with("^[[2;1H\n"), "{f}");
    assert_eq!((v.history_entered, v.history_len), (7001, HISTORY_ROWS));
}

#[test]
fn more_rows_than_the_history_holds_is_a_full_redraw() {
    let g = grid(2, &["aa", "bb"]);
    let mut v = synced(&g);
    let mut g2 = g.clone();
    g2.history = vec![hrow("", false); HISTORY_ROWS];
    g2.entered = HISTORY_ROWS as u64 + 1;
    assert_full(&g2, &mut v);
}

#[test]
fn a_history_that_shrank_is_a_full_redraw() {
    let g1 = scrolled(&grid(2, &["aa", "bb"]), 1, &["cc"]);
    let mut v = synced(&g1);
    let mut g2 = g1.clone();
    g2.history.clear();
    assert_full(&g2, &mut v);
    assert_eq!(v.history_len, 0);
}

#[test]
fn a_rebuild_is_a_full_redraw_with_the_history() {
    let g1 = scrolled(&grid(2, &["aa", "bb"]), 1, &["cc"]);
    let mut v = synced(&g1);
    let mut g2 = g1.clone();
    g2.rebuilds = 1;
    assert_full(&g2, &mut v);
}

#[test]
fn the_alternate_screen_pushes_no_history() {
    let g = grid(2, &["aa", "bb"]);
    let mut v = synced(&g);
    let mut g2 = scrolled(&g, 1, &["cc"]);
    g2.modes.alt_screen = true;
    let f = body(&g2, &mut v).unwrap();
    assert!(!f.contains('\n'), "{f}");
}

// --- the client's scrollback against the engine's history -------------------

fn feed(e: &mut AlacrittyEngine, s: &str) {
    e.feed(s.as_bytes());
}

fn texts(rows: &[RowDump]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            r.cells
                .iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The client holds the server's history and screen, row for row. Wrap flags
/// are compared only when `flags` (a full redraw; see `tests_engine`), and
/// never on the oldest history row, whose predecessor is gone.
fn assert_history_matches(server: &AlacrittyEngine, client: &AlacrittyEngine, flags: bool) {
    let (s, c) = (server.dump(), client.dump());
    assert_eq!(texts(&s.history), texts(&c.history), "history text");
    for (i, (a, b)) in s.history.iter().zip(&c.history).enumerate() {
        let (mut a, mut b) = (a.clone(), b.clone());
        if !flags || i == 0 {
            a.wrapped = false;
            b.wrapped = false;
        }
        assert_eq!(a, b, "history row {i}");
    }
    assert_eq!(texts(&s.screen), texts(&c.screen), "screen text");
}

const LONG: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

/// `n` lines numbered from `from`: wrapping, wide, linked, blank and plain.
fn lines(n: usize, from: usize) -> String {
    (from..from + n)
        .map(|i| match i % 5 {
            0 => format!("{i} {LONG}\r\n"),
            1 => format!("\u{65e5}\u{672c} {i}\r\n"),
            2 => format!("\x1b]8;;https://a.test/{i}\x07link {i}\x1b]8;;\x07\r\n"),
            3 => "\r\n".to_owned(),
            _ => format!("row {i}\r\n"),
        })
        .collect()
}

#[test]
fn a_full_redraw_leaves_the_client_with_the_engines_history() {
    for (rows, cols) in [(4, 13), (6, 20), (3, 7)] {
        let mut s = engine(rows, cols, &lines(30, 0));
        feed(&mut s, "tail that wraps across the margin of the screen");
        let mut c = AlacrittyEngine::new(rows, cols);
        c.feed(&s.frame(&mut ViewerState::new()).unwrap());
        assert!(!s.dump().history.is_empty());
        assert_history_matches(&s, &c, true);
    }
}

#[test]
fn pushes_in_every_size_keep_the_client_and_a_fresh_viewer_in_step() {
    for (rows, cols, step) in [(4, 13, 1), (6, 20, 7), (5, 11, 40), (3, 9, 100)] {
        let mut s = AlacrittyEngine::new(rows, cols);
        let mut c = AlacrittyEngine::new(rows, cols);
        let mut v = ViewerState::new();
        for from in (0..300).step_by(step) {
            feed(&mut s, &lines(step, from));
            if let Some(f) = s.frame(&mut v) {
                c.feed(&f);
            }
            assert_history_matches(&s, &c, false);
            let mut fresh = AlacrittyEngine::new(rows, cols);
            fresh.feed(&s.frame(&mut ViewerState::new()).unwrap());
            assert_history_matches(&s, &fresh, true);
        }
    }
}

#[test]
fn a_cleared_scrollback_redraws_in_full_and_the_client_follows() {
    let mut s = engine(4, 10, &lines(20, 0));
    let mut c = AlacrittyEngine::new(4, 10);
    let mut v = ViewerState::new();
    c.feed(&s.frame(&mut v).unwrap());
    feed(&mut s, "\x1b[3Jafter\r\n");
    let f = s.frame(&mut v).unwrap();
    assert!(show(&f).contains("^[[2J^[[3J"));
    c.feed(&f);
    assert_history_matches(&s, &c, true);
}

#[test]
fn history_at_its_bound_keeps_pushing_to_a_bounded_client() {
    let mut s = AlacrittyEngine::new(3, 10);
    let mut c = AlacrittyEngine::new(3, 10);
    let mut v = ViewerState::new();
    for chunk in 0..52 {
        feed(&mut s, &lines(100, chunk * 100));
        c.feed(&s.frame(&mut v).unwrap());
    }
    assert_eq!(s.history_len(), HISTORY_ROWS);
    assert!(s.history_rows_entered() > HISTORY_ROWS as u64);
    assert_history_matches(&s, &c, false);
    // More than the bound between two frames: a full redraw, same result.
    feed(&mut s, &lines(HISTORY_ROWS + 50, 9000));
    let f = s.frame(&mut v).unwrap();
    assert!(show(&f).contains("^[[2J^[[3J"));
    c.feed(&f);
    assert_history_matches(&s, &c, true);
}
