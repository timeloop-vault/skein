//! What a request handler has access to, and how it tells the UI that
//! something changed.

use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;

use crate::db::Database;

/// The event the review pane listens for.
///
/// An agent reply touches sqlite and nothing else — no file changes, so
/// the worktree watcher that drives every other refresh never fires.
/// Without this the user would be looking at a pane that is quietly out
/// of date, which is worse than one that is obviously empty.
pub const REVIEW_CHANGED_EVENT: &str = "skein://review-changed";

/// Shared by every route.
///
/// `app` is optional for the same reason it is in the harness-event
/// managers: a unit test has a `Database` but no Tauri runtime, and
/// building a real `AppHandle` needs a running app.
pub struct AgentApiState {
    pub db: Arc<Database>,
    pub app: Option<tauri::AppHandle>,
}

/// Payload of [`REVIEW_CHANGED_EVENT`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChanged {
    pub room_id: String,
}

impl AgentApiState {
    pub fn new(db: Arc<Database>, app: tauri::AppHandle) -> Self {
        Self { db, app: Some(app) }
    }

    /// Test constructor — no `AppHandle`, so writes persist and simply
    /// notify nobody.
    #[cfg(test)]
    pub fn for_test(db: Arc<Database>) -> Self {
        Self { db, app: None }
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
