//! The `Handler` wrapper in `counting.rs` forwards 71 `vte::ansi::Handler`
//! methods by an explicit list. `Handler`'s defaults do nothing, so a method
//! missing from the list would be silently dropped. These tests make that
//! visible (#793):
//!
//! 1. the vte and `alacritty_terminal` versions the list was checked against
//!    are pinned (read from the repo's `Cargo.lock`, embedded at compile
//!    time), so a bump fails here and says to re-diff the list;
//! 2. every forwarded method has a row in the table below, and the reverse;
//! 3. each row's bytes are run through the wrapped engine and through a bare
//!    `Term` fed by the same `Processor`: grid, history, cursor, modes,
//!    cursor style, title, bell and replies must be identical.
//!
//! What this cannot do: notice a method a future vte adds to `Handler` (only
//! the version pin does), or detect a dropped method whose effect the bare
//! `Term` does not show either. Those rows have `effect: false` and are
//! marked in the table; `substitute`, `unset_keypad_application_mode` and
//! `pop_keyboard_modes` are only half-visible by themselves (their partner
//! sequences are covered), the rest (colour set/reset/query, clipboard,
//! pixel size, mouse icon, modifyOtherKeys, SCP) are not covered at all.
#![allow(clippy::assert_is_empty)]

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::alacritty::Listener;
use super::counting::FORWARDED;
use super::tests::feed;
use super::{AlacrittyEngine, HISTORY_ROWS};

const ROWS: u16 = 4;
const COLS: u16 = 10;
const SEED: &str = "abc\r\ndef\r\nghi\x1b[1;1H";

struct Case {
    methods: &'static [&'static str],
    bytes: &'static str,
    /// The sequence changes something this test can see.
    effect: bool,
}

const fn case(methods: &'static [&'static str], bytes: &'static str, effect: bool) -> Case {
    Case {
        methods,
        bytes,
        effect,
    }
}

