//! The Hermes screen reader (#809): what the engine's rows say about the
//! harness's turn. Port of `app/src/screenPhase.ts`, which read the xterm
//! buffer in the webview; in frames mode the daemon reads its own engine
//! instead. The rules are pinned to captured Hermes v0.21.7 screens in
//! English; every conjunctive rule falls through to `Unknown` on any doubt.
//!
//! Strings are sliced by char, never by byte offset: the box-drawing
//! characters are multi-byte.

use super::screen::ScreenReading;

/// How many rows from the bottom the status line is searched in. It is the
/// last row on every captured frame; a little slack covers a taller footer.
const STATUS_SEARCH_ROWS: usize = 4;

/// Hermes's status line is `─ <state> │ <model> │ <field> │ … ─ <title>`. The
/// fields vary (no provider connected: no usage token), so it is identified
/// by a leading `─` and at least two ` │ ` separators. A shell that merely
/// prints "ready │" fails the leading rule. Returns the text after the
/// leading `─ `.
fn find_status_line(rows: &[String]) -> Option<&str> {
    let from = rows.len().saturating_sub(STATUS_SEARCH_ROWS);
    rows.get(from..)?.iter().rev().find_map(|row| {
        let full = row.trim();
        let text = full.strip_prefix("─ ")?;
        (full.split(" │ ").count() >= 3).then_some(text)
    })
}

/// The approval box: a `║`-edged row carrying the title and another carrying
/// the footer hint. Both are required so output that quotes one of the
/// phrases is not mistaken for a dialog.
fn has_approval_box(rows: &[String]) -> bool {
    let (mut title, mut footer) = (false, false);
    for row in rows {
        let text = row.trim();
        if !text.starts_with('║') {
            continue;
        }
        title |= text.contains("approval required");
        footer |= text.contains("Esc/Ctrl+C deny");
    }
    title && footer
}

