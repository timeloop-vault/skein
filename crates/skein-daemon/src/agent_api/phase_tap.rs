//! The hook routes' tap into the phase machine (#732). The
//! daemon's machine decides; this feeds it from the hook routes.
//!
//! Claude `SessionStart` / `SessionEnd` are NOT fed here. The webview decides
//! them (shell claims, session follow) and forwards its decision as a client
//! input.

use std::sync::Arc;

use skein_proto::rooms::Harness;

use super::hermes_event::HarnessHermesHook;
use super::state::AgentApiState;
use crate::harness_argv::shell_argv;
use crate::harness_kind::HarnessKind;
use crate::harness_phase::PhaseService;
use crate::harness_work_hermes::HermesWorkAttacher;
use crate::room_store::RoomStore;

/// What the hook routes need to feed the phase service: the service itself
/// and the room records, which say whether a harness is in post-exit-shell
/// mode.
#[derive(Clone)]
pub struct PhaseTap {
    pub service: Arc<PhaseService>,
    pub rooms: Arc<RoomStore>,
    /// Attaches the Hermes work poller when a hook names the session.
    pub hermes_work: Option<Arc<HermesWorkAttacher>>,
}

/// Is the Claude harness in post-exit-shell mode (#318)? The record says so
/// (#752): its launch mode is `Shell`. Boot preparation migrates a legacy
/// record (no `launch`, `cmd[0]` not `claude`) before anything spawns, so the
/// argv is no longer read here. A record with no argv at all is still treated
/// as shell mode, as the webview's `noteSpawn` did.
fn in_shell_mode(h: &Harness) -> bool {
    if HarnessKind::from_name(&h.kind) != Some(HarnessKind::Claude) {
        return false;
    }
    shell_argv(h).is_some() || h.cmd.as_ref().and_then(|c| c.first()).is_none()
}

/// `shellClaim.acceptsPermission`: outside shell mode a ping is always
/// accepted; in shell mode only with a claim, and then only when the ping's
/// session id is absent or equals the claim's.
fn accepts_permission(h: Option<&Harness>, session_id: Option<&str>) -> bool {
    let Some(h) = h.filter(|h| in_shell_mode(h)) else {
        return true;
    };
    let Some(claim) = &h.shell_claim else {
        return false;
    };
    match session_id {
        None | Some("") => true,
        Some(sid) => sid == claim.session_id,
    }
}

impl AgentApiState {
    /// The daemon's phase authority, when this state has one.
    pub(crate) fn phase_service(&self) -> Option<&Arc<PhaseService>> {
        self.phase_tap.as_ref().map(|t| &t.service)
    }

    /// Feed a Claude `PermissionRequest` ping to the phase service, gated the
    /// way the webview gates it. No-op without a tap.
    pub(crate) fn phase_permission(
        &self,
        room_id: &str,
        harness_id: &str,
        tool_name: Option<&str>,
        agent_type: Option<&str>,
        agent_id: Option<&str>,
        session_id: Option<&str>,
    ) {
        let Some(tap) = &self.phase_tap else {
            return;
        };
        let harness = tap.rooms.harness_record(room_id, harness_id);
        let accepted = accepts_permission(harness.as_ref(), session_id);
        if accepted {
            tap.service
                .claude_permission_hook(harness_id, tool_name, agent_type, agent_id);
        } else {
            tracing::debug!(
                harness_id,
                "phase: dropped permission ping from an unclaimed shell"
            );
        }
    }

    /// Feed a Hermes hook to the phase service. Most of the outcome (what the
    /// client still has to do) is ignored: the client does it. The session an
    /// `on_session_start` names is where the daemon starts the work poller.
    pub(crate) fn phase_hermes_hook(&self, hook: &HarnessHermesHook) {
        let Some(tap) = &self.phase_tap else {
            return;
        };
        let outcome = tap.service.hermes_hook(&hook.harness_id, hook);
        if let (Some(work), Some((session_id, _))) = (&tap.hermes_work, outcome.session_started) {
            work.session_started(&tap.rooms, &hook.room_id, &hook.harness_id, &session_id);
        }
    }
}

#[cfg(test)]
mod tests;
