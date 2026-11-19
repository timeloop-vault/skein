//! Shared fixtures for the frame hub tests: a recording sink, a sink that
//! can be made to fail, a fake PTY (replies, resizes) and a recording observer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use super::{FrameHub, HubIo};
use crate::pty::PtyEvent;
use crate::pty::observer::PtyObserver;
use crate::stream_sink::StreamSink;

pub const FULL_REDRAW: &str = "\x1b[?2026h";
pub const ERASE_DISPLAY: &str = "\x1b[2J";

/// Records every event it is sent; can be switched to fail from then on.
#[derive(Clone, Default)]
pub struct Rec {
    events: Arc<Mutex<Vec<String>>>,
    fail: Arc<AtomicBool>,
}

impl Rec {
    /// Every event as `grid:RxC`, `data:<text>` or `exit:<code>`.
    pub fn events(&self) -> Vec<String> {
        self.events.lock().clone()
    }

    /// Just the event kinds, in order: `grid`, `data`, `exit`.
    pub fn kinds(&self) -> Vec<&'static str> {
        self.events()
            .iter()
            .map(|e| match e.split(':').next() {
                Some("grid") => "grid",
                Some("data") => "data",
                _ => "exit",
            })
            .collect()
    }

    /// The text of every `Data` event.
    pub fn datas(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter_map(|e| e.strip_prefix("data:").map(str::to_owned))
            .collect()
    }

    /// The `Grid` events as `RxC`.
    pub fn grids(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter_map(|e| e.strip_prefix("grid:").map(str::to_owned))
            .collect()
    }

    /// Make every later send fail.
    pub fn fail_from_now(&self) {
        self.fail.store(true, Ordering::SeqCst);
    }
}

impl StreamSink<PtyEvent> for Rec {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("gone".into());
        }
        self.events.lock().push(match item {
            PtyEvent::Data { chunk } => format!("data:{chunk}"),
            PtyEvent::Exit { code } => format!("exit:{code:?}"),
            PtyEvent::Grid { rows, cols } => format!("grid:{rows}x{cols}"),
        });
        Ok(())
    }
}

/// A sink whose receiver is gone from the start.
pub struct Dead;

impl StreamSink<PtyEvent> for Dead {
    fn send(&self, _: PtyEvent) -> Result<(), String> {
        Err("gone".into())
    }
}

/// A recording observer.
#[derive(Default)]
pub struct Obs {
    pub outputs: Mutex<Vec<String>>,
    pub exits: Mutex<Vec<Option<u32>>>,
}

impl PtyObserver for Obs {
    fn started(&self, _: bool) {}
    fn output(&self, chunk: &str) {
        self.outputs.lock().push(chunk.to_owned());
    }
    fn exited(&self, code: Option<u32>) {
        self.exits.lock().push(code);
    }
}

/// A hub on a fake PTY.
pub struct Fx {
    pub hub: FrameHub,
    pub replies: Arc<Mutex<Vec<Vec<u8>>>>,
    pub resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    pub refuse: Arc<AtomicBool>,
}

impl Fx {
    /// A 24x80 hub.
    pub fn new() -> Self {
        Self::sized(24, 80)
    }

    pub fn sized(rows: u16, cols: u16) -> Self {
        Self::build(rows, cols, None)
    }

    pub fn observed(obs: &Arc<Obs>) -> Self {
        Self::build(24, 80, Some(Arc::clone(obs) as Arc<dyn PtyObserver>))
    }

    fn build(rows: u16, cols: u16, observer: Option<Arc<dyn PtyObserver>>) -> Self {
        let replies = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let refuse = Arc::new(AtomicBool::new(false));
        let (r, z, f) = (
            Arc::clone(&replies),
            Arc::clone(&resizes),
            Arc::clone(&refuse),
        );
        let io = HubIo {
            reply: Box::new(move |b| r.lock().push(b)),
            resize_pty: Box::new(move |rows, cols| {
                z.lock().push((rows, cols));
                if f.load(Ordering::SeqCst) {
                    Err("refused".into())
                } else {
                    Ok(())
                }
            }),
        };
        let mut hub = FrameHub::new(rows, cols, io);
        if let Some(o) = observer {
            hub = hub.with_observer(o);
        }
        Self {
            hub,
            replies,
            resizes,
            refuse,
        }
    }

    /// Attach a fresh recording viewer; its id and recorder.
    pub fn viewer(&self, size: Option<(u16, u16)>) -> (u64, Rec) {
        let rec = Rec::default();
        let (_, id) = self.hub.attach(Box::new(rec.clone()), size);
        (id, rec)
    }

    pub fn resize_calls(&self) -> Vec<(u16, u16)> {
        self.resizes.lock().clone()
    }
}
