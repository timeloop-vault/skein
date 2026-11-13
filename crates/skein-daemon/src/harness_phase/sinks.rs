//! `StreamSink` adapters that feed the phase service from the adapter event
//! streams (#732).
//!
//! The Claude, opencode and Hermes-work adapters each deliver into one
//! `StreamSink`. The webview's sink keeps working while the daemon's machine hears the
//! same events, so the runner hands the adapter a [`Tee`] of the client sink and a phase sink.

use std::sync::Arc;

use super::service::PhaseService;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_work_hermes::HermesWorkEvent;
use crate::stream_sink::StreamSink;

macro_rules! phase_sink {
    ($(#[$doc:meta])* $name:ident, $event:ty, $method:ident) => {
        $(#[$doc])*
        pub struct $name {
            pub service: Arc<PhaseService>,
            pub harness_id: String,
        }

        impl StreamSink<$event> for $name {
            fn send(&self, item: $event) -> Result<(), String> {
                self.service.$method(&self.harness_id, &item);
                Ok(())
            }
        }
    };
}

phase_sink!(
    /// Claude tail events into [`PhaseService::claude_event`]. Never fails: the
    /// machine ignores what it cannot use.
    ClaudePhaseSink,
    ClaudeEvent,
    claude_event
);
phase_sink!(
    /// opencode SSE events into [`PhaseService::opencode_event`].
    OpencodePhaseSink,
    OpencodeEvent,
    opencode_event
);
phase_sink!(
    /// Hermes disk-work events into [`PhaseService::hermes_work`].
    HermesWorkPhaseSink,
    HermesWorkEvent,
    hermes_work
);

/// Delivers each event to both sinks and answers with `A`'s result only.
///
/// `A` is the client sink, `B` the phase sink. The adapters read the result:
/// the Claude attach logs the first failure and carries on, the opencode
/// attach ignores it, and the Hermes work poller STOPS on an `Err`. So `B`
/// must never be able to turn a good client send into a failure (a phase sink
/// bug would otherwise end the client's stream), and `B` is sent first and its
/// result dropped, so a dead client sink cannot starve it either.
pub struct Tee<A, B> {
    pub a: A,
    pub b: B,
}

impl<A, B> Tee<A, B> {
    pub fn new(a: A, b: B) -> Self {
        Self { a, b }
    }
}

impl<T, A, B> StreamSink<T> for Tee<A, B>
where
    T: Clone + Send + 'static,
    A: StreamSink<T>,
    B: StreamSink<T>,
{
    fn send(&self, item: T) -> Result<(), String> {
        // The phase sink first: it is in-process and cheap, and a slow or
        // closed client channel must not delay or skip it.
        let _ = self.b.send(item.clone());
        self.a.send(item)
    }
}
