//! History: the rows that left the screen, and the client's scrollback.
//!
//! The client's scrollback is filled only by scrolling, and only by this
//! module. Two ways, both inside the subset (`CUP`, text, `EL`, `LF`):
//!
//! - **Push** (a diff frame): the engine's newest `k` history rows are the
//!   rows that left the top of the screen since the viewer's last frame. They
//!   are repainted over the viewer's top `k` rows (only those that differ),
//!   then `k` line feeds at the bottom row scroll them into the client's
//!   scrollback. Rows enter in batches of at most `rows`.
//! - **Full redraw**: the history and the screen are written as one run of
//!   flowing lines from the top of a cleared screen. Every line past the
//!   `rows`-th scrolls the screen, so the first `history_len` lines end up in
//!   the client's scrollback and the last `rows` on its screen. No blank row
//!   ever enters the scrollback, and a screen row that continues the last
//!   history row is written through the margin like any other continuation.
//!
//! A pushed row that continues the row above (a wrapped line across the
//! history/screen boundary) keeps the wrap flag the client set while that row
//! was on its screen: lines carry their flag into the scrollback. A row of the
//! batch that is repainted (it differs from the viewer's copy) cannot be
//! written through the margin of a row that is already in the scrollback, so
//! its flag stays what it was. Known difference, see `write_push`.

use crate::pty::engine::HISTORY_ROWS;
use crate::pty::engine::dump::RowDump;

use super::rows::{continues_above, cup, ends_in_leading_spacer, is_blank_row, write_row};
use super::sgr::Pen;
use super::source::GridSource;
use super::state::ViewerState;

/// How many rows to push to a viewer that is up to date on the main screen,
/// or `None` when it needs a full redraw: more rows entered than the history
/// holds, or the history is not what the client was sent plus those rows (the
/// application cleared the scrollback, `CSI 3 J`; or rows were lost).
pub(super) fn rows_to_push(view: &ViewerState, entered: u64, hlen: usize) -> Option<usize> {
    let k = usize::try_from(entered.checked_sub(view.history_entered)?).ok()?;
    if k > HISTORY_ROWS || hlen != (view.history_len + k).min(HISTORY_ROWS) {
        return None;
    }
    Some(k)
}

/// `screen` after `k` rows scrolled off its top: the rest moved up, blank
/// rows at the bottom.
pub(super) fn shifted(screen: &[RowDump], k: usize) -> Vec<RowDump> {
    if k == 0 {
        return screen.to_vec();
    }
    let cols = screen.first().map_or(0, |r| r.cells.len());
    let k = k.min(screen.len());
    screen[k..]
        .iter()
        .cloned()
        .chain((0..k).map(|_| RowDump::blank(cols)))
        .collect()
}

/// Pushes the newest `k` history rows into the client's scrollback. `held`
/// is the screen the viewer holds before the push.
///
/// Per batch: repaint the top `m` rows that differ with history rows, reset
/// the pen (the new bottom rows take the pen's background), `CUP` to the
/// bottom row, `m` line feeds. A batch is at most `rows` rows long and ends
/// where the next batch's first row is not a continuation, when it can: that
/// row's wrap flag would otherwise be lost, since its predecessor is already
/// in the scrollback and a `CUP` write sets no flag. Only a single logical
/// line longer than the screen cannot be cut that way; there the flag of the
/// row after each full batch is the known difference.
pub(super) fn write_push<S: GridSource + ?Sized>(
    out: &mut Vec<u8>,
    pen: &mut Pen,
    src: &S,
    held: &[RowDump],
    k: usize,
) {
    let rows = held.len();
    let first = src.history_len() - k;
    let mut stored = held.to_vec();
    let mut done = 0;
    while done < k {
        let start = first + done;
        let m = batch_len(src, start, k - done, rows);
        let batch: Vec<RowDump> = (0..m).map(|i| src.history_row(start + i)).collect();
        let dirty: Vec<bool> = batch.iter().zip(&stored).map(|(b, s)| b != s).collect();
        super::encode::write_screen(out, pen, &batch, &dirty, false);
        pen.reset(out);
        cup(out, rows - 1, 0);
        out.extend(std::iter::repeat_n(b'\n', m));
        stored = shifted(&stored, m);
        done += m;
    }
}

/// How many of the `remaining` rows from history row `start` go in one batch.
fn batch_len<S: GridSource + ?Sized>(
    src: &S,
    start: usize,
    remaining: usize,
    rows: usize,
) -> usize {
    let cap = remaining.min(rows);
    if cap == remaining {
        return cap;
    }
    for m in (1..=cap).rev() {
        let (last, next) = (src.history_row(start + m - 1), src.history_row(start + m));
        if !next.wrapped || ends_in_leading_spacer(&last) {
            return m;
        }
    }
    cap
}

/// Writes `lines` (the history, then the screen) from the top of a cleared
/// screen of `rows` rows, scrolling as it goes.
pub(super) fn write_flow(out: &mut Vec<u8>, pen: &mut Pen, lines: &[RowDump], rows: usize) {
    let bottom = rows.saturating_sub(1);
    // The client's cursor is on the bottom row.
    let mut at_bottom = false;
    for (j, row) in lines.iter().enumerate() {
        let wrapped = continues_above(lines, j);
        let continues = j + 1 < lines.len() && continues_above(lines, j + 1);
        if j >= rows && !wrapped {
            if !at_bottom {
                cup(out, bottom, 0);
            }
            pen.reset(out);
            out.push(b'\n');
            at_bottom = true;
        }
        if !wrapped && !continues && is_blank_row(row) {
            // Already blank: cleared, or a fresh row from the scroll.
            continue;
        }
        let r = j.min(bottom);
        write_row(out, pen, r, row, wrapped, continues);
        at_bottom = r == bottom;
    }
}
