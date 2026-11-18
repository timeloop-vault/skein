//! Gate for the `alacritty_terminal` engine (#793, design T3): text, styling,
//! hyperlinks, history and wide characters.
//!
//! Each test pins what the engine really does. A comment names a shortfall
//! against the design instead of bending the test. The other concerns are in
//! `tests_history.rs` (reflow, rows entering history), `tests_modes.rs`
//! (alternate screen, modes, damage, split input), `tests_sync.rs`
//! (synchronized output), `tests_replies.rs` (device queries) and
//! `tests_guard.rs` (panic survival).
#![allow(clippy::assert_is_empty)]

use super::dump::{CellDump, ColorDump, RowDump};
use super::{AlacrittyEngine, HISTORY_ROWS, TerminalEngine};

pub(super) fn feed(e: &mut AlacrittyEngine, s: &str) -> Vec<u8> {
    e.feed(s.as_bytes())
}

/// A row's text, wide-character spacers skipped, trailing blanks trimmed.
pub(super) fn row_str(r: &RowDump) -> String {
    let mut s = String::new();
    for c in &r.cells {
        s.push_str(&c.text);
    }
    s.trim_end().to_owned()
}

pub(super) fn history(e: &AlacrittyEngine) -> Vec<String> {
    e.dump().history.iter().map(row_str).collect()
}

pub(super) fn screen(e: &AlacrittyEngine) -> Vec<String> {
    e.dump().screen.iter().map(row_str).collect()
}

pub(super) fn wrapped_flags(rows: &[RowDump]) -> Vec<bool> {
    rows.iter().map(|r| r.wrapped).collect()
}

/// `(text, width)` of every cell of a row.
pub(super) fn layout(r: &RowDump) -> Vec<(String, u8)> {
    r.cells.iter().map(|c| (c.text.clone(), c.width)).collect()
}

pub(super) fn lay(items: &[(&str, u8)]) -> Vec<(String, u8)> {
    items.iter().map(|(t, w)| ((*t).to_owned(), *w)).collect()
}

pub(super) fn cell(e: &AlacrittyEngine, row: usize, col: usize) -> CellDump {
    e.dump().screen[row].cells[col].clone()
}

#[test]
fn styled_text_reads_back_per_cell() {
    let mut e = AlacrittyEngine::new(3, 30);
    feed(
        &mut e,
        "\x1b[1;31mR\x1b[0m\x1b[3;4;38;2;1;2;3mG\x1b[0m\x1b[48;5;200;38;5;7mB\x1b[0m\
         \x1b[2;7;8;9mX\x1b[0m\x1b[94mY\x1b[0mZ",
    );
    let red = cell(&e, 0, 0);
    assert_eq!(red.text, "R");
    assert!(red.bold && !red.italic && !red.underline);
    assert_eq!(
        red.fg,
        ColorDump::Palette { index: 1 },
        "SGR 31 reads as palette 1"
    );

    let green = cell(&e, 0, 1);
    assert!(green.italic && green.underline && !green.bold);
    assert_eq!(green.fg, ColorDump::Rgb { r: 1, g: 2, b: 3 });
    assert_eq!(green.bg, ColorDump::Default);

    let blue = cell(&e, 0, 2);
    assert_eq!(blue.bg, ColorDump::Palette { index: 200 });
    assert_eq!(blue.fg, ColorDump::Palette { index: 7 });
    assert!(!blue.bold, "SGR 0 reset the attributes");

    let hidden = cell(&e, 0, 3);
    assert!(hidden.dim && hidden.inverse && hidden.invisible && hidden.strikethrough);
    assert_eq!(hidden.text, "X", "an invisible cell keeps its character");

    assert_eq!(
        cell(&e, 0, 4).fg,
        ColorDump::Palette { index: 12 },
        "SGR 94 is bright blue"
    );
    let plain = cell(&e, 0, 5);
    assert_eq!(
        (plain.fg, plain.bg),
        (ColorDump::Default, ColorDump::Default)
    );
    assert_eq!(e.screen_text()[0], "RGBXYZ");
    // 256-colour and the named 0-15 colours are told apart by nothing a viewer sees.
    feed(&mut e, "\x1b[2;1H\x1b[38;5;1mA\x1b[31mB");
    assert_eq!(cell(&e, 1, 0).fg, cell(&e, 1, 1).fg);
}

