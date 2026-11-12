//! A tap on one PTY's output and exit (#732).
//!
//! The view sink (the [`SinkSlot`](super::sink_slot::SinkSlot)'s `StreamSink`)
//! comes and goes; the daemon's phase machine must hear every chunk and the
//! exit whether or not a view is attached. An observer is bound to one spawn
//! and called from the PTY's own threads, never while the slot's lock is
//! held, so it may take its own locks but must stay quick.

/// Hears one PTY process from start to exit.
pub trait PtyObserver: Send + Sync {
    /// The child exists and its threads are about to start, so this precedes
    /// every [`output`](Self::output). `injected` is whether #215's config
    /// injection was applied. Called on the spawning thread.
    fn started(&self, injected: bool);

    /// One decoded output chunk, as the view sink would get it. Called on the
    /// coalescer thread.
    fn output(&self, chunk: &str);

    /// The child exited. Called once, on the waiter thread. Not repeated for
    /// a view that attaches afterwards.
    fn exited(&self, code: Option<u32>);
}
