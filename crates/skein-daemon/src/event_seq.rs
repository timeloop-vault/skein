//! One monotonic seq for every broadcast event, plus a bounded replay log
//! (#757, docs/skeind-p2b-design.md "The event stream"). Not wired yet.
//!
//! [`SeqEventSink`] wraps the real [`EventSink`]. Each event gets the next
//! seq, written as the top-level `seq` key of its JSON object, and is kept
//! in memory so `subscribe(since)` can answer what a client missed, or say
//! `gap` when it cannot.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use skein_proto::{LoggedEvent, Replay};

use crate::event_sink::{EventSink, SharedEventSink};

/// Most events the replay log keeps.
pub const LOG_MAX_EVENTS: usize = 4096;
/// Most payload bytes the replay log keeps.
pub const LOG_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Events that get a seq but are not kept: they are backed by sqlite rows
/// and re-read from there.
pub const UNLOGGED_EVENTS: &[&str] = &["harness-action"];

pub use crate::event_sink::Stamped;

struct State {
    seq: u64,
    log: VecDeque<LoggedEvent>,
    bytes: usize,
    /// Highest seq dropped from the log; 0 when nothing was evicted.
    evicted: u64,
}

/// Stamps every event with the next seq and logs it.
///
/// Seq order is the log's order. Live delivery order is per producer: a
/// producer that stamps under its own lock and delivers later (as the phase
/// stream will) can be delivered after a higher seq of another kind, so a
/// client orders by `seq`, not by arrival.
pub struct SeqEventSink {
    inner: SharedEventSink,
    instance_id: String,
    max_events: usize,
    max_bytes: usize,
    state: Mutex<State>,
}

impl SeqEventSink {
    #[must_use]
    pub fn new(inner: SharedEventSink) -> Arc<Self> {
        Self::with_caps(inner, LOG_MAX_EVENTS, LOG_MAX_BYTES)
    }

    pub(crate) fn with_caps(
        inner: SharedEventSink,
        max_events: usize,
        max_bytes: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            instance_id: uuid::Uuid::new_v4().to_string(),
            max_events,
            max_bytes,
            state: Mutex::new(State {
                seq: 0,
                log: VecDeque::new(),
                bytes: 0,
                evicted: 0,
            }),
        })
    }

    /// Identifies this daemon run; a client holding another id gets `gap`.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// The last seq handed out; 0 before any.
    #[must_use]
    pub fn current(&self) -> u64 {
        self.state.lock().seq
    }

    /// Take the next seq, write it into the payload and log the event, all
    /// under one lock. A payload that is not a JSON object passes through
    /// unchanged: it still consumes a seq but is not logged.
    pub fn stamp(&self, event: &str, payload: String) -> Stamped {
        let mut st = self.state.lock();
        st.seq += 1;
        let seq = st.seq;
        let object = match serde_json::from_str::<Value>(&payload) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        };
        let Some(mut map) = object else {
            tracing::warn!(
                event,
                seq,
                "event_seq: payload is not a JSON object, not stamped"
            );
            return Stamped { seq, payload };
        };
        map.insert("seq".to_owned(), Value::from(seq));
        let Ok(stamped) = serde_json::to_string(&Value::Object(map)) else {
            return Stamped { seq, payload };
        };
        if !UNLOGGED_EVENTS.contains(&event) {
            st.bytes += stamped.len();
            st.log.push_back(LoggedEvent {
                seq,
                event: event.to_owned(),
                payload: stamped.clone(),
            });
            while st.log.len() > self.max_events || st.bytes > self.max_bytes {
                let Some(old) = st.log.pop_front() else { break };
                st.bytes -= old.payload.len();
                st.evicted = old.seq;
            }
        }
        Stamped {
            seq,
            payload: stamped,
        }
    }

    /// Hand a stamped event to the inner sink. Not under the log lock.
    ///
    /// # Errors
    /// Whatever the inner sink reports; the event stays in the log.
    pub fn deliver(&self, event: &str, stamped: Stamped) -> Result<(), String> {
        self.inner.emit_json(event, stamped.payload)
    }

    /// The events after `since`, or `gap` when `instance` is not this daemon,
    /// `since` is older than the log, or `since` is ahead of the daemon.
    #[must_use]
    pub fn subscribe(&self, instance: &str, since: u64) -> Replay {
        let st = self.state.lock();
        if instance != self.instance_id || since < st.evicted || since > st.seq {
            return Replay::Gap { seq: st.seq };
        }
        Replay::Events {
            events: st.log.iter().filter(|e| e.seq > since).cloned().collect(),
            seq: st.seq,
        }
    }
}

impl EventSink for SeqEventSink {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        let stamped = self.stamp(event, payload);
        self.deliver(event, stamped)
    }

    fn current_seq(&self) -> u64 {
        self.current()
    }

    fn stamp_json(&self, event: &str, payload: String) -> Stamped {
        self.stamp(event, payload)
    }

    fn deliver_json(&self, event: &str, stamped: Stamped) -> Result<(), String> {
        self.deliver(event, stamped)
    }
}

#[cfg(test)]
mod tests;
