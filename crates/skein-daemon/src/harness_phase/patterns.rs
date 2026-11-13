//! Generic prompt detection for harness kinds without a native adapter
//! (epic #50 L2b). Port of `app/src/harnessPatterns.ts`.
//!
//! Claude (L2c-1) and opencode (L2c-2) signal `waiting` via their own
//! authoritative event sources. The remaining kinds (copilot, byoh: shell,
//! custom commands) have no structured signal, just bytes flowing through
//! the PTY. This module is the fallback: a small curated set of patterns
//! that match commonly-blocking prompts (sudo password, [y/n] confirmation,
//! "Press Enter to continue").
//!
//! Scope is deliberately narrow. We don't try to detect "the user is at a
//! shell prompt": that's not a blocked state, that's idle. We only fire on
//! prompts that indicate a *child process is waiting on user input*. False
//! positives are worse than false negatives here: missing a prompt just
//! means the L2a 8 s idle threshold catches it; misclassifying every shell
//! prompt as "waiting" would pulse the dot every time you sit at a terminal.
//!
//! Patterns are anchored at end-of-tail with `$` (multiline). Mid-history
//! mentions of "password:" don't trigger: the prompt has to be the last
//! thing on the screen.
//!
//! No `regex` crate is a dependency of this crate, so the eight TS regexes
//! are written as a tiny backtracking matcher (a continuation-passing walk
//! over [`Node`]s) that reproduces the JS semantics they rely on: the `i`
//! flag (ASCII-only case folding, no `u` flag), the `m` flag (`^`/`$` at
//! line terminators `\n \r    `), JS `\s` (not Rust's
//! `char::is_whitespace`) and ASCII `\b`.

// The doc comments are the ported spec and quote TS identifiers verbatim.
#![allow(clippy::doc_markdown)]

use std::sync::OnceLock;

/// Strip ANSI escape sequences + bare control chars from raw PTY
/// output so the pattern matcher sees plain text. Claude / opencode
/// TUIs are heavy escape-sequence emitters, but those kinds use the
/// L2c adapters and skip this path; this strip handles whatever a
/// shell or non-TUI CLI might emit (color codes, bell, cursor
/// moves, OSC for terminal-title updates).
///
/// Three passes in the TS order: CSI (`ESC [ <0x30-0x3f>* <0x20-0x2f>*
/// <0x40-0x7e>`), then OSC (`ESC ] [^BEL ESC]* (BEL | ESC \)`), then bare
/// control characters. Preserves `\n \r \t`: newlines matter for
/// end-of-line anchoring in [`matches_waiting_prompt`]; tabs survive for
/// prompt formatting.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let no_csi = remove_matches(&chars, csi_len);
    let no_osc = remove_matches(&no_csi, osc_len);
    no_osc
        .into_iter()
        .filter(|c| {
            !matches!(*c, '\u{00}'..='\u{08}' | '\u{0b}' | '\u{0c}' | '\u{0e}'..='\u{1f}' | '\u{7f}')
        })
        .collect()
}

