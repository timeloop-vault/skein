//! An engine-neutral picture of a terminal (#793).
//!
//! The cross-engine test compares two engines (and, later, an engine and the
//! client's xterm.js) through this shape, never through either engine's own
//! types. The dump is for reading: building one never changes the engine.

use serde::{Deserialize, Serialize};

use super::Modes;

/// A cell colour. Named ANSI colours (`ESC[31m`) and the same palette index
/// written as `ESC[38;5;1m` both read as `Palette(1)`: the engines store
/// them differently and a viewer cannot tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ColorDump {
    /// The terminal's own foreground or background.
    Default,
    /// A palette index, 0 to 255.
    Palette { index: u8 },
    /// A 24-bit colour.
    Rgb { r: u8, g: u8, b: u8 },
}

/// One cell of one row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // independent SGR flags, not a state machine
pub struct CellDump {
    /// The cell's character followed by any zero-width characters stored
    /// with it. Empty for the spacer half of a wide character. A blank cell
    /// is a single space.
    pub text: String,
    /// 0 for the spacer half of a wide character, 2 for the wide character
    /// itself, 1 otherwise.
    pub width: u8,
    pub fg: ColorDump,
    pub bg: ColorDump,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    /// Any underline style (single, double, curly, dotted, dashed).
    pub underline: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    /// The OSC 8 URI the cell carries, if any. The link's `id=` parameter is
    /// not part of the dump.
    pub link: Option<String>,
}

/// One row of cells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowDump {
    /// This row is a continuation of the previous row (xterm.js's
    /// `isWrapped`): the previous row filled its last column and the text
    /// went on here. The first row of history is never wrapped, and neither
    /// is the first row of an active screen that has no history above it.
    pub wrapped: bool,
    pub cells: Vec<CellDump>,
}

/// The cursor of the active screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorDump {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// Everything a viewer can see of an engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridDump {
    pub rows: u16,
    pub cols: u16,
    pub alt_screen: bool,
    pub cursor: CursorDump,
    pub modes: Modes,
    /// The main screen's scrollback, oldest first. Empty while the alternate
    /// screen is active: `alacritty_terminal` keeps the main grid private
    /// then and offers no way to read it without destroying the alternate
    /// grid (see `AlacrittyEngine::dump`).
    pub history: Vec<RowDump>,
    /// The active screen, top row first.
    pub screen: Vec<RowDump>,
    /// The main screen while the alternate screen is active. Always `None`
    /// for this engine (unreachable, see `history`).
    pub hidden_screen: Option<Vec<RowDump>>,
}
