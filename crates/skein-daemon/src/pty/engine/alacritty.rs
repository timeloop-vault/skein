//! The `alacritty_terminal`-backed [`TerminalEngine`] (#793, design T3).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Timeout};

use super::counting::{Counting, OVERFLOW_ROWS, trim_history};
use super::dump::{CellDump, ColorDump, CursorDump, GridDump, RowDump};
use super::{CursorShape, HISTORY_ROWS, Modes, MouseEncoding, MouseTracking, TerminalEngine};
use crate::pty::redraw::{ViewerState, encode_frame};

/// The smallest terminal the engine builds. `alacritty_terminal` itself
/// documents a minimum of two columns (`term::MIN_COLUMNS`, 2), so a request for
/// fewer is raised to this.
pub(in crate::pty) const MIN_COLS: u16 = 2;
pub(in crate::pty) const MIN_ROWS: u16 = 1;

/// What the `Term` reports through its event listener.
#[derive(Default)]
pub(super) struct Pending {
    pub(super) replies: Vec<u8>,
    pub(super) title: Option<String>,
    pub(super) bell: bool,
    /// Every bell since this listener was built (see `bell_count`).
    pub(super) bells: u64,
}

/// Collects the events the daemon cares about. Everything else a UI would
/// act on (clipboard, colour and size queries, wakeups) is dropped: the
/// engine answers no OSC 4/10/11 colour query and no `CSI 14 t`.
#[derive(Clone, Default)]
pub(super) struct Listener(Arc<Mutex<Pending>>);

impl Listener {
    pub(super) fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut p = self.pending();
        match event {
            Event::PtyWrite(text) => p.replies.extend_from_slice(text.as_bytes()),
            Event::Title(title) => p.title = Some(title),
            Event::ResetTitle => p.title = None,
            Event::Bell => {
                p.bell = true;
                p.bells += 1;
            }
            _ => {}
        }
    }
}

