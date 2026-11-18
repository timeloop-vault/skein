//! A hand-built grid for the encoder's table tests.

use crate::pty::engine::dump::{CellDump, RowDump};
use crate::pty::engine::{AlacrittyEngine, Modes, TerminalEngine};

use super::{GridSource, ViewerState, encode_frame};

#[derive(Clone)]
pub(super) struct TestGrid {
    pub rows: u16,
    pub cols: u16,
    pub cursor: (u16, u16),
    pub modes: Modes,
    pub title: Option<String>,
    pub bells: u64,
    pub rebuilds: u32,
    pub resizes: u32,
    pub entered: u64,
    pub history: Vec<RowDump>,
    pub screen: Vec<RowDump>,
}

impl TestGrid {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows,
            cols,
            cursor: (0, 0),
            modes: Modes::default(),
            title: None,
            bells: 0,
            rebuilds: 0,
            resizes: 0,
            entered: 0,
            history: Vec::new(),
            screen: vec![RowDump::blank(usize::from(cols)); usize::from(rows)],
        }
    }

    /// Row `r` set to `text`, one cell per `char` (all width 1), blank after.
    pub fn text(mut self, r: usize, text: &str) -> Self {
        self.set_text(r, text);
        self
    }

    pub fn set_text(&mut self, r: usize, text: &str) {
        for (c, ch) in text.chars().enumerate() {
            self.screen[r].cells[c].text = ch.to_string();
        }
        for c in text.chars().count()..self.screen[r].cells.len() {
            self.screen[r].cells[c] = CellDump::blank();
        }
    }

    pub fn cell(&mut self, r: usize, c: usize) -> &mut CellDump {
        &mut self.screen[r].cells[c]
    }

    /// A wide character at `c` and its spacer at `c + 1`.
    pub fn wide(&mut self, r: usize, c: usize, ch: char) {
        self.screen[r].cells[c].text = ch.to_string();
        self.screen[r].cells[c].width = 2;
        let spacer = &mut self.screen[r].cells[c + 1];
        spacer.text = String::new();
        spacer.width = 0;
    }

    pub fn wrapped(mut self, r: usize) -> Self {
        self.screen[r].wrapped = true;
        self
    }
}

impl GridSource for TestGrid {
    fn rows(&self) -> u16 {
        self.rows
    }
    fn cols(&self) -> u16 {
        self.cols
    }
    fn cursor(&self) -> (u16, u16) {
        self.cursor
    }
    fn modes(&self) -> Modes {
        self.modes
    }
    fn title(&self) -> Option<String> {
        self.title.clone()
    }
    fn bell_count(&self) -> u64 {
        self.bells
    }
    fn rebuilds(&self) -> u32 {
        self.rebuilds
    }
    fn resizes(&self) -> u32 {
        self.resizes
    }
    fn history_rows_entered(&self) -> u64 {
        self.entered
    }
    fn history_len(&self) -> usize {
        self.history.len()
    }
    fn screen_row(&self, i: usize) -> RowDump {
        self.screen[i].clone()
    }
    fn history_row(&self, i: usize) -> RowDump {
        self.history[i].clone()
    }
}

/// The frame as readable text: ESC shown as `^[`, BEL as `^G`.
pub(super) fn show(frame: &[u8]) -> String {
    String::from_utf8_lossy(frame)
        .replace('\x1b', "^[")
        .replace('\x07', "^G")
}

pub(super) const OPEN: &str = "^[[?2026h^[[0m";
pub(super) const CLOSE: &str = "^[[?2026l";

/// A frame for a viewer that is already up to date with `grid`.
pub(super) fn synced(grid: &TestGrid) -> ViewerState {
    let mut v = ViewerState::new();
    assert!(encode_frame(grid, &mut v).is_some());
    v
}

/// The frame for `grid`, as readable text, with its fixed open and close
/// stripped.
pub(super) fn body(grid: &impl GridSource, v: &mut ViewerState) -> Option<String> {
    let f = show(&encode_frame(grid, v)?);
    let f = f.strip_prefix(OPEN).expect("opens with sync and a reset");
    Some(f.strip_suffix(CLOSE).expect("closes the sync").to_owned())
}

pub(super) fn engine(rows: u16, cols: u16, bytes: &str) -> AlacrittyEngine {
    let mut e = AlacrittyEngine::new(rows, cols);
    e.feed(bytes.as_bytes());
    e
}
