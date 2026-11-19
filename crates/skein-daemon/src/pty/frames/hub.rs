//! The per-PTY frame hub (#760, T-3): one engine, many viewers, one size
//! owner. A viewer attaches as a full redraw and then gets diff frames.
//! Design: `docs/skeind-terminal-design.md`, "Attach, reconnect, several
//! viewers".
//!
//! One lock covers the engine, the viewers and the owner. Sinks are sent to
//! under it (each viewer's events stay ordered, as in `SinkSlot`); the
//! observer and the reply callback never run with it held.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use super::state::{Core, Viewer, send};
use crate::pty::PtyEvent;
use crate::pty::engine::{Modes, TerminalEngine};
use crate::pty::observer::PtyObserver;
use crate::pty::redraw::ViewerState;
use crate::pty::sink_slot::{AttachOutcome, ExitRecord};
use crate::stream_sink::StreamSink;

/// What the hub needs from its PTY.
pub struct HubIo {
    /// Writes bytes to the PTY's stdin (engine query replies). Only enqueues.
    pub reply: Box<dyn Fn(Vec<u8>) + Send + Sync>,
    /// Resizes the real PTY. Err = the PTY refused; the engine is then not resized.
    pub resize_pty: Box<dyn Fn(u16, u16) -> Result<(), String> + Send + Sync>,
}

/// One PTY's engine and its viewers.
pub struct FrameHub {
    core: Mutex<Core>,
    io: HubIo,
    observer: Option<Arc<dyn PtyObserver>>,
}

impl FrameHub {
    /// A hub whose engine starts at `rows` x `cols`.
    pub fn new(rows: u16, cols: u16, io: HubIo) -> Self {
        Self {
            core: Mutex::new(Core::new(rows, cols)),
            io,
            observer: None,
        }
    }