struct Size {
    rows: usize,
    cols: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// The engine state that is dropped and rebuilt after a panic.
struct Inner {
    term: Term<Listener>,
    parser: Processor<SyncGate>,
    clock: Clock,
    /// When the open synchronized update ends, by `clock`.
    deadline: Option<Instant>,
    /// `SyncGate::arms` as of the last stamp.
    seen_arms: u64,
    listener: Listener,
}

impl Inner {
    fn new(rows: u16, cols: u16, clock: Clock) -> Self {
        let listener = Listener::default();
        let config = Config {
            // Room for the rows `counting` cuts away after each call.
            scrolling_history: HISTORY_ROWS + OVERFLOW_ROWS,
            kitty_keyboard: true,
            ..Config::default()
        };
        let size = Size {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        Self {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            clock,
            deadline: None,
            seen_arms: 0,
            listener,
        }
    }

    fn advance(&mut self, bytes: &[u8], entered: &mut u64) {
        let mut handler = Counting {
            term: &mut self.term,
            entered,
        };
        self.parser.advance(&mut handler, bytes);
        self.stamp();
    }

    /// Gives a newly opened or extended synchronized update its deadline
    /// from our clock. `vte` only tells the gate that a timeout was armed;
    /// the stamp is taken when the call that armed it returns.
    fn stamp(&mut self) {
        let gate = self.parser.sync_timeout();
        if !gate.active {
            self.deadline = None;
        } else if gate.arms != self.seen_arms {
            self.seen_arms = gate.arms;
            self.deadline = Some((self.clock)() + SYNC_TIMEOUT);
        }
    }

    fn sync_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    fn end_sync(&mut self, entered: &mut u64) {
        if self.parser.sync_timeout().active {
            let mut handler = Counting {
                term: &mut self.term,
                entered,
            };
            self.parser.stop_sync(&mut handler);
            self.stamp();
        }
    }

    fn take_replies(&self) -> Vec<u8> {
        std::mem::take(&mut self.listener.pending().replies)
    }
}

/// How long a synchronized update may stay open (`vte`'s private
/// `SYNC_UPDATE_TIMEOUT`).
const SYNC_TIMEOUT: Duration = Duration::from_millis(150);

/// The time source of the engine's one time-dependent behaviour.
pub(super) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// `vte`'s sync timeout hook. `Processor` is generic over it, but the hook
/// is only told "arm for this long" and is built with `Default`, so it
/// cannot hold a clock; it just records that it was armed and the engine
/// stamps the deadline from its own clock (see `Inner::stamp`).
#[derive(Default)]
struct SyncGate {
    active: bool,
    arms: u64,
}

impl Timeout for SyncGate {
    fn set_timeout(&mut self, _: Duration) {
        self.active = true;
        self.arms += 1;
    }
    fn clear_timeout(&mut self) {
        self.active = false;
    }
    fn pending_timeout(&self) -> bool {
        self.active
    }
}

/// The damage `alacritty_terminal` recorded since it was last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    /// Everything: after creation, a resize, a screen switch or a scroll.
    Full,
    /// These lines of the screen, each with the leftmost and rightmost
    /// damaged column (inclusive). Only damaged lines are listed.
    Lines(Vec<LineDamage>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineDamage {
    pub line: usize,
    pub left: usize,
    pub right: usize,
}

/// A [`TerminalEngine`] over `alacritty_terminal` 0.26.0.
pub struct AlacrittyEngine {
    inner: Inner,
    rows: u16,
    cols: u16,
    /// Rows that entered scrollback by output scrolling, since creation.
    /// Survives a panic rebuild so it stays monotonic.
    entered: u64,
    rebuilds: u32,
    /// Calls to `resize` that changed the size. Survives a panic rebuild.
    resizes: u32,
    /// Bells counted by listeners a rebuild dropped.
    bells_before: u64,
    clock: Clock,
}

impl AlacrittyEngine {
    /// A fresh engine. A zero (or, for columns, a one) is raised to the
    /// smallest size `alacritty_terminal` supports: one row, two columns.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::build(rows, cols, Arc::new(Instant::now))
    }

    /// An engine whose synchronized-update timeout reads `clock` instead of
    /// the wall clock, so a test decides exactly when an update expires.
    #[cfg(test)]
    pub(super) fn with_clock(rows: u16, cols: u16, clock: Clock) -> Self {
        Self::build(rows, cols, clock)
    }

    fn build(rows: u16, cols: u16, clock: Clock) -> Self {
        let (rows, cols) = (rows.max(MIN_ROWS), cols.max(MIN_COLS));
        Self {
            inner: Inner::new(rows, cols, clock.clone()),
            clock,
            rows,
            cols,
            entered: 0,
            rebuilds: 0,
            resizes: 0,
            bells_before: 0,
        }
    }

    /// Runs `f` against the engine state. A panic inside it is caught: the
    /// state is dropped and rebuilt empty at the last requested size, and
    /// `default` is returned.
    fn guarded<R>(&mut self, default: R, f: impl FnOnce(&mut Inner, &mut u64) -> R) -> R {
        let Self { inner, entered, .. } = self;
        match catch_unwind(AssertUnwindSafe(|| f(inner, entered))) {
            Ok(r) => r,
            Err(payload) => {
                let what = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                tracing::warn!(rows = self.rows, cols = self.cols, panic = %what,
                    "terminal engine panicked; rebuilt it empty");
                self.bells_before += self.inner.listener.pending().bells;
                self.inner = Inner::new(self.rows, self.cols, self.clock.clone());
                self.rebuilds += 1;
                default
            }
        }
    }

    /// Test seam: the `Term` behind the engine.
    #[cfg(test)]
    pub(super) fn term(&self) -> &Term<Listener> {
        &self.inner.term
    }

    /// Test seam: runs `f` through the same guard every engine call uses.
    #[cfg(test)]
    pub(super) fn run_guarded(&mut self, f: impl FnOnce()) {
        self.guarded((), |_, _| f());
    }

    /// Test seam: every screen cell's dump, with `cell_is(cell, dump_j)`
    /// against every cell's dump `j` (row-major), so a test can check that
    /// `cell_is` agrees with `cell_dump` without them being visible.
    #[cfg(test)]
    pub(super) fn cell_is_matrix(&self) -> Vec<(CellDump, Vec<bool>)> {
        let grid = self.inner.term.grid();
        let cells: Vec<&Cell> = (0..to_i32(grid.screen_lines()))
            .flat_map(|l| (0..grid.columns()).map(move |c| (l, c)))
            .map(|(l, c)| &grid[Line(l)][Column(c)])
            .collect();
        let dumps: Vec<CellDump> = cells.iter().map(|c| cell_dump(c)).collect();
        cells
            .iter()
            .zip(&dumps)
            .map(|(cell, d)| (d.clone(), dumps.iter().map(|o| cell_is(cell, o)).collect()))
            .collect()
    }

    /// How many times a caught panic rebuilt the engine empty. A viewer
    /// must be sent a full redraw when it changes.
    pub fn rebuilds(&self) -> u32 {
        self.rebuilds
    }

    /// How many `resize` calls changed the size. A viewer must be sent a
    /// full redraw when it changes: a resize away and back keeps the size
    /// but can rewrite the history.
    pub fn resizes(&self) -> u32 {
        self.resizes
    }

    /// The number of rows that have entered scrollback history since the
    /// engine was created (a rebuild does not reset it). Monotonic, and it
    /// keeps growing after the history is full.
    ///
    /// Counted: rows pushed off the top of the main screen by output (line
    /// feed at the bottom row, `CSI n S`, an insert or delete of lines, a
    /// wrapped character). Also counted: a scroll region that starts at the
    /// top row but ends above the bottom (alacritty, like xterm, sends those
    /// rows to history too). Not counted: scrolls inside a region that
    /// starts below the top row; anything on the alternate screen (it keeps
    /// no history); rows a resize pushes into or pulls out of history; rows
    /// dropped by `ESC[3J` or a reset (history shrinking is not an entry).
    ///
    /// Exact for every output sequence, including one flushed from a
    /// synchronized update. Wrong in one case: a resize while the history
    /// holds rows makes the count and the history's contents unrelated
    /// from then on, because the resize itself moves rows across the
    /// boundary.
    pub fn history_rows_entered(&self) -> u64 {
        self.entered
    }

    /// Rows currently in the main screen's scrollback (0 on the alternate
    /// screen, whose grid keeps none).
    pub fn history_len(&self) -> usize {
        self.inner.term.history_size()
    }

    /// The window title set by OSC 0/2, if any.
    pub fn title(&self) -> Option<String> {
        self.inner.listener.pending().title.clone()
    }

    /// Whether a bell rang since the last call; clears the flag.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.inner.listener.pending().bell)
    }

