//! What the encoder reads: the engine's grid, one row at a time.
//!
//! The encoder is pure over this trait, so it is table-tested with a
//! hand-built grid and run for real over [`AlacrittyEngine`]. Reads are lazy:
//! a frame that changes one row never converts the 5000 rows of history.

use crate::pty::engine::dump::RowDump;
use crate::pty::engine::{AlacrittyEngine, Modes, TerminalEngine};

pub trait GridSource {
    fn rows(&self) -> u16;
    fn cols(&self) -> u16;
    /// The active screen's cursor, `(row, col)`, zero-based.
    fn cursor(&self) -> (u16, u16);
    /// The viewer-visible modes, the alternate screen flag and the cursor's
    /// visibility and shape included.
    fn modes(&self) -> Modes;
    fn title(&self) -> Option<String>;
    /// Every bell since the source was created. Monotonic.
    fn bell_count(&self) -> u64;
    /// How many times the source was rebuilt empty after a panic.
    fn rebuilds(&self) -> u32;
    /// How many times the size changed since the source was created.
    fn resizes(&self) -> u32;
    /// Rows that have entered scrollback history since the source was
    /// created. Monotonic, keeps growing once history is full.
    fn history_rows_entered(&self) -> u64;
    /// Rows in the scrollback now. Only meaningful on the main screen.
    fn history_len(&self) -> usize;
    /// Row `i` of the active screen, 0 at the top.
    fn screen_row(&self, i: usize) -> RowDump;
    /// Whether row `i` of the active screen equals `row`. Lets the encoder
    /// tell that nothing changed without cloning the screen; the default goes
    /// through [`screen_row`](Self::screen_row).
    fn screen_row_is(&self, i: usize, row: &RowDump) -> bool {
        self.screen_row(i) == *row
    }
    /// Row `i` of the scrollback, 0 the oldest. Only meaningful on the main
    /// screen.
    fn history_row(&self, i: usize) -> RowDump;
}

impl GridSource for AlacrittyEngine {
    fn rows(&self) -> u16 {
        self.size().0
    }
    fn cols(&self) -> u16 {
        self.size().1
    }
    fn cursor(&self) -> (u16, u16) {
        TerminalEngine::cursor(self)
    }
    fn modes(&self) -> Modes {
        TerminalEngine::modes(self)
    }
    fn title(&self) -> Option<String> {
        AlacrittyEngine::title(self)
    }
    fn bell_count(&self) -> u64 {
        AlacrittyEngine::bell_count(self)
    }
    fn rebuilds(&self) -> u32 {
        AlacrittyEngine::rebuilds(self)
    }
    fn resizes(&self) -> u32 {
        AlacrittyEngine::resizes(self)
    }
    fn history_rows_entered(&self) -> u64 {
        AlacrittyEngine::history_rows_entered(self)
    }
    fn history_len(&self) -> usize {
        AlacrittyEngine::history_len(self)
    }
    fn screen_row(&self, i: usize) -> RowDump {
        self.screen_row_dump(i)
    }
    fn screen_row_is(&self, i: usize, row: &RowDump) -> bool {
        self.screen_row_is_dump(i, row)
    }
    fn history_row(&self, i: usize) -> RowDump {
        self.history_row_dump(i)
    }
}
