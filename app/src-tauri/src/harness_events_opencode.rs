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

/// Budget for the `GET /session/<id>` root/child lookup (#116),
/// used only when a user message arrives in a session the adapter has
/// no cached answer for. Short: it blocks the SSE read loop from the
/// next frame while it runs, and a slow/failed lookup should give up
/// fast rather than stall the loop.
const SESSION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

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
    ///
    /// `parent_id` is `/event`'s own root/child signal (#116): a
    /// subagent (task-tool) child session always carries its parent's
    /// id here; a root session never does. Carried verbatim — `null`
    /// on the wire, not omitted — so the frontend can tell "root" from
    /// "we don't know yet" without a second round trip.
    SessionCreated {
        session_id: String,
        parent_id: Option<String>,
    },
    /// A user-role message landed in a session known to be a ROOT
    /// (#116). The frontend's cue to follow opencode's own `/new` and
    /// in-TUI `/sessions` picker switches onto the resumed/new session
    /// — the picker publishes nothing to `/event` itself
    /// (sst/opencode#5409), so the first observable sign of a switch
    /// is this user message. Never fires for a subagent's own child
    /// session, which also gets user-role messages but must never be
    /// followed. Independent of `UserMessageAgent` below — this fires
    /// whether or not opencode stamped an `agent`/`mode` field.
    RootSessionPrompted { session_id: String },
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
    /// A user message was recorded, and opencode stamped it with the
    /// agent it was sent to (#248). The closest thing opencode exposes to
    /// "which agent is this harness on": the user can switch agent
    /// mid-session (Tab, `switch_agent`, `@` mentions), and every user
    /// message records the one selected when it was sent.
    ///
    /// User messages only. An assistant message reports the agent that
    /// *produced* it, and a compaction turn reports `compaction` — which
    /// is not an agent anyone picked. `session_id` is carried because
    /// `/event` is instance-wide: a subagent's child session reports its
    /// own agent on the same stream, and only the frontend knows which
    /// session is the harness's own.
    UserMessageAgent { session_id: String, agent: String },
    /// A permission dialog opened for a tool call — opencode's
    /// equivalent of Claude Code's `PermissionRequest` hook (#86). The
    /// harness is blocked until the user answers. Not filtered by
    /// session: a subagent's child session blocks the same turn just as
    /// much as the harness's own.
    PermissionAsked {
        request_id: String,
        session_id: String,
    },
    /// The dialog above was answered — `allow`, `deny` or `once` all
    /// land here, since all three unblock the harness the same way.
    PermissionReplied {
        request_id: String,
        session_id: String,
    },
    /// opencode's `question` tool blocked mid-turn on user input.
    /// Distinct from a permission dialog (a different tool, no
    /// allow/deny/patterns), but the same "harness cannot proceed"
    /// consequence for the activity phase.
    QuestionAsked {
        request_id: String,
        session_id: String,
    },
    /// The question above was answered (`question.replied`) or
    /// dismissed (`question.rejected`) — both unblock the harness, so
    /// both collapse to one event here.
    QuestionResolved {
        request_id: String,
        session_id: String,
    },
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
        cwd: String,
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
            run_adapter(
                port,
                cancel_for_task,
                on_event,
                db,
                app,
                hid,
                rid,
                cwd,
                session_id,
            )
            .await;
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
    cwd: String,
    session_id: Option<String>,
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
    // Per-adapter root/child cache (#116), keyed by opencode session
    // id. Lives across reconnects deliberately: `session.created` only
    // fires once, at session birth, so a reconnect that misses it must
    // not forget what an earlier connection already learned.
    let root_cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    // Seed with the harness's own attached/resumed session: Skein only
    // ever captures or resumes a ROOT session for a harness, never a
    // subagent child, so this is safe to assume without a lookup — and
    // it means a boot resume's first user message doesn't pay a
    // network round trip for something already known.
    if let Some((sid, is_root)) = seed_own_session_root(session_id.as_deref()) {
        root_cache.lock().insert(sid, is_root);
    }
    let mut attempt: usize = 0;
    loop {
        // Race the SSE attempt against cancellation. If `cancel` ever
        // fires (Drop of Adapter), exit immediately.
        let connected_for_call = Arc::clone(&connected_once);
        let connect = stream_events(
            &client,
            &url,
            port,
            &cancel,
            on_event.as_ref(),
            &connected_for_call,
            &db,
            app.as_ref(),
            &harness_id,
            &room_id,
            &cwd,
            &root_cache,
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
    port: u16,
    cancel: &Notify,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    connected_once: &AtomicBool,
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    cwd: &str,
    root_cache: &Mutex<HashMap<String, bool>>,
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
                let unresolved =
                    process_buffer(&mut buf, on_event, db, app, harness_id, room_id, cwd, root_cache);
                // Resolve outside process_buffer: this is the only
                // point in the adapter allowed to do network I/O, and
                // process_buffer stays synchronous and unit-testable.
                // Sequential (order-preserving), but each lookup races
                // `cancel` too — up to SESSION_LOOKUP_TIMEOUT per id is
                // otherwise unobserved by the outer select!, so closing
                // a harness mid-lookup would hang the detach on it.
                for session_id in unresolved {
                    let lookup = lookup_session_is_root(client, port, &session_id);
                    tokio::select! {
                        biased;
                        () = cancel.notified() => {
                            tracing::debug!(port, "opencode_events: cancelled during root lookup");
                            return Ok(());
                        }
                        is_root = lookup => {
                            if let Some(is_root) = is_root {
                                root_cache.lock().insert(session_id.clone(), is_root);
                                if let Some(event) = decide_root_session_prompted(&session_id, Some(is_root)) {
                                    on_event(event);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Walk `buf` for complete SSE frames (`...\n\n`), parse each, emit
/// matched events. Leaves any trailing partial frame in `buf` for
/// the next chunk to extend.
///
/// Returns the session ids of any user-role `message.updated` whose
/// root/child status isn't in `root_cache` yet — the caller (the only
/// place in this module allowed to do network I/O) resolves those via
/// `GET /session/<id>` and re-emits `RootSessionPrompted` itself. Kept
/// out of this function so it stays synchronous and testable without a
/// network.
#[allow(clippy::too_many_arguments)]
fn process_buffer(
    buf: &mut Vec<u8>,
    on_event: &(dyn Fn(OpencodeEvent) + Send + Sync),
    db: &Database,
    app: Option<&tauri::AppHandle>,
    harness_id: &str,
    room_id: &str,
    cwd: &str,
    root_cache: &Mutex<HashMap<String, bool>>,
) -> Vec<String> {
    let mut unresolved = Vec::new();
    loop {
        let Some(sep) = find_double_newline(buf) else {
            return unresolved;
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
        // Root/child cache maintenance (#116). `session.created` only
        // fires at session birth; `session.updated` re-states the same
        // fact and is the only signal available for a session already
        // alive when the adapter attaches, so it's read here even
        // though it never surfaces its own `OpencodeEvent`.
        if let Some((session_id, parent_id)) = parse_session_updated_parent(&payload) {
            root_cache.lock().insert(session_id, parent_id.is_none());
        }
        // Phase event (existing path).
        if let Some(event) = parse_event(&payload) {
            if let OpencodeEvent::SessionCreated {
                session_id,
                parent_id,
            } = &event
            {
                root_cache
                    .lock()
                    .insert(session_id.clone(), parent_id.is_none());
            }
            on_event(event);
        }
        // Root-session follow decision (#116): independent of
        // `UserMessageAgent` above, which requires an `agent` field —
        // this must not, since the only thing that matters here is
        // "a user typed something in a session we know is root."
        if let Some(session_id) = user_message_session_id(&payload) {
            match root_cache.lock().get(&session_id).copied() {
                Some(is_root) => {
                    if let Some(event) = decide_root_session_prompted(&session_id, Some(is_root)) {
                        on_event(event);
                    }
                }
                None => unresolved.push(session_id),
            }
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
                        // SSE rows are live by definition; the DB
                        // backfill path deliberately does not capture
                        // baselines (see review.rs).
                        if action.kind == crate::db::action_kind::PATCH {
                            crate::review::note_patch(
                                db,
                                room_id,
                                cwd,
                                harness_id,
                                &action.payload,
                            );
                        }
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
            // `properties.info` is the full session object; `parentID`
            // set means a subagent (task-tool) child, absent means a
            // root session — verified upstream, no ambiguous case
            // (#116). Leniently optional: a `session.created` shape
            // that ever omits `info` still yields a `SessionCreated`
            // for resume-id capture, just with an unknown parent.
            let parent_id = props
                .and_then(|p| p.get("info"))
                .and_then(|info| info.get("parentID"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            Some(OpencodeEvent::SessionCreated {
                session_id,
                parent_id,
            })
        }
        "message.part.delta" => Some(OpencodeEvent::MessageDelta),
        "message.updated" => {
            // Shape measured against opencode 1.18.30 (#248):
            // `{"type":"message.updated","properties":{"sessionID":…,
            //   "info":{"role":"user","sessionID":…,"agent":"plan",…}}}`.
            // `mode` is the older name for the same field, as in
            // `skein_harness::opencode`.
            let info = props.and_then(|p| p.get("info"))?;
            if info.get("role").and_then(serde_json::Value::as_str) != Some("user") {
                return None;
            }
            let session_id = info
                .get("sessionID")
                .or_else(|| props.and_then(|p| p.get("sessionID")))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let agent = info
                .get("agent")
                .or_else(|| info.get("mode"))
                .and_then(serde_json::Value::as_str)
                .filter(|a| !a.is_empty())?
                .to_owned();
            Some(OpencodeEvent::UserMessageAgent { session_id, agent })
        }
        // `id` names the request on `*.asked`; `requestID` names it on
        // the matching `*.replied`/`*.rejected` — verified against a
        // live opencode 1.18.30 binary (#86). Not the same field, and
        // not a typo to "fix" into consistency.
        "permission.asked" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("id"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::PermissionAsked {
                request_id,
                session_id,
            })
        }
        "permission.replied" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("requestID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::PermissionReplied {
                request_id,
                session_id,
            })
        }
        "question.asked" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("id"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::QuestionAsked {
                request_id,
                session_id,
            })
        }
        "question.replied" | "question.rejected" => {
            let session_id = props
                .and_then(|p| p.get("sessionID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            let request_id = props
                .and_then(|p| p.get("requestID"))
                .and_then(serde_json::Value::as_str)?
                .to_owned();
            Some(OpencodeEvent::QuestionResolved {
                request_id,
                session_id,
            })
        }
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
        // `session.updated` (read separately by
        // `parse_session_updated_parent` for the #116 root/child
        // cache, but it has no `OpencodeEvent` of its own),
        // `session.diff`, `server.heartbeat`, `mcp.tools.changed`,
        // future event types — is silent at the policy layer. See
        // recon §3 for the catalog. Permission and question events
        // are handled above, not here.
        _ => None,
    }
}

/// `session.updated`'s root/child fact (#116), extracted the same way
/// as `session.created`'s but never surfaced as its own
/// `OpencodeEvent` — the frontend has no use for "session metadata
/// changed." Read purely to backfill the root/child cache for a
/// session the adapter attached to mid-life, whose `session.created`
/// (fired once, at birth) it never saw.
fn parse_session_updated_parent(payload: &str) -> Option<(String, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session.updated") {
        return None;
    }
    let props = value.get("properties")?;
    // Both the id and the parent fact live under `info` — unlike
    // `session.created`, `session.updated` has no top-level
    // `sessionID` to fall back on, so a frame without `info` carries
    // nothing usable and is skipped rather than guessed at.
    let info = props.get("info")?;
    let session_id = info
        .get("id")
        .or_else(|| info.get("sessionID"))
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let parent_id = info
        .get("parentID")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Some((session_id, parent_id))
}

/// Session id of a user-role `message.updated`, independent of the
/// `agent`/`mode` field that `UserMessageAgent` requires (#116) — the
/// root/child follow decision cares only that a user typed something,
/// not what agent it went to.
fn user_message_session_id(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("message.updated") {
        return None;
    }
    let props = value.get("properties")?;
    let info = props.get("info")?;
    if info.get("role").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    info.get("sessionID")
        .or_else(|| props.get("sessionID"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Turn "we now know whether `session_id` is root" into the follow
/// event, or not (#116). Pure, so the cache-hit path (in
/// `process_buffer`) and the lookup-hit path (after the async
/// `GET /session/<id>`, in `stream_events`) share one decision, and it
/// is testable without touching the network: `is_root = None` stands
/// in for a failed or ambiguous lookup, which must never be followed
/// on a guess.
fn decide_root_session_prompted(session_id: &str, is_root: Option<bool>) -> Option<OpencodeEvent> {
    is_root
        .filter(|&root| root)
        .map(|_| OpencodeEvent::RootSessionPrompted {
            session_id: session_id.to_owned(),
        })
}

/// opencode session ids look like `ses_<alnum>`. Reject anything else
/// before it goes into a URL path segment — defense in depth, since
/// the id already came off the wire as a JSON string, not user input,
/// but a malformed one has no business being ranged over `/session/`.
fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Seed value for the harness's own attached/resumed session (#116
/// follow-up). Pure so the seeding decision is testable without
/// touching the cache/`Mutex` it feeds. `None` when there's no session
/// yet (fresh spawn) or the id fails validation — the same guard the
/// network lookup uses, so a malformed id is refused everywhere it
/// could reach a cache or a URL, not just one of them.
fn seed_own_session_root(session_id: Option<&str>) -> Option<(String, bool)> {
    let sid = session_id?;
    is_valid_session_id(sid).then(|| (sid.to_owned(), true))
}

/// Authoritative root/child lookup for a session the adapter has no
/// cached answer for (attached mid-life, after both `session.created`
/// and any `session.updated` for it had already passed) — `GET
/// /session/<id>` against opencode's own HTTP API. `None` on any
/// failure (bad id, timeout, network error, non-200, unparseable
/// body): conservative, since guessing risks following the wrong
/// session. Bounded by `SESSION_LOOKUP_TIMEOUT` so a wedged opencode
/// can't stall the SSE read loop indefinitely.
async fn lookup_session_is_root(
    client: &reqwest::Client,
    port: u16,
    session_id: &str,
) -> Option<bool> {
    if !is_valid_session_id(session_id) {
        tracing::warn!(
            session_id,
            "opencode_events: refusing malformed session id for root lookup"
        );
        return None;
    }
    let url = format!("http://127.0.0.1:{port}/session/{session_id}");
    let response = tokio::time::timeout(SESSION_LOOKUP_TIMEOUT, client.get(&url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    Some(
        value
            .get("parentID")
            .and_then(serde_json::Value::as_str)
            .is_none(),
    )
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
        let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
        let mut buf = Vec::new();
        for (i, chunk) in input.iter().enumerate() {
            buf.extend_from_slice(&frame_each(i, chunk));
            process_buffer(&mut buf, &cb, &db, None, "h-test", "r-test", "", &cache);
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
            matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, .. }) if session_id == "ses_abc123"),
            "expected SessionCreated(ses_abc123), got {events:?}"
        );
    }

    #[test]
    fn session_created_reports_root_when_no_parent_id() {
        let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, parent_id })
                if session_id == "ses_root" && parent_id.is_none()),
            "expected root SessionCreated, got {events:?}"
        );
    }

    #[test]
    fn session_created_reports_child_when_parent_id_present() {
        // A subagent (task-tool) child session always carries its
        // parent's id — verified upstream, not a guessed symmetry
        // (#116).
        let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_child","info":{"id":"ses_child","parentID":"ses_root"}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, parent_id })
                if session_id == "ses_child" && parent_id.as_deref() == Some("ses_root")),
            "expected child SessionCreated, got {events:?}"
        );
    }

    #[test]
    fn user_message_in_root_session_emits_root_session_prompted() {
        // opencode's `/sessions` picker publishes nothing to `/event`
        // itself (sst/opencode#5409) — a user-role message in a known
        // root session is the first observable sign of a switch.
        let events = drain_events(
            &[
                r#"{"type":"session.created","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#,
                r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_root"}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            events.iter().any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { session_id } if session_id == "ses_root")),
            "expected RootSessionPrompted for a known root session, got {events:?}"
        );
    }

    #[test]
    fn user_message_in_child_session_never_emits_root_session_prompted() {
        let events = drain_events(
            &[
                r#"{"type":"session.created","properties":{"sessionID":"ses_child","info":{"id":"ses_child","parentID":"ses_root"}}}"#,
                r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_child"}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { .. })),
            "a subagent's own child session must never be followed, got {events:?}"
        );
    }

    #[test]
    fn session_updated_alone_populates_the_root_cache() {
        // `session.updated` never surfaces its own `OpencodeEvent`, but
        // it's the only signal available for a session already alive
        // when the adapter attaches (no `session.created` to see).
        let events = drain_events(
            &[
                r#"{"type":"session.updated","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#,
                r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_root"}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OpencodeEvent::SessionCreated { .. })),
            "session.updated must never surface its own event, got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { session_id } if session_id == "ses_root")),
            "session.updated should have cached root status, got {events:?}"
        );
    }

    #[test]
    fn user_message_in_unknown_session_is_returned_for_lookup_not_guessed() {
        let (_dir, db) = test_db();
        let (tx, rx) = mpsc::channel();
        let cb = move |e: OpencodeEvent| {
            tx.send(e).unwrap();
        };
        let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
        let mut buf = frame(
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_unknown"}}}"#,
        );
        let unresolved = process_buffer(&mut buf, &cb, &db, None, "h-test", "r-test", "", &cache);
        assert_eq!(unresolved, vec!["ses_unknown".to_owned()]);
        assert!(
            rx.try_recv().is_err(),
            "an unknown session must never emit before the lookup resolves it"
        );
    }

    #[test]
    fn decide_root_session_prompted_only_fires_for_a_confirmed_root() {
        assert!(matches!(
            decide_root_session_prompted("ses_1", Some(true)),
            Some(OpencodeEvent::RootSessionPrompted { session_id }) if session_id == "ses_1"
        ));
        assert!(decide_root_session_prompted("ses_1", Some(false)).is_none());
        assert!(
            decide_root_session_prompted("ses_1", None).is_none(),
            "a failed or ambiguous lookup must never be followed on a guess"
        );
    }

    #[test]
    fn session_id_validation_rejects_anything_not_alnum_or_underscore() {
        assert!(is_valid_session_id("ses_abc123"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("ses/abc"));
        assert!(!is_valid_session_id("ses abc"));
        assert!(!is_valid_session_id("../../etc"));
    }

    #[test]
    fn seed_own_session_root_accepts_a_valid_id_as_root() {
        assert_eq!(
            seed_own_session_root(Some("ses_abc123")),
            Some(("ses_abc123".to_owned(), true))
        );
    }

    #[test]
    fn seed_own_session_root_rejects_absent_or_malformed_ids() {
        assert_eq!(
            seed_own_session_root(None),
            None,
            "fresh spawn, no session yet"
        );
        assert_eq!(seed_own_session_root(Some("")), None);
        assert_eq!(seed_own_session_root(Some("ses/../abc")), None);
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
    fn permission_asked_and_replied_use_different_id_fields() {
        // `id` on asked, `requestID` on replied — verified against a
        // live opencode 1.18.30 binary, not a guessed symmetry.
        let events = drain_events(
            &[
                r#"{"type":"permission.asked","properties":{"id":"perm_1","sessionID":"ses_1","permission":"bash","patterns":["rm -rf"]}}"#,
                r#"{"type":"permission.replied","properties":{"sessionID":"ses_1","requestID":"perm_1","reply":"once"}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            matches!(events.as_slice(), [
                OpencodeEvent::PermissionAsked { request_id, session_id },
                OpencodeEvent::PermissionReplied { request_id: r2, session_id: s2 },
            ] if request_id == "perm_1" && session_id == "ses_1" && r2 == "perm_1" && s2 == "ses_1"),
            "got {events:?}"
        );
    }

    #[test]
    fn question_asked_replied_and_rejected_all_parse() {
        let events = drain_events(
            &[
                r#"{"type":"question.asked","properties":{"id":"q_1","sessionID":"ses_1","questions":[]}}"#,
                r#"{"type":"question.replied","properties":{"sessionID":"ses_1","requestID":"q_1","answers":[]}}"#,
                r#"{"type":"question.asked","properties":{"id":"q_2","sessionID":"ses_1","questions":[]}}"#,
                r#"{"type":"question.rejected","properties":{"sessionID":"ses_1","requestID":"q_2"}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            matches!(events.as_slice(), [
                OpencodeEvent::QuestionAsked { request_id: r1, .. },
                OpencodeEvent::QuestionResolved { request_id: r2, .. },
                OpencodeEvent::QuestionAsked { request_id: r3, .. },
                OpencodeEvent::QuestionResolved { request_id: r4, .. },
            ] if r1 == "q_1" && r2 == "q_1" && r3 == "q_2" && r4 == "q_2"),
            "question.replied and question.rejected must both resolve: {events:?}"
        );
    }

    /// A subagent's child session blocks the same turn just as much as
    /// the harness's own — these events are never filtered by session,
    /// unlike `UserMessageAgent`.
    #[test]
    fn permission_and_question_events_are_not_filtered_by_session() {
        let events = drain_events(
            &[
                r#"{"type":"permission.asked","properties":{"id":"p1","sessionID":"ses_child_subagent"}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(
            matches!(events.first(), Some(OpencodeEvent::PermissionAsked { session_id, .. }) if session_id == "ses_child_subagent"),
            "got {events:?}"
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
    fn user_message_reports_the_agent_it_was_sent_to() {
        // Verbatim from a live `opencode serve` 1.18.30 (#248).
        let payload = r#"{"type":"message.updated","properties":{"sessionID":"ses_1","info":{"id":"msg_1","role":"user","sessionID":"ses_1","time":{"created":1789240141274},"agent":"plan","model":{"providerID":"p","modelID":"m"}}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.as_slice(), [OpencodeEvent::UserMessageAgent { session_id, agent }]
                if session_id == "ses_1" && agent == "plan"),
            "expected UserMessageAgent(ses_1, plan), got {events:?}"
        );
    }

    #[test]
    fn older_mode_field_still_names_the_agent() {
        let payload = r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_1","mode":"build"}}}"#;
        let events = drain_events(&[payload], |_, p| frame(p));
        assert!(
            matches!(events.as_slice(), [OpencodeEvent::UserMessageAgent { agent, .. }] if agent == "build"),
            "got {events:?}"
        );
    }

    #[test]
    fn assistant_and_agentless_messages_do_not_report_an_agent() {
        // A compaction turn's assistant message says `compaction`, which
        // is not an agent the user picked — the reason this is user-only.
        let events = drain_events(
            &[
                r#"{"type":"message.updated","properties":{"info":{"role":"assistant","sessionID":"s","agent":"compaction","mode":"compaction"}}}"#,
                r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"s"}}}"#,
                r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"s","agent":""}}}"#,
            ],
            |_, p| frame(p),
        );
        assert!(events.is_empty(), "got {events:?}");
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
        let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
        let mut buf = Vec::new();
        buf.extend_from_slice(
            br#"data: {"type":"session.status","properties":{"sessionID":"s","status":{"type":"#,
        );
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test", "", &cache);
        assert!(rx.try_recv().is_err(), "partial frame must not emit");
        buf.extend_from_slice(br#""idle"}}}"#);
        buf.extend_from_slice(b"\n\n");
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test", "", &cache);
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
        let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test", "", &cache);
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
        let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
        process_buffer(&mut buf, &cb, &tdb, None, "h-test", "r-test", "", &cache);
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