#[test]
fn a_long_line_wraps_and_marks_the_continuation_rows() {
    let mut e = AlacrittyEngine::new(5, 10);
    feed(&mut e, &format!("{}\r\nnext", "0123456789abcdefghijklmno"));
    assert_eq!(
        screen(&e),
        ["0123456789", "abcdefghij", "klmno", "next", ""]
    );
    // `wrapped` = this row continues the previous one (xterm.js isWrapped).
    assert_eq!(
        wrapped_flags(&e.dump().screen),
        [false, true, true, false, false]
    );
    // A line that exactly fills the row does not wrap until the next character.
    let mut e = AlacrittyEngine::new(3, 4);
    feed(&mut e, "abcd");
    assert_eq!(e.cursor(), (0, 3), "the cursor waits on the last column");
    feed(&mut e, "e");
    assert_eq!(screen(&e), ["abcd", "e", ""]);
    assert_eq!(wrapped_flags(&e.dump().screen), [false, true, false]);
    // A hard line break after a full row is not a wrap.
    let mut e = AlacrittyEngine::new(3, 4);
    feed(&mut e, "abcd\r\nef");
    assert_eq!(wrapped_flags(&e.dump().screen), [false, false, false]);
}

#[test]
fn rows_scrolled_off_become_history_oldest_first() {
    let mut e = AlacrittyEngine::new(5, 10);
    for i in 0..20 {
        feed(&mut e, &format!("l{i:02}\r\n"));
    }
    let hist = history(&e);
    assert_eq!(hist.len(), 16);
    assert_eq!(hist.first().map(String::as_str), Some("l00"));
    assert_eq!(hist.last().map(String::as_str), Some("l15"));
    assert_eq!(screen(&e), ["l16", "l17", "l18", "l19", ""]);
    assert!(!e.dump().history[0].wrapped);
    assert_eq!(e.history_len(), 16);
}

#[test]
fn history_is_bounded_at_history_rows() {
    let mut e = AlacrittyEngine::new(5, 10);
    let total = HISTORY_ROWS + 1000;
    for i in 0..total {
        feed(&mut e, &format!("n{i}\r\n"));
    }
    let d = e.dump();
    assert_eq!(
        d.history.len(),
        HISTORY_ROWS,
        "exactly the bound, never more"
    );
    assert_eq!(e.history_len(), HISTORY_ROWS);
    // The oldest rows went; the newest history row sits right above the screen.
    let oldest = total - 4 - HISTORY_ROWS;
    assert_eq!(row_str(&d.history[0]), format!("n{oldest}"));
    assert_eq!(
        row_str(&d.history[HISTORY_ROWS - 1]),
        format!("n{}", total - 5)
    );
    assert_eq!(row_str(&d.screen[0]), format!("n{}", total - 4));
    // One chunk that scrolls far more than the bound is cut the same way.
    let mut e = AlacrittyEngine::new(5, 10);
    let mut big = String::new();
    for i in 0..total {
        big.push('m');
        big.push_str(&i.to_string());
        big.push_str("\r\n");
    }
    feed(&mut e, &big);
    assert_eq!(e.history_len(), HISTORY_ROWS);
    assert_eq!(row_str(&e.dump().history[0]), format!("m{oldest}"));
}

#[test]
fn osc8_hyperlink_is_stored_per_cell_and_read_back_exact() {
    let mut e = AlacrittyEngine::new(3, 30);
    // BEL-terminated, ST-terminated, with and without an id, then closed.
    feed(
        &mut e,
        "\x1b]8;id=x;https://a.example/p?q=1&r=2#f\x07link\x1b]8;;\x07 \
         \x1b]8;;http://b.example\x1b\\bb\x1b]8;;\x1b\\ c",
    );
    let d = e.dump();
    let links: Vec<Option<&str>> = d.screen[0].cells[..10]
        .iter()
        .map(|c| c.link.as_deref())
        .collect();
    let first = Some("https://a.example/p?q=1&r=2#f");
    let second = Some("http://b.example");
    assert_eq!(
        links,
        [
            first, first, first, first, None, second, second, None, None, None
        ]
    );
    // The `id=` parameter is kept inside the engine (the cell holds it) but
    // `Cell::hyperlink()` is the only reader and the dump carries the URI
    // alone. A link without an id is given an engine-made one, so two
    // adjacent runs of the same URI are told apart by it, not by the URI.
    // Styling and links are independent.
    feed(&mut e, "\x1b[2;1H\x1b]8;;u://x\x07\x1b[1mB\x1b[0mP");
    let bold_link = cell(&e, 1, 0);
    assert!(bold_link.bold && bold_link.link.as_deref() == Some("u://x"));
    assert!(
        cell(&e, 1, 1).link.is_some(),
        "the link stays open until closed"
    );
    assert_eq!(cell(&e, 2, 0).link, None);
}

