//! Where a per-call stream of items is delivered. (#578, #575)
//!
//! The per-call counterpart of [`EventSink`](crate::event_sink::EventSink)
//! (#577): `EventSink` carries broadcast events, a `StreamSink` carries the
//! stream one command opens (PTY output, harness events, watcher ticks).
//! The Tauri implementation lives in `tauri_stream_sink.rs` and a network
//! transport implements the same trait later. This module deliberately
//! imports nothing from `tauri`. Each call site decides what a failed send
//! means: some stop their producer, some ignore it.

/// One per-call stream from server to client (PTY output, harness events,
/// watcher ticks).
pub trait StreamSink<T>: Send + Sync + 'static {
    /// # Errors
    /// The receiver is gone, or the item could not be delivered.
    fn send(&self, item: T) -> Result<(), String>;
}
