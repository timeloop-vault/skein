//! The daemon's one terminal engine (#793, epic #575, design T3 in
//! `docs/skeind-terminal-design.md`).
//!
//! The daemon feeds every byte a PTY emits into one engine. The engine sits
//! behind [`TerminalEngine`] so a fallback stays a local change if a later
//! gate fails; [`AlacrittyEngine`] (`alacritty_terminal` 0.26.0) is the
//! implementation.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::redraw::ViewerState;

mod alacritty;
#[cfg(test)]
mod compact;
mod counting;
pub mod dump;

#[cfg(test)]
pub(in crate::pty) mod replay_tests;
#[cfg(test)]
pub(in crate::pty) mod tests;
#[cfg(test)]
mod tests_forward;
#[cfg(test)]
mod tests_guard;
#[cfg(test)]
mod tests_history;
#[cfg(test)]
mod tests_modes;
#[cfg(test)]
mod tests_replies;
#[cfg(test)]
mod tests_sync;

pub use self::alacritty::{AlacrittyEngine, Damage, LineDamage};

/// How many rows of scrollback history the engine keeps (#793).
///
/// This is the history bound and it must equal the client terminal's
/// xterm.js `scrollback` in `app/src/terminalSetup.ts` (line 57, 5000), or a
/// re-attached terminal would hold more or less history than a connected
/// one.
pub const HISTORY_ROWS: usize = 5000;

/// Which mouse events the application asked for. The engine keeps one
/// tracking level at a time (a later `?1000`/`?1002`/`?1003` replaces the
/// earlier one, unlike xterm, which keeps them independent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseTracking {
    #[default]
    Off,
    /// `?1000`: button press and release.
    Click,
    /// `?1002`: also motion while a button is held.
    Drag,
    /// `?1003`: all motion.
    Motion,
}

/// How mouse events are encoded for the application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseEncoding {
    /// The legacy `ESC [ M` encoding, the default.
    #[default]
    Default,
    /// `?1005`.
    Utf8,
    /// `?1006`.
    Sgr,
}

/// The cursor's shape (`DECSCUSR`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// The modes a viewer must mirror so it encodes input the way the
/// application expects (design, "The frame").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // independent terminal flags, not a state machine
pub struct Modes {
    /// `?1`, application cursor keys (DECCKM).
    pub app_cursor_keys: bool,
    /// `ESC =` / `ESC >`, application keypad (DECKPAM).
    pub app_keypad: bool,
    /// `?2004`.
    pub bracketed_paste: bool,
    /// `?1004`.
    pub focus_reporting: bool,
    pub mouse_tracking: MouseTracking,
    pub mouse_encoding: MouseEncoding,
    /// The alternate screen is active (`?1049`).
    pub alt_screen: bool,
    /// `?25`.
    pub cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub cursor_blinking: bool,
    /// The kitty keyboard protocol flags of the active screen's stack top:
    /// 1 disambiguate, 2 event types, 4 alternate keys, 8 all keys as
    /// escapes, 16 associated text. The stack is per screen.
    pub kitty_keyboard_flags: u8,
    /// A synchronized update (`?2026`) is open: the engine is holding the
    /// bytes back, and the grid does not show them yet. It ends at the
    /// closing `?2026l`, after 150 ms or after 2 MiB. The timeout is NOT
    /// self-driven: see [`TerminalEngine::sync_deadline`].
    pub sync_pending: bool,
}

impl Default for Modes {
    /// The modes of a freshly created terminal.
    fn default() -> Self {
        Self {
            app_cursor_keys: false,
            app_keypad: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_tracking: MouseTracking::Off,
            mouse_encoding: MouseEncoding::Default,
            alt_screen: false,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            cursor_blinking: false,
            kitty_keyboard_flags: 0,
            sync_pending: false,
        }
    }
}

/// A terminal engine that follows one PTY's output.
///
/// [`frame`](Self::frame) is the design's `frame(view)`: a diff against one
/// viewer's last frame, the attach snapshot included (slice T-2, #807).
///
/// Receivers: every read (`screen_text`, `modes`, `cursor`,
/// `sync_deadline`) takes `&self`, because `alacritty_terminal` exposes the
/// grid and the mode flags by shared reference. `feed`, `resize` and
/// `flush_sync` take `&mut self`. Damage tracking (an inherent method of
/// [`AlacrittyEngine`], not on this trait) needs `&mut self` even to read,
/// because reading it records the cursor's last position.
pub trait TerminalEngine: Send {
    /// Processes bytes the PTY emitted and returns the engine's replies to
    /// device queries, which the daemon must write back to the PTY. Empty
    /// when there are none. Never panics, whatever the bytes.
    ///
    /// Only the harness's own output may be fed: injected bytes would abort
    /// a pending escape sequence.
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8>;

    /// Follows the PTY's size. A zero in either dimension is clamped (see
    /// [`AlacrittyEngine::new`] for the smallest size).
    fn resize(&mut self, rows: u16, cols: u16);

    /// The text of each row of the active screen, wide-character spacers
    /// skipped and trailing blanks trimmed.
    fn screen_text(&self) -> Vec<String>;

    fn modes(&self) -> Modes;

    /// The active screen's cursor, `(row, col)`, zero-based.
    fn cursor(&self) -> (u16, u16);

    /// When an open synchronized update must be ended, if one is open. The
    /// engine has no clock of its own: the caller sleeps until this instant
    /// and then calls [`flush_sync`](Self::flush_sync). (`feed` also ends an
    /// expired update before it processes new bytes.)
    fn sync_deadline(&self) -> Option<Instant>;

    /// Ends an open synchronized update now, applying the bytes held back,
    /// and returns any replies they produced. Does nothing when none is open.
    fn flush_sync(&mut self) -> Vec<u8>;

    /// The bytes that bring one viewer from what it was last sent to what
    /// the engine shows now, or `None` when there is nothing to send: a
    /// synchronized update is open (the viewer never sees a half-drawn
    /// frame) or nothing differs. A fresh [`ViewerState`] gets a full redraw,
    /// which is the attach snapshot. Updates `view` to what it sent.
    fn frame(&self, view: &mut ViewerState) -> Option<Vec<u8>>;
}
