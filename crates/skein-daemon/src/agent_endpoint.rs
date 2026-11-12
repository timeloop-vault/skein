//! Where the agent API is reachable, and what a harness needs to reach it.

use serde::Serialize;

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
/// `AgentApiState` because the handlers do not care where they are
/// reachable — only the UI and the spawn path do.
pub struct AgentApiEndpoint {
    pub status: parking_lot::RwLock<AgentApiStatus>,
}

/// A copy of the status as it is now. It is set once at build and never
/// changes, so a copy (the harness runner's, #731) cannot go stale.
impl Clone for AgentApiEndpoint {
    fn clone(&self) -> Self {
        Self {
            status: parking_lot::RwLock::new(self.snapshot()),
        }
    }
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
