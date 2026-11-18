//! Reflow on resize and the count of rows entering history (#793, design T3:
//! "reflow of the main screen", "T-1 must show the engine can report rows
//! entering history").
#![allow(clippy::assert_is_empty)]

use super::tests::{feed, history, row_str, screen, wrapped_flags};
use super::{AlacrittyEngine, HISTORY_ROWS, TerminalEngine};

fn long_line() -> String {
    (0..200)
        .map(|i| char::from(b'a' + u8::try_from(i % 26).unwrap_or(0)))
        .collect()
}

fn chunks(s: &str, n: usize) -> Vec<String> {
    s.as_bytes()
        .chunks(n)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect()
}

#[test]
fn main_screen_and_history_reflow_with_the_width() {
    let line = long_line();
    let mut e = AlacrittyEngine::new(6, 80);
    feed(&mut e, &format!("one\r\n{line}\r\nend"));
    // 80 columns: the 200 characters take 3 rows, 80 + 80 + 40.
    assert_eq!(
        screen(&e),
        ["one", &line[..80], &line[80..160], &line[160..], "end", ""]
    );
    assert_eq!(
        wrapped_flags(&e.dump().screen),
        [false, false, true, true, false, false]
    );
    assert!(history(&e).is_empty());

    // 40 columns: the line is now 5 rows. The cursor row ("end") stays on
    // the bottom row and the content above it no longer fits: "one" and the
    // first row of the line move into history (two rows, although seven
    // rows of text would need only one: the blank row below the cursor is
    // kept). The wrap flags follow the text, across the history boundary.
    e.resize(6, 40);
    let d = e.dump();
    assert_eq!(history(&e), ["one", &line[..40]]);
    assert_eq!(
        d.screen.iter().map(row_str).collect::<Vec<_>>(),
        [
            &line[40..80],
            &line[80..120],
            &line[120..160],
            &line[160..],
            "end",
            ""
        ]
    );
    assert_eq!(wrapped_flags(&d.history), [false, false]);
    assert_eq!(
        wrapped_flags(&d.screen),
        [true, true, true, true, false, false]
    );
    assert_eq!(e.cursor(), (4, 3));

    // Back to 80: reflowed whole again, and "one" is pulled back out of
    // history. The cursor follows its text.
    e.resize(6, 80);
    assert!(history(&e).is_empty());
    assert_eq!(
        screen(&e),
        ["one", &line[..80], &line[80..160], &line[160..], "end", ""]
    );
    assert_eq!(
        wrapped_flags(&e.dump().screen),
        [false, false, true, true, false, false]
    );
    assert_eq!(e.cursor(), (4, 3));
}

#[test]
fn history_rows_reflow_too_and_keep_their_wrap_flags() {
    let line = long_line();
    let mut e = AlacrittyEngine::new(3, 80);
    feed(&mut e, &format!("{line}\r\na\r\nb\r\nc"));
    // The whole long line (three rows) scrolled into history.
    assert_eq!(history(&e), [&line[..80], &line[80..160], &line[160..]]);
    e.resize(3, 40);
    // At 40 columns the same line is 5 rows, all of them history; the chain
    // of wrap flags is unbroken, and the screen rows follow it.
    let d = e.dump();
    let all: Vec<_> = d.history.iter().chain(d.screen.iter()).collect();
    let text: String = all.iter().map(|r| row_str(r)).collect::<Vec<_>>().join("|");
    assert_eq!(text, format!("{}|a|b|c", chunks(&line, 40).join("|")));
    let flags: Vec<bool> = all.iter().map(|r| r.wrapped).collect();
    assert_eq!(flags, [false, true, true, true, true, false, false, false]);
    e.resize(3, 80);
    assert_eq!(
        history(&e).concat() + &screen(&e).concat(),
        format!("{line}abc")
    );
}

#[test]
fn the_alternate_screen_does_not_reflow() {
    let line = long_line();
    let mut e = AlacrittyEngine::new(4, 80);
    feed(
        &mut e,
        &format!("main {line}\x1b[?1049h\x1b[2J\x1b[H{}", &line[..60]),
    );
    assert_eq!(screen(&e)[0], &line[..60]);
    e.resize(4, 40);
    // Cut at the new width: the alternate grid keeps 40 columns of row 0 and
    // nothing wrapped.
    assert_eq!(screen(&e)[0], &line[..40]);
    assert!(e.dump().screen.iter().all(|r| !r.wrapped));
    e.resize(4, 80);
    // And widening does not bring the cut text back.
    assert_eq!(screen(&e)[0], &line[..40]);
    // The main screen, which reflowed while hidden, is back after leaving.
    feed(&mut e, "\x1b[?1049l");
    assert_eq!(screen(&e).concat(), format!("main {line}"));
    assert_eq!(
        wrapped_flags(&e.dump().screen)[..4],
        [false, true, true, false]
    );
}

