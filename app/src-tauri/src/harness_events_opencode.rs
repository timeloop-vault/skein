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

mod connection;
mod frames;
mod session;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::db::Database;
use crate::harness_actions_opencode;
use connection::run_adapter;

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
    /// SSE connected. Fires on every successful (re)connect, not just
    /// the first; the frontend relies on that to re-arm the
    /// authoritative source that `SessionEnd` detached.
    Connected,
    /// Assumed-idle baseline emitted after each (re)connect. Not an
    /// observed transition: opencode sends no baseline, so this is a
    /// guess the frontend applies quietly (no notification), unlike a
    /// real `session.status idle`, which stays `SessionIdle`.
    SessionBaselineIdle,
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

#[cfg(test)]
use connection::{StreamError, emit_connect_events, stream_events};
#[cfg(test)]
use frames::{process_buffer, should_emit_prompt};
#[cfg(test)]
use session::{decide_root_session_prompted, is_valid_session_id, seed_own_session_root};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
mod tests;
