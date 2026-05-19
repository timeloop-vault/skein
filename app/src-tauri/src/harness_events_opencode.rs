//! opencode event-stream adapter — epic #50 L2c-2.
//!
//! opencode's TUI embeds an HTTP server (each invocation gets its own
//! port — Skein pins it via `--port <N>`). `GET /event` returns a
//! Server-Sent Events stream of session activity. We subscribe, parse
//! `data: {json}\n\n` framing, and emit a small `OpencodeEvent` enum
//! the frontend translates into harness-activity phase transitions.
//!
//! The authoritative "opencode is awaiting user input" signal is the
//! `session.status` event with `properties.status.type === "idle"`.
//! `"busy"` means a turn is running. See `docs/epic-50-l2c-2-recon.md`
//! for the full event catalog observed in a real session.
//!
//! Reconnect: tied to manager lifetime. While the harness is alive,
//! we keep trying to (re)connect with exponential backoff capped at
//! 30 s. Detach (`LiveTerminal` cleanup → `opencode_events_detach`)
//! stops the loop. opencode's own crash → PTY exit → frontend tears
//! us down anyway, so we don't need a "max attempts" budget.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Reconnect backoff schedule (seconds), capped at the last value
/// for any further attempts. 1 → 2 → 4 → 8 → 16 → 30 → 30 … gets us
/// from "opencode hasn't bound yet" to "opencode is dead" with
/// minimal wasted polling.
const BACKOFF_SCHEDULE_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

/// HTTP request timeout. The SSE connect itself is fast on localhost;
/// the timeout exists so a wedged opencode (bound but not responding)
/// doesn't hang the reconnect loop forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Semantic events emitted to the frontend. Mirrors the shape of
/// `ClaudeEvent` so the translator in `harnessEvents.ts` can keep
/// the same policy structure.
///
/// We keep this slightly richer than the state machine needs today —
/// `SessionCreated` carries the sessionID so the frontend can take
/// over session-id capture from chapter 5's sqlite snapshot-poll.
/// L7 (cross-harness activity feed) will want `ToolUseStart` /
/// `MessageDelta` for the "h1b just used the Edit tool" line.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpencodeEvent {
    /// SSE connected. Emitted exactly once per attach (subsequent
    /// reconnects do not re-fire this — they're transparent to the
    /// policy layer).
    Connected,
    /// opencode created a session in-process. Frontend uses
    /// `session_id` to capture the auto-allocated id for resume
    /// (replaces the chapter 5 sqlite snapshot-poll path).
    SessionCreated { session_id: String },
    /// `session.status` with `status.type === "busy"`.
    SessionBusy,
    /// `session.status` with `status.type === "idle"`. The "Claude
    /// is done, awaiting user" signal.
    SessionIdle,
    /// Streaming model output. Aggregated by the frontend — many
    /// per turn, treated as "still running."
    MessageDelta,
    /// Tool call started. Currently unused for state policy; will
    /// power the L7 activity feed.
    ToolUseStart { name: String },
    /// Server disconnected after a successful connect. Frontend
    /// falls back to L2a until the next reconnect succeeds. Initial
    /// cold-connect failures don't fire this — that case is normal
    /// while opencode is still binding its port.
    SessionEnd,
}

/// One live adapter. Holds the cancellation signal (dropping fires
/// it via the `Notify` so the background task wakes up and exits)
/// and the task handle for completeness.
struct Adapter {
    cancel: Arc<Notify>,
    _handle: JoinHandle<()>,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // Tell the background loop to give up its current sleep /
        // pending reqwest call and exit. The Notify is shared with
        // the task; this is the standard tokio "cancel this task"
        // pattern without pulling in tokio_util's CancellationToken.
        self.cancel.notify_one();
    }
}

#[derive(Default)]
pub struct OpencodeEventsManager {
    inner: Mutex<HashMap<String, Adapter>>,
}