    /// The terminal's size, `(rows, cols)`, after the clamp.
    pub(crate) fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Every bell since the engine was created, a rebuild included.
    /// Monotonic, and unlike [`take_bell`](Self::take_bell) it has no single
    /// consumer: each viewer remembers the count it was last sent.
    pub fn bell_count(&self) -> u64 {
        self.bells_before + self.inner.listener.pending().bells
    }

    /// One row of the active screen (0 is the top), read from the grid
    /// without dumping the rest. Past the last row it is a blank row.
    pub(crate) fn screen_row_dump(&self, row: usize) -> RowDump {
        let grid = self.inner.term.grid();
        if row >= grid.screen_lines() {
            return RowDump::blank(usize::from(self.cols));
        }
        row_dump(grid, to_i32(row), -to_i32(grid.history_size()))
    }

    /// One row of the main screen's scrollback (0 is the oldest). Past the
    /// end, and on the alternate screen, it is a blank row.
    pub(crate) fn history_row_dump(&self, row: usize) -> RowDump {
        let grid = self.inner.term.grid();
        let history = grid.history_size();
        if row >= history {
            return RowDump::blank(usize::from(self.cols));
        }
        row_dump(grid, to_i32(row) - to_i32(history), -to_i32(history))
    }

    /// Reads the damage `alacritty_terminal` collected and resets it. Takes
    /// `&mut self` because reading records the cursor's position for the
    /// next read.
    pub fn take_damage(&mut self) -> Damage {
        self.guarded(Damage::Full, |inner, _| {
            let damage = match inner.term.damage() {
                TermDamage::Full => Damage::Full,
                TermDamage::Partial(lines) => Damage::Lines(
                    lines
                        .map(|d| LineDamage {
                            line: d.line,
                            left: d.left,
                            right: d.right,
                        })
                        .collect(),
                ),
            };
            inner.term.reset_damage();
            damage
        })
    }

