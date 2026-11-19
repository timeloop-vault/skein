//! When to cut a frame (design: "The frame" > "Framing and rate"): the
//! frame-path counterpart of `run_coalescer`, with the engine as the buffer.

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What the scheduler drives: the engine and its viewers, behind four calls
/// so the timing can be pinned with a recording fake and a bare channel.
pub(in crate::pty) trait FrameDriver {
    /// Bytes the PTY emitted; called for every chunk as it arrives, never delayed.
    fn feed(&mut self, bytes: &[u8]);
    /// Cut one frame per viewer.
    fn cut(&mut self);
    /// When an open synchronized update must be ended, if one is open.
    fn sync_deadline(&self) -> Option<Instant>;
    /// End the open synchronized update now.
    fn flush_sync(&mut self);
}

/// Feed PTY chunks from `rx` to `driver` and say when to cut frames.
///
/// - **Feed at once:** every chunk goes to `feed` as it arrives. The engine
///   is the buffer, and its query replies must not wait for a frame.
/// - **Leading edge:** a chunk that arrives at least `window` after the last
///   cut is cut right after its feed, so typed-character echo isn't delayed.
/// - **Trailing cut:** otherwise the scheduler is dirty and cuts once `window`
///   has passed since the last cut, so there is at most one cut per `window`
///   and a burst's tail is never left uncut.
/// - **Sync:** while `sync_deadline` is `Some(t)` the loop wakes by `t` even
///   with no output, calls `flush_sync`, and cuts under the same window rule.
///   The deadline is re-read after every feed and flush. Under continuous
///   output an expired deadline is ended by the engine's own `feed` (it ends
///   an expired update before processing new bytes), so the loop only needs
///   to wake for the deadline when no output arrives.
/// - **Idle:** not dirty and no deadline means a blocking `recv`.
/// - **Disconnect:** an open sync is flushed, then one final cut if anything
///   was fed or flushed since the last cut. The caller relies on this to
///   keep the last frame ahead of the exit the waiter sends independently.
pub(in crate::pty) fn run_frame_scheduler<D: FrameDriver>(
    rx: &mpsc::Receiver<Vec<u8>>,
    window: Duration,
    driver: &mut D,
) {
    // Backdated like `run_coalescer`'s, so the first chunk takes the leading edge.
    let mut last_cut = Instant::now()
        .checked_sub(window)
        .unwrap_or_else(Instant::now);
    // Something was fed or flushed since the last cut.
    let mut dirty = false;
    let cut_if_due = |driver: &mut D, dirty: &mut bool, last_cut: &mut Instant| {
        let now = Instant::now();
        if *dirty && now.duration_since(*last_cut) >= window {
            driver.cut();
            *dirty = false;
            *last_cut = Instant::now();
        }
    };
    loop {
        let cut_at = dirty.then(|| last_cut + window);
        let wake = match (cut_at, driver.sync_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let received = match wake {
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
        };
        match received {
            Ok(chunk) => {
                driver.feed(&chunk);
                dirty = true;
                cut_if_due(driver, &mut dirty, &mut last_cut);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if driver.sync_deadline().is_some_and(|t| Instant::now() >= t) {
                    driver.flush_sync();
                    dirty = true;
                }
                cut_if_due(driver, &mut dirty, &mut last_cut);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if driver.sync_deadline().is_some() {
                    driver.flush_sync();
                    dirty = true;
                }
                if dirty {
                    driver.cut();
                }
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
