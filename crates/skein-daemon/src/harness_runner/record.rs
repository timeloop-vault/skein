//! What the runner writes back to the harness record after a spawn: the port
//! of `ptySpawnOutcome.ts` (`capturedSessionId`) and the opencode version
//! claim (`setOpencodeVersion`).

use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::OpencodeVersion;

use super::{PtySpawnResult, RUNNER_ORIGIN};
use crate::harness_kind::HarnessKind;
use crate::room_store::RoomStore;

/// Write the spawn's session id (when it differs from the record's) and, for
/// opencode, the probed CLI version (none clears a stored one: a harness
/// recorded as v2 must not keep reading as v2 once the CLI is gone). The
/// harness is re-read just before the write, so only these two fields move.
/// A failure is logged; the process is running regardless.
pub(super) fn write_back(
    store: &RoomStore,
    room_id: &str,
    harness_id: &str,
    kind: HarnessKind,
    spawn: &PtySpawnResult,
    session_before: Option<&str>,
    version_before: Option<&OpencodeVersion>,
) {
    let new_session = spawn
        .session_id
        .as_deref()
        .filter(|sid| Some(*sid) != session_before);
    let new_version = (kind == HarnessKind::Opencode).then(|| {
        spawn.opencode_version.as_ref().map(|v| OpencodeVersion {
            major: v.major,
            version: v.version.clone(),
        })
    });
    let version_changed = new_version
        .as_ref()
        .is_some_and(|v| v.as_ref() != version_before);
    if new_session.is_none() && !version_changed {
        return;
    }
    let rooms = store.rooms();
    let Some(mut harness) = rooms
        .iter()
        .find(|r| r.id == room_id)
        .and_then(|r| r.harnesses.iter().find(|h| h.id == harness_id))
        .cloned()
    else {
        return;
    };
    if let Some(sid) = new_session {
        harness.session_id = Some(sid.to_owned());
    }
    if version_changed && let Some(v) = new_version {
        harness.opencode_version = v;
    }
    let mutation = RoomMutation::PutHarness {
        room_id: room_id.to_owned(),
        harness,
    };
    if let Err(e) = store.apply(RUNNER_ORIGIN, vec![mutation]) {
        tracing::warn!(harness = harness_id, error = %e, "harness runner: recording the spawn outcome failed");
    }
}
