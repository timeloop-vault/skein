//! Frame assembly: what to send a viewer, in what order.

use std::io::Write;

use crate::pty::engine::dump::RowDump;

use super::history;
use super::modes;
use super::rows::{continues_above, cup, is_blank_row, write_margin_cell, write_row};
use super::sgr::Pen;
use super::source::GridSource;
use super::state::ViewerState;

/// How much of the screen is sent regardless of what the viewer holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Redraw {
    /// A diff.
    None,
    /// The alternate screen was just entered: every row of it, nothing
    /// cleared (`?1049h` cleared the client's alternate buffer, and a
    /// `CSI 3 J` would wipe the main scrollback behind it). The viewer's main
    /// rows and history counters are kept.
    EnterAlt,
    /// Clear screen and scrollback, then the history as flowing lines and
    /// every screen row that is not blank, and every mode and the title. The
    /// attach snapshot.
    Full,
}

/// Cuts the frame that brings `view` up to what `src` shows, and records
/// what was sent in `view`. `None` while a synchronized update is open (no
/// viewer sees a half-drawn frame; `view` is untouched) and when nothing
/// differs.
///
/// Frame, in order, all inside `?2026h` ... `?2026l`: a reset, the
/// alternate-screen mirror, the clear (full redraw only), the history (the
/// push of the rows that left the screen, or on a full redraw all of it as
/// flowing lines), the screen rows, the title, the other mode mirrors, the bell, a reset, `CUP` to the
/// cursor, the cursor's shape and visibility.
///
/// The alternate screen (`?1049` is the only way in or out): entering it
/// repaints every row of it with an explicit cursor, keeping the viewer's
/// stored main rows and history counters; while it is active the frames are
/// plain row diffs against the stored alternate rows, no history is pushed
/// and the counters are left alone; leaving it diffs the main screen against
/// the stored main rows and pushes history if the counters moved. A viewer
/// that attached while it was active, or that was resized or rebuilt there,
/// has no stored main rows, so leaving it is a full redraw.
///
/// A size change, of rows or columns alike, is a full redraw, which also
/// re-baselines the history counters to the engine's (a resize moves rows
/// across the history boundary without counting them). A height-only change
/// could not send the screen alone: the client resizes the screen it holds,
/// the engine the one it holds now, and they differ as soon as anything
/// happened after the last frame. On the alternate screen the same redraw
/// leaves the main unknown, so leaving it is a full redraw too.
pub fn encode_frame<S: GridSource + ?Sized>(src: &S, view: &mut ViewerState) -> Option<Vec<u8>> {
    let modes = src.modes();
    if modes.sync_pending {
        return None;
    }
    let (rows, cols) = (src.rows(), src.cols());
    let was_alt = view.primed && view.modes.alt_screen;
    let mut redraw = if !view.primed
        || src.rebuilds() != view.rebuilds
        // Resized away and back: the size matches, the history may not.
        || src.resizes() != view.resizes
        || rows != view.rows
        || cols != view.cols
        // Leaving the alternate screen for a client whose main is unknown.
        || (was_alt && !modes.alt_screen && view.main_screen.is_none())
    {
        Redraw::Full
    } else if modes.alt_screen && !was_alt {
        Redraw::EnterAlt
    } else {
        Redraw::None
    };
    // The rows the client holds on the screen this frame draws on: on a
    // leave, the stored main rows.
    let held: &[RowDump] = match (&view.main_screen, was_alt && !modes.alt_screen) {
        (Some(main), true) if redraw == Redraw::None => main,
        _ => &view.screen,
    };
    let (entered, hlen) = (src.history_rows_entered(), src.history_len());
    let mut push = 0;
    if redraw == Redraw::None && !modes.alt_screen {
        match history::rows_to_push(view, entered, hlen) {
            Some(k) => push = k,
            None => redraw = Redraw::Full,
        }
    }
    let all = redraw == Redraw::Full;

    // What the client's screen holds once the pushed rows have scrolled off:
    // row `i` is `held[i + k]`, and the rows the push opened at the bottom
    // are blank. Compared in place, so a frame where nothing differs clones
    // no rows.
    let k = push.min(held.len());
    let blank = (k > 0).then(|| RowDump::blank(held.first().map_or(0, |r| r.cells.len())));
    let base_row = |i: usize| -> Option<&RowDump> {
        if i + k < held.len() {
            Some(&held[i + k])
        } else if i < held.len() {
            blank.as_ref()
        } else {
            None
        }
    };
    let dirty: Vec<bool> = (0..usize::from(rows))
        .map(|i| redraw != Redraw::None || base_row(i).is_none_or(|row| !src.screen_row_is(i, row)))
        .collect();
    let cursor = (
        src.cursor().0.min(rows.saturating_sub(1)),
        src.cursor().1.min(cols.saturating_sub(1)),
    );
    let title = src.title();
    let bells = src.bell_count();
    // A viewer's first frame does not ring for bells from before it.
    let ring = view.primed && bells > view.bell_count;

    if redraw == Redraw::None
        && push == 0
        && !dirty.contains(&true)
        && modes == view.modes
        && title == view.title
        && cursor == view.cursor
        && !ring
    {
        return None;
    }

    let screen: Vec<RowDump> = (0..usize::from(rows)).map(|i| src.screen_row(i)).collect();
    let mut out = b"\x1b[?2026h\x1b[0m".to_vec();
    let mut pen = Pen::new();
    if all || modes.alt_screen != view.modes.alt_screen {
        modes::alt_screen(&mut out, modes.alt_screen);
    }
    if all {
        out.extend_from_slice(b"\x1b[H\x1b[2J\x1b[3J");
    }
    if all && !modes.alt_screen && hlen > 0 {
        // The history and the screen are one run of lines; the client's
        // scrolling files the first `hlen` of them into its scrollback.
        let lines: Vec<RowDump> = (0..hlen)
            .map(|i| src.history_row(i))
            .chain(screen.iter().cloned())
            .collect();
        history::write_flow(&mut out, &mut pen, &lines, usize::from(rows));
    } else {
        if push > 0 {
            history::write_push(&mut out, &mut pen, src, held, push);
        }
        write_screen(&mut out, &mut pen, &screen, &dirty, all);
    }
    if all || title != view.title {
        out.extend_from_slice(b"\x1b]2;");
        out.extend(
            title
                .as_deref()
                .unwrap_or("")
                .bytes()
                .filter(|b| *b >= 0x20 && *b != 0x7f),
        );
        out.extend_from_slice(b"\x1b\\");
    }
    modes::mirror(&mut out, &view.modes, &modes, all);
    if ring {
        out.push(0x07);
    }
    pen.reset(&mut out);
    cup(&mut out, usize::from(cursor.0), usize::from(cursor.1));
    modes::cursor_look(&mut out, &view.modes, &modes, all);
    let _ = write!(out, "\x1b[?2026l");

    if modes.alt_screen {
        view.main_screen = match redraw {
            Redraw::EnterAlt => Some(std::mem::take(&mut view.screen)),
            Redraw::Full => None,
            Redraw::None => view.main_screen.take(),
        };
    } else {
        view.main_screen = None;
    }
    view.primed = true;
    view.rows = rows;
    view.cols = cols;
    view.screen = screen;
    view.modes = modes;
    view.title = title;
    view.cursor = cursor;
    view.bell_count = bells;
    view.rebuilds = src.rebuilds();
    view.resizes = src.resizes();
    if !modes.alt_screen {
        view.history_entered = entered;
        view.history_len = hlen;
    }
    Some(out)
}

/// The dirty rows. `blank_skip`: the screen was just cleared, so a blank row
/// is already right.
pub(super) fn write_screen(
    out: &mut Vec<u8>,
    pen: &mut Pen,
    screen: &[RowDump],
    dirty: &[bool],
    blank_skip: bool,
) {
    // The client's cursor is in pending-wrap at the end of the previous row,
    // which this row continues.
    let mut flowing = false;
    for (r, row) in screen.iter().enumerate() {
        if !dirty[r] {
            flowing = false;
            continue;
        }
        let wrapped = continues_above(screen, r);
        let continues = r + 1 < screen.len() && continues_above(screen, r + 1);
        if blank_skip && !wrapped && !continues && is_blank_row(row) {
            flowing = false;
            continue;
        }
        if wrapped && !flowing {
            // Only this row changed: re-emit the last cell above, so the
            // client wraps into this row and marks it wrapped.
            write_margin_cell(out, pen, r - 1, &screen[r - 1]);
        }
        write_row(out, pen, r, row, wrapped, continues);
        flowing = continues;
    }
}