fn table() -> Vec<Case> {
    vec![
        case(&["set_title"], "\x1b]0;t\x07", true),
        case(&["set_cursor_style"], "\x1b[5 q", true),
        case(&["set_cursor_shape"], "\x1b]50;CursorShape=1\x07", true),
        case(&["input"], "x", true),
        case(&["goto"], "\x1b[2;3H", true),
        case(&["goto_line"], "\x1b[3d", true),
        case(&["goto_col"], "\x1b[4G", true),
        case(&["insert_blank"], "\x1b[2@", true),
        case(&["move_up"], "\x1b[3;1H\x1b[A", true),
        case(&["move_down"], "\x1b[B", true),
        case(&["identify_terminal"], "\x1b[c\x1b[>c", true),
        case(&["device_status"], "\x1b[6n", true),
        case(&["move_forward"], "\x1b[3C", true),
        case(&["move_backward"], "\x1b[3;5H\x1b[2D", true),
        case(&["move_down_and_cr"], "\x1b[1;3H\x1b[E", true),
        case(&["move_up_and_cr"], "\x1b[3;3H\x1b[F", true),
        case(&["put_tab"], "\t", true),
        case(&["backspace"], "\x1b[2;3H\x08", true),
        case(&["carriage_return"], "\x1b[2;3H\r", true),
        case(&["linefeed"], "\n", true),
        case(&["bell"], "\x07", true),
        // Term only logs a warning for SUB.
        case(&["substitute"], "\x1a", false),
        case(&["newline"], "\x1bE", true),
        case(
            &["set_horizontal_tabstop"],
            "\x1b[1;4H\x1bH\x1b[1;1H\t",
            true,
        ),
        case(&["scroll_up"], "\x1b[2S", true),
        case(&["scroll_down"], "\x1b[2T", true),
        case(&["insert_blank_lines"], "\x1b[L", true),
        case(&["delete_lines"], "\x1b[2;1H\x1b[M", true),
        case(&["erase_chars"], "\x1b[2X", true),
        case(&["delete_chars"], "\x1b[P", true),
        case(&["move_backward_tabs"], "\x1b[1;12H\x1b[Z", true),
        case(&["move_forward_tabs"], "\x1b[2I", true),
        case(
            &["save_cursor_position", "restore_cursor_position"],
            "\x1b[2;2H\x1b7\x1b[1;1H\x1b8x",
            true,
        ),
        case(&["clear_line"], "\x1b[2;2H\x1b[K", true),
        case(&["clear_screen"], "\x1b[2J", true),
        case(&["clear_tabs"], "\x1b[3g\x1b[1;1H\t", true),
        case(&["set_tabs"], "\x1b[3g\x1b[?5W\x1b[1;1H\t", true),
        case(&["reset_state"], "\x1bc", true),
        case(&["reverse_index"], "\x1bM", true),
        case(&["terminal_attribute"], "\x1b[1;31mx", true),
        case(
            &["set_mode", "unset_mode"],
            "\x1b[4h\x1b[20hx\x1b[20l",
            true,
        ),
        case(&["report_mode"], "\x1b[4$p", true),
        case(
            &["set_private_mode", "unset_private_mode"],
            "\x1b[?1h\x1b[?2004h\x1b[?25l\x1b[?2004l",
            true,
        ),
        case(&["report_private_mode"], "\x1b[?25$p", true),
        case(&["set_scrolling_region"], "\x1b[2;3r\x1b[3;1H\n\n", true),
        case(&["set_keypad_application_mode"], "\x1b=", true),
        case(&["unset_keypad_application_mode"], "\x1b=\x1b>", false),
        case(
            &["configure_charset", "set_active_charset"],
            "\x1b)0\x0elqk",
            true,
        ),
        // Colour changes, the clipboard, pixel size, the mouse pointer icon,
        // modifyOtherKeys and SCP are not kept in the grid and their events
        // go nowhere (the listener drops them), so a dropped call is not
        // observable. They are NOT covered.
        case(&["set_color"], "\x1b]4;1;rgb:ff/00/00\x07", false),
        case(&["reset_color"], "\x1b]104;1\x07", false),
        case(&["dynamic_color_sequence"], "\x1b]10;?\x07", false),
        case(&["clipboard_store"], "\x1b]52;c;aGk=\x07", false),
        case(&["clipboard_load"], "\x1b]52;c;?\x07", false),
        case(&["decaln"], "\x1b#8", true),
        case(
            &["push_title", "pop_title"],
            "\x1b]0;a\x07\x1b[22t\x1b]0;b\x07\x1b[23t",
            true,
        ),
        case(&["text_area_size_pixels"], "\x1b[14t", false),
        case(&["text_area_size_chars"], "\x1b[18t", true),
        case(&["set_hyperlink"], "\x1b]8;;http://x\x07h", true),
        case(&["set_mouse_cursor_icon"], "\x1b]22;text\x1b\\", false),
        case(&["report_keyboard_mode"], "\x1b[?u", true),
        case(&["push_keyboard_mode"], "\x1b[>1u", true),
        case(&["pop_keyboard_modes"], "\x1b[>1u\x1b[<u", false),
        case(&["set_keyboard_mode"], "\x1b[=3u", true),
        case(&["set_modify_other_keys"], "\x1b[>4;2m", false),
        case(&["report_modify_other_keys"], "\x1b[?4m", false),
        case(&["set_scp"], "\x1b[1;0 k", false),
    ]
}

