use std::time::Instant;

use super::output::{run_coalescer, spawn_pty_writer};
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::thread;
use std::time::Duration;

/// A `Write` that hangs on its very first call until released,
/// simulating a child that has stopped reading stdin — the scenario
/// that used to freeze `PtyManager::write` (#171d). Every write,
/// gated or not, is recorded in order so FIFO can be checked once
/// released.
struct GatedWriter {
    release_rx: mpsc::Receiver<()>,
    released: bool,
    sink: Arc<StdMutex<Vec<u8>>>,
}

impl Write for GatedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.released {
            let _ = self.release_rx.recv();
            self.released = true;
        }
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn write_returns_promptly_even_when_the_pty_stops_reading() {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let sink = Arc::new(StdMutex::new(Vec::new()));
    let writer = GatedWriter {
        release_rx,
        released: false,
        sink: Arc::clone(&sink),
    };
    let tx = spawn_pty_writer("test".to_owned(), Box::new(writer));

    // The writer thread is about to hang on its first write; sending
    // more must never block on that, or on each other.
    let started = Instant::now();
    tx.send(b"first".to_vec()).expect("send 1");
    tx.send(b"second".to_vec()).expect("send 2");
    tx.send(b"third".to_vec()).expect("send 3");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "send() blocked on the wedged writer"
    );

    // Confirm it really is still gated before releasing it.
    thread::sleep(Duration::from_millis(50));
    assert!(
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty(),
        "writer drained before being released"
    );

    release_tx.send(()).expect("release the gate");

    // Generous bound: we only care that all three eventually land,
    // in order, not on the exact timing.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if sink.lock().unwrap_or_else(PoisonError::into_inner).len() >= b"firstsecondthird".len() {
            break;
        }
        assert!(Instant::now() < deadline, "writer never drained the queue");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *sink.lock().unwrap_or_else(PoisonError::into_inner),
        b"firstsecondthird".to_vec()
    );
}

fn collect(rx: &mpsc::Receiver<Vec<u8>>, window: Duration, byte_cap: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    run_coalescer(rx, window, byte_cap, |chunk| out.push(chunk));
    out
}

#[test]
fn a_single_chunk_after_idle_passes_straight_through() {
    let (tx, rx) = mpsc::channel();
    tx.send(b"hello".to_vec()).expect("send");
    drop(tx);
    let out = collect(&rx, Duration::from_millis(50), 64 * 1024);
    assert_eq!(out, vec![b"hello".to_vec()]);
}

#[test]
fn a_burst_is_merged_into_fewer_messages_than_chunks() {
    let (tx, rx) = mpsc::channel();
    let chunks: Vec<Vec<u8>> = (0..20).map(|i| vec![i; 10]).collect();
    for c in &chunks {
        tx.send(c.clone()).expect("send");
    }
    drop(tx);

    let out = collect(&rx, Duration::from_millis(200), 1024 * 1024);

    let expected: Vec<u8> = chunks.concat();
    let got: Vec<u8> = out.concat();
    assert_eq!(got, expected, "byte order must be preserved");
    assert!(
        out.len() < chunks.len(),
        "expected coalescing to reduce {} chunks to fewer messages, got {}",
        chunks.len(),
        out.len()
    );
}

#[test]
fn the_byte_cap_forces_an_early_flush() {
    let (tx, rx) = mpsc::channel();
    // Long enough that the timer never fires in this test — only
    // the byte cap should force a flush before disconnect.
    let window = Duration::from_secs(10);
    let lead = b"lead".to_vec();
    let a = vec![b'x'; 5];
    let b = vec![b'y'; 5];
    let tail = b"tail".to_vec();
    tx.send(lead.clone()).expect("send");
    tx.send(a.clone()).expect("send");
    tx.send(b.clone()).expect("send");
    tx.send(tail.clone()).expect("send");
    drop(tx);

    let out = collect(&rx, window, 8);

    assert!(
        out.len() >= 3,
        "expected the 8-byte cap to force a flush before disconnect, got {out:?}"
    );
    let expected: Vec<u8> = [lead, a, b, tail].concat();
    let got: Vec<u8> = out.concat();
    assert_eq!(got, expected, "byte order must be preserved");
}

#[test]
fn pending_bytes_are_flushed_on_disconnect_even_below_the_cap() {
    let (tx, rx) = mpsc::channel();
    tx.send(b"a".to_vec()).expect("send"); // leading edge, own message
    tx.send(b"b".to_vec()).expect("send"); // far below any cap
    drop(tx);

    let out = collect(&rx, Duration::from_secs(10), 64 * 1024);
    assert_eq!(out, vec![b"a".to_vec(), b"b".to_vec()]);
}

// #171: the waiter's exit-ordering fix is one `recv_timeout` call on
// the `done_tx`/`done_rx` pair the coalescer thread is given — cheap
// enough to pin directly, with no PTY or child process involved.

#[test]
fn a_dropped_done_sender_lets_the_waiter_return_before_the_bound() {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    drop(done_tx); // the coalescer's "I'm drained and exiting" signal
    let started = Instant::now();
    let result = done_rx.recv_timeout(Duration::from_millis(250));
    assert!(matches!(result, Err(mpsc::RecvTimeoutError::Disconnected)));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "a disconnected sender must not wait out the bound, took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_coalescer_that_never_finishes_bounds_the_wait_at_the_timeout() {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    // Still alive for the whole wait — the "coalescer still
    // draining" case, which must degrade to the old best-effort
    // behaviour rather than block forever.
    let result = done_rx.recv_timeout(Duration::from_millis(50));
    assert!(matches!(result, Err(mpsc::RecvTimeoutError::Timeout)));
    drop(done_tx);
}
