//! The daemon pieces the agent API reaches for beyond sqlite (#734).
//!
//! The runner needs the agent endpoint, which exists only once the listener is
//! bound, so these handles cannot be constructor arguments of
//! [`AgentApiState`]: `Daemon::build` sets them once, after both exist.

use std::sync::Arc;

use super::state::AgentApiState;
use crate::harness_config::HarnessConfig;
use crate::harness_runner::HarnessRunner;
use crate::room_store::RoomStore;

/// The authoritative room records and, outside a unit test, the runner that
/// starts and stops the harnesses.
pub struct DaemonHandles {
    pub rooms: Arc<RoomStore>,
    /// `None` in a unit test.
    pub runner: Option<Arc<HarnessRunner>>,
    /// What the spawn injects, so an agent listing matches what a spawn
    /// accepts. `None` in a unit test.
    pub harness_config: Option<HarnessConfig>,
    /// Serialises the final re-read, re-check and store apply of the four
    /// daemon-executed verbs (`create_room`, `open_harness`, `close_harness`,
    /// `close_room`) against each other. A verb takes it only after its slow
    /// awaits (the `files.dirty` question, agent resolution, worktree and
    /// record building) and holds it across store calls alone.
    ///
    /// Client changes arrive through `rooms_apply` and do not take it; the
    /// re-read immediately before the apply shrinks that window to the
    /// critical section, which is the accepted limit.
    pub verb_lock: tokio::sync::Mutex<()>,
}

impl AgentApiState {
    /// Attach the handles. Set once; a second call keeps the first and is
    /// reported as an error carrying the rejected handles.
    ///
    /// # Errors
    /// The handles were already set.
    pub fn set_daemon_handles(&self, handles: DaemonHandles) -> Result<(), Box<DaemonHandles>> {
        self.daemon_handles.set(handles).map_err(Box::new)
    }

    /// The room store, once the daemon (or a test) attached it.
    #[must_use]
    pub fn room_store(&self) -> Option<&Arc<RoomStore>> {
        self.daemon_handles.get().map(|h| &h.rooms)
    }

    /// The [`DaemonHandles::verb_lock`]; `None` before the handles are set.
    #[must_use]
    pub(super) fn verb_lock(&self) -> Option<&tokio::sync::Mutex<()>> {
        self.daemon_handles.get().map(|h| &h.verb_lock)
    }

    /// Whether a client is attached and able to answer
    /// [`Self::request_frontend`] right now ([`ClientRequests::is_attached`]).
    /// In a unit test, a test frontend counts as attached.
    ///
    /// [`ClientRequests::is_attached`]: super::client::ClientRequests::is_attached
    #[must_use]
    pub fn has_client(&self) -> bool {
        #[cfg(test)]
        if self.test_frontend.lock().is_some() {
            return true;
        }
        self.client.as_ref().is_some_and(|c| c.is_attached())
    }

    /// The injection bundle paths, for agent discovery.
    #[must_use]
    pub fn harness_config(&self) -> Option<&HarnessConfig> {
        self.daemon_handles
            .get()
            .and_then(|h| h.harness_config.as_ref())
    }

    /// The harness runner; `None` before it is attached and in a unit test.
    #[must_use]
    pub fn runner(&self) -> Option<&Arc<HarnessRunner>> {
        self.daemon_handles.get().and_then(|h| h.runner.as_ref())
    }
}