    /// Tap this hub: the observer hears the exit (and, through
    /// [`observe_output`](Self::observe_output), the output), never with the
    /// lock held.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn PtyObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn reply(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            (self.io.reply)(bytes);
        }
    }

    /// PTY output into the engine. Query replies go to the PTY, unlocked.
    pub fn feed(&self, bytes: &[u8]) {
        let replies = self.core.lock().engine.feed(bytes);
        self.reply(replies);
    }

    /// Tell the observer about a decoded output chunk, with no lock held.
    pub fn observe_output(&self, text: &str) {
        if let Some(observer) = &self.observer {
            observer.output(text);
        }
    }

    /// Cut one frame for every viewer.
    pub fn cut(&self) {
        let mut core = self.core.lock();
        core.cut_all();
        core.settle(&self.io);
        drop(core);
        self.observe_screen();
    }

    /// Hand the observer the screen after a cut, if it reads it (#809). The
    /// rows are taken under the lock and the observer is called after it is
    /// released. Nothing is delivered while a synchronized update is open:
    /// the screen is half drawn then.
    fn observe_screen(&self) {
        let Some(observer) = &self.observer else {
            return;
        };
        if !observer.wants_screen() {
            return;
        }
        let rows = {
            let core = self.core.lock();
            if core.engine.modes().sync_pending {
                return;
            }
            core.engine.screen_text()
        };
        observer.screen(&rows);
    }

    /// The text of each row of the engine's active screen (#809).
    pub fn screen_text(&self) -> Vec<String> {
        self.core.lock().engine.screen_text()
    }

    /// The engine's terminal modes (#809).
    pub fn modes(&self) -> Modes {
        self.core.lock().engine.modes()
    }

    /// When an open synchronized update must be ended, if one is open.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.core.lock().engine.sync_deadline()
    }

    /// End the open synchronized update now; its replies go to the PTY.
    pub fn flush_sync(&self) {
        let replies = self.core.lock().engine.flush_sync();
        self.reply(replies);
    }

    /// Add a viewer: it gets the grid size, then a full redraw. `size` is
    /// the size its widget has. With no owner it becomes the owner and its
    /// size is applied first, so it sees one grid and one redraw. Returns the
    /// outcome and the viewer's id (monotonic per hub, the first is 1).
    pub fn attach(
        &self,
        sink: Box<dyn StreamSink<PtyEvent>>,
        size: Option<(u16, u16)>,
    ) -> (AttachOutcome, u64) {
        let mut core = self.core.lock();
        core.next_id += 1;
        let id = core.next_id;
        let stamp = core.next_stamp();
        if core.owner.is_none() {
            core.owner = Some(id);
            if let Some((r, c)) = size
                && (r, c) != core.grid
            {
                core.apply(&self.io, r, c);
            }
        }
        let (rows, cols) = core.grid;
        core.viewers.push(Viewer {
            id,
            sink,
            size,
            state: ViewerState::new(),
            stamp,
            dead: false,
        });
        let idx = core.viewers.len() - 1;
        send_at(&mut core, idx, PtyEvent::Grid { rows, cols });
        core.cut_one(idx);
        let outcome = match core.exited {
            Some(ExitRecord { code }) => {
                send_at(&mut core, idx, PtyEvent::Exit { code });
                AttachOutcome::Exited { code }
            }
            None => AttachOutcome::Live,
        };
        core.settle(&self.io);
        (outcome, id)
    }

    /// Remove viewer `viewer` if it is still attached.
    pub fn detach_if(&self, viewer: u64) -> bool {
        let mut core = self.core.lock();
        let Some(idx) = core.index_of(viewer) else {
            return false;
        };
        core.viewers.remove(idx);
        if core.owner == Some(viewer) {
            core.owner = None;
        }
        core.settle(&self.io);
        true
    }

    /// Remove every viewer. Whether any was attached.
    pub fn detach_all(&self) -> bool {
        let mut core = self.core.lock();
        let had = !core.viewers.is_empty();
        core.viewers.clear();
        core.owner = None;
        had
    }

    /// The most recently attached viewer that is still attached.
    pub fn latest_viewer(&self) -> Option<u64> {
        let core = self.core.lock();
        core.viewers.iter().filter(|v| !v.dead).map(|v| v.id).max()
    }

    /// A viewer reports its size, or `None` is a caller with no viewer
    /// identity. The size is recorded for a viewer; it reaches the PTY only
    /// from the owner (or, with no owner, makes it the owner) and only when
    /// it differs from the grid. Returns whether the PTY was resized.
    pub fn resize(&self, viewer: Option<u64>, rows: u16, cols: u16) -> bool {
        let mut core = self.core.lock();
        let applied = match viewer {
            Some(id) => {
                let Some(idx) = core.index_of(id) else {
                    return false;
                };
                if let Some(v) = core.viewers.get_mut(idx) {
                    v.size = Some((rows, cols));
                }
                if core.owner.is_none() {
                    core.owner = Some(id);
                }
                core.owner == Some(id)
                    && (rows, cols) != core.grid
                    && core.apply(&self.io, rows, cols)
            }
            None => (rows, cols) != core.grid && core.apply(&self.io, rows, cols),
        };
        core.settle(&self.io);
        applied
    }

    /// Viewer `viewer` sent input: it becomes the owner, and its size is
    /// applied if it has one and it differs.
    pub fn active(&self, viewer: u64) {
        let mut core = self.core.lock();
        let Some(idx) = core.index_of(viewer) else {
            return;
        };
        let stamp = core.next_stamp();
        if let Some(v) = core.viewers.get_mut(idx) {
            v.stamp = stamp;
        }
        if core.owner != Some(viewer) {
            core.owner = Some(viewer);
            core.apply_size_of(&self.io, viewer);
        }
        core.settle(&self.io);
    }

    /// The viewer asks for a full redraw now. False for an unknown viewer.
    pub fn request_redraw(&self, viewer: u64) -> bool {
        let mut core = self.core.lock();
        let Some(idx) = core.index_of(viewer) else {
            return false;
        };
        if let Some(v) = core.viewers.get_mut(idx) {
            v.state.invalidate();
        }
        core.cut_one(idx);
        core.settle(&self.io);
        true
    }

    /// The child exited: the output's last frame for every viewer, then (#809)
    /// the exit notice the observer supplies and the notices queued while it
    /// lived, appended to the engine and cut as one more frame, then `Exit`.
    /// An open synchronized update is ended first, so a notice never lands
    /// inside it. The engine and the viewers stay, so a later attach gets
    /// the screen, notices included.
    pub fn exit(&self, code: Option<u32>) {
        self.flush_sync();
        let notice = if self.exited().is_none() {
            self.observer
                .as_ref()
                .and_then(|observer| observer.exit_notice(code))
        } else {
            None
        };
        let (first, replies) = {
            let mut st = self.core.lock();
            st.cut_all();
            let first = st.exited.is_none();
            st.exited = Some(ExitRecord { code });
            let mut replies = Vec::new();
            if first {
                let queued = std::mem::take(&mut st.pending_notices);
                let notices = notice.into_iter().chain(queued);
                let mut any = false;
                for bytes in notices {
                    replies.extend(st.engine.feed(&bytes));
                    any = true;
                }
                if any {
                    st.cut_all();
                }
            }
            st.broadcast(&PtyEvent::Exit { code });
            st.settle(&self.io);
            (first, replies)
        };
        self.reply(replies);
        if first && let Some(observer) = &self.observer {
            observer.exited(code);
        }
    }

    /// Append a pane notice (#809). Once the child has exited it goes to the
    /// engine and to every viewer at once; while it lives it is queued and
    /// comes out at exit, after the exit notice, so nothing but the child's
    /// own output reaches the live parser.
    pub fn append_notice(&self, bytes: &[u8]) {
        let replies = {
            let mut st = self.core.lock();
            if st.exited.is_none() {
                st.pending_notices.push(bytes.to_vec());
                return;
            }
            let replies = st.engine.feed(bytes);
            st.cut_all();
            st.settle(&self.io);
            replies
        };
        self.reply(replies);
    }

    /// `Some` once the child has exited.
    pub fn exited(&self) -> Option<ExitRecord> {
        self.core.lock().exited
    }

    /// The grid the frames are drawn at, `(rows, cols)`.
    pub fn grid_size(&self) -> (u16, u16) {
        self.core.lock().grid
    }

    /// The viewer whose size the PTY follows.
    pub fn size_owner(&self) -> Option<u64> {
        self.core.lock().owner
    }

    pub fn viewer_count(&self) -> usize {
        self.core.lock().viewers.iter().filter(|v| !v.dead).count()
    }
}

fn send_at(core: &mut Core, idx: usize, event: PtyEvent) {
    if let Some(v) = core.viewers.get_mut(idx) {
        send(v, event);
    }
}