/// Global replace with "": at each position try `len_at`; on a match skip
/// that many chars, else keep the char (leftmost, non-overlapping).
fn remove_matches(chars: &[char], len_at: fn(&[char], usize) -> Option<usize>) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(n) = len_at(chars, i) {
            i += n;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]`. The three classes are
/// disjoint, so there is nothing to backtrack over.
fn csi_len(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'\u{1b}') || c.get(i + 1) != Some(&'[') {
        return None;
    }
    let mut p = i + 2;
    while c.get(p).is_some_and(|x| ('\u{30}'..='\u{3f}').contains(x)) {
        p += 1;
    }
    while c.get(p).is_some_and(|x| ('\u{20}'..='\u{2f}').contains(x)) {
        p += 1;
    }
    c.get(p)
        .is_some_and(|x| ('\u{40}'..='\u{7e}').contains(x))
        .then(|| p + 1 - i)
}

/// `\x1b\][^\x07\x1b]*(\x07|\x1b\\)`.
fn osc_len(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'\u{1b}') || c.get(i + 1) != Some(&']') {
        return None;
    }
    let mut p = i + 2;
    while c.get(p).is_some_and(|x| *x != '\u{07}' && *x != '\u{1b}') {
        p += 1;
    }
    match c.get(p) {
        Some('\u{07}') => Some(p + 1 - i),
        Some('\u{1b}') if c.get(p + 1) == Some(&'\\') => Some(p + 2 - i),
        _ => None,
    }
}

/// A single-character matcher.
#[derive(Clone, Copy)]
enum Cls {
    /// JS `\s`.
    Space,
    /// JS `\S`.
    NotSpace,
    /// `[^...]`, listing the excluded chars.
    Not(&'static [char]),
    /// `[...]`, listing the allowed chars.
    OneOf(&'static [char]),
}

enum Node {
    /// One literal char, case-insensitive (the `i` flag, ASCII folding).
    Ch(char),
    /// `cls{min,}` greedy with backtracking; `max` is unbounded when `usize::MAX`.
    Rep(Cls, usize, usize),
    /// `(?:a|b|...)`.
    Alt(Vec<Vec<Node>>),
    /// `\b`.
    WordB,
    /// `^` with the `m` flag.
    Bol,
    /// `$` with the `m` flag.
    Eol,
}

/// JS `\s`: WhiteSpace and LineTerminator of ECMA-262.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

const fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

const fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Cls {
    fn matches(self, c: char) -> bool {
        match self {
            Self::Space => is_js_space(c),
            Self::NotSpace => !is_js_space(c),
            Self::Not(set) => !set.contains(&c),
            Self::OneOf(set) => set.iter().any(|x| x.eq_ignore_ascii_case(&c)),
        }
    }
}

/// Match `nodes` at `pos`, then hand the end position to `k`; true when
/// any way of matching satisfies `k`. Greedy repetition backs off one char
/// at a time, like the JS engine.
fn run(nodes: &[Node], s: &[char], pos: usize, k: &dyn Fn(usize) -> bool) -> bool {
    let Some((node, rest)) = nodes.split_first() else {
        return k(pos);
    };
    match node {
        Node::Ch(c) => {
            s.get(pos).is_some_and(|x| x.eq_ignore_ascii_case(c)) && run(rest, s, pos + 1, k)
        }
        Node::Rep(cls, min, max) => {
            let mut n = 0;
            while n < *max && s.get(pos + n).is_some_and(|x| cls.matches(*x)) {
                n += 1;
            }
            (*min..=n)
                .rev()
                .any(|take| n >= *min && run(rest, s, pos + take, k))
        }
        Node::Alt(alts) => alts
            .iter()
            .any(|alt| run(alt, s, pos, &|p| run(rest, s, p, k))),
        Node::WordB => {
            let before = pos > 0 && s.get(pos - 1).is_some_and(|c| is_word(*c));
            let after = s.get(pos).is_some_and(|c| is_word(*c));
            before != after && run(rest, s, pos, k)
        }
        Node::Bol => {
            (pos == 0 || s.get(pos - 1).is_some_and(|c| is_line_terminator(*c)))
                && run(rest, s, pos, k)
        }
        Node::Eol => {
            (pos == s.len() || s.get(pos).is_some_and(|c| is_line_terminator(*c)))
                && run(rest, s, pos, k)
        }
    }
}

/// `pattern.test(s)`: a match starting at any position.
fn test(pattern: &[Node], s: &[char]) -> bool {
    (0..=s.len()).any(|start| run(pattern, s, start, &|_| true))
}

fn lit(text: &str) -> Vec<Node> {
    text.chars().map(Node::Ch).collect()
}

fn seq(parts: Vec<Vec<Node>>) -> Vec<Node> {
    parts.into_iter().flatten().collect()
}

fn one(n: Node) -> Vec<Node> {
    vec![n]
}

const fn space_star() -> Node {
    Node::Rep(Cls::Space, 0, usize::MAX)
}

/// `\s*\??\s*$`, the shared tail of the three `(y/n)` patterns.
fn yn_tail() -> Vec<Node> {
    vec![
        space_star(),
        Node::Alt(vec![vec![Node::Ch('?')], vec![]]),
        space_star(),
        Node::Eol,
    ]
}

/// The curated regex set. Each entry must anchor at end-of-line, avoid
/// matching plain shell prompts (`$`, `%`, `#`, `>` alone) and be
/// case-insensitive. Adding a new pattern: prefer narrow over broad. A
/// false positive here lights the dot up incorrectly across every shell
/// session.
fn waiting_patterns() -> &'static [Vec<Node>] {
    static PATTERNS: OnceLock<Vec<Vec<Node>>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let alt = |words: &[&str]| Node::Alt(words.iter().map(|w| lit(w)).collect());
        vec![
            // (y/n), (yes/no), [y/N], [Y/n]: apt, npm, custom prompts.
            // /\((?:y|yes)\/(?:n|no)\)\s*\??\s*$/im
            seq(vec![
                lit("("),
                one(alt(&["y", "yes"])),
                lit("/"),
                one(alt(&["n", "no"])),
                lit(")"),
                yn_tail(),
            ]),
            // /\((?:n|no)\/(?:y|yes)\)\s*\??\s*$/im
            seq(vec![
                lit("("),
                one(alt(&["n", "no"])),
                lit("/"),
                one(alt(&["y", "yes"])),
                lit(")"),
                yn_tail(),
            ]),
            // /\[(?:y\/N|Y\/n|yes\/no|y\/n)\]\s*\??\s*$/im
            seq(vec![
                lit("["),
                one(alt(&["y/N", "Y/n", "yes/no", "y/n"])),
                lit("]"),
                yn_tail(),
            ]),
            // "Press Enter to continue", "Press any key": CLI pause prompts.
            // /\bpress\s+(?:enter|any\s+key)\b[^\n]*$/im
            seq(vec![
                one(Node::WordB),
                lit("press"),
                one(Node::Rep(Cls::Space, 1, usize::MAX)),
                one(Node::Alt(vec![
                    lit("enter"),
                    seq(vec![
                        lit("any"),
                        one(Node::Rep(Cls::Space, 1, usize::MAX)),
                        lit("key"),
                    ]),
                ])),
                one(Node::WordB),
                one(Node::Rep(Cls::Not(&['\n']), 0, usize::MAX)),
                one(Node::Eol),
            ]),
            // "Password:", "[sudo] password for user:": sudo, ssh, custom auth.
            // /^\s*password\s*:\s*$/im
            seq(vec![
                one(Node::Bol),
                vec![space_star()],
                lit("password"),
                vec![space_star()],
                lit(":"),
                vec![space_star()],
                one(Node::Eol),
            ]),
            // /\bpassword\s+for\s+\S+\s*:\s*$/im
            seq(vec![
                one(Node::WordB),
                lit("password"),
                one(Node::Rep(Cls::Space, 1, usize::MAX)),
                lit("for"),
                one(Node::Rep(Cls::Space, 1, usize::MAX)),
                one(Node::Rep(Cls::NotSpace, 1, usize::MAX)),
                vec![space_star()],
                lit(":"),
                vec![space_star()],
                one(Node::Eol),
            ]),
            // "passphrase for /path:": ssh-add, git over ssh.
            // /\bpassphrase\b[^:\n]*:\s*$/im
            seq(vec![
                one(Node::WordB),
                lit("passphrase"),
                one(Node::WordB),
                one(Node::Rep(Cls::Not(&[':', '\n']), 0, usize::MAX)),
                lit(":"),
                vec![space_star()],
                one(Node::Eol),
            ]),
            // "Continue? [y/N]": git, gh, custom CLIs.
            // /\bcontinue\b[^?\n]*\?\s*\[[ynYN/]+\]\s*$/im
            seq(vec![
                one(Node::WordB),
                lit("continue"),
                one(Node::WordB),
                one(Node::Rep(Cls::Not(&['?', '\n']), 0, usize::MAX)),
                lit("?"),
                vec![space_star()],
                lit("["),
                one(Node::Rep(
                    Cls::OneOf(&['y', 'n', 'Y', 'N', '/']),
                    1,
                    usize::MAX,
                )),
                lit("]"),
                vec![space_star()],
                one(Node::Eol),
            ]),
        ]
    })
}