/// Read the rows of a Hermes screen (`--tui`, English). It redraws a clock and
/// a spinner constantly, so byte-quiet never happens; the phase comes from the
/// status line. A draft in the composer does not matter: `ready` means it is
/// still the user's turn. The rotating face and verb of the busy status are
/// never matched.
#[must_use]
pub fn read_hermes(rows: &[String]) -> ScreenReading {
    if has_approval_box(rows) {
        return ScreenReading::Waiting;
    }
    match find_status_line(rows) {
        None => ScreenReading::Unknown,
        Some(status) if status.starts_with("ready │ ") => ScreenReading::Waiting,
        Some(_) => ScreenReading::Running,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use ScreenReading::{Running, Unknown, Waiting};

    const ROWS: usize = 40;
    const STATUS_ROW: usize = ROWS - 1;

    fn screen(lines: &[(usize, &str)]) -> Vec<String> {
        let map: HashMap<usize, &str> = lines.iter().copied().collect();
        (0..ROWS)
            .map(|y| map.get(&y).copied().unwrap_or("").to_owned())
            .collect()
    }

    fn busy(face: &str, verb: &str) -> String {
        format!(
            " ─ {face} {verb}… · 35s │ haiku 4.5 │ 13.9k/200k │ [█░░░░░░░░░] 7%                ─  Example session"
        )
    }

    const READY: &str = " ─ ready │ haiku 4.5 │ 14.4k/200k │ [█░░░░░░░░░] 7% │ 3m 22s │ ✓ 56s │ ◎ 49%            ─  Example session";

    fn approval() -> Vec<String> {
        let busy = busy("(¬_¬)", "contemplating");
        screen(&[
            (27, " ❯ Run the delete command"),
            (29, " └─ ▾ Tool calls (1)"),
            (
                30,
                " ╔══════════════════════════════════════════════════════════════════╗",
            ),
            (
                31,
                " ║ ⚠ approval required · recursive delete                          │║",
            ),
            (
                32,
                " ║  rm -rf ./example                                                 ║",
            ),
            (
                33,
                " ║ ▸ 1. Allow once                                                   ║",
            ),
            (
                34,
                " ║   2. Allow this session                                           ║",
            ),
            (
                35,
                " ║   3. Always allow                                                 ║",
            ),
            (
                36,
                " ║   4. Deny                                                         ║",
            ),
            (
                37,
                " ║ ↑/↓ select · Enter confirm · 1-4 quick pick · Esc/Ctrl+C deny     ║",
            ),
            (
                38,
                " ╚══════════════════════════════════════════════════════════════════╝",
            ),
            (STATUS_ROW, &busy),
        ])
    }

    #[test]
    fn an_open_approval_box_is_waiting_though_the_status_bar_reads_busy() {
        assert_eq!(read_hermes(&approval()), Waiting);
    }

    #[test]
    fn an_idle_ready_status_line_is_waiting() {
        let snap = screen(&[(36, " ❯"), (38, " ┊  Done."), (STATUS_ROW, READY)]);
        assert_eq!(read_hermes(&snap), Waiting);
    }

    #[test]
    fn a_draft_typed_in_the_composer_is_still_waiting() {
        let snap = screen(&[(36, " ❯ half a thought"), (STATUS_ROW, READY)]);
        assert_eq!(read_hermes(&snap), Waiting);
    }

    #[test]
    fn a_busy_status_line_without_a_box_is_running_whatever_the_face_and_verb() {
        for (face, verb) in [
            ("(◔_◔)", "deliberating"),
            ("(¬_¬)", "contemplating"),
            ("(•‿•)", "pondering"),
        ] {
            let b = busy(face, verb);
            let snap = screen(&[(36, " ❯ Ctrl+C to interrupt…"), (STATUS_ROW, &b)]);
            assert_eq!(read_hermes(&snap), Running);
        }
    }

    #[test]
    fn an_empty_screen_is_unknown() {
        assert_eq!(read_hermes(&screen(&[])), Unknown);
        assert_eq!(read_hermes(&[]), Unknown);
    }

    #[test]
    fn garbled_output_is_unknown() {
        let snap = screen(&[(38, " {u uu"), (STATUS_ROW, " ─ ready")]);
        assert_eq!(read_hermes(&snap), Unknown);
    }

    #[test]
    fn the_no_provider_idle_row_is_waiting() {
        let snap = screen(&[(
            STATUS_ROW,
            " ─ ready │ glm 5.2 │ 16s │ voice off │ 1 session                       ─ …\\project (main)",
        )]);
        assert_eq!(read_hermes(&snap), Waiting);
    }

    #[test]
    fn a_busy_row_without_the_usage_token_is_running() {
        let snap = screen(&[(
            STATUS_ROW,
            " ─ (◔_◔) deliberating… · 9s │ glm 5.2 │ voice off │ 1 session      ─ …\\project (main)",
        )]);
        assert_eq!(read_hermes(&snap), Running);
    }

    #[test]
    fn a_status_line_with_fewer_than_two_separators_is_not_recognised() {
        let snap = screen(&[(STATUS_ROW, " ─ ready │ glm 5.2")]);
        assert_eq!(read_hermes(&snap), Unknown);
    }

    #[test]
    fn approval_phrases_outside_a_box_do_not_count() {
        let snap = screen(&[
            (20, "approval required · recursive delete"),
            (21, "↑/↓ select · Esc/Ctrl+C deny"),
        ]);
        assert_eq!(read_hermes(&snap), Unknown);
    }

    #[test]
    fn a_shell_that_prints_ready_mid_screen_is_not_waiting() {
        let snap = screen(&[
            (30, "$ echo 'ready │ haiku 4.5 │ 1k/2k │ x'"),
            (31, "ready │ haiku 4.5 │ 1k/2k │ x"),
            (39, "$ "),
        ]);
        assert_eq!(read_hermes(&snap), Unknown);
    }

    #[test]
    fn only_the_bottom_rows_are_searched_for_the_status_line() {
        assert_eq!(read_hermes(&screen(&[(10, READY)])), Unknown);
    }
}
