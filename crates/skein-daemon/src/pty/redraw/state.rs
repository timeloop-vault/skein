//! What one viewer has been sent so far.

use crate::pty::engine::Modes;
use crate::pty::engine::dump::RowDump;

/// One viewer's last-sent state. A frame is the difference between this and
/// what the engine shows; the encoder updates it to what it sent. Many
/// viewers per PTY, and their lifecycle, are slice T-3's.
///
/// `ViewerState::new()` is a fresh viewer: its first frame is a full redraw,
/// which is the attach snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerState {
    /// A frame has been cut for this viewer. False until the first one.
    pub(super) primed: bool,
    pub(super) rows: u16,
    pub(super) cols: u16,
    /// The screen rows the viewer shows, top first: the alternate screen's
    /// while `modes.alt_screen`.
    pub(super) screen: Vec<RowDump>,
    /// While the alternate screen is active: the main rows the viewer holds
    /// behind it, or `None` when they are unknown (it attached, was resized
    /// or was rebuilt there). Always `None` on the main screen.
    pub(super) main_screen: Option<Vec<RowDump>>,
    pub(super) modes: Modes,
    pub(super) title: Option<String>,
    pub(super) cursor: (u16, u16),
    /// The engine's bell count at the last frame.
    pub(super) bell_count: u64,
    /// The engine's panic-rebuild count at the last frame.
    pub(super) rebuilds: u32,
    /// The engine's size-change count at the last frame.
    pub(super) resizes: u32,
    /// The engine's `history_rows_entered` at the last frame on the main
    /// screen (kept while the alternate screen is active). The difference
    /// to now is the number of rows to push.
    pub(super) history_entered: u64,
    /// The engine's `history_len` at the last frame on the main screen. With
    /// `history_entered` it tells whether the history is still the one the
    /// client was sent: a different length is a cleared scrollback.
    pub(super) history_len: usize,
}

impl ViewerState {
    pub fn new() -> Self {
        Self {
            primed: false,
            rows: 0,
            cols: 0,
            screen: Vec::new(),
            main_screen: None,
            modes: Modes::default(),
            title: None,
            cursor: (0, 0),
            bell_count: 0,
            rebuilds: 0,
            resizes: 0,
            history_entered: 0,
            history_len: 0,
        }
    }

    /// The next frame is a full redraw. A view can ask for one at any time,
    /// and the engine does after a failure it cannot vouch for.
    pub fn invalidate(&mut self) {
        *self = Self::new();
    }

    /// True until the first frame has been cut.
    pub fn is_fresh(&self) -> bool {
        !self.primed
    }
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
    }
}
