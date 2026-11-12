//! The replaceable output sink of one PTY (#731).
//!
//! The reader/coalescer and waiter threads never hold a sink themselves:
//! every [`PtyEvent`] goes through the PTY's [`SinkSlot`], so a terminal
//! view can detach and a later one attach without the child, the threads
//! or the manager entry noticing.
//!
//! With no sink attached `Data` is dropped (buffering is #584) and `Exit`
//! is only recorded, so a view that attaches afterwards is told at once.
//!
//! `ConPTY` (#484) withholds all child output until the host answers its
//! `ESC[6n` cursor query. While a view is attached its xterm answers; while
//! none is, the slot answers through the injected `reply` instead, so a
//! detached PTY is not wedged at spawn. It never answers while a sink is
//! attached, or the child would see a stray second report as input.

use std::sync::Arc;

use parking_lot::Mutex;

use super::PtyEvent;
use super::observer::PtyObserver;
use crate::stream_sink::StreamSink;

/// The cursor-position query `ConPTY` sends at start-up.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// A report for row 1, column 1.
const CURSOR_REPORT: &[u8] = b"\x1b[1;1R";

/// What `attach` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The child is still running; events will flow to the new sink.
    Live,
    /// The child had already exited; the new sink was sent `Exit` once.
    Exited { code: Option<u32> },
}

/// A recorded child exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitRecord {
    pub code: Option<u32>,
}

struct State {
    sink: Option<Box<dyn StreamSink<PtyEvent>>>,
    /// The id of the current attachment (0 = none ever).
    attach_id: u64,
    /// `Some(code)` once the waiter has fired.
    exited: Option<ExitRecord>,
    /// Trailing bytes of the last unattended chunk that could be the start
    /// of a cursor query split across two chunks.
    query_carry: Vec<u8>,
}

/// A PTY's output sink, swappable at any time.
pub struct SinkSlot {
    state: Mutex<State>,
    reply: Box<dyn Fn(Vec<u8>) + Send + Sync>,
    /// Hears every chunk and the exit, whatever the sink (#732).
    observer: Option<Arc<dyn PtyObserver>>,
}

impl SinkSlot {
    /// `reply` writes bytes to the PTY's stdin; it is only called for the
    /// unattended cursor-query answer.
    pub fn new(
        sink: Option<Box<dyn StreamSink<PtyEvent>>>,
        reply: impl Fn(Vec<u8>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            state: Mutex::new(State {
                attach_id: u64::from(sink.is_some()),
                sink,
                exited: None,
                query_carry: Vec::new(),
            }),
            reply: Box::new(reply),
            observer: None,
        }
    }

    /// Tap this slot (#732). The observer is called from `data` and `exit`
    /// before the slot's lock is taken, and never from `attach`, so a view
    /// attaching to an exited process does not repeat the exit.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn PtyObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Output from the PTY. A failed send stays non-fatal: no kill, no
    /// auto-detach. A sink whose channel is dead (a reloaded webview that
    /// never detached) cannot answer a cursor query, so for that purpose it
    /// counts as absent and the slot answers.
    pub fn data(&self, chunk: String) {
        if let Some(observer) = &self.observer {
            observer.output(&chunk);
        }
        let mut st = self.state.lock();
        let queries = if let Some(sink) = st.sink.as_ref() {
            // Keep a copy only when a query could be in it: the chunk has
            // an ESC, or a split query is pending from an earlier failed
            // send (the hot path for a healthy sink stays allocation-free).
            let keep_copy = !st.query_carry.is_empty() || chunk.contains('\x1b');
            let bytes = keep_copy.then(|| chunk.clone().into_bytes());
            let bytes = bytes.unwrap_or_default();
            if sink.send(PtyEvent::Data { chunk }).is_ok() {
                // The view saw this chunk, so any half-seen query is its.
                st.query_carry.clear();
                return;
            }
            count_queries(&mut st.query_carry, &bytes)
        } else {
            count_queries(&mut st.query_carry, chunk.as_bytes())
        };
        drop(st);
        for _ in 0..queries {
            (self.reply)(CURSOR_REPORT.to_vec());
        }
    }

    /// The child exited: record it, and tell the sink if there is one.
    pub fn exit(&self, code: Option<u32>) {
        let first = {
            let mut st = self.state.lock();
            let first = st.exited.is_none();
            st.exited = Some(ExitRecord { code });
            if let Some(sink) = st.sink.as_ref() {
                let _ = sink.send(PtyEvent::Exit { code });
            }
            first
        };
        // Outside the lock, and only for the first exit.
        if first && let Some(observer) = &self.observer {
            observer.exited(code);
        }
    }

    /// Install `sink`, replacing any current one.
    pub fn attach(&self, sink: Box<dyn StreamSink<PtyEvent>>) -> AttachOutcome {
        self.attach_tracked(sink).0
    }

    /// [`attach`](Self::attach), also returning the new attachment's id:
    /// monotonically increasing per slot, the first sink (given to `new`)
    /// being 1.
    pub fn attach_tracked(&self, sink: Box<dyn StreamSink<PtyEvent>>) -> (AttachOutcome, u64) {
        let mut st = self.state.lock();
        st.query_carry.clear();
        let outcome = match st.exited {
            Some(ExitRecord { code }) => {
                let _ = sink.send(PtyEvent::Exit { code });
                AttachOutcome::Exited { code }
            }
            None => AttachOutcome::Live,
        };
        st.sink = Some(sink);
        st.attach_id += 1;
        (outcome, st.attach_id)
    }

    /// The id of the current attachment, `None` when no sink is attached.
    pub fn attach_id(&self) -> Option<u64> {
        let st = self.state.lock();
        st.sink.as_ref().map(|_| st.attach_id)
    }

    /// Clear the sink only if `attach_id` is still the current attachment.
    /// Returns whether it was cleared.
    pub fn detach_if(&self, attach_id: u64) -> bool {
        let mut st = self.state.lock();
        if st.sink.is_some() && st.attach_id == attach_id {
            st.sink = None;
            return true;
        }
        false
    }

    /// Clear the sink. Returns whether one was attached.
    pub fn detach(&self) -> bool {
        self.state.lock().sink.take().is_some()
    }

    /// `Some(code)` once the child has exited.
    pub fn exited(&self) -> Option<ExitRecord> {
        self.state.lock().exited
    }
}

/// Count complete cursor queries in `carry + data`, then leave in `carry`
/// the longest suffix that is a proper prefix of the query.
fn count_queries(carry: &mut Vec<u8>, data: &[u8]) -> usize {
    carry.extend_from_slice(data);
    let n = carry
        .windows(CURSOR_QUERY.len())
        .filter(|w| *w == CURSOR_QUERY)
        .count();
    let keep = (1..CURSOR_QUERY.len())
        .rev()
        .find(|&k| carry.len() >= k && carry.ends_with(&CURSOR_QUERY[..k]))
        .unwrap_or(0);
    let start = carry.len() - keep;
    carry.drain(..start);
    n
}

#[cfg(test)]
mod tests;
