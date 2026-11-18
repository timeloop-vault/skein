//! Panic survival (#793, design: "every call is wrapped in `catch_unwind`; a
//! caught panic drops that PTY's engine and rebuilds it empty").
//!
//! What was tried against `alacritty_terminal` 0.26.0 to find a panic:
//! odd sizes (1x1, 0x0, 2x1), a wide character in one column, shrinking with
//! wide characters at the edge, shrinking on the alternate screen, huge CSI
//! counts for every cursor/erase/insert/delete/scroll/tab sequence,
//! scroll regions (valid, inverted, out of range) then a resize, DECALN and
//! insert mode with wide characters, deleting half of a wide character,
//! oversized OSC strings, title and keyboard-mode stack overflows. Findings:
//!
//! - REAL PANIC: the 4097th kitty keyboard push (`ESC[>1u`) with an empty title
//!   stack (`push_keyboard_mode` removes from `title_stack` where it means
//!   `keyboard_mode_stack`). Reproduced below.
//! - HANG (not catchable): resizing a grid that holds text to ONE column
//!   loops forever; a wide character written to a ONE-column grid panics.
//!   Both are avoided by never building a grid narrower than two columns
//!   (`AlacrittyEngine::new`/`resize`), see `tests_history.rs`.
//! - Everything else survived.
#![allow(clippy::assert_is_empty)]

use super::tests::{feed, screen};
use super::{AlacrittyEngine, TerminalEngine};

#[test]
fn the_4097th_kitty_push_panics_in_the_engine_and_the_guard_rebuilds_it() {
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, "text\r\n\r\n\r\n\r\n\r\nmore");
    let entered = e.history_rows_entered();
    assert!(entered > 0);
    // 4096 pushes are within the stack's depth.
    feed(&mut e, &"\x1b[>1u".repeat(4096));
    assert_eq!(e.rebuilds(), 0);
    assert_eq!(screen(&e)[3], "more");
    // The next one removes element 0 of an empty `title_stack`.
    let reply = feed(&mut e, "\x1b[>1u");
    assert!(reply.is_empty());
    assert_eq!(e.rebuilds(), 1, "the panic was caught and counted");
    // The state was dropped and rebuilt empty at the same size...
    assert!(e.screen_text().iter().all(String::is_empty));
    assert_eq!((e.dump().rows, e.dump().cols), (4, 20));
    assert_eq!(e.history_len(), 0);
    assert_eq!(e.modes().kitty_keyboard_flags, 0);
    // ...the monotonic count was kept...
    assert_eq!(e.history_rows_entered(), entered);
    // ...and the engine keeps working.
    feed(&mut e, "alive\r\n\r\n\r\n\r\n");
    assert_eq!(e.history_rows_entered(), entered + 1);
    assert_eq!(e.rebuilds(), 1);
}

#[test]
fn a_panic_inside_the_guard_leaves_the_engine_usable_at_the_last_requested_size() {
    let mut e = AlacrittyEngine::new(4, 20);
    feed(&mut e, "kept?");
    e.resize(7, 33);
    // The same guard every engine call goes through.
    e.run_guarded(|| panic!("injected"));
    assert_eq!(e.rebuilds(), 1);
    assert_eq!((e.dump().rows, e.dump().cols), (7, 33));
    assert!(
        e.screen_text().iter().all(String::is_empty),
        "rebuilt empty"
    );
    assert_eq!(e.cursor(), (0, 0));
    feed(&mut e, "again");
    assert_eq!(screen(&e)[0], "again");
    // Repeated panics keep being absorbed.
    e.run_guarded(|| panic!("injected twice"));
    e.run_guarded(|| {});
    assert_eq!(e.rebuilds(), 2);
}

#[test]
fn every_size_combination_survives_a_resize_with_wide_text_on_both_screens() {
    for from in [(6, 10), (3, 4), (1, 2), (8, 80)] {
        for rows in 1..=5u16 {
            for cols in [0u16, 1, 2, 3, 4, 5, 7, 11] {
                let mut e = AlacrittyEngine::new(from.0, from.1);
                let text =
                    "ab\u{4e2d}\u{4e2d}\u{4e2d}cd\u{1f600}ef\u{301}ghijkl\r\n\u{4e2d}\u{4e2d}\r\nx";
                feed(&mut e, text);
                e.resize(rows, cols);
                feed(&mut e, "\x1b[?1049h\x1b[H");
                feed(&mut e, text);
                e.resize(cols.max(1) + 3, rows.max(1) + 1);
                e.resize(rows, cols);
                feed(&mut e, "\x1b[?1049l");
                feed(&mut e, text);
                e.resize(from.0, from.1);
                let _ = (e.dump(), e.screen_text(), e.modes(), e.take_damage());
                assert_eq!(e.rebuilds(), 0, "from {from:?} via {rows}x{cols}");
            }
        }
    }
}

