//! `alacritty_terminal` wrapper. Owns the parser + the `Term` grid;
//! exposes a thin advance/resize surface. Output that the term itself
//! wants to write back to the PTY (cursor reports, mouse reports, OSC
//! responses) is delivered through a `pty_input_tx` channel — the UI
//! layer drains it and forwards via `PtyManager::write`.
//!
//! Pattern lifted from `lapce/lapce-app/src/terminal/raw.rs` for the
//! pure-Rust prototype (issue #36).

use alacritty_terminal::{
    Term,
    event::{Event as AlacEvent, EventListener},
    term::{Config, test::TermSize},
    vte::ansi,
};
use crossbeam_channel::Sender;

pub struct EventProxy {
    pty_input_tx: Sender<Vec<u8>>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        if let AlacEvent::PtyWrite(s) = event {
            let _ = self.pty_input_tx.send(s.into_bytes());
        }
    }
}

pub struct RawTerm {
    pub parser: ansi::Processor,
    pub term: Term<EventProxy>,
    /// Pixel-precise scroll accumulator. macOS trackpad emits sub-line
    /// wheel deltas; we batch them and only step the alacritty grid
    /// when we cross a full line. Pattern lifted from Lapce.
    pub scroll_delta: f64,
}

impl RawTerm {
    pub fn new(rows: u16, cols: u16, pty_input_tx: Sender<Vec<u8>>) -> Self {
        let config = Config {
            semantic_escape_chars: ",│`|\"' ()[]{}<>\t".to_string(),
            ..Config::default()
        };
        let proxy = EventProxy { pty_input_tx };
        let size = TermSize::new(cols.into(), rows.into());
        let term = Term::new(config, &size, proxy);
        let parser = ansi::Processor::new();
        Self {
            parser,
            term,
            scroll_delta: 0.0,
        }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.term.resize(TermSize::new(cols.into(), rows.into()));
    }
}