impl OpencodeEventsManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start watching opencode's SSE stream on `127.0.0.1:<port>`.
    /// `on_event` fires for every parsed event. Spawns a tokio task
    /// that owns the reqwest connection + reconnect loop; cancelled
    /// by `detach` (or `Drop`).
    ///
    /// Idempotent: re-attaching the same `harness_id` cancels the
    /// previous adapter first.
    pub fn attach<F>(&self, harness_id: String, port: u16, on_event: F)
    where
        F: Fn(OpencodeEvent) + Send + Sync + 'static,
    {
        // Cancel any prior attach for this id — caller is asking for
        // a fresh stream and we don't want two tasks racing.
        {
            let mut inner = self.inner.lock();
            inner.remove(&harness_id);
        }
        let cancel = Arc::new(Notify::new());
        let cancel_for_task = Arc::clone(&cancel);
        let on_event = Arc::new(on_event);
        let handle = tokio::spawn(async move {
            run_adapter(port, cancel_for_task, on_event).await;
        });
        self.inner.lock().insert(
            harness_id,
            Adapter {
                cancel,
                _handle: handle,
            },
        );
    }

    /// Stop the adapter for `harness_id`. Idempotent.
    pub fn detach(&self, harness_id: &str) {
        // Dropping the Adapter triggers Drop → notify the task.
        self.inner.lock().remove(harness_id);
    }
}

/// The reconnect / read loop. Lives in a tokio task. Loops until
/// `cancel` fires, with exponential backoff between attempts.
async fn run_adapter(
    port: u16,
    cancel: Arc<Notify>,
    on_event: Arc<dyn Fn(OpencodeEvent) + Send + Sync>,
) {
    let url = format!("http://127.0.0.1:{port}/event");
    let client = match reqwest::Client::builder().timeout(CONNECT_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(port, error = %e, "opencode_events: client build failed");
            on_event(OpencodeEvent::SessionEnd);
            return;
        }
    };

    // Shared with stream_events so it can flip the flag the moment
    // a 200 response opens — used to decide whether a later error
    // gets a `SessionEnd` signal (yes, real disconnect) or stays
    // quiet (no, this is still the initial cold-connect race while
    // opencode hasn't bound yet).
    let connected_once = Arc::new(AtomicBool::new(false));
    let mut attempt: usize = 0;
    loop {
        // Race the SSE attempt against cancellation. If `cancel` ever
        // fires (Drop of Adapter), exit immediately.
        let connected_for_call = Arc::clone(&connected_once);
        let connect = stream_events(
            &client,
            &url,
            &cancel,
            on_event.as_ref(),
            &connected_for_call,
        );
        tokio::select! {
            biased;
            () = cancel.notified() => {
                tracing::debug!(port, "opencode_events: cancelled");
                return;
            }
            result = connect => {
                match result {
                    Ok(()) => {
                        // Stream ended cleanly. Reset backoff so
                        // reconnect after an opencode restart is
                        // fast.
                        attempt = 0;
                    }
                    Err(e) => {
                        if connected_once.load(Ordering::Acquire) {
                            tracing::warn!(port, attempt, error = %e, "opencode_events: stream error after connect");
                            // Surface as SessionEnd so the frontend
                            // falls back to L2a until we reconnect.
                            on_event(OpencodeEvent::SessionEnd);
                        } else {
                            tracing::debug!(port, attempt, error = %e, "opencode_events: cold connect refused");
                        }
                    }
                }
            }
        }

        // Pick the backoff for this attempt and sleep, but wake
        // early on cancel. Index saturates at the last entry of
        // the schedule so we plateau at 30 s.
        let secs = BACKOFF_SCHEDULE_SECS
            .get(attempt)
            .copied()
            .unwrap_or_else(|| BACKOFF_SCHEDULE_SECS.last().copied().unwrap_or(30));
        let sleep = tokio::time::sleep(Duration::from_secs(secs));
        tokio::select! {
            biased;
            () = cancel.notified() => {
                tracing::debug!(port, "opencode_events: cancelled during backoff");
                return;
            }
            () = sleep => {}
        }
        attempt = attempt.saturating_add(1);
    }
}

/// Open the SSE stream and dispatch events until it closes or errors.
/// Returns `Ok(())` on clean stream end (server closed), `Err` on
/// any failure (connect refused, HTTP non-200, transport error).
/// Mutates the `connected_once` signal indirectly by emitting
/// `Connected` on first message.
async fn stream_events(
    client: &reqwest::Client,
    url: &str,
    cancel: &Notify,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    connected_once: &AtomicBool,
) -> Result<(), reqwest::Error> {
    use futures_util::StreamExt;

    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        // Coerce non-2xx into an error so the caller bumps backoff.
        return Err(response.error_for_status().unwrap_err());
    }
    // Flip the shared flag so the caller knows we made it past the
    // HTTP handshake. Any error from this point on is a real
    // disconnect, not a cold-connect race.
    connected_once.store(true, Ordering::Release);
    on_event(OpencodeEvent::Connected);

    let mut byte_stream = response.bytes_stream();
    // Carries incomplete bytes across chunk boundaries. SSE framing
    // is `data: {json}\n\n` but TCP can split a frame anywhere; we
    // buffer and split on the `\n\n` separator.
    let mut buf: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.notified() => return Ok(()),
            chunk = byte_stream.next() => {
                let Some(chunk) = chunk else {
                    // Stream ended cleanly.
                    return Ok(());
                };
                let chunk = chunk?;
                buf.extend_from_slice(&chunk);
                process_buffer(&mut buf, on_event);
            }
        }
    }
}

