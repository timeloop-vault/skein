//! What a request handler has access to, and how it tells the UI that
//! something changed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;

use crate::db::Database;
use crate::spawn_settings::SpawnSettings;

/// The event the review pane listens for.
///
/// An agent reply touches sqlite and nothing else — no file changes, so
/// the worktree watcher that drives every other refresh never fires.
/// Without this the user would be looking at a pane that is quietly out
/// of date, which is worse than one that is obviously empty.
pub const REVIEW_CHANGED_EVENT: &str = "skein://review-changed";

/// The event a harness tab listens for to show "permission required" as
/// its own activity phase (#86), distinct from end-of-turn `waiting`.
/// Fired by `POST /api/harness/permission`, which the injected Claude
/// plugin hook calls; opencode's equivalent is a phase transition the
/// frontend derives straight from `OpencodeEvent` and never touches
/// this route.
pub const HARNESS_PERMISSION_EVENT: &str = "skein://harness-permission";

/// The event a harness tab listens for to leave the `spawning` activity
/// phase (#273) — a freshly spawned Claude harness has no JSONL
/// transcript at all until the first prompt, so the transcript tail
/// (the phase store's authoritative source everywhere else) has
/// nothing to read until then. Fired by `POST
/// /api/harness/session-start`, which the injected Claude plugin's
/// `SessionStart` hook calls on every session start, resume, clear,
/// compact and fork — opencode reports its own liveness over its
/// `/event` SSE stream and never touches this route. Since #116 the
/// payload also carries Claude's own `session_id` and `source`, so the
/// frontend can follow a `/clear` onto the new conversation id.
pub const HARNESS_SESSION_START_EVENT: &str = "skein://harness-session-start";

/// The event a harness's mailbox badge listens for (#327, feeding
/// #329's unread indicator). Fired on a successful `send_message` (for
/// the *recipient*, which is usually not the caller's own room) and on
/// a `read_messages` call that actually marked something read (for the
/// caller). A mailbox write touches only sqlite — no file changes, so
/// nothing else would tell a room its inbox moved.
pub const MAIL_CHANGED_EVENT: &str = "skein://mail-changed";

/// The event the frontend listens for to answer a question only the
/// webview can answer — starting with #328's needs, where the backend
/// has no other way to reach into UI state. Every other signal in this
/// module is one-way (Skein tells the UI something changed); this one
/// is a call the UI is expected to reply to via `agent_request_complete`,
/// which is why it is paired with a pending-request map rather than
/// just another `notify_*` emit.
///
/// `#[allow(dead_code)]` because #328 lands the request/response
/// plumbing ahead of the verb PRs that call [`AgentApiState::request_frontend`]
/// with a real `kind`.
#[allow(dead_code)]
pub const AGENT_REQUEST_EVENT: &str = "skein://agent-request";

/// Payload of [`AGENT_REQUEST_EVENT`]. Frontend contract — do not
/// rename a field without checking `app/src/` for the listener.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRequest {
    pub id: String,
    pub kind: String,
    pub args: serde_json::Value,
}

/// A promise waiting on [`AgentApiState::complete_request`]. Boxed
/// behind a plain `Sender` rather than anything fancier — the receiving
/// end is a single `await` in [`AgentApiState::await_response`], never
/// polled or cloned.
type PendingSender = tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>;

/// Removes `id` from `state.pending_requests` on drop, not only on a
/// normal return.
///
/// A plain `remove()` placed after the `.await` in
/// [`AgentApiState::await_response`] only runs when that `.await`
/// itself resolves — a timeout or a completed reply. It does NOT run
/// when the *outer* future is dropped before either happens, which is
/// exactly what an axum handler does when the client disconnects
/// mid-request (the route #330 wires up). Without this guard that
/// cancellation leaks the entry forever: the `oneshot::Sender` sits in
/// the map with nothing left to poll its receiver, and a later
/// `complete_request` for the same id would wrongly look like it
/// worked. Holding the guard across the whole await, instead of
/// removing manually, means every exit path — success, timeout, AND
/// cancellation — goes through the same `Drop`.
///
/// Safe to coexist with [`AgentApiState::complete_request`]'s own
/// `remove()`: whichever runs first empties the entry, so the other is
/// a harmless no-op on an already-missing key.
struct PendingRequestGuard<'a> {
    state: &'a AgentApiState,
    id: String,
}

