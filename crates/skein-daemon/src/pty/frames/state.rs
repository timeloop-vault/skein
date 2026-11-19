//! The state behind one [`FrameHub`](super::FrameHub)'s lock: the engine,
//! its viewers and the size owner. Every method runs with that lock held, so
//! none of them calls the observer or the reply callback; they only send to
//! sinks (which keeps each viewer's events ordered) and call `resize_pty`
//! (which must be atomic with the engine resize).

use crate::pty::PtyEvent;
use crate::pty::engine::{AlacrittyEngine, MIN_COLS, MIN_ROWS, TerminalEngine};
use crate::pty::redraw::ViewerState;
use crate::pty::sink_slot::ExitRecord;
use crate::stream_sink::StreamSink;

use super::hub::HubIo;

pub(super) struct Viewer {
    pub id: u64,
    pub sink: Box<dyn StreamSink<PtyEvent>>,
    /// The size this viewer last reported, if any.
    pub size: Option<(u16, u16)>,
    pub state: ViewerState,
    /// When it last attached or was active; from the hub's one counter.
    pub stamp: u64,
    /// A send failed; removed at the end of the pass.
    pub dead: bool,
}

pub(super) struct Core {
    pub engine: AlacrittyEngine,
    pub viewers: Vec<Viewer>,
    pub owner: Option<u64>,
    pub grid: (u16, u16),
    pub exited: Option<ExitRecord>,
    pub next_id: u64,
    pub activity: u64,
}

impl Core {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            engine: AlacrittyEngine::new(rows, cols),
            viewers: Vec::new(),
            owner: None,
            grid: (rows, cols),
            exited: None,
            next_id: 0,
            activity: 0,
        }
    }

    pub fn next_stamp(&mut self) -> u64 {
        self.activity += 1;
        self.activity
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.viewers.iter().position(|v| v.id == id && !v.dead)
    }

    /// Cut one frame for viewer `idx` and send it.
    pub fn cut_one(&mut self, idx: usize) {
        let Some(v) = self.viewers.get_mut(idx) else {
            return;
        };
        if v.dead {
            return;
        }
        if let Some(bytes) = self.engine.frame(&mut v.state) {
            send(
                v,
                PtyEvent::Data {
                    chunk: to_text(bytes),
                },
            );
        }
    }

    pub fn cut_all(&mut self) {
        for idx in 0..self.viewers.len() {
            self.cut_one(idx);
        }
    }

    /// Send `event` to every viewer.
    pub fn broadcast(&mut self, event: &PtyEvent) {
        for v in &mut self.viewers {
            send(v, event.clone());
        }
    }

    /// Resize the PTY, then the engine, tell every viewer the new grid and
    /// cut every viewer. False, with nothing changed, for a size below the
    /// engine's minimum (which would clamp, so the grid would lie), once the
    /// child has exited (sizes are then only recorded), or when the PTY
    /// refuses. May mark viewers dead; the caller settles.
    pub fn apply(&mut self, io: &HubIo, rows: u16, cols: u16) -> bool {
        if rows < MIN_ROWS || cols < MIN_COLS || self.exited.is_some() {
            return false;
        }
        if let Err(e) = (io.resize_pty)(rows, cols) {
            tracing::warn!(rows, cols, error = %e, "pty resize refused; grid unchanged");
            return false;
        }
        self.engine.resize(rows, cols);
        self.grid = (rows, cols);
        self.broadcast(&PtyEvent::Grid { rows, cols });
        self.cut_all();
        true
    }

    /// Apply viewer `id`'s recorded size when it has one and it differs.
    pub fn apply_size_of(&mut self, io: &HubIo, id: u64) -> bool {
        let size = self
            .viewers
            .iter()
            .find(|v| v.id == id && !v.dead)
            .and_then(|v| v.size);
        match size {
            Some((r, c)) if (r, c) != self.grid => self.apply(io, r, c),
            _ => false,
        }
    }

    /// Remove the viewers whose sink failed; clears the owner if it was one.
    fn remove_dead(&mut self) {
        let owner = self.owner;
        self.viewers.retain(|v| !v.dead);
        if owner.is_some_and(|o| self.viewers.iter().all(|v| v.id != o)) {
            self.owner = None;
        }
    }

    /// Remove dead viewers, then, while there is no owner, hand ownership to
    /// the remaining viewer with the latest activity and apply its size. A
    /// pass can kill more viewers, so it repeats, at most once per viewer.
    pub fn settle(&mut self, io: &HubIo) {
        for _ in 0..=self.viewers.len() {
            self.remove_dead();
            if self.owner.is_some() {
                return;
            }
            let Some(next) = self.viewers.iter().max_by_key(|v| v.stamp).map(|v| v.id) else {
                return;
            };
            self.owner = Some(next);
            self.apply_size_of(io, next);
        }
        self.remove_dead();
    }
}

/// Send to one viewer; a failure marks it dead (non-fatal: no kill, no panic).
pub(super) fn send(v: &mut Viewer, event: PtyEvent) {
    if v.dead {
        return;
    }
    if let Err(e) = v.sink.send(event) {
        tracing::info!(viewer = v.id, error = %e, "frame viewer dropped: send failed");
        v.dead = true;
    }
}

/// Frame bytes are text; a lossy fallback keeps a bad byte from dropping a frame.
fn to_text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}
