//! Tests for `patterns.rs`, the hand-rolled port of `harnessPatterns.ts`.
//!
//! The crate has no `regex` dependency, so the eight JS regexes are a small
//! backtracking matcher and need their own tests. Every expectation below is
//! GROUND TRUTH, not a reading of the regex: each string was run through the
//! real `WAITING_PATTERNS` (each regex alone, and `matchesWaitingPrompt` for
//! the `any` column) and `stripAnsi` in Node, then pasted in. A row is
//! `(regex index, text, that regex alone matches, matchesWaitingPrompt)`.
//! Per regex: at least three strings that match and three near-misses.

use super::patterns::{PATTERN_COUNT, matches_pattern, matches_waiting_prompt, strip_ansi};

type Row = (usize, &'static str, bool, bool);

/// The cases of regex `k`, checked both against that regex alone and
/// against the public function.
fn check(rows: &[Row], k: usize) {
    assert!(rows.iter().all(|r| r.0 == k));
    assert!(
        rows.iter().filter(|r| r.2).count() >= 3,
        "regex {k}: matches"
    );
    assert!(
        rows.iter().filter(|r| !r.2).count() >= 3,
        "regex {k}: misses"
    );
    for &(i, text, alone, any) in rows {
        assert_eq!(matches_pattern(i, text), alone, "regex {i} alone: {text:?}");
        assert_eq!(matches_waiting_prompt(text), any, "any: {text:?}");
    }
}

// `\((?:y|yes)\/(?:n|no)\)\s*\??\s*$` /im
const RE0: &[Row] = &[
    (0, "Proceed (y/n)", true, true),
    (0, "Install? (yes/no) ", true, true),
    (0, "Delete (Y/N)?\u{a}", true, true),
    (0, "ok (yes/no) ?  \u{a}", true, true),
    (0, "x (y/n)\u{a}more text", true, true),
    (0, "(Y/NO)", true, true),
    (0, "Really?\u{d}\u{a}(y/n)\u{d}\u{a}", true, true),
    (0, "a\u{a0}(y/n)\u{a0}?\u{2028}b", true, true),
    (0, "(y/n) continue", false, false),
    (0, "(maybe/n)", false, false),
    (0, "(y|n)", false, false),
    (0, "(y/n", false, false),
    (0, "(yes/n0)", false, false),
    (0, "(yn)", false, false),
    (0, "(y/n)?? ", false, false),
    (0, "(y/n)\u{180e}", false, false),
    (0, "(y/n) \u{85}x", false, false),
];

#[test]
fn yn_parens_y_first() {
    check(RE0, 0);
}

// `\((?:n|no)\/(?:y|yes)\)\s*\??\s*$` /im
const RE1: &[Row] = &[
    (1, "(n/y)", true, true),
    (1, "Overwrite (no/yes)?", true, true),
    (1, "(N/Y) ", true, true),
    (1, "q (No/Yes) ? \u{a}next", true, true),
    (1, "x(n/yes)\u{9}", true, true),
    (1, "(n/yes", false, false),
    (1, "(no/ye)", false, false),
    (1, "(nope/y)", false, false),
    (1, "(n/y) then", false, false),
    (1, "(n-y)", false, false),
    (1, "(n/y)?!", false, false),
];

#[test]
fn yn_parens_n_first() {
    check(RE1, 1);
}

// `\[(?:y\/N|Y\/n|yes\/no|y\/n)\]\s*\??\s*$` /im
const RE2: &[Row] = &[
    (2, "[y/N]", true, true),
    (2, "[Y/n] ", true, true),
    (2, "[yes/no]?", true, true),
    (2, "[y/n]  ?  ", true, true),
    (2, "[Y/N]", true, true),
    (2, "Proceed [YES/NO]\u{a}", true, true),
    (2, "[y/n]\u{d}", true, true),
    (2, "[y/N] ok", false, false),
    (2, "[yes/n]", false, false),
    (2, "[n/y]", false, false),
    (2, "[y/N", false, false),
    (2, "[y/Ny]", false, false),
    (2, "[y / n]", false, false),
    (2, "y/n]", false, false),
];

#[test]
fn yn_brackets() {
    check(RE2, 2);
}

// `\bpress\s+(?:enter|any\s+key)\b[^\n]*$` /im
const RE3: &[Row] = &[
    (3, "Press Enter to continue", true, true),
    (3, "press any key", true, true),
    (3, "PRESS   ANY   KEY to exit", true, true),
    (3, "foo\u{a}Press enter\u{a}bar", true, true),
    (3, "Press Enter.", true, true),
    (3, "x\u{e9}press enter", true, true),
    (3, "Press\u{9}Enter\u{d}\u{a}foo", true, true),
    (3, "-- press ANY\u{a}key", true, true),
    (3, "Pressing Enter", false, false),
    (3, "compress enter", false, false),
    (3, "press entering", false, false),
    (3, "press anykey", false, false),
    (3, "press any keys", false, false),
    (3, "press the Enter key", false, false),
    (3, "press any \u{212a}ey", false, false),
    (3, "press_enter", false, false),
    (3, "_press enter", false, false),
];

#[test]
fn press_enter_or_any_key() {
    check(RE3, 3);
}

// `^\s*password\s*:\s*$` /im
const RE4: &[Row] = &[
    (4, "Password:", true, true),
    (4, "  password :  ", true, true),
    (4, "PASSWORD:", true, true),
    (4, "login\u{a}Password:", true, true),
    (4, "foo\u{a}\u{a} Password: \u{a}", true, true),
    (4, "Password:\u{a}Welcome back", true, true),
    (4, "\u{a0}Password:\u{a0}", true, true),
    (4, "x\u{d}\u{9}password:", true, true),
    (4, "Enter Password:", false, false),
    (4, "Password: hunter2", false, false),
    (4, "Passwords:", false, false),
    (4, "password", false, false),
    (4, "my password:", false, false),
    (4, "[sudo] password:", false, false),
    (4, "pass word:", false, false),
    (4, "pasSword :x", false, false),
    (4, "pas\u{17f}word:", false, false),
    (4, "Password\u{180e}:", false, false),
];

#[test]
fn bare_password_line() {
    check(RE4, 4);
}

// `\bpassword\s+for\s+\S+\s*:\s*$` /im
const RE5: &[Row] = &[
    (5, "[sudo] password for alice:", true, true),
    (5, "Password for bob@host: ", true, true),
    (5, "password   for\u{9}user :", true, true),
    (5, "password for user:\u{a}", true, true),
    (5, "PASSWORD FOR Root:", true, true),
    (5, "a\u{a}password for u:\u{a}b", true, true),
    (5, "password for user", false, false),
    (5, "password for :", false, false),
    (5, "passwords for alice:", false, false),
    (5, "password for alice: secret", false, false),
    (5, "password fora:", false, false),
    (5, "xpassword for u:", false, false),
    (5, "password for u v:", false, false),
];

#[test]
fn password_for_user() {
    check(RE5, 5);
}

// `\bpassphrase\b[^:\n]*:\s*$` /im
const RE6: &[Row] = &[
    (6, "Enter passphrase for key '/home/u/.ssh/id':", true, true),
    (6, "passphrase:", true, true),
    (6, "Passphrase for /path/to/key:  ", true, true),
    (6, "PASSPHRASE (empty for none):", true, true),
    (6, "passphrase - \u{e9}:\u{a}", true, true),
    (6, "x\u{a}passphrase for k:", true, true),
    (6, "passphrase for key", false, false),
    (6, "passphrases:", false, false),
    (6, "passphrase: abc", false, false),
    (6, "passphrase\u{a}foo:", false, false),
    (6, "my_passphrase:", false, false),
    (6, "passphrase for k: x", false, false),
];

#[test]
fn passphrase() {
    check(RE6, 6);
}

// `\bcontinue\b[^?\n]*\?\s*\[[ynYN/]+\]\s*$` /im
const RE7: &[Row] = &[
    (7, "Continue? [y/N]", true, true),
    (7, "Do you want to continue? [Y/n] ", true, true),
    (7, "continue anyway? [yn]", true, true),
    (7, "CONTINUE ? [y/n]\u{a}", true, true),
    (7, "continue? [YYNN//]", true, true),
    (7, "Continue (really)?\u{9}[ynYN]\u{d}\u{a}", true, true),
    (7, "Continue? [y/N] yes", false, false),
    (7, "continue [yn]", false, false),
    (7, "Continuer? [yn]", false, false),
    (7, "Continue? [x/N]", false, false),
    (7, "continue?? [yn]", false, false),
    (7, "continue\u{a}? [yn]", false, false),
    (7, "discontinue? [yn]", false, false),
    (7, "Continue? []", false, false),
    (7, "Continue? [y n]", false, false),
];

#[test]
fn continue_question_yn() {
    check(RE7, 7);
}

#[test]
fn every_regex_is_covered() {
    assert_eq!(PATTERN_COUNT, 8);
}

/// `(input, stripAnsi(input))`, from Node.
const STRIP: &[(&str, &str)] = &[
    ("\u{1b}[31mred\u{1b}[0m", "red"),
    ("\u{1b}[1;32mgreen\u{1b}[m!", "green!"),
    ("\u{1b}[2K\u{1b}[1A\u{1b}[10;20Hx", "x"),
    ("\u{1b}[?25lhidden\u{1b}[?25h", "hidden"),
    ("\u{1b}]0;my title\u{7}prompt$ ", "prompt$ "),
    ("\u{1b}]2;title\u{1b}\\text", "text"),
    ("\u{1b}]0;title", "]0;title"),
    ("abc\u{1b}", "abc"),
    ("abc\u{1b}[", "abc["),
    (
        "plain text\u{a}\u{9}tabs kept\u{d}\u{a}",
        "plain text\u{a}\u{9}tabs kept\u{d}\u{a}",
    ),
    (
        "a\u{7}b\u{0}c\u{8}d\u{7f}e\u{b}f\u{c}g\u{e}h\u{1f}i",
        "abcdefghi",
    ),
    ("\u{1b}(Bfoo", "(Bfoo"),
    ("\u{1b}]0;a\u{1b}b\u{7}", "]0;ab"),
    ("\u{1b}[1 qx", "x"),
    ("\u{1b}]0;\u{1b}[31mT\u{7}x", "x"),
    (
        "h\u{e9}llo \u{1f600} \u{9b}31m",
        "h\u{e9}llo \u{1f600} \u{9b}31m",
    ),
    ("\u{1b}[31", "[31"),
    ("\u{1b}[38;2;255;0;0mtruecolor\u{1b}[0m", "truecolor"),
    ("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\", "link"),
    ("\u{1b}]0;a\u{7}\u{1b}]0;b\u{7}z", "z"),
    ("\u{1b}\u{1b}[0mq", "q"),
    ("\u{1b}[;H\u{1b}[3J", ""),
];

#[test]
fn strip_ansi_table_matches_node() {
    for &(input, expected) in STRIP {
        assert_eq!(strip_ansi(input), expected, "{input:?}");
    }
}

#[test]
fn strip_ansi_csi_colour() {
    assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
    assert_eq!(strip_ansi("\u{1b}[1;32mgreen\u{1b}[m!"), "green!");
    assert_eq!(
        strip_ansi("\u{1b}[38;2;255;0;0mtruecolor\u{1b}[0m"),
        "truecolor"
    );
}

#[test]
fn strip_ansi_cursor_moves() {
    assert_eq!(strip_ansi("\u{1b}[2K\u{1b}[1A\u{1b}[10;20Hx"), "x");
    assert_eq!(strip_ansi("\u{1b}[?25lhidden\u{1b}[?25h"), "hidden");
    assert_eq!(strip_ansi("\u{1b}[1 qx"), "x");
}

#[test]
fn strip_ansi_osc_title_ended_by_bel() {
    assert_eq!(strip_ansi("\u{1b}]0;my title\u{7}prompt$ "), "prompt$ ");
}

#[test]
fn strip_ansi_osc_title_ended_by_st() {
    assert_eq!(strip_ansi("\u{1b}]2;title\u{1b}\\text"), "text");
}

#[test]
fn strip_ansi_unterminated_osc_loses_only_the_escape() {
    assert_eq!(strip_ansi("\u{1b}]0;title"), "]0;title");
}

#[test]
fn strip_ansi_bare_esc_at_end_of_chunk() {
    assert_eq!(strip_ansi("abc\u{1b}"), "abc");
    assert_eq!(strip_ansi("abc\u{1b}["), "abc[");
    assert_eq!(strip_ansi("\u{1b}[31"), "[31");
}

#[test]
fn strip_ansi_text_with_no_escapes_is_unchanged() {
    for s in [
        "",
        "plain text",
        "plain text\n\ttabs kept\r\n",
        "h\u{e9}llo \u{1f600}",
    ] {
        assert_eq!(strip_ansi(s), s);
    }
}

#[test]
fn strip_ansi_pass_order_csi_then_osc_then_controls() {
    // The CSI inside the OSC goes first, which lets the OSC then close.
    assert_eq!(strip_ansi("\u{1b}]0;\u{1b}[31mT\u{7}x"), "x");
    // An ESC that is not part of `ESC \` keeps the OSC from closing.
    assert_eq!(strip_ansi("\u{1b}]0;a\u{1b}b\u{7}"), "]0;ab");
}

// ----- matches_waiting_prompt: the tail window -----------------------------
// `len` is the JS string length (UTF-16 units); (y/n) at the very start of a
// 256-unit string is still inside the window, one unit more cuts its "(".

#[test]
fn window_is_the_last_256_units() {
    let at_256 = format!("(y/n)\n{}", "x".repeat(250));
    let at_257 = format!("(y/n)\n{}", "x".repeat(251));
    assert_eq!(at_256.len(), 256);
    assert!(matches_waiting_prompt(&at_256));
    assert!(!matches_waiting_prompt(&at_257));
}

#[test]
fn window_hides_an_early_prompt() {
    assert!(!matches_waiting_prompt(&format!(
        "Password:\n{}",
        "x".repeat(300)
    )));
    assert!(matches_waiting_prompt(&format!(
        "Password:\n{}",
        "x".repeat(100)
    )));
}

#[test]
fn window_finds_a_late_prompt_after_a_long_history() {
    assert!(matches_waiting_prompt(&format!(
        "{} (y/n)",
        "x".repeat(300)
    )));
    assert!(matches_waiting_prompt(&format!(
        "{}\nProceed (y/n)",
        "x".repeat(300)
    )));
}

#[test]
fn window_cut_mid_line_starts_a_new_line_for_the_caret() {
    // 257 units: the cut drops "z", leaving "zPassword:": `^` is satisfied
    // by the window start but `\s*` cannot skip the "z".
    assert!(!matches_waiting_prompt(&format!(
        "zzPassword:\n{}",
        "x".repeat(245)
    )));
    // 258 units: the cut lands exactly on the line start.
    assert!(matches_waiting_prompt(&format!(
        "abPassword:\n{}",
        "x".repeat(246)
    )));
}

#[test]
fn window_counts_astral_characters_as_two_units() {
    // 6 + 125 * 2 = 256 units: everything is inside the window.
    assert!(matches_waiting_prompt(&format!(
        "(y/n)\n{}",
        "\u{1f600}".repeat(125)
    )));
    // 258 units: Node drops "(y"; counting scalar values would keep it.
    assert!(!matches_waiting_prompt(&format!(
        "(y/n)\n{}",
        "\u{1f600}".repeat(126)
    )));
}

#[test]
fn window_cut_inside_a_surrogate_pair_leaves_a_blocker() {
    // 257 units: the cut drops the emoji's high half. The lone low half
    // still sits in front of "Password:", so `^\s*` cannot match.
    assert!(!matches_waiting_prompt(&format!(
        "\u{1f600}Password:\n{}",
        "x".repeat(245)
    )));
}

#[test]
fn window_of_empty_input_matches_nothing() {
    assert!(!matches_waiting_prompt(""));
}

#[test]
fn mid_history_prompt_on_an_earlier_line_still_matches_in_multiline_mode() {
    // `$` is per line (the `m` flag), so a prompt followed by more output
    // still matches, whatever the header comment of the TS says.
    assert!(matches_waiting_prompt("Password:\nWelcome back"));
    assert!(matches_waiting_prompt("x (y/n)\nmore text"));
}
