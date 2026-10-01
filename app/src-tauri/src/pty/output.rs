//! The PTY output and stdin plumbing: the coalescer thread and the
//! writer thread (see the module doc in `super`).

use std::io::Write;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long the coalescer waits for more output before deciding a burst
/// is over. Doubles as the "was it idle?" threshold for the leading-edge
/// send: a chunk arriving after at least this much quiet goes straight
/// out, so typed-character echo isn't delayed. One frame at 60 Hz is a
/// reasonable amateur's proxy for "still feels live" and comfortably
/// under the waiter's 250 ms exit-flush sleep.
pub(super) const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Force an early flush once buffered output crosses this many bytes, so
/// one `PtyEvent::Data` message stays bounded even mid-firehose.
pub(super) const COALESCE_BYTE_CAP: usize = 64 * 1024;

/// Batch raw PTY bytes from `rx` into fewer, larger `sink` calls (#171b).
///
/// Without this, a `cargo build` (or anything else that writes faster
/// than a human types) turns into one Tauri IPC message — and one
/// xterm.js write — per up-to-8-KB `read()`, at whatever rate the pipe
/// delivers them.
///
/// - **Leading edge:** a chunk that arrives at least `window` after the
///   last flush is forwarded immediately, so interactive echo latency is
///   unchanged.
/// - **Trailing accumulation:** anything else is buffered and flushed at
///   most once per `window` (so a burst's tail is never stuck waiting
///   for more data that never comes), or sooner once the buffer passes
///   `byte_cap` (so a single message stays bounded).
/// - **Disconnect:** whatever is pending is flushed before returning.
///   The caller relies on this to keep output ordered ahead of the
///   `PtyEvent::Exit` the waiter thread sends independently.
///
/// A plain function over a `Receiver` and a `FnMut(Vec<u8>)` sink,
/// deliberately not touching `PtyEvent`/Tauri, so it can be pinned with
/// a bare `mpsc::channel` and no PTY at all — see the tests below.
pub(super) fn run_coalescer<F: FnMut(Vec<u8>)>(
    rx: &mpsc::Receiver<Vec<u8>>,
    window: Duration,
    byte_cap: usize,
    mut sink: F,
) {
    let mut pending: Vec<u8> = Vec::new();
    // Backdated so the very first chunk always sees "it's been idle
    // `window` or more" and takes the leading-edge path. Falls back to
    // "now" if `window` somehow outlives the monotonic clock's epoch —
    // then the first chunk just takes the trailing path instead.
    let mut last_flush = Instant::now()
        .checked_sub(window)
        .unwrap_or_else(Instant::now);
    loop {
        let received = if pending.is_empty() {
            // Nothing buffered to age out; block for the next chunk.
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(window.saturating_sub(last_flush.elapsed()))
        };
        match received {
            Ok(chunk) => {
                let now = Instant::now();
                if pending.is_empty() && now.duration_since(last_flush) >= window {
                    sink(chunk);
                    last_flush = now;
                } else {
                    pending.extend_from_slice(&chunk);
                    if pending.len() >= byte_cap {
                        sink(std::mem::take(&mut pending));
                        last_flush = Instant::now();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !pending.is_empty() {
                    sink(std::mem::take(&mut pending));
                    last_flush = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !pending.is_empty() {
                    sink(pending);
                }
                break;
            }
        }
    }
}

/// Spin up the dedicated writer thread for one PTY (#171d). It owns
/// `writer` and drains `Vec<u8>` messages sent on the returned channel,
/// doing the blocking `write_all` + `flush` itself.
///
/// This is what keeps `PtyManager::write` from ever touching the PTY's
/// own I/O: previously the write happened under the manager-wide lock,
/// so a child that stopped reading stdin (a full tty input queue) froze
/// that write forever — and with it `resize`/`kill`/`spawn` for every
/// *other* PTY, since they all share the same lock.
///
/// FIFO by construction: `mpsc::channel` preserves send order and the
/// loop drains one message at a time. The thread ends when the sender
/// is dropped (kill/remove) or a write errors — nothing joins it, so a
/// wedged child can never make `kill` block.
pub(super) fn spawn_pty_writer(
    id: String,
    mut writer: Box<dyn Write + Send>,
) -> mpsc::Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        while let Ok(data) = rx.recv() {
            if let Err(e) = writer.write_all(&data).and_then(|()| writer.flush()) {
                tracing::warn!(id = %id, error = %e, "pty writer error, stopping");
                break;
            }
        }
        tracing::info!(id = %id, "pty writer exit");
    });
    tx
}