/// Everything the `Term` and its listener show of a run.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    grid: Vec<String>,
    cursor: String,
    modes: u32,
    style: String,
    title: Option<String>,
    bell: bool,
    replies: Vec<u8>,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn seen(term: &Term<Listener>, title: Option<String>, bell: bool, replies: Vec<u8>) -> Seen {
    let grid = term.grid();
    let mut rows = Vec::new();
    for line in -(grid.history_size() as i32)..grid.screen_lines() as i32 {
        let mut row = String::new();
        for col in 0..grid.columns() {
            let c = &grid[Line(line)][Column(col)];
            let _ = write!(
                row,
                "{:?}/{:?}/{:?}/{}/{:?}/{:?};",
                c.c,
                c.fg,
                c.bg,
                c.flags.bits(),
                c.zerowidth(),
                c.hyperlink().map(|h| h.uri().to_owned())
            );
        }
        rows.push(row);
    }
    Seen {
        grid: rows,
        cursor: format!("{:?}/{}", grid.cursor.point, grid.cursor.input_needs_wrap),
        modes: term.mode().bits(),
        style: format!("{:?}", term.cursor_style()),
        title,
        bell,
        replies,
    }
}

fn through_engine(bytes: &str) -> Seen {
    let mut e = AlacrittyEngine::new(ROWS, COLS);
    feed(&mut e, SEED);
    let _ = e.take_bell();
    let replies = feed(&mut e, bytes);
    let bell = e.take_bell();
    seen(e.term(), e.title(), bell, replies)
}

fn through_bare_term(bytes: &str) -> Seen {
    let listener = Listener::default();
    let config = Config {
        scrolling_history: HISTORY_ROWS,
        kitty_keyboard: true,
        ..Config::default()
    };
    let mut term = Term::new(
        config,
        &TermSize::new(usize::from(COLS), usize::from(ROWS)),
        listener.clone(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, SEED.as_bytes());
    listener.pending().replies.clear();
    listener.pending().bell = false;
    parser.advance(&mut term, bytes.as_bytes());
    let (title, bell, replies) = {
        let mut p = listener.pending();
        (p.title.clone(), p.bell, std::mem::take(&mut p.replies))
    };
    seen(&term, title, bell, replies)
}

#[test]
fn the_forward_list_was_checked_against_these_versions() {
    // If this fails, re-diff the `forward!` list in counting.rs against
    // `vte::ansi::Handler` in the new version before changing the numbers.
    let lock = include_str!("../../../../../Cargo.lock").replace('\r', "");
    assert!(
        lock.contains("name = \"vte\"\nversion = \"0.15.0\"\n"),
        "vte moved"
    );
    assert!(
        lock.contains("name = \"alacritty_terminal\"\nversion = \"0.26.0\"\n"),
        "alacritty_terminal moved"
    );
    assert_eq!(FORWARDED.len(), 71);
    assert_eq!(
        FORWARDED.iter().collect::<BTreeSet<_>>().len(),
        71,
        "no duplicates"
    );
}

#[test]
fn every_forwarded_method_has_a_row_and_every_row_a_forwarded_method() {
    let forwarded: BTreeSet<&str> = FORWARDED.iter().copied().collect();
    let rows: BTreeSet<&str> = table()
        .iter()
        .flat_map(|c| c.methods.iter().copied())
        .collect();
    assert_eq!(
        forwarded.difference(&rows).collect::<Vec<_>>(),
        Vec::<&&str>::new()
    );
    assert_eq!(
        rows.difference(&forwarded).collect::<Vec<_>>(),
        Vec::<&&str>::new()
    );
}

#[test]
fn the_wrapped_engine_behaves_exactly_like_a_bare_term_for_every_method() {
    let baseline = through_bare_term("");
    let mut problems = Vec::new();
    for c in table() {
        let wrapped = through_engine(c.bytes);
        let bare = through_bare_term(c.bytes);
        if wrapped != bare {
            problems.push(format!(
                "{:?}: the wrapper differs from a bare Term",
                c.methods
            ));
        }
        if (bare != baseline) != c.effect {
            problems.push(format!(
                "{:?}: `effect` is {} but the bytes {} change what the test sees",
                c.methods,
                c.effect,
                if c.effect { "do not" } else { "do" }
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