    /// An engine-neutral picture of the terminal, for the cross-engine test
    /// and for the frame encoder. Reads the grid only.
    ///
    /// While the alternate screen is active `history` is empty and
    /// `hidden_screen` is `None`: the main grid is private to the `Term`, and
    /// the only public way to reach it (`swap_alt` twice) wipes the alternate
    /// grid on the way back.
    pub fn dump(&self) -> GridDump {
        let term = &self.inner.term;
        let grid = term.grid();
        let history = grid.history_size();
        let rows = grid.screen_lines();
        let row_at = |line: i32| row_dump(grid, line, -to_i32(history));
        let point = grid.cursor.point;
        GridDump {
            rows: self.rows,
            cols: self.cols,
            alt_screen: term.mode().contains(TermMode::ALT_SCREEN),
            cursor: CursorDump {
                row: line_u16(point.line),
                col: to_u16(point.column.0),
                visible: term.mode().contains(TermMode::SHOW_CURSOR),
            },
            modes: self.modes(),
            history: (-to_i32(history)..0).map(row_at).collect(),
            screen: (0..to_i32(rows)).map(row_at).collect(),
            hidden_screen: None,
        }
    }
}

fn to_i32(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

fn line_u16(line: Line) -> u16 {
    u16::try_from(line.0.max(0)).unwrap_or(u16::MAX)
}

fn to_u16(v: usize) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

impl TerminalEngine for AlacrittyEngine {
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.guarded(Vec::new(), |inner, entered| {
            if inner.sync_deadline().is_some_and(|d| (inner.clock)() >= d) {
                inner.end_sync(entered);
            }
            inner.advance(bytes, entered);
            inner.take_replies()
        })
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(MIN_ROWS), cols.max(MIN_COLS));
        if (rows, cols) != (self.rows, self.cols) {
            self.resizes += 1;
        }
        self.rows = rows;
        self.cols = cols;
        self.guarded((), |inner, _| {
            inner.term.resize(Size {
                rows: usize::from(rows),
                cols: usize::from(cols),
            });
            trim_history(&mut inner.term);
        });
    }

    fn screen_text(&self) -> Vec<String> {
        let grid = self.inner.term.grid();
        (0..to_i32(grid.screen_lines()))
            .map(|l| row_text(grid, l))
            .collect()
    }

    fn modes(&self) -> Modes {
        let term = &self.inner.term;
        let m = *term.mode();
        let style = term.cursor_style();
        let mouse_tracking = if m.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::Motion
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Click
        } else {
            MouseTracking::Off
        };
        let mouse_encoding = if m.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if m.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        };
        let kitty = [
            (TermMode::DISAMBIGUATE_ESC_CODES, 1),
            (TermMode::REPORT_EVENT_TYPES, 2),
            (TermMode::REPORT_ALTERNATE_KEYS, 4),
            (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
            (TermMode::REPORT_ASSOCIATED_TEXT, 16),
        ]
        .into_iter()
        .filter(|(flag, _)| m.contains(*flag))
        .fold(0u8, |acc, (_, bit)| acc | bit);
        Modes {
            app_cursor_keys: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            focus_reporting: m.contains(TermMode::FOCUS_IN_OUT),
            mouse_tracking,
            mouse_encoding,
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            cursor_visible: m.contains(TermMode::SHOW_CURSOR),
            cursor_shape: match style.shape {
                alacritty_terminal::vte::ansi::CursorShape::Underline => CursorShape::Underline,
                alacritty_terminal::vte::ansi::CursorShape::Beam => CursorShape::Bar,
                _ => CursorShape::Block,
            },
            cursor_blinking: style.blinking,
            kitty_keyboard_flags: kitty,
            sync_pending: self.inner.sync_deadline().is_some(),
        }
    }

    fn cursor(&self) -> (u16, u16) {
        let point = self.inner.term.grid().cursor.point;
        (line_u16(point.line), to_u16(point.column.0))
    }

    fn sync_deadline(&self) -> Option<Instant> {
        self.inner.sync_deadline()
    }

    fn flush_sync(&mut self) -> Vec<u8> {
        self.guarded(Vec::new(), |inner, entered| {
            inner.end_sync(entered);
            inner.take_replies()
        })
    }

    fn frame(&self, view: &mut ViewerState) -> Option<Vec<u8>> {
        self.frame_with(view, encode_frame)
    }
}

impl AlacrittyEngine {
    /// Runs `encode` under a panic guard. A panic in the encoder says
    /// nothing about the grid, so the engine is kept; the viewer is reset so
    /// that its next frame is a full redraw, and this frame is `None`.
    pub(super) fn frame_with(
        &self,
        view: &mut ViewerState,
        encode: impl FnOnce(&Self, &mut ViewerState) -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        match catch_unwind(AssertUnwindSafe(|| encode(self, view))) {
            Ok(frame) => frame,
            Err(payload) => {
                let what = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                tracing::warn!(rows = self.rows, cols = self.cols, panic = %what,
                    "frame encoder panicked; the viewer gets a full redraw");
                view.invalidate();
                None
            }
        }
    }