#[test]
fn hostile_sequences_do_not_panic_or_stall() {
    let big = "999999999";
    let mut seqs: Vec<String> = Vec::new();
    for f in [
        "A", "B", "C", "D", "E", "F", "G", "d", "@", "P", "L", "M", "S", "T", "X", "I", "Z", "g",
    ] {
        seqs.push(format!("\x1b[{big}{f}"));
        seqs.push(format!("\x1b[65535;{big}{f}"));
    }
    for s in [
        "\x1b[999999999;999999999H",
        "\x1b[0;0H",
        "\x1b[5;2r",
        "\x1b[0;0r",
        "\x1b[99999;99999r",
        "\x1b[3;1r\x1b[99T\x1b[99S\x1bM\x1bD\x1bE",
        "\x1b#8",
        "\x1b[4h\x1b[1;1Habc\u{4e2d}\u{4e2d}\u{4e2d}\x1b[4l",
        "\x1b[1;2H\x1b[P\x1b[1X\x1b[3@",
        "\x1b[?1049h\x1b[?1049h\x1b[?1049l\x1b[?1049l\x1b[?1049h",
        "\x1b[?3h\x1b[?3l\x1b[?6h\x1b[1;1H\x1b[?6l",
        "\x1b[22;0t\x1b[23;0t\x1b[23;0t\x1b[22;2t",
        "\x1b7\x1b8\x1b8\x1b[s\x1b[u",
        "\x1b(0lqqk\x1b(B\x0e\x0f\x1bn\x1bo",
        "\x1b[38;2;999;999;999m\x1b[38;5;999m\x1b[48;2m\x1b[58;5;1m",
        "\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17;18;19;20;21;22;23;24;25m",
        "\x1b]8;;\x07\x1b]8;id=;\x07\x1b]8;;x\x07x\x1b]8;;\x07",
        "\x1b]52;c;!!!notbase64!!!\x07\x1b]52;c;?\x07",
        "\x1b]4;999;rgb:zz/zz/zz\x07\x1b]104;999\x07\x1b]10;?\x07",
        "\x1b]0;\x07\x1b]2;\x1b\\\x1b]\x1b\\",
        "\x1bP$qm\x1b\\\x1bP1$r\x1b\\\x1b_x\x1b\\\x1b^x\x1b\\\x1bXx\x1b\\",
        "\x1b[?u\x1b[=0;9u\x1b[<999u\x1b[>0u",
        "\x1b[65535 q\x1b[ q\x1b[0 q",
        "\x1b[?1;2;3;4;5;6;7;8;9;10;1000;1002;1003;1004;1005;1006;1007;1042;2004h",
    ] {
        seqs.push(s.to_owned());
    }
    // Long OSC strings and long runs of one character.
    seqs.push(format!("\x1b]0;{}\x07", "t".repeat(2_000_000)));
    seqs.push(format!(
        "\x1b]8;;{}\x07x\x1b]8;;\x07",
        "u".repeat(1_000_000)
    ));
    seqs.push("\u{4e2d}".repeat(100_000));
    seqs.push("\u{301}".repeat(100_000));

    for (rows, cols) in [(1, 1), (2, 2), (5, 10), (24, 80)] {
        for seq in &seqs {
            let mut e = AlacrittyEngine::new(rows, cols);
            feed(
                &mut e,
                "line\r\n\u{4e2d}\u{4e2d}x\r\nmore text here\r\n1\r\n2\r\n3",
            );
            let started = std::time::Instant::now();
            feed(&mut e, seq);
            e.resize(rows + 1, cols + 3);
            feed(&mut e, seq);
            e.resize(rows, cols);
            let _ = (e.dump(), e.take_damage());
            assert_eq!(
                e.rebuilds(),
                0,
                "{rows}x{cols}: {:?}",
                &seq[..seq.len().min(40)]
            );
            assert!(
                started.elapsed().as_secs() < 5,
                "{rows}x{cols}: stalled on {:?}",
                &seq[..seq.len().min(40)]
            );
        }
    }
}

#[test]
fn a_huge_repeat_count_stays_cheap() {
    // `CSI n b` repeats the previous character n times. 65535 is the largest
    // count a parameter holds, so one sequence costs at most that much work;
    // timed here so a change in the library shows up as a slow test.
    let mut e = AlacrittyEngine::new(24, 80);
    let started = std::time::Instant::now();
    feed(&mut e, "x\x1b[65535b".repeat(200).as_str());
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    assert_eq!(e.rebuilds(), 0);
}
