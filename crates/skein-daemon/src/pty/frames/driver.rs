//! The glue between the frame scheduler and one [`FrameHub`].

use std::sync::Arc;
use std::time::Instant;

use super::hub::FrameHub;
use super::scheduler::FrameDriver;

/// Drives a hub from the scheduler: feeds it, tells its observer the decoded
/// text of what was fed, and cuts frames.
pub(in crate::pty) struct HubDriver {
    hub: Arc<FrameHub>,
    /// Bytes fed since the last cut, not yet decoded for the observer.
    pending: Vec<u8>,
}

impl HubDriver {
    pub fn new(hub: Arc<FrameHub>) -> Self {
        Self {
            hub,
            pending: Vec::new(),
        }
    }
}

impl FrameDriver for HubDriver {
    fn feed(&mut self, bytes: &[u8]) {
        self.hub.feed(bytes);
        self.pending.extend_from_slice(bytes);
    }

    fn cut(&mut self) {
        let text = take_valid_utf8(&mut self.pending);
        if !text.is_empty() {
            self.hub.observe_output(&text);
        }
        self.hub.cut();
    }

    fn sync_deadline(&self) -> Option<Instant> {
        self.hub.sync_deadline()
    }

    fn flush_sync(&mut self) {
        self.hub.flush_sync();
    }
}

/// Take the valid UTF-8 text off the front of `pending`. Bytes that are
/// certainly invalid are dropped; an incomplete sequence at the end stays for
/// the next round (the rule of the coalescer's closure in `pty/mod.rs`).
pub(in crate::pty) fn take_valid_utf8(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or(""));
                if let Some(bad) = e.error_len() {
                    pending.drain(..valid + bad);
                } else {
                    pending.drain(..valid);
                    return out;
                }
            }
        }
    }
}
