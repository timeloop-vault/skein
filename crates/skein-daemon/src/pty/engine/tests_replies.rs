//! Replies to device queries, which `feed` returns for the daemon to write
//! back to the PTY (#793, design: "device-query replies delivered through its
//! event listener" was claimed, not verified), plus title and bell.
#![allow(clippy::assert_is_empty)]

use super::tests::{feed, screen};
use super::{AlacrittyEngine, TerminalEngine};

fn reply(setup: &str, query: &str) -> Vec<u8> {
    let mut e = AlacrittyEngine::new(5, 20);
    feed(&mut e, setup);
    feed(&mut e, query)
}

#[test]
fn cursor_position_report_is_one_based_row_then_column() {
    assert_eq!(reply("", "\x1b[6n"), b"\x1b[1;1R");
    assert_eq!(reply("abc", "\x1b[6n"), b"\x1b[1;4R");
    assert_eq!(reply("\x1b[4;9H", "\x1b[6n"), b"\x1b[4;9R");
    // In the pending-wrap state at the last column the report names that column.
    assert_eq!(reply(&"x".repeat(20), "\x1b[6n"), b"\x1b[1;20R");
}

#[test]
fn device_attributes_primary_and_secondary() {
    // Primary: a VT102 (`6`), nothing more. xterm.js answers `?1;2c`; a TUI
    // that probes for features sees fewer here.
    assert_eq!(reply("", "\x1b[c"), b"\x1b[?6c");
    assert_eq!(reply("", "\x1b[0c"), b"\x1b[?6c");
    // Secondary: `>0;<version>;1c`, the version being 0.26.0 as 2600.
    assert_eq!(reply("", "\x1b[>c"), b"\x1b[>0;2600;1c");
    assert_eq!(reply("", "\x1b[>0c"), b"\x1b[>0;2600;1c");
}

#[test]
fn kitty_keyboard_query_reports_the_flags_in_force() {
    assert_eq!(reply("", "\x1b[?u"), b"\x1b[?0u");
    assert_eq!(reply("\x1b[>1u", "\x1b[?u"), b"\x1b[?1u");
    assert_eq!(reply("\x1b[>1u\x1b[>5u", "\x1b[?u"), b"\x1b[?5u");
    assert_eq!(reply("\x1b[>5u\x1b[<u", "\x1b[?u"), b"\x1b[?0u");
}

#[test]
fn other_reports() {
    assert_eq!(reply("", "\x1b[5n"), b"\x1b[0n", "device status: ready");
    assert_eq!(
        reply("", "\x1b[18t"),
        b"\x1b[8;5;20t",
        "text area in characters"
    );
    // DECRQM: `?<mode>;<state>$y`, 1 set, 2 reset, 0 unknown.
    assert_eq!(reply("", "\x1b[?25$p"), b"\x1b[?25;1$y");
    assert_eq!(reply("\x1b[?25l", "\x1b[?25$p"), b"\x1b[?25;2$y");
    assert_eq!(reply("\x1b[?2004h", "\x1b[?2004$p"), b"\x1b[?2004;1$y");
    assert_eq!(reply("", "\x1b[?9999$p"), b"\x1b[?9999;0$y");
    // Synchronized output reports as supported but never "set".
    assert_eq!(reply("", "\x1b[?2026$p"), b"\x1b[?2026;2$y");
}

#[test]
fn queries_the_engine_does_not_answer() {
    // SHORTFALL: no reply to colour queries (OSC 4/10/11), pixel size
    // (`CSI 14 t`), XTVERSION (`CSI > q`) or tertiary attributes. The engine
    // hands colour and size requests to an embedder as events; this embedder
    // answers none. xterm.js answers the colour and size ones today.
    for q in [
        "\x1b]10;?\x07",
        "\x1b]11;?\x07",
        "\x1b]4;1;?\x07",
        "\x1b[14t",
        "\x1b[>q",
        "\x1b[=c",
    ] {
        assert!(reply("", q).is_empty(), "{q:?}");
    }
}

#[test]
fn several_queries_in_one_chunk_answer_in_order() {
    let mut e = AlacrittyEngine::new(5, 20);
    assert_eq!(
        feed(&mut e, "ab\x1b[6ncd\x1b[6n\x1b[c"),
        b"\x1b[1;3R\x1b[1;5R\x1b[?6c"
    );
    assert_eq!(screen(&e)[0], "abcd", "a query writes nothing to the grid");
    assert!(feed(&mut e, "ef").is_empty(), "replies are taken once");
}

#[test]
fn a_query_split_across_feeds_answers_when_it_completes() {
    for q in ["\x1b[6n", "\x1b[>c", "\x1b[?u", "\x1b[?25$p"] {
        for split in 1..q.len() {
            let mut e = AlacrittyEngine::new(5, 20);
            assert!(
                e.feed(&q.as_bytes()[..split]).is_empty(),
                "{q:?} split at {split}"
            );
            let _ = (e.dump(), e.modes());
            let r = e.feed(&q.as_bytes()[split..]);
            assert_eq!(r, reply("", q), "{q:?} split at {split}");
            assert!(!r.is_empty());
        }
    }
}

#[test]
fn title_and_bell_are_available_to_the_daemon() {
    let mut e = AlacrittyEngine::new(5, 20);
    assert_eq!(e.title(), None);
    feed(&mut e, "\x1b]0;first\x07");
    assert_eq!(e.title().as_deref(), Some("first"));
    feed(&mut e, "\x1b]2;second\x1b\\");
    assert_eq!(e.title().as_deref(), Some("second"));
    assert!(!e.take_bell());
    feed(&mut e, "\x07\x07");
    assert!(e.take_bell(), "several bells read as one");
    assert!(!e.take_bell(), "and reading clears it");
    // A bell that ends an OSC is not a bell.
    feed(&mut e, "\x1b]0;third\x07");
    assert!(!e.take_bell());
}
