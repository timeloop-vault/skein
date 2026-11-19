//! The production [`PtyPort`]: the runner's entry for the harness, and the
//! PTY manager that writes to it.

use std::sync::Arc;

use super::daemon_env::PtyPort;
use crate::harness_runner::HarnessRunner;
use crate::pty::PtyManager;

pub struct RunnerPty {
    pub runner: Arc<HarnessRunner>,
    pub pty: PtyManager,
}

impl PtyPort for RunnerPty {
    fn live_pty(&self, harness_id: &str) -> Option<String> {
        self.runner
            .pty_id(harness_id)
            .filter(|id| self.pty.is_alive(id))
    }

    fn write(&self, pty_id: &str, bytes: &[u8]) -> bool {
        self.pty.write(pty_id, bytes).is_ok()
    }

    fn bracketed_paste(&self, harness_id: &str) -> Option<bool> {
        let id = self.live_pty(harness_id)?;
        self.pty.modes(&id).map(|m| m.bracketed_paste)
    }
}