#[test]
fn resizing_rows_moves_rows_between_screen_and_history() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, "1\r\n2\r\n3\r\n4\r\n5");
    e.resize(3, 10);
    assert_eq!(history(&e), ["1", "2"]);
    assert_eq!(screen(&e), ["3", "4", "5"]);
    e.resize(5, 10);
    assert!(history(&e).is_empty());
    assert_eq!(screen(&e), ["1", "2", "3", "4", "5"]);
}

#[test]
fn a_zero_or_one_column_resize_is_raised_to_two() {
    // alacritty_terminal HANGS (an endless loop, not a panic) when a main
    // grid with text is resized to one column, and panics when a wide
    // character is written to a one-column grid. Neither can be caught, so
    // the engine never builds a grid narrower than two columns.
    let mut e = AlacrittyEngine::new(0, 0);
    feed(&mut e, "ab\u{4e2d}");
    assert_eq!((e.dump().rows, e.dump().cols), (1, 2));
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, "hello world foo bar\r\n\u{4e2d}\u{4e2d}");
    e.resize(0, 1);
    assert_eq!((e.dump().rows, e.dump().cols), (1, 2));
    feed(&mut e, "\u{4e2d}x");
    assert_eq!(e.rebuilds(), 0);
}

// history_rows_entered

fn lines(n: usize) -> String {
    "\r\n".repeat(n)
}

#[test]
fn entered_counts_exactly_the_rows_scrolled_off() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, "a");
    assert_eq!(e.history_rows_entered(), 0);
    feed(&mut e, &lines(4)); // the cursor reaches the bottom row, nothing scrolled
    assert_eq!(e.history_rows_entered(), 0);
    feed(&mut e, &lines(1));
    assert_eq!(e.history_rows_entered(), 1);
    feed(&mut e, &lines(7));
    assert_eq!(e.history_rows_entered(), 8);
    assert_eq!(e.history_len(), 8);
    // `ESC[nS` scrolls n rows at once; `ESC D` (index) at the bottom, one.
    feed(&mut e, "\x1b[3S");
    assert_eq!(e.history_rows_entered(), 11);
    feed(&mut e, "\x1b[5;1H\x1bD");
    assert_eq!(e.history_rows_entered(), 12);
    // A wrapped character at the bottom-right corner scrolls too.
    feed(&mut e, "\x1b[5;1H0123456789x");
    assert_eq!(e.history_rows_entered(), 13);
    // Splitting the stream anywhere changes nothing.
    let stream = format!(
        "x{}\x1b[2;4r{}\x1b[r{}",
        lines(12),
        lines(9),
        "\x1b[3S\x1b[4M"
    );
    let mut whole = AlacrittyEngine::new(5, 10);
    feed(&mut whole, &stream);
    let mut bytes = AlacrittyEngine::new(5, 10);
    for b in stream.bytes() {
        bytes.feed(&[b]);
    }
    assert_eq!(bytes.history_rows_entered(), whole.history_rows_entered());
    assert_eq!(history(&bytes), history(&whole));
}

#[test]
fn entered_keeps_growing_after_history_is_full() {
    let mut e = AlacrittyEngine::new(5, 10);
    let n = HISTORY_ROWS + 700;
    feed(&mut e, &lines(n));
    assert_eq!(e.history_len(), HISTORY_ROWS);
    assert_eq!(e.history_rows_entered(), (n - 4) as u64);
    // The history size can no longer show a scroll; the count does.
    feed(&mut e, &lines(3));
    assert_eq!(e.history_len(), HISTORY_ROWS);
    assert_eq!(e.history_rows_entered(), (n - 1) as u64);
    feed(&mut e, "\x1b[4S");
    assert_eq!(e.history_rows_entered(), (n + 3) as u64);
}

#[test]
fn entered_ignores_scrolls_inside_a_region_that_starts_below_the_top() {
    let mut e = AlacrittyEngine::new(6, 10);
    feed(&mut e, "\x1b[2;4r\x1b[4;1H"); // rows 2-4 of 6
    feed(&mut e, &lines(10));
    assert_eq!(e.history_rows_entered(), 0);
    assert_eq!(e.history_len(), 0);
    feed(&mut e, "\x1b[2S\x1b[2T");
    assert_eq!(e.history_rows_entered(), 0);
}