impl Drop for PendingRequestGuard<'_> {
    fn drop(&mut self) {
        self.state.pending_requests.lock().remove(&self.id);
    }
}

/// Shared by every route.
///
/// `app` is optional for the same reason it is in the harness-event
/// managers: a unit test has a `Database` but no Tauri runtime, and
/// building a real `AppHandle` needs a running app.
pub struct AgentApiState {
    pub db: Arc<Database>,
    pub app: Option<tauri::AppHandle>,
    /// Requests sent to the webview and not yet answered, keyed by a
    /// fresh id per request. Every insertion is matched by exactly one
    /// removal — on completion, on timeout, or immediately if the
    /// request could never be sent — so a leaked entry here would mean
    /// a `oneshot::Sender` nobody will ever use, not a stuck UI.
    /// `pub(crate)` so tests can assert it drains on every exit path.
    pub(crate) pending_requests: parking_lot::Mutex<HashMap<String, PendingSender>>,
}

/// Payload of [`REVIEW_CHANGED_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChanged {
    pub room_id: String,
}

/// Payload of [`HARNESS_PERMISSION_EVENT`]. Frontend contract — do not
/// rename a field without checking `app/src/` for the listener.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessPermission {
    pub room_id: String,
    pub harness_id: String,
    pub tool_name: Option<String>,
    pub agent_type: Option<String>,
    /// Which subagent opened the dialog, when any did (`None` for a
    /// main-session dialog). Lets the frontend avoid letting one
    /// subagent's tool result clear another subagent's still-open
    /// dialog (#276). Upstream documents `agent_id` as present only
    /// inside a subagent call, confirmed on a live run (2026-09-20)
    /// — so the frontend must still treat `None` as "clear as
    /// before", never as "never clear": it legitimately means a
    /// main-session dialog, and injection (#215) can be switched off
    /// entirely.
    pub agent_id: Option<String>,
}

/// Payload of [`MAIL_CHANGED_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailChanged {
    pub room_id: String,
    pub harness_id: String,
}

/// Payload of [`HARNESS_SESSION_START_EVENT`]. Frontend contract — do
/// not rename a field without checking `app/src/` for the listener.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionStart {
    pub room_id: String,
    pub harness_id: String,
    /// Claude's own new conversation id. After a `/clear` this differs
    /// from the id Skein spawned the harness with — that's what the
    /// frontend follows. `None` when the hook payload had no
    /// `session_id`, or one that didn't look like a plausible id (see
    /// `session_start_fields` in `http.rs`).
    pub session_id: Option<String>,
    /// One of Claude's own `startup`/`resume`/`clear`/`compact`/`fork`.
    /// `None` when the hook payload had no `source`.
    pub source: Option<String>,
}

