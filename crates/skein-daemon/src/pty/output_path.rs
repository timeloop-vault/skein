//! The two ways a PTY's output reaches its viewers (#760, T-3): the raw
//! path (a replaceable [`SinkSlot`] fed by the coalescer) and the frames
//! path (a [`FrameHub`] fed by the frame scheduler). [`PtyOutput`] is what a
//! live `Pty` holds; `PtyManager` calls only its methods.

use std::io::Read;
use std::sync::{Arc, Weak, mpsc};
use std::thread;

use parking_lot::Mutex;
use portable_pty::{MasterPty, PtySize};

use super::PtyEvent;
use super::frames::{FrameHub, HubDriver, HubIo, TerminalMode, run_frame_scheduler};
use super::observer::PtyObserver;
use super::output::{COALESCE_BYTE_CAP, COALESCE_WINDOW, run_coalescer};
use super::sink_slot::{AttachOutcome, ExitRecord, SinkSlot};
use crate::stream_sink::StreamSink;

/// The PTY's master, shared so the frames hub can resize it from its own lock.
pub(super) type SharedMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;

/// Resize the PTY behind `master`.
pub(super) fn resize_master(master: &SharedMaster, rows: u16, cols: u16) -> Result<(), String> {
    master
        .lock()
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

/// Where one PTY's output goes.
#[derive(Clone)]
pub(super) enum PtyOutput {
    Raw(Arc<SinkSlot>),
    Frames(Arc<FrameHub>),
}

/// What [`PtyOutput::build`] needs from the spawn.
pub(super) struct OutputInit {
    pub mode: TerminalMode,
    pub rows: u16,
    pub cols: u16,
    pub sink: Option<Box<dyn StreamSink<PtyEvent>>>,
    pub observer: Option<Arc<dyn PtyObserver>>,
    pub injected: bool,
    pub writer_tx: mpsc::Sender<Vec<u8>>,
    pub master: SharedMaster,
}

impl PtyOutput {
    /// Build the output for `init.mode`, attach the first sink and tell the
    /// observer the child exists. Runs before any thread can produce output.
    pub fn build(init: OutputInit) -> Self {
        let OutputInit {
            mode,
            rows,
            cols,
            sink,
            observer,
            injected,
            writer_tx,
            master,
        } = init;
        match mode {
            TerminalMode::Raw => {
                // #731: its unattended `ConPTY` cursor answer goes out
                // through the same writer channel.
                let mut slot = SinkSlot::new(sink, move |bytes| {
                    let _ = writer_tx.send(bytes);
                });
                // #732: before any thread can produce output.
                if let Some(observer) = observer {
                    observer.started(injected);
                    slot = slot.with_observer(observer);
                }
                Self::Raw(Arc::new(slot))
            }
            TerminalMode::Frames => {
                // Weak: the `Pty` entry must hold the only strong reference,
                // or `kill` would not close the master and the reader (and
                // so the scheduler thread and the hub) would never see EOF.
                let master = Arc::downgrade(&master);
                let io = HubIo {
                    reply: Box::new(move |bytes| {
                        let _ = writer_tx.send(bytes);
                    }),
                    resize_pty: Box::new(move |r, c| {
                        let master = Weak::upgrade(&master).ok_or("pty is gone")?;
                        resize_master(&master, r, c)
                    }),
                };
                let mut hub = FrameHub::new(rows, cols, io);
                if let Some(observer) = observer {
                    observer.started(injected);
                    hub = hub.with_observer(observer);
                }
                if let Some(sink) = sink {
                    hub.attach(sink, Some((rows, cols)));
                }
                Self::Frames(Arc::new(hub))
            }
        }
    }

    /// Test hook: a weak handle on the frames hub (`None` on the raw path).
    #[cfg(test)]
    pub fn hub_weak(&self) -> Option<Weak<FrameHub>> {
        match self {
            Self::Raw(_) => None,
            Self::Frames(hub) => Some(Arc::downgrade(hub)),
        }
    }

    pub fn exited(&self) -> Option<ExitRecord> {
        match self {
            Self::Raw(slot) => slot.exited(),
            Self::Frames(hub) => hub.exited(),
        }
    }

    pub fn exit(&self, code: Option<u32>) {
        match self {
            Self::Raw(slot) => slot.exit(code),
            Self::Frames(hub) => hub.exit(code),
        }
    }

    /// Install a sink (raw: replacing the current one; frames: one more
    /// viewer, `size` its widget's size).
    pub fn attach(
        &self,
        sink: Box<dyn StreamSink<PtyEvent>>,
        size: Option<(u16, u16)>,
    ) -> (AttachOutcome, u64) {
        match self {
            Self::Raw(slot) => slot.attach_tracked(sink),
            Self::Frames(hub) => hub.attach(sink, size),
        }
    }

    pub fn detach_if(&self, attach_id: u64) -> bool {
        match self {
            Self::Raw(slot) => slot.detach_if(attach_id),
            Self::Frames(hub) => hub.detach_if(attach_id),
        }
    }

    pub fn detach(&self) -> bool {
        match self {
            Self::Raw(slot) => slot.detach(),
            Self::Frames(hub) => hub.detach_all(),
        }
    }

    pub fn attach_id(&self) -> Option<u64> {
        match self {
            Self::Raw(slot) => slot.attach_id(),
            Self::Frames(hub) => hub.latest_viewer(),
        }
    }

    /// Ownership-making input from a viewer (frames only).
    pub fn active(&self, viewer: u64) {
        if let Self::Frames(hub) = self {
            hub.active(viewer);
        }
    }

    /// A viewer's size report. Raw has no viewers: the caller resizes the PTY.
    pub fn resize(&self, viewer: Option<u64>, rows: u16, cols: u16) -> Option<bool> {
        match self {
            Self::Raw(_) => None,
            Self::Frames(hub) => Some(hub.resize(viewer, rows, cols)),
        }
    }

    pub fn request_redraw(&self, viewer: u64) -> bool {
        match self {
            Self::Raw(_) => false,
            Self::Frames(hub) => hub.request_redraw(viewer),
        }
    }
}

/// The reader thread (#171b): moves bytes off the PTY as fast as the OS
/// delivers them and never calls a sink itself.
pub(super) fn spawn_reader(
    id: String,
    mut reader: Box<dyn Read + Send>,
    raw_tx: mpsc::Sender<Vec<u8>>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if raw_tx.send(buf[..n].to_vec()).is_err() {
                        // Coalescer already gone; nothing more to do.
                        break;
                    }
                }
            }
        }
        // Dropping `raw_tx` here is the EOF signal the coalescer
        // acts on to flush and exit.
        tracing::info!(id = %id, "pty reader exit");
    });
}

