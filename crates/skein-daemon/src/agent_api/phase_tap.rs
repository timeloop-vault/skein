//! The hook routes' tap into the phase machine (#732). Shadow mode: the
//! webview still decides, this only feeds the daemon's copy of the machine.
//!
//! Claude `SessionStart` / `SessionEnd` are NOT fed here. The webview decides
//! them (shell claims, session follow) and forwards its decision as a client
//! input.

use std::sync::Arc;

use skein_proto::rooms::Harness;

use super::hermes_event::HarnessHermesHook;
use super::state::AgentApiState;
use crate::harness_kind::HarnessKind;
use crate::harness_phase::PhaseService;
use crate::room_store::RoomStore;

/// What the hook routes need to feed the phase service: the service itself
/// and the room records, which say whether a harness is in post-exit-shell
/// mode.
#[derive(Clone)]
pub struct PhaseTap {
    pub service: Arc<PhaseService>,
    pub rooms: Arc<RoomStore>,
}

/// Is the Claude harness in post-exit-shell mode (#318)? The webview keeps
/// this in memory (`shellClaim.noteSpawn`: a Claude pane whose argv[0] is not
/// `claude` is in shell mode). The daemon derives it from the record: "Enter
/// for shell" rewrites the stored `cmd` to the shell, and a resume rewrites it
/// back to `claude ...` only when it consumes the claim.
fn in_shell_mode(h: &Harness) -> bool {
    if HarnessKind::from_name(&h.kind) != Some(HarnessKind::Claude) {
        return false;
    }
    // No argv0 is "not running the kind's program" too (`noteSpawn`).
    let Some(argv0) = h.cmd.as_ref().and_then(|c| c.first()) else {
        return true;
    };
    // `runsKindProgram`: the stem, without a Windows executable extension.
    let base = argv0.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = match base.rsplit_once('.') {
        Some((stem, ext))
            if ["exe", "cmd", "bat", "com"]
                .iter()
                .any(|e| ext.eq_ignore_ascii_case(e)) =>
        {
            stem
        }
        _ => base,
    };
    !stem.eq_ignore_ascii_case("claude")
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

    /// Feed a Hermes hook to the phase service. The outcome (what the client
    /// still has to do) is ignored: the client does it.
    pub(crate) fn phase_hermes_hook(&self, hook: &HarnessHermesHook) {
        if let Some(tap) = &self.phase_tap {
            let _ = tap.service.hermes_hook(&hook.harness_id, hook);
        }
    }
}

#[cfg(test)]
mod tests;
