//! The body of the `claude_events_attach` command, free of any shell
//! types (#578): the command hands over a [`StreamSink`] and this attaches
//! the JSONL tail to it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{ClaudeEvent, ClaudeEventsManager};
use crate::stream_sink::StreamSink;

/// Attach the Claude JSONL tail for `harness_id` and stream its events
/// into `sink`. Blocking: the first attach reads the whole existing
/// transcript (#80), so callers run it off the async executor.
///
/// A failed send does not stop delivery: it is logged once per attach,
/// because a dead channel fails on every later event and that is noise,
/// not new information.
pub(crate) fn claude_events_attach_impl(
    manager: &ClaudeEventsManager,
    harness_id: String,
    room_id: String,
    session_id: &str,
    cwd: &str,
    fresh_process: bool,
    sink: impl StreamSink<ClaudeEvent>,
) -> Result<(), String> {
    // #362: the frontend only `console.warn`s an Err from this command
    // (see harnessEvents.ts) and otherwise falls back silently to the
    // L2a idle heuristic — so an attach that fails, or never happens,
    // has been invisible on the Rust side. Every line below carries
    // `harness_id` so one harness's attach/detach/tick history can be
    // grepped out of a log with many rooms interleaved.
    tracing::info!(
        harness_id,
        room_id,
        session_id,
        fresh_process,
        "claude_events_attach: called"
    );
    let started = std::time::Instant::now();
    let log_harness_id = harness_id.clone();
    let send_failed = Arc::new(AtomicBool::new(false));
    let send_failed_cb = Arc::clone(&send_failed);
    let send_failed_harness_id = harness_id.clone();
    let result = manager.attach(
        harness_id,
        room_id,
        session_id,
        cwd,
        move |event| {
            if sink.send(event).is_err() && !send_failed_cb.swap(true, Ordering::Relaxed) {
                // Only logged once per attach — a dead webview channel
                // (window closed, harness torn down) fails on every
                // subsequent event otherwise, and that's noise, not
                // new information.
                tracing::warn!(
                    harness_id = %send_failed_harness_id,
                    "claude_events_attach: channel send failed; webview likely gone"
                );
            }
        },
        fresh_process,
    );
    match result {
        Ok(info) => {
            tracing::info!(
                harness_id = %log_harness_id,
                path = %info.path.display(),
                already_existed = info.already_existed,
                subagents_armed = info.subagents_armed,
                duration_ms = started.elapsed().as_millis(),
                "claude_events_attach: attached"
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                harness_id = %log_harness_id,
                error = %e,
                "claude_events_attach: failed"
            );
            Err(e.to_string())
        }
    }
}