#[test]
fn entered_counts_a_region_that_starts_at_the_top_row() {
    // QUIRK, same as xterm: a region that starts at row 1 but ends above the
    // bottom still sends its scrolled-off rows to history. The count follows
    // the engine, which follows the history.
    let mut e = AlacrittyEngine::new(6, 10);
    feed(&mut e, "\x1b[1;4r\x1b[4;1H");
    feed(&mut e, &lines(10));
    assert_eq!(e.history_rows_entered(), 10);
    assert_eq!(e.history_len(), 10);
}

#[test]
fn entered_counts_delete_lines_on_the_top_row() {
    // QUIRK: alacritty implements DL (`ESC[nM`) as a scroll of the region
    // from the cursor row, so with the cursor on row 1 the deleted rows go
    // to history (xterm drops them). Pinned so a later reader is not
    // surprised; a TUI that uses DL at the top of a full-screen region
    // grows the history.
    let mut e = AlacrittyEngine::new(4, 10);
    feed(&mut e, "a\r\nb\r\nc\r\nd\x1b[1;1H\x1b[2M");
    assert_eq!(e.history_rows_entered(), 2);
    assert_eq!(history(&e), ["a", "b"]);
    // On any other row it is not.
    feed(&mut e, "\x1b[2;1H\x1b[1M");
    assert_eq!(e.history_rows_entered(), 2);
}

#[test]
fn entered_ignores_the_alternate_screen_and_leaving_it() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, &lines(20));
    let (entered, len) = (e.history_rows_entered(), e.history_len());
    assert_eq!((entered, len), (16, 16));
    feed(&mut e, "\x1b[?1049h");
    assert_eq!(e.history_len(), 0, "the alternate grid has no history");
    feed(&mut e, &lines(100));
    feed(&mut e, "\x1b[3S");
    assert_eq!(e.history_rows_entered(), entered);
    feed(&mut e, "\x1b[?1049l");
    assert_eq!(e.history_rows_entered(), entered, "leaving is not a scroll");
    assert_eq!(e.history_len(), len, "the main history is intact");
    feed(&mut e, &lines(2));
    assert_eq!(e.history_rows_entered(), entered + 2);
}

#[test]
fn entered_is_not_a_history_size_and_survives_clears_and_resets() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, &lines(20));
    feed(&mut e, "\x1b[3J");
    assert_eq!(e.history_len(), 0, "ESC[3J drops the history");
    assert_eq!(e.history_rows_entered(), 16, "and is not an entry");
    feed(&mut e, &lines(5));
    assert_eq!(e.history_rows_entered(), 21);
    feed(&mut e, "\x1bc");
    assert_eq!(e.history_len(), 0);
    assert_eq!(
        e.history_rows_entered(),
        21,
        "RIS drops history, not the count"
    );
}

#[test]
fn entered_is_untouched_by_a_resize_which_moves_history_on_its_own() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, "1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!((e.history_rows_entered(), e.history_len()), (0, 0));
    e.resize(3, 10);
    // Rows 1 and 2 entered history, by the resize, not by output: not counted.
    assert_eq!((e.history_rows_entered(), e.history_len()), (0, 2));
    e.resize(5, 10);
    assert_eq!((e.history_rows_entered(), e.history_len()), (0, 0));
    // Reflow can change the history length too (200 chars: 3 rows at 80, 5 at 40).
    let mut e = AlacrittyEngine::new(3, 80);
    feed(&mut e, &format!("{}\r\n\r\n\r\n", long_line()));
    let entered = e.history_rows_entered();
    let len = e.history_len();
    e.resize(3, 40);
    assert_eq!(e.history_rows_entered(), entered);
    assert_ne!(
        e.history_len(),
        len,
        "a reflow moved the history size, not the count"
    );
}

#[test]
fn entered_counts_rows_of_one_call_that_exceed_the_remaining_room() {
    // 4000 rows of history, then one `ESC[4S` per step: the cut-back after
    // each call keeps the count exact right through the bound.
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, &lines(HISTORY_ROWS - 2 + 4));
    assert_eq!(e.history_len(), HISTORY_ROWS - 2);
    feed(&mut e, "\x1b[4S");
    assert_eq!(e.history_len(), HISTORY_ROWS);
    assert_eq!(e.history_rows_entered(), (HISTORY_ROWS - 2 + 4) as u64);
}