#[test]
fn osc8_link_survives_scrolling_into_history() {
    let mut e = AlacrittyEngine::new(2, 10);
    feed(&mut e, "\x1b]8;;http://h\x07ab\x1b]8;;\x07\r\nx\r\ny");
    let h = &e.dump().history[0];
    assert_eq!(h.cells[0].link.as_deref(), Some("http://h"));
    assert_eq!(h.cells[2].link, None);
}

#[test]
fn wide_characters_take_two_cells_with_a_spacer() {
    let mut e = AlacrittyEngine::new(3, 6);
    feed(&mut e, "ab\u{4e2d}\u{4e2d}\u{4e2d}x\u{4e2d}\r\n\u{1f600}!");
    let d = e.dump();
    assert_eq!(
        layout(&d.screen[0]),
        lay(&[
            ("a", 1),
            ("b", 1),
            ("\u{4e2d}", 2),
            ("", 0),
            ("\u{4e2d}", 2),
            ("", 0)
        ])
    );
    // The third CJK character did not fit and wrapped whole to the next row.
    assert_eq!(
        layout(&d.screen[1]),
        lay(&[
            ("\u{4e2d}", 2),
            ("", 0),
            ("x", 1),
            ("\u{4e2d}", 2),
            ("", 0),
            (" ", 1)
        ])
    );
    assert!(d.screen[1].wrapped);
    assert_eq!(
        layout(&d.screen[2])[..3],
        lay(&[("\u{1f600}", 2), ("", 0), ("!", 1)])[..]
    );
    assert_eq!(
        e.screen_text()[1],
        "\u{4e2d}x\u{4e2d}",
        "spacers are skipped in text"
    );
}

#[test]
fn a_wide_character_that_does_not_fit_leaves_a_padding_cell_and_wraps() {
    let mut e = AlacrittyEngine::new(3, 5);
    feed(&mut e, "abcd\u{4e2d}z");
    let d = e.dump();
    // The last column is padding (alacritty's LEADING_WIDE_CHAR_SPACER),
    // reported as a blank one-cell column, not as a spacer half.
    assert_eq!(
        layout(&d.screen[0]),
        lay(&[("a", 1), ("b", 1), ("c", 1), ("d", 1), (" ", 1)])
    );
    assert_eq!(
        layout(&d.screen[1])[..3],
        lay(&[("\u{4e2d}", 2), ("", 0), ("z", 1)])[..]
    );
    assert!(
        d.screen[1].wrapped,
        "the row is a continuation of the padded one"
    );
    assert_eq!(e.screen_text()[0], "abcd");
}

#[test]
fn zero_width_and_combining_characters_stay_with_their_cell() {
    let mut e = AlacrittyEngine::new(3, 8);
    feed(&mut e, "e\u{301}x\u{200b}y\u{1f468}\u{200d}\u{1f469}!");
    let d = e.dump();
    let texts: Vec<&str> = d.screen[0].cells.iter().map(|c| c.text.as_str()).collect();
    // Combining accent and zero-width space join the cell before them. The
    // ZWJ joins the first emoji; the second emoji is its own wide cell
    // (no grapheme-cluster mode, as the design says).
    assert_eq!(
        texts[..8],
        [
            "e\u{301}",
            "x\u{200b}",
            "y",
            "\u{1f468}\u{200d}",
            "",
            "\u{1f469}",
            "",
            "!"
        ]
    );
    assert_eq!(e.cursor(), (0, 7));
    // A zero-width character with nothing before it (column 0) is dropped.
    let mut e = AlacrittyEngine::new(2, 8);
    feed(&mut e, "\u{301}a");
    assert_eq!(e.screen_text()[0], "a");
}

/// A clock that moves only when a test says so.
#[derive(Clone)]
pub(super) struct ManualClock {
    base: std::time::Instant,
    offset: std::sync::Arc<std::sync::Mutex<std::time::Duration>>,
}

impl ManualClock {
    pub(super) fn new() -> Self {
        Self {
            base: std::time::Instant::now(),
            offset: std::sync::Arc::default(),
        }
    }

    pub(super) fn advance(&self, by: std::time::Duration) {
        *self.offset.lock().expect("clock") += by;
    }

    pub(super) fn now(&self) -> std::time::Instant {
        self.base + *self.offset.lock().expect("clock")
    }

    pub(super) fn clock(&self) -> super::alacritty::Clock {
        let c = self.clone();
        std::sync::Arc::new(move || c.now())
    }

    pub(super) fn engine(&self, rows: u16, cols: u16) -> AlacrittyEngine {
        AlacrittyEngine::with_clock(rows, cols, self.clock())
    }
}
