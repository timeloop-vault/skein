//! The encoder over a real engine, and a round trip: the frames are fed to a
//! second engine standing in for the client, which must end up showing what
//! the first one shows.

use std::fmt::Write;

use crate::pty::engine::dump::RowDump;
use crate::pty::engine::{AlacrittyEngine, TerminalEngine};

use super::ViewerState;
use super::test_grid::{body, engine, show};

fn feed(e: &mut AlacrittyEngine, s: &str) {
    e.feed(s.as_bytes());
}

#[test]
fn a_fresh_viewer_of_a_real_engine_gets_the_screen() {
    let e = engine(3, 10, "hello\r\nworld");
    let mut v = ViewerState::new();
    let f = show(&e.frame(&mut v).unwrap());
    assert!(f.contains("^[[1;1H^[[Khello^[[2;1H^[[Kworld"), "{f}");
    assert!(f.contains("^[[2;6H"), "cursor after world: {f}");
    assert!(e.frame(&mut v).is_none());
}

#[test]
fn an_open_synchronized_update_holds_the_frame_back() {
    let mut e = engine(2, 10, "a");
    let mut v = ViewerState::new();
    e.frame(&mut v).unwrap();
    feed(&mut e, "\x1b[?2026hhalf drawn");
    assert!(e.frame(&mut v).is_none());
    feed(&mut e, "\x1b[?2026l");
    let f = body(&e, &mut v).unwrap();
    assert!(f.contains("half drawn"), "{f}");
}