/// The thread that turns raw chunks into events: the coalescer on the raw
/// path, the frame scheduler on the frames path. `done_tx` is dropped when it
/// has flushed everything and is about to exit, which the waiter hears.
pub(super) fn spawn_pump(
    id: String,
    raw_rx: mpsc::Receiver<Vec<u8>>,
    out: &PtyOutput,
    done_tx: mpsc::Sender<()>,
) {
    match out {
        PtyOutput::Raw(slot) => {
            let slot = Arc::clone(slot);
            thread::spawn(move || {
                // Held only so it is dropped when this thread ends, after
                // `run_coalescer`'s final flush — that drop is the signal.
                let _done_tx = done_tx;
                // Issue #23: keep a small carry buffer so a multi-byte
                // UTF-8 sequence split across two coalesced chunks (em-
                // dash, box-drawing chars, emoji — all 3 or 4 bytes)
                // doesn't get replaced with U+FFFD on each side of the
                // boundary. Append each chunk into `carry`, slice off the
                // longest valid UTF-8 prefix, emit it, and carry the
                // trailing partial bytes into the next chunk.
                //
                // For genuinely malformed bytes (not just incomplete), we
                // still drop them — same as before — but only for bytes
                // we're *certain* are invalid (`Utf8Error::error_len()` is
                // `Some`).
                let mut carry: Vec<u8> = Vec::with_capacity(8192);
                run_coalescer(&raw_rx, COALESCE_WINDOW, COALESCE_BYTE_CAP, |chunk| {
                    carry.extend_from_slice(&chunk);
                    match std::str::from_utf8(&carry) {
                        Ok(s) => {
                            slot.data(s.to_owned());
                            carry.clear();
                        }
                        Err(e) => {
                            let valid_up_to = e.valid_up_to();
                            if valid_up_to > 0 {
                                // Safe by construction: bytes [..valid_up_to]
                                // are valid UTF-8.
                                let valid =
                                    std::str::from_utf8(&carry[..valid_up_to]).unwrap_or("");
                                slot.data(valid.to_owned());
                            }
                            if let Some(invalid_len) = e.error_len() {
                                // Definitely-malformed bytes — drop them
                                // (same as the old lossy path).
                                let drain_to = valid_up_to + invalid_len;
                                carry.drain(..drain_to);
                            } else {
                                // Trailing bytes are an incomplete sequence —
                                // wait for the next chunk.
                                carry.drain(..valid_up_to);
                            }
                        }
                    }
                });
                tracing::info!(id = %id, "pty coalescer exit");
            });
        }
        PtyOutput::Frames(hub) => {
            let hub = Arc::clone(hub);
            thread::spawn(move || {
                // Dropped after the scheduler's final cut, like the coalescer's.
                let _done_tx = done_tx;
                run_frame_scheduler(&raw_rx, COALESCE_WINDOW, &mut HubDriver::new(hub));
                tracing::info!(id = %id, "pty frame scheduler exit");
            });
        }
    }
}