/// Walk `buf` for complete SSE frames (`...\n\n`), parse each, emit
/// matched events. Leaves any trailing partial frame in `buf` for
/// the next chunk to extend.
fn process_buffer(buf: &mut Vec<u8>, on_event: &(dyn Fn(OpencodeEvent) + Send + Sync)) {
    loop {
        let Some(sep) = find_double_newline(buf) else {
            return;
        };
        // Drain the frame (including the terminator) out of buf.
        let frame: Vec<u8> = buf.drain(..sep + 2).collect();
        // Trim the trailing `\n\n` before parsing.
        let Ok(frame_str) = std::str::from_utf8(&frame[..frame.len().saturating_sub(2)]) else {
            continue;
        };
        // Each frame may contain `event:`, `id:`, `data:` lines.
        // We only care about `data:`. Multiple `data:` lines join
        // with `\n` per SSE spec, but opencode emits single-line
        // data frames in practice — we still concatenate defensively.
        let mut payload = String::new();
        for line in frame_str.split('\n') {
            if let Some(rest) = line.strip_prefix("data:") {
                if !payload.is_empty() {
                    payload.push('\n');
                }
                payload.push_str(rest.trim_start());
            }
        }
        if payload.is_empty() {
            continue;
        }
        if let Some(event) = parse_event(&payload) {
            on_event(event);
        }
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Parse one SSE `data:` payload into an `OpencodeEvent`. Returns
/// `None` for events we don't surface (`server.heartbeat`,
/// `session.updated`, etc.) and for malformed JSON. Server schema
/// catalog lives in `docs/epic-50-l2c-2-recon.md`.
fn parse_event(payload: &str) -> Option<OpencodeEvent> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let ty = value.get("type")?.as_str()?;
    let props = value.get("properties");

    match ty {
        "session.status" => {
            let status_type = props
                .and_then(|p| p.get("status"))
                .and_then(|s| s.get("type"))
                .and_then(serde_json::Value::as_str)?;
            match status_type {
                "busy" => Some(OpencodeEvent::SessionBusy),
                "idle" => Some(OpencodeEvent::SessionIdle),
                // Other status sub-types observed in the wild ("text"
                // showed up briefly during dogfooding) — ignore until
                // we know they're meaningful for the state machine.
                _ => None,
            }
        }
        "session.created" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::SessionCreated { session_id })
        }
        "message.part.delta" => Some(OpencodeEvent::MessageDelta),
        "message.part.updated" => {
            // Look at the part type — tool calls surface here as a
            // sub-object with type=tool. Other part types
            // (step-start, step-finish, text) are noisy for our
            // purposes today.
            let part = props.and_then(|p| p.get("part"))?;
            let part_type = part.get("type").and_then(serde_json::Value::as_str)?;
            match part_type {
                "tool" => {
                    let name = part
                        .get("name")
                        .and_then(serde_json::Value::as_str)?
                        .to_owned();
                    // Some tool rows are mid-call updates; for v1 we
                    // emit one per row and let the frontend dedupe by
                    // staying in `running`. L7 will need finer
                    // resolution.
                    Some(OpencodeEvent::ToolUseStart { name })
                }
                _ => None,
            }
        }
        // Everything else — `session.idle` (redundant with the
        // `session.status` idle that fires alongside it),
        // `session.updated`, `session.diff`, `server.heartbeat`,
        // `mcp.tools.changed`, future event types — is silent at
        // the policy layer. See recon §3 for the catalog.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// `process_buffer` is the unit-testable core. We feed it canned
    /// SSE bytes and assert the emitted events. Reconnect / reqwest
    /// concerns are covered by integration dogfood.
    fn drain_events<F>(input: &[&str], frame_each: F) -> Vec<OpencodeEvent>
    where
        F: Fn(usize, &str) -> Vec<u8>,
    {
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        let mut buf = Vec::new();
        for (i, chunk) in input.iter().enumerate() {
            buf.extend_from_slice(&frame_each(i, chunk));
            process_buffer(&mut buf, &cb);
        }
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    fn frame(s: &str) -> Vec<u8> {
        // SSE frame: `data: {payload}\n\n`.
        let mut v = b"data: ".to_vec();
        v.extend_from_slice(s.as_bytes());
        v.extend_from_slice(b"\n\n");
        v
    }

    #[test]
    fn session_status_busy_and_idle_map_correctly() {
        let events = drain_events(
            &[
                r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"busy"}}}"#,
                r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(matches!(events.first(), Some(OpencodeEvent::SessionBusy)));
        assert!(matches!(events.get(1), Some(OpencodeEvent::SessionIdle)));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn session_created_captures_session_id() {
        let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_abc123","info":{"id":"ses_abc123"}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id }) if session_id == "ses_abc123"),
            "expected SessionCreated(ses_abc123), got {events:?}"
        );
    }

    #[test]
    fn session_idle_redundant_event_is_suppressed() {
        // opencode emits both `session.status` idle AND a separate
        // `session.idle` event. We surface only the former so the
        // policy layer doesn't see double transitions.
        let events = drain_events(
            &[r#"{"type":"session.idle","properties":{"sessionID":"s"}}"#],
            |_, p| frame(p),
        );
        assert!(
            events.is_empty(),
            "session.idle should be suppressed, got {events:?}"
        );
    }

    #[test]
    fn metadata_rows_do_not_emit() {
        let events = drain_events(
            &[
                r#"{"type":"server.heartbeat","properties":{}}"#,
                r#"{"type":"server.connected","properties":{}}"#,
                r#"{"type":"session.updated","properties":{"sessionID":"s"}}"#,
                r#"{"type":"session.diff","properties":{"sessionID":"s"}}"#,
                r#"{"type":"mcp.tools.changed","properties":{"server":"github"}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            events.is_empty(),
            "metadata rows should be silent, got {events:?}"
        );
    }

    #[test]
    fn tool_use_part_emits_tool_use_start() {
        let payload = r#"{"type":"message.part.updated","properties":{"sessionID":"s","part":{"type":"tool","name":"Edit","id":"t1"}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.first(), Some(OpencodeEvent::ToolUseStart { name }) if name == "Edit"),
            "expected ToolUseStart(Edit), got {events:?}"
        );
    }

    #[test]
    fn malformed_json_skipped_other_events_still_emit() {
        let events = drain_events(
            &[
                "this-is-not-json",
                r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            matches!(events.first(), Some(OpencodeEvent::SessionIdle)),
            "valid event after garbage should emit, got {events:?}"
        );
    }

    #[test]
    fn partial_chunk_split_across_writes() {
        // First write contains only the prefix; second write completes
        // the frame. process_buffer must not emit until the full frame
        // is present.
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(
            br#"data: {"type":"session.status","properties":{"sessionID":"s","status":{"type":"#,
        );
        process_buffer(&mut buf, &cb);
        assert!(rx.try_recv().is_err(), "partial frame must not emit");
        buf.extend_from_slice(br#""idle"}}}"#);
        buf.extend_from_slice(b"\n\n");
        process_buffer(&mut buf, &cb);
        let mut out: Vec<OpencodeEvent> = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        assert!(
            matches!(out.first(), Some(OpencodeEvent::SessionIdle)),
            "completed frame should emit, got {out:?}"
        );
    }

    #[test]
    fn multiple_frames_in_one_chunk_all_emit() {
        // TCP can deliver several SSE frames in one read. process_buffer
        // must loop until the buffer no longer contains a terminator.
        let mut buf = Vec::new();
        for payload in [
            r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"busy"}}}"#,
            r#"{"type":"message.part.delta","properties":{"sessionID":"s","delta":"a"}}"#,
            r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
        ] {
            buf.extend_from_slice(&frame(payload));
        }
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        process_buffer(&mut buf, &cb);
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        assert!(matches!(out.first(), Some(OpencodeEvent::SessionBusy)));
        assert!(matches!(out.get(1), Some(OpencodeEvent::MessageDelta)));
        assert!(matches!(out.get(2), Some(OpencodeEvent::SessionIdle)));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn data_field_with_no_space_after_colon_still_parsed() {
        // SSE spec allows `data:...` (no space). Validate we strip
        // the colon + optional whitespace.
        let mut buf = Vec::new();
        buf.extend_from_slice(
            br#"data:{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
        );
        buf.extend_from_slice(b"\n\n");
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        process_buffer(&mut buf, &cb);
        let mut out: Vec<OpencodeEvent> = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        assert!(
            matches!(out.first(), Some(OpencodeEvent::SessionIdle)),
            "should handle no-space `data:` form, got {out:?}"
        );
    }
}
