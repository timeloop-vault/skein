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

use crate::db::Database;
use crate::harness_actions_opencode;

/// Reconnect backoff schedule (seconds), capped at the last value
/// for any further attempts. 1 → 2 → 4 → 8 → 16 → 30 → 30 … gets us
/// from "opencode hasn't bound yet" to "opencode is dead" with
/// minimal wasted polling.
const BACKOFF_SCHEDULE_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

/// TCP connect timeout. Localhost is sub-millisecond when opencode is
/// alive; we keep a small budget so a wedged opencode (port bound but
/// listener dead) doesn't hang the reconnect loop forever.
///
/// Critical: this is `connect_timeout`, NOT `timeout`. `timeout` is
/// the overall request budget — SSE responses are infinite by
/// design, so a `timeout` would abort the stream after the budget
/// expires with "error decoding response body" (observed against a
/// live opencode session in v1: 5 s timeout, stream healthy, error
/// fired after 5 s every reconnect cycle).
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

pub struct OpencodeEventsManager {
    inner: Mutex<HashMap<String, Adapter>>,
    db: Arc<Database>,
    /// Frontend emitter for live action rows. `None` in tests.
    /// Issue #80 D1.
    app: Option<tauri::AppHandle>,
}

impl OpencodeEventsManager {
    pub fn new(db: Arc<Database>, app: tauri::AppHandle) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            db,
            app: Some(app),
        }
    }

    /// Start watching opencode's SSE stream on `127.0.0.1:<port>`.
    /// `on_event` fires for every parsed event. Spawns a tokio task
    /// that owns the reqwest connection + reconnect loop; cancelled
    /// by `detach` (or `Drop`).
    ///
    /// `room_id` stamps every persisted action row (issue #80).
    /// `session_id` enables backfill from the opencode `SQLite` DB —
    /// `None` for fresh sessions (backfill will happen once the SSE
    /// `session.created` event fires; fresh sessions have no history
    /// anyway).
    ///
    /// Idempotent: re-attaching the same `harness_id` cancels the
    /// previous adapter first.
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub fn attach<F>(
        &self,
        harness_id: String,
        room_id: String,
        port: u16,
        session_id: Option<String>,
        on_event: F,
    ) where
        F: Fn(OpencodeEvent) + Send + Sync + 'static,
    {
        {
            let mut inner = self.inner.lock();
            inner.remove(&harness_id);
        }
        // Backfill from opencode's SQLite when we know the session.
        if let Some(sid) = &session_id {
            let max_ts = self
                .db
                .recent_harness_actions_by_harness(&harness_id, -1, 1)
                .ok()
                .and_then(|rows| rows.into_iter().next())
                .map_or(0, |r| r.timestamp_ms);
            harness_actions_opencode::backfill_from_db(
                sid,
                &harness_id,
                &room_id,
                max_ts,
                &self.db,
            );
        }
        let cancel = Arc::new(Notify::new());
        let cancel_for_task = Arc::clone(&cancel);
        let on_event = Arc::new(on_event);
        let db = Arc::clone(&self.db);
        let app = self.app.clone();
        let hid = harness_id.clone();
        let rid = room_id;
        let handle = tokio::spawn(async move {
            run_adapter(port, cancel_for_task, on_event, db, app, hid, rid).await;
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
#[allow(clippy::too_many_arguments)]
async fn run_adapter(
    port: u16,
    cancel: Arc<Notify>,
    on_event: Arc<dyn Fn(OpencodeEvent) + Send + Sync>,
    db: Arc<Database>,
    app: Option<tauri::AppHandle>,
    harness_id: String,
    room_id: String,
) {
    let url = format!("http://127.0.0.1:{port}/event");
    let client = match reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
    {
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
            &db,
            app.as_ref(),
            &harness_id,
            &room_id,
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
#[allow(clippy::too_many_arguments)]
async fn stream_events(
    client: &reqwest::Client,
    url: &str,
    cancel: &Notify,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    connected_once: &AtomicBool,
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
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
    // Synthetic "assume idle" emit. opencode only broadcasts
    // `session.status` on *transitions*; subscribers don't get a
    // baseline. A session that's been sitting at its prompt
    // (fresh spawn or restart-of-idle-session) never emits
    // `session.status idle` until the user types something — so
    // without this, the dot stays in `spawning|running` forever
    // for opencode rooms that started Skein already idle.
    //
    // If opencode is actually mid-turn (rare on attach), the next
    // `session.status busy` arrives within ~100 ms and overrides
    // this. The user sees a brief blue-then-green flash, which is
    // fine. False idle is cheaper UX-wise than false running.
    on_event(OpencodeEvent::SessionIdle);

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
                process_buffer(&mut buf, on_event, db, app, harness_id, room_id);
            }
        }
    }
}

/// Walk `buf` for complete SSE frames (`...\n\n`), parse each, emit
/// matched events. Leaves any trailing partial frame in `buf` for
/// the next chunk to extend.
#[allow(clippy::too_many_arguments)]
fn process_buffer(
    buf: &mut Vec<u8>,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
) {
    loop {
        let Some(sep) = find_double_newline(buf) else {
            return;
        };
        let frame: Vec<u8> = buf.drain(..sep + 2).collect();
        let Ok(frame_str) = std::str::from_utf8(&frame[..frame.len().saturating_sub(2)]) else {
            continue;
        };
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
        // Phase event (existing path).
        if let Some(event) = parse_event(&payload) {
            on_event(event);
        }
        // Action extraction (issue #80). Live rows broadcast to the
        // frontend; backfill (in attach()) is silent.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
            for action in harness_actions_opencode::extract_from_sse(&value) {
                match db.record_harness_action(
                    harness_id,
                    room_id,
                    action.timestamp_ms,
                    action.kind,
                    &action.payload,
                    action.source.as_deref(),
                ) {
                    Ok(id) => {
                        if let Some(app) = app {
                            crate::harness_action_event::emit(
                                app,
                                id,
                                harness_id,
                                room_id,
                                action.timestamp_ms,
                                action.kind,
                                &action.payload,
                                action.source.as_deref(),
                            );
                        }
                    }
                    Err(e) => {
                        tracing::trace!(harness_id, kind = action.kind, error = %e,
                            "opencode_events: record_harness_action failed");
                    }
                }
            }
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

    fn test_db() -> (tempfile::TempDir, crate::db::Database) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

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
        let (_dir, db) = test_db();
        let mut buf = Vec::new();
        for (i, chunk) in input.iter().enumerate() {
            buf.extend_from_slice(&frame_each(i, chunk));
            process_buffer(&mut buf, &cb, &db, None, "h-test", "r-test");
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
        let (_tdir, tdb) = test_db();
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(
            br#"data: {"type":"session.status","properties":{"sessionID":"s","status":{"type":"#,
        );
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test");
        assert!(rx.try_recv().is_err(), "partial frame must not emit");
        buf.extend_from_slice(br#""idle"}}}"#);
        buf.extend_from_slice(b"\n\n");
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test");
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
        let (_tdir, tdb) = test_db();
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
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test");
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
        let (_tdir, tdb) = test_db();
        let mut buf = Vec::new();
        buf.extend_from_slice(
            br#"data:{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
        );
        buf.extend_from_slice(b"\n\n");
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test");
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