impl AgentApiState {
    pub fn new(db: Arc<Database>, app: tauri::AppHandle) -> Self {
        Self {
            db,
            app: Some(app),
            pending_requests: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Test constructor — no `AppHandle`, so writes persist and simply
    /// notify nobody.
    #[cfg(test)]
    pub fn for_test(db: Arc<Database>) -> Self {
        Self {
            db,
            app: None,
            pending_requests: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Tell the frontend that this room's review moved.
    pub fn notify_review_changed(&self, room_id: &str) {
        let Some(app) = self.app.as_ref() else { return };
        if let Err(e) = app.emit(
            REVIEW_CHANGED_EVENT,
            ReviewChanged {
                room_id: room_id.to_owned(),
            },
        ) {
            // Not fatal — the write landed, only the live nudge did
            // not — but silence here is exactly what #176 is about.
            tracing::warn!(room_id, error = %e, "agent api: review-changed emit failed");
        }
    }

    /// Tell the frontend that this harness is blocked on a permission
    /// dialog (#86). Never logs `tool_input` — the caller has already
    /// dropped it before this is reached, and nothing here re-derives
    /// it.
    pub fn notify_harness_permission(
        &self,
        room_id: &str,
        harness_id: &str,
        tool_name: Option<String>,
        agent_type: Option<String>,
        agent_id: Option<String>,
    ) {
        let Some(app) = self.app.as_ref() else { return };
        if let Err(e) = app.emit(
            HARNESS_PERMISSION_EVENT,
            HarnessPermission {
                room_id: room_id.to_owned(),
                harness_id: harness_id.to_owned(),
                tool_name,
                agent_type,
                agent_id,
            },
        ) {
            tracing::warn!(room_id, harness_id, error = %e, "agent api: harness-permission emit failed");
        }
    }

    /// Tell the frontend that a harness's mailbox moved (#327) — a
    /// message just landed for it, or it just read some of its own.
    /// `room_id`/`harness_id` name whoever's inbox changed, which for a
    /// send is almost always a different room than the caller's.
    pub fn notify_mail_changed(&self, room_id: &str, harness_id: &str) {
        let Some(app) = self.app.as_ref() else { return };
        if let Err(e) = app.emit(
            MAIL_CHANGED_EVENT,
            MailChanged {
                room_id: room_id.to_owned(),
                harness_id: harness_id.to_owned(),
            },
        ) {
            tracing::warn!(room_id, harness_id, error = %e, "agent api: mail-changed emit failed");
        }
    }

    /// The live spawn settings, for the mailbox's [`MailPolicy`]
    /// (#327) — read fresh on every request rather than cached, since
    /// the Settings pane can flip the injection or messaging toggles at
    /// any time. `None` in a unit test (`for_test` has no `AppHandle`,
    /// so no managed [`crate::SpawnEnvState`] to read) falls back to
    /// `SpawnSettings::default()`, whose defaults are exactly as
    /// permissive as `MailPolicy`'s own test default.
    pub fn spawn_settings(&self) -> SpawnSettings {
        self.app
            .as_ref()
            .and_then(tauri::Manager::try_state::<crate::SpawnEnvState>)
            .map(|s| s.snapshot())
            .unwrap_or_default()
    }

    /// Tell the frontend that this harness has (re)started a session
    /// (#273), so a harness stuck in `spawning` can move on even
    /// before its transcript exists. `session_id`/`source` (#116) let
    /// the frontend follow a `/clear` onto the new conversation id.
    pub fn notify_harness_session_start(
        &self,
        room_id: &str,
        harness_id: &str,
        session_id: Option<String>,
        source: Option<String>,
    ) {
        let Some(app) = self.app.as_ref() else { return };
        if let Err(e) = app.emit(
            HARNESS_SESSION_START_EVENT,
            HarnessSessionStart {
                room_id: room_id.to_owned(),
                harness_id: harness_id.to_owned(),
                session_id,
                source,
            },
        ) {
            tracing::warn!(room_id, harness_id, error = %e, "agent api: harness-session-start emit failed");
        }
    }

    /// Register a pending request and return the id and the receiving
    /// half. Split out of [`Self::request_frontend`] so a test can
    /// exercise the map (double-complete, unknown id, timeout leaving
    /// it empty) without a real `AppHandle` to emit through.
    ///
    /// `#[allow(dead_code)]` on this and the two methods below because
    /// #328 lands the request/response plumbing itself ahead of the
    /// verb PRs that will call [`Self::request_frontend`] with a real
    /// `kind` — today only the test module reaches this cluster.
    #[allow(dead_code)]
    pub(crate) fn register(
        &self,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_requests.lock().insert(id.clone(), tx);
        (id, rx)
    }

    /// Wait for `id`'s answer, time out, or be cancelled — `id` is gone
    /// from the map on every one of those, via [`PendingRequestGuard`]
    /// rather than a manual `remove()` after the await: a manual
    /// removal only runs when the await itself resolves, and misses
    /// the outer future being dropped mid-wait (an axum handler
    /// cancelled by client disconnect, #330). Leaving a sender nobody
    /// will ever complete would make a later [`Self::complete_request`]
    /// for the same id look like it worked.
    #[allow(dead_code)]
    pub(crate) async fn await_response(
        &self,
        id: &str,
        rx: tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>,
        timeout: Duration,
        kind: &str,
    ) -> Result<serde_json::Value, String> {
        let _guard = PendingRequestGuard {
            state: self,
            id: id.to_owned(),
        };
        let outcome = tokio::time::timeout(timeout, rx).await;
        match outcome {
            // The sender side completed normally.
            Ok(Ok(result)) => result,
            // The sender was dropped without completing — the webview
            // went away (navigation, close) mid-request.
            Ok(Err(_)) => Err(format!(
                "no answer for agent request {id} ({kind}): the webview dropped the request"
            )),
            Err(_) => Err(format!(
                "agent request {id} ({kind}) timed out after {timeout:?}"
            )),
        }
    }

    /// Ask the webview a question only it can answer, and await the
    /// reply.
    ///
    /// Every other signal in this module is one-way: Skein tells the UI
    /// something changed and moves on regardless of whether anyone was
    /// listening. This one is different on purpose — the caller needs
    /// the answer to proceed, so a missing webview or a timeout must be
    /// a loud `Err`, never a request that silently never completes. If
    /// the returned future is itself dropped before answering — an
    /// axum handler cancelled by client disconnect (#330) — the pending
    /// entry is still cleaned up, via [`PendingRequestGuard`] inside
    /// [`Self::await_response`].
    #[allow(dead_code)]
    pub async fn request_frontend(
        &self,
        kind: &str,
        args: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let Some(app) = self.app.as_ref() else {
            return Err("no webview is listening for agent requests".to_owned());
        };
        let (id, rx) = self.register();
        if let Err(e) = app.emit(
            AGENT_REQUEST_EVENT,
            AgentRequest {
                id: id.clone(),
                kind: kind.to_owned(),
                args,
            },
        ) {
            self.pending_requests.lock().remove(&id);
            return Err(format!("no webview is listening for agent requests: {e}"));
        }
        self.await_response(&id, rx, timeout, kind).await
    }

    /// Deliver the frontend's answer to a pending [`Self::request_frontend`]
    /// call.
    ///
    /// An unknown id — never issued, already completed, or already
    /// timed out — is an error rather than a silent no-op: the frontend
    /// answered something nobody is (or is still) waiting for, which is
    /// worth surfacing rather than swallowing.
    pub fn complete_request(
        &self,
        id: &str,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        let sender = self.pending_requests.lock().remove(id);
        match sender {
            Some(tx) => tx
                .send(result)
                .map_err(|_| format!("agent request {id} is no longer being awaited")),
            None => Err(format!("unknown or expired agent request {id}")),
        }
    }
}

/// What the settings pane shows about the server (#176: a server that
/// failed to start must say so, not look identical to one nobody used).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApiStatus {
    /// The bound port, or `None` when the listener never came up.
    pub port: Option<u16>,
    /// Why not, when not.
    pub error: Option<String>,
}

/// Managed state holding the bind outcome. Separate from
/// [`AgentApiState`] because the handlers do not care where they are
/// reachable — only the UI and the spawn path do.
pub struct AgentApiEndpoint {
    pub status: parking_lot::RwLock<AgentApiStatus>,
}

impl AgentApiEndpoint {
    pub fn bound(port: u16) -> Self {
        Self {
            status: parking_lot::RwLock::new(AgentApiStatus {
                port: Some(port),
                error: None,
            }),
        }
    }

    pub fn failed(error: String) -> Self {
        Self {
            status: parking_lot::RwLock::new(AgentApiStatus {
                port: None,
                error: Some(error),
            }),
        }
    }

    pub fn snapshot(&self) -> AgentApiStatus {
        self.status.read().clone()
    }

    /// The MCP endpoint URL to hand a harness, or `None` when the
    /// server is not up — in which case the spawn path sets no
    /// variables at all rather than pointing the agent at a dead port.
    pub fn mcp_url(&self) -> Option<String> {
        self.status
            .read()
            .port
            .map(|p| format!("http://127.0.0.1:{p}/mcp"))
    }
}

/// What one harness needs in its environment to reach its room's
/// review (#213).
///
/// Delivered as environment variables at spawn rather than as a file in
/// the worktree: a file is per-room only, would land in the review's own
/// diff, and could not say *which* harness is calling — which is what
/// makes an agent's reply attributable to one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessIdentity {
    /// The MCP endpoint, `http://127.0.0.1:<port>/mcp`.
    pub url: String,
    /// The room's bearer token.
    pub token: String,
    pub room_id: String,
    pub harness_id: String,
}