/// Test the tail of a harness's output buffer for a "waiting-on-user-input"
/// prompt. Only scans the last ~256 chars: the prompt is by definition the
/// last thing on screen, and limiting the scan keeps the matcher cheap (the
/// L2a tick calls this every second for every non-authoritative harness).
///
/// Returns `true` if any curated pattern matches.
///
/// The window is the last 256 UTF-16 code units, as in TS (`slice(-256)`),
/// so an astral character counts two. When the cut falls between the two
/// halves of one, TS is left with a lone surrogate at index 0 that no
/// pattern can match but that still blocks `^` and `\s*` from reaching past
/// it; U+FFFD stands in for it (not whitespace, not a word, not a line
/// terminator).
#[must_use]
pub fn matches_waiting_prompt(tail: &str) -> bool {
    let window = tail_window(tail);
    waiting_patterns().iter().any(|p| test(p, &window))
}

const WINDOW_UNITS: usize = 256;

fn tail_window(tail: &str) -> Vec<char> {
    let chars: Vec<char> = tail.chars().collect();
    let mut units = 0;
    let mut start = chars.len();
    let mut split = false;
    for (i, c) in chars.iter().enumerate().rev() {
        let len = c.len_utf16();
        if units + len > WINDOW_UNITS {
            split = units < WINDOW_UNITS;
            break;
        }
        units += len;
        start = i;
    }
    let mut window = Vec::with_capacity(chars.len() - start + 1);
    if split {
        window.push('\u{fffd}');
    }
    window.extend_from_slice(&chars[start..]);
    window
}

/// Test hook: does pattern `index` (the order of `WAITING_PATTERNS` in
/// `harnessPatterns.ts`) match `text`, with no window applied.
#[cfg(test)]
pub(super) fn matches_pattern(index: usize, text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    test(&waiting_patterns()[index], &chars)
}

#[cfg(test)]
pub(super) const PATTERN_COUNT: usize = 8;