#[test]
fn text_that_wraps_in_the_engine_wraps_in_the_frame() {
    let e = engine(3, 5, "abcdefgh");
    let mut v = ViewerState::new();
    let f = body(&e, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabcdefgh^[[K"), "{f}");
}

#[test]
fn a_wide_character_that_did_not_fit_is_positioned() {
    let e = engine(3, 5, "abcd\u{65e5}");
    let mut v = ViewerState::new();
    let f = body(&e, &mut v).unwrap();
    assert!(f.contains("^[[1;1H^[[Kabcd\u{65e5}^[[2;3H^[[K"), "{f}");
}

#[test]
fn three_bells_are_one_bell_per_viewer() {
    let mut e = engine(2, 5, "");
    let mut a = ViewerState::new();
    e.frame(&mut a).unwrap();
    let mut b = a.clone();
    feed(&mut e, "\x07\x07\x07");
    assert_eq!(body(&e, &mut a).unwrap().matches("^G").count(), 1);
    // The bell was not consumed: another viewer is told too.
    assert_eq!(body(&e, &mut b).unwrap().matches("^G").count(), 1);
    assert!(e.take_bell(), "take_bell still works");
}

#[test]
fn title_and_modes_come_from_the_engine() {
    let mut e = engine(2, 5, "");
    let mut v = ViewerState::new();
    e.frame(&mut v).unwrap();
    feed(
        &mut e,
        "\x1b]2;make\x07\x1b[?1h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[5 q\x1b[?25l",
    );
    let f = body(&e, &mut v).unwrap();
    assert_eq!(
        f,
        "^]2;make^[\\^[[?1h^[[?1002h^[[?1006h^[[?2004h^[[1;1H^[[5 q^[[?25l".replace("^]", "^[]")
    );
}

// --- the round trip ---------------------------------------------------------

/// Where two rows first differ, readable.
fn row_diff(a: &RowDump, b: &RowDump) -> String {
    let text = |r: &RowDump| r.cells.iter().map(|c| c.text.as_str()).collect::<String>();
    let cell = a.cells.iter().zip(&b.cells).position(|(x, y)| x != y);
    format!(
        "wrapped {} vs {}; server {:?}, client {:?}; first differing cell {cell:?}: {:?} vs {:?}",
        a.wrapped,
        b.wrapped,
        text(a),
        text(b),
        cell.map(|i| &a.cells[i]),
        cell.map(|i| &b.cells[i]),
    )
}

fn assert_client_matches(
    server: &AlacrittyEngine,
    client: &AlacrittyEngine,
    at: &str,
    wrap_flags: bool,
) {
    let (mut s, mut c) = (server.dump(), client.dump());
    s.modes.sync_pending = false;
    c.modes.sync_pending = false;
    if !wrap_flags {
        // A diff that rewrites the last cell of a row leaves alacritty (the
        // stand-in client) without its wrap flag on that cell, where xterm.js
        // keeps it on the next row. Only a full redraw is compared for them.
        s.screen
            .iter_mut()
            .chain(c.screen.iter_mut())
            .for_each(|r| r.wrapped = false);
    }
    for (i, (a, b)) in s.screen.iter().zip(&c.screen).enumerate() {
        assert!(a == b, "row {i} differs {at}: {}", row_diff(a, b));
    }
    // The scrollback too (empty on the alternate screen, on both sides). The
    // oldest row's flag says it continues a row that is gone.
    assert_eq!(s.history.len(), c.history.len(), "history length {at}");
    for (i, (a, b)) in s.history.iter().zip(&c.history).enumerate() {
        let (mut a, mut b) = (a.clone(), b.clone());
        if !wrap_flags || i == 0 {
            a.wrapped = false;
            b.wrapped = false;
        }
        assert!(a == b, "history row {i} differs {at}: {}", row_diff(&a, &b));
    }
    assert_eq!(s.cursor, c.cursor, "cursor {at}");
    assert_eq!(s.modes, c.modes, "modes {at}");
    assert_eq!(s.alt_screen, c.alt_screen, "{at}");
    // An empty title and none are the same to a viewer.
    assert_eq!(
        server.title().unwrap_or_default(),
        client.title().unwrap_or_default(),
        "{at}"
    );
}

/// A script that exercises wrapping, wide characters, styles, links,
/// scrolling, erasing, the alternate screen and modes.
const SCRIPT: &[&str] = &[
    "hello world\r\n",
    "\x1b[1;31mred bold\x1b[0m and \x1b[48;5;21mblue bg\x1b[0m\r\n",
    "abcdefghijklmnopqrstuvwxyz0123456789 wraps across the margin of this narrow screen",
    "\r\n\u{65e5}\u{672c}\u{8a9e} wide \u{b1} ambiguous e\u{301} combined\r\n",
    "x\x1b]8;;https://a.test/p?q#f\x07link text\x1b]8;;\x07 after\r\n",
    "\x1b[3;5Hmoved\x1b[K\x1b[2Kcleared\x1b[1;1H",
    "\x1b]2;a title\x07\x1b[?1h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[3 q",
    "1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n",
    "\x07\x07",
    "\x1b[?1049h\x1b[2J\x1b[Halt screen\x1b[2;3Htext\x1b[?25l",
    "\x1b[5;1H\u{65e5}\u{672c}",
    "\x1b[?1049l",
    "back on main\x1b[0K\r\n\x1b[?25h\x1b[?1000l\x1b[?1002h\x1b]2;\x07",
    "\x1b[2Jfull\x1b[H\x1b[?1049h\x1b[?1049l",
    "\x1b[?2026h\x1b[2Jsync\x1b[?2026l",
];

fn run_script(size: (u16, u16), chunk: usize) {
    run_parts(
        size,
        chunk,
        &SCRIPT.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
    );
}

/// Lines that scroll far past a screenful: wrapping, wide, linked, styled,
/// blank.
fn scrolling_script() -> Vec<String> {
    (0..60)
        .map(|i| match i % 6 {
            0 => format!("{i} abcdefghijklmnopqrstuvwxyz0123456789 and some more\r\n"),
            1 => format!("\u{65e5}\u{672c}\u{8a9e} wide {i}\r\n"),
            2 => format!("\x1b]8;;https://a.test/{i}\x07link {i}\x1b]8;;\x07\r\n"),
            3 => "\r\n".to_owned(),
            4 => format!("\x1b[1;31m{i} styled and long enough to wrap\x1b[0m\r\n"),
            _ => format!("row {i}\r\n"),
        })
        .collect()
}

fn run_parts(size: (u16, u16), chunk: usize, script: &[String]) {
    let (rows, cols) = size;
    let mut server = AlacrittyEngine::new(rows, cols);
    let mut client = AlacrittyEngine::new(rows, cols);
    let mut continuous = ViewerState::new();
    for (i, part) in script.iter().enumerate() {
        // Cut every part at `chunk` bytes (on a char boundary), a frame after each.
        let mut rest = part.as_str();
        while !rest.is_empty() {
            let mut cut = chunk.min(rest.len());
            while !rest.is_char_boundary(cut) {
                cut += 1;
            }
            let (head, tail) = rest.split_at(cut);
            rest = tail;
            server.feed(head.as_bytes());
            if let Some(f) = server.frame(&mut continuous) {
                client.feed(&f);
            }
            let at = format!("after part {i} ({head:?}), chunk size {chunk}, size {size:?}");
            assert_client_matches(&server, &client, &at, false);

            // A viewer attaching now, from scratch, ends in the same state.
            let mut fresh = ViewerState::new();
            let mut attached = AlacrittyEngine::new(rows, cols);
            if let Some(f) = server.frame(&mut fresh) {
                attached.feed(&f);
                assert_client_matches(&server, &attached, &format!("fresh {at}"), true);
                if server.dump().alt_screen {
                    // A viewer that attached on the alternate screen knows
                    // nothing of the main screen: no rows, no history counters.
                    fresh.history_len = continuous.history_len;
                    fresh.history_entered = continuous.history_entered;
                    fresh.main_screen.clone_from(&continuous.main_screen);
                }
                assert_eq!(fresh, continuous, "viewer state {at}");
            }
        }
    }
}

#[test]
fn a_continuous_viewer_and_a_fresh_one_agree_with_the_engine_whole_script() {
    run_script((6, 20), usize::MAX);
}

#[test]
fn the_round_trip_holds_when_the_output_is_cut_into_small_chunks() {
    run_script((6, 20), 7);
    run_script((5, 11), 3);
}

#[test]
fn the_round_trip_holds_one_byte_at_a_time() {
    run_script((4, 13), 1);
}

#[test]
fn the_round_trip_holds_for_a_script_that_scrolls_far_past_a_screenful() {
    run_parts((6, 20), usize::MAX, &scrolling_script());
    run_parts((6, 20), 23, &scrolling_script());
    run_parts((4, 13), 5, &scrolling_script());
    run_parts((3, 9), 1, &scrolling_script());
}

/// Main output that scrolls, the alternate screen entered, drawn on and left,
/// and main output that scrolls again, twice over; the second visit enters and
/// leaves inside one chunk.
fn alt_script() -> Vec<String> {
    let lines = |tag: &str, n: usize| -> String {
        let mut s = String::new();
        for i in 0..n {
            let _ = write!(s, "{tag} {i} some text that wraps around\r");
            s.push('\n');
        }
        s
    };
    vec![
        lines("before", 14),
        "\x1b[?1049h\x1b[Halt one\x1b[3;4Hdraw\x1b[?25l".to_owned(),
        "\n\n\n\n\n\n\n\nscrolled in alt\x1b[2;1H\u{65e5}\u{672c}".to_owned(),
        "\x1b[?1049l".to_owned(),
        lines("after", 9),
        "\x1b[?1049hsecond\x1b[?1049l".to_owned(),
        lines("again", 3),
        "\x1b[?1049h\x1b[2Jalt two".to_owned(),
        lines("scroll in alt", 8),
        "\x1b[?1049l\x1b[?25h".to_owned(),
        lines("end", 4),
    ]
}

/// Alongside the continuous and fresh viewers, one that attaches the moment
/// the alternate screen is first active and stays, so it leaves it without
/// ever having seen the main screen.
fn run_alt(size: (u16, u16), chunk: usize) {
    let (rows, cols) = size;
    let mut server = AlacrittyEngine::new(rows, cols);
    let mut client = AlacrittyEngine::new(rows, cols);
    let mut late_client = AlacrittyEngine::new(rows, cols);
    let mut continuous = ViewerState::new();
    let mut late: Option<ViewerState> = None;
    for (i, part) in alt_script().iter().enumerate() {
        for piece in part.as_bytes().chunks(chunk) {
            // A split inside a multi-byte character is fine: the engine
            // decodes across feeds.
            server.feed(piece);
            let at = format!("after part {i}, chunk size {chunk}, size {size:?}");
            if let Some(f) = server.frame(&mut continuous) {
                client.feed(&f);
            }
            assert_client_matches(&server, &client, &at, false);
            if late.is_none() && server.dump().alt_screen {
                late = Some(ViewerState::new());
            }
            if let Some(l) = late.as_mut()
                && let Some(f) = server.frame(l)
            {
                late_client.feed(&f);
            }
            if late.is_some() {
                assert_client_matches(&server, &late_client, &format!("late {at}"), false);
            }
            let mut fresh = ViewerState::new();
            let mut attached = AlacrittyEngine::new(rows, cols);
            if let Some(f) = server.frame(&mut fresh) {
                attached.feed(&f);
                assert_client_matches(&server, &attached, &format!("fresh {at}"), true);
            }
        }
    }
    // Everything agrees with the server once the script is over.
    assert!(!server.dump().alt_screen);
    assert!(late.is_some(), "the script enters the alternate screen");
}

#[test]
fn the_alternate_screen_round_trips_with_main_scrolling_before_and_after() {
    run_alt((6, 20), usize::MAX);
    run_alt((6, 20), 11);
    run_alt((4, 13), 1);
}

/// Resizes in the middle of scrolling, wrapping and the alternate screen. The
/// client is resized before it is fed the frame that follows. A continuous
/// viewer, a fresh one and one that is resized only when the frame arrives
/// all end up showing what the server shows.
fn run_resize(chunk: usize) {
    let mut server = AlacrittyEngine::new(6, 20);
    let mut client = AlacrittyEngine::new(6, 20);
    let mut continuous = ViewerState::new();
    let mut script = scrolling_script();
    script.extend(alt_script());
    let sizes = [(6, 14), (9, 14), (4, 14), (4, 25), (7, 9), (2, 9), (5, 20)];
    for (i, part) in script.iter().enumerate() {
        for piece in part.as_bytes().chunks(chunk) {
            server.feed(piece);
            if let Some(f) = server.frame(&mut continuous) {
                client.feed(&f);
            }
            if i % 5 != 4 {
                continue;
            }
            let (rows, cols) = sizes[(i / 5) % sizes.len()];
            server.resize(rows, cols);
            client.resize(rows, cols);
            let at = format!("after part {i} resized to {rows}x{cols}");
            if let Some(f) = server.frame(&mut continuous) {
                client.feed(&f);
            }
            assert_client_matches(&server, &client, &at, false);
            let mut fresh = ViewerState::new();
            let mut attached = AlacrittyEngine::new(rows, cols);
            attached.feed(&server.frame(&mut fresh).unwrap());
            assert_client_matches(&server, &attached, &format!("fresh {at}"), true);
        }
    }
}

#[test]
fn the_round_trip_holds_across_resizes_of_either_dimension() {
    run_resize(usize::MAX);
    run_resize(13);
}
