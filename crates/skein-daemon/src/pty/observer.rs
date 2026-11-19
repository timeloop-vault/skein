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

    /// Does this observer read the rendered screen (#809)? Asked at each
    /// frame cut in frames mode; `false` (the default) costs the hub nothing.
    fn wants_screen(&self) -> bool {
        false
    }

    /// The rows of the engine's screen after a cut, taken outside any sync
    /// update and with the hub's lock released. Only called when
    /// [`wants_screen`](Self::wants_screen) is true. Called on the frame
    /// scheduler's thread.
    fn screen(&self, _rows: &[String]) {}

    /// The notice to append to the engine's screen once the child's output
    /// has ended (#809, T-5): the exit banner. Frames mode only, asked once,
    /// on the waiter thread with no lock held, before
    /// [`exited`](Self::exited); `None` (the default) appends nothing. The
    /// hub knows no wording: the observer's owner supplies it.
    fn exit_notice(&self, _code: Option<u32>) -> Option<Vec<u8>> {
        None
    }
}
