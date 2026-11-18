//! Writing one screen row.
//!
//! Position (absolute), then the cells in runs, then reset and erase the rest
//! of the row. The client's cursor is re-positioned after every character
//! whose width the client might read differently from the engine, so a
//! disagreement stays inside one cell.

use std::io::Write;

use unicode_width::UnicodeWidthChar;

use crate::pty::engine::dump::{CellDump, RowDump};

use super::sgr::{Pen, Style, link_of};

/// Absolute cursor position, zero-based in, one-based on the wire.
pub(super) fn cup(out: &mut Vec<u8>, row: usize, col: usize) {
    let _ = write!(out, "\x1b[{};{}H", row + 1, col + 1);
}

/// A cell that shows nothing and that `EL` reproduces: a space in the
/// default style, no link.
fn is_blank(c: &CellDump) -> bool {
    c.width == 1 && c.text == " " && c.link.is_none() && Style::of(c) == Style::default_style()
}

pub(super) fn is_blank_row(row: &RowDump) -> bool {
    row.cells.iter().all(is_blank)
}

/// The cells up to the last one `EL` cannot reproduce.
fn trimmed_len(cells: &[CellDump]) -> usize {
    cells
        .iter()
        .rposition(|c| !is_blank(c))
        .map_or(0, |i| i + 1)
}

/// Whether the client might give this cell another width than the engine
/// did, so the cursor must be re-positioned after it. A wide character is
/// always re-positioned; of the width-1 ones, those East Asian Ambiguous
/// (`unicode-width`: the CJK width differs from the plain one), and any cell
/// that carries zero-width characters, whose clustering clients disagree on.
fn width_uncertain(c: &CellDump) -> bool {
    if c.width == 2 {
        return true;
    }
    let mut chars = c.text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    chars.next().is_some() || (!first.is_ascii() && first.width() != first.width_cjk())
}

/// A row ends in a leading spacer when a wide character did not fit in its
/// last column. The next row starts with that character.
pub(super) fn ends_in_leading_spacer(row: &RowDump) -> bool {
    row.cells.last().is_some_and(|c| c.leading_spacer)
}

/// A row is a flowing continuation of the one above when the engine marked
/// it wrapped. That includes a row above that ended early in a leading spacer
/// (#807): the spacer is not written, so the client's cursor stops before the
/// last column, the wide character that starts this row does not fit there,
/// and the client wraps it itself and sets its wrap flag, as it does for the
/// same bytes from a program. This supersedes the earlier "no wrap flag"
/// fallback (mismatch 5).
pub(super) fn continues_above(rows: &[RowDump], r: usize) -> bool {
    r > 0 && rows[r].wrapped
}

/// One cell's text. A control character cannot reach a grid through the
/// parser, but one in a frame would be executed by the client, so it is
/// written as a space.
fn put_text(out: &mut Vec<u8>, text: &str) {
    if text.chars().any(char::is_control) {
        let clean: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        out.extend_from_slice(clean.as_bytes());
    } else {
        out.extend_from_slice(text.as_bytes());
    }
}

/// Writes `cell` at the cursor in its own style and link. `col` is its
/// column and `cols` the row's width: a wide character that starts in the
/// last column cannot be drawn (the client would wrap it and leave a ghost),
/// so one blank in the cell's style is written there instead (#807). The
/// stored row stays the engine's.
fn put_cell(out: &mut Vec<u8>, pen: &mut Pen, cell: &CellDump, col: usize, cols: usize) {
    pen.set_link(out, link_of(cell));
    pen.set_style(out, &Style::of(cell));
    if cell.width == 2 && col + 1 >= cols {
        out.push(b' ');
    } else {
        put_text(out, &cell.text);
    }
}

/// Writes row `r`.
///
/// `positioned`: the client's cursor is already where this row's text goes
/// (pending-wrap at the end of the row above, which this row continues), so
/// no `CUP`. `through_margin`: the row continues into the next one, so every
/// cell is written to the last column and nothing is erased; the client's
/// autowrap does the rest.
pub(super) fn write_row(
    out: &mut Vec<u8>,
    pen: &mut Pen,
    r: usize,
    row: &RowDump,
    positioned: bool,
    through_margin: bool,
) {
    let cells = &row.cells;
    let cols = cells.len();
    if !positioned {
        pen.set_link(out, None);
        cup(out, r, 0);
        // `EL` at column 1 also clears the wrap flag a client keeps from an
        // earlier autowrap (CUP and text do not), and fills with the current
        // background, so reset first (#807).
        pen.set_style(out, &Style::default_style());
        out.extend_from_slice(b"\x1b[K");
    }
    let end = if through_margin {
        cols
    } else if positioned {
        // At least the first cell: a blank row that continues the one above
        // needs a character to wrap into it, or `EL` would hit the row above.
        trimmed_len(cells).max(1)
    } else {
        trimmed_len(cells)
    };
    for (c, cell) in cells.iter().enumerate().take(end) {
        // Not written: the client's cursor stops before it and the wide
        // character that starts the next row wraps by itself (#807).
        if cell.width == 0 || (through_margin && cell.leading_spacer) {
            continue;
        }
        put_cell(out, pen, cell, c, cols);
        if width_uncertain(cell) {
            let next = c + usize::from(cell.width.max(1));
            if next < cols {
                cup(out, r, next);
            }
        }
    }
    // A row positioned at column 1 was erased up front.
    if positioned && end < cols {
        pen.set_style(out, &Style::default_style());
        out.extend_from_slice(b"\x1b[K");
    }
}

/// Puts the client's cursor at the end of row `r` by writing the row's last
/// character again, so the next text written continues into row `r + 1` and
/// the client marks that row as wrapped. A row ending in a leading spacer is
/// re-emitted from its last real cell before the spacer (the spacer is never
/// written): the cursor then stops short of the last column and the wide
/// character that starts row `r + 1` wraps (#807).
pub(super) fn write_margin_cell(out: &mut Vec<u8>, pen: &mut Pen, r: usize, row: &RowDump) {
    let Some(last) = row
        .cells
        .iter()
        .rposition(|c| c.width != 0 && !c.leading_spacer)
    else {
        return;
    };
    pen.set_link(out, None);
    cup(out, r, last);
    put_cell(out, pen, &row.cells[last], last, row.cells.len());
}