    /// Whether row `row` of the active screen equals `other`, compared cell
    /// by cell in place with no allocation.
    pub(crate) fn screen_row_is_dump(&self, row: usize, other: &RowDump) -> bool {
        let grid = self.inner.term.grid();
        if row >= grid.screen_lines() {
            return self.screen_row_dump(row) == *other;
        }
        let (line, topmost) = (to_i32(row), -to_i32(grid.history_size()));
        let wrapped = line > topmost
            && grid[Line(line - 1)][grid.last_column()]
                .flags
                .contains(Flags::WRAPLINE);
        let cells = &grid[Line(line)];
        wrapped == other.wrapped
            && cells.len() == other.cells.len()
            && (0..cells.len()).all(|c| cell_is(&cells[Column(c)], &other.cells[c]))
    }
}

/// `cell_dump(cell) == *dump`, without building the dump.
fn cell_is(cell: &Cell, d: &CellDump) -> bool {
    let f = cell.flags;
    let spacer = f.contains(Flags::WIDE_CHAR_SPACER);
    let text =
        if spacer {
            d.text.is_empty()
        } else {
            d.text.chars().eq(std::iter::once(cell.c)
                .chain(cell.zerowidth().map_or(&[][..], |z| z).iter().copied()))
        };
    let width = if spacer {
        0
    } else if f.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    };
    text && d.width == width
        && d.fg == color_dump(cell.fg)
        && d.bg == color_dump(cell.bg)
        && d.bold == f.contains(Flags::BOLD)
        && d.italic == f.contains(Flags::ITALIC)
        && d.dim == f.contains(Flags::DIM)
        && d.underline == f.intersects(Flags::ALL_UNDERLINES)
        && d.inverse == f.contains(Flags::INVERSE)
        && d.invisible == f.contains(Flags::HIDDEN)
        && d.strikethrough == f.contains(Flags::STRIKEOUT)
        && d.leading_spacer == f.contains(Flags::LEADING_WIDE_CHAR_SPACER)
        && cell
            .hyperlink()
            .as_ref()
            .map(alacritty_terminal::term::cell::Hyperlink::uri)
            == d.link.as_deref()
}

fn row_text(grid: &Grid<Cell>, line: i32) -> String {
    let row = &grid[Line(line)];
    let mut out = String::new();
    for col in 0..row.len() {
        let cell = &row[Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        out.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            out.extend(extra);
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// One row; `topmost` is the line number of the oldest row in the grid.
fn row_dump(grid: &Grid<Cell>, line: i32, topmost: i32) -> RowDump {
    let wrapped = line > topmost
        && grid[Line(line - 1)][grid.last_column()]
            .flags
            .contains(Flags::WRAPLINE);
    let row = &grid[Line(line)];
    RowDump {
        wrapped,
        cells: (0..row.len()).map(|c| cell_dump(&row[Column(c)])).collect(),
    }
}

fn cell_dump(cell: &Cell) -> CellDump {
    let f = cell.flags;
    let spacer = f.contains(Flags::WIDE_CHAR_SPACER);
    let mut text = String::new();
    if !spacer {
        text.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra);
        }
    }
    CellDump {
        text,
        width: if spacer {
            0
        } else if f.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        },
        fg: color_dump(cell.fg),
        bg: color_dump(cell.bg),
        bold: f.contains(Flags::BOLD),
        italic: f.contains(Flags::ITALIC),
        dim: f.contains(Flags::DIM),
        underline: f.intersects(Flags::ALL_UNDERLINES),
        inverse: f.contains(Flags::INVERSE),
        invisible: f.contains(Flags::HIDDEN),
        strikethrough: f.contains(Flags::STRIKEOUT),
        link: cell.hyperlink().map(|h| h.uri().to_owned()),
        leading_spacer: f.contains(Flags::LEADING_WIDE_CHAR_SPACER),
    }
}

fn color_dump(color: Color) -> ColorDump {
    match color {
        Color::Spec(rgb) => ColorDump::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
        Color::Indexed(index) => ColorDump::Palette { index },
        Color::Named(named) => match named {
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => ColorDump::Default,
            // 0-15 are the ANSI colours; the Dim* variants (never produced
            // by the parser) sit at 261 and up and fold onto 0-7.
            other => {
                let n = other as usize;
                let index = if n < 16 {
                    n
                } else {
                    n - NamedColor::DimBlack as usize
                };
                ColorDump::Palette {
                    index: u8::try_from(index).unwrap_or(0),
                }
            }
        },
    }
}
