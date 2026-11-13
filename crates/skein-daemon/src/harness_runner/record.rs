//! What the runner writes back to the harness record after a spawn: the port
//! of `ptySpawnOutcome.ts` (`capturedSessionId`) and the opencode version
//! claim (`setOpencodeVersion`), plus a client's session observation
//! ([`report_session`]).

use skein_proto::rooms::OpencodeVersion;

use super::{PtySpawnResult, RUNNER_ORIGIN};
use crate::harness_kind::HarnessKind;
use crate::room_store::RoomStore;

/// Origin of the rooms-changed event a session report produces.
pub const SESSION_REPORT_ORIGIN: &str = "daemon-session";

/// A client's observation about a harness's conversation id (#755). The
/// session id is daemon-owned: a whole-record put cannot change it, so a
/// client that sees a new one says so here.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionReport {
    /// First writer wins: set only when the record has no (or a blank) session id.
    Capture { session_id: String },
    /// Authoritative: the conversation moved to this id.
    Replace { session_id: String },
    /// The stored id no longer applies (the folder was repointed).
    Clear,
}

/// Apply a [`SessionReport`] to the stored harness, field-level under the
/// store lock. Returns `Ok(true)` when the record changed; `Ok(false)` for an
/// unknown harness, a blank id, or no change.
///
/// # Errors
/// The store refused the edit (not loaded, or persisting failed).
pub fn report_session(
    store: &RoomStore,
    room_id: &str,
    harness_id: &str,
    report: SessionReport,
) -> Result<bool, String> {
    let (id, only_if_blank) = match report {
        SessionReport::Capture { session_id } => (Some(session_id), true),
        SessionReport::Replace { session_id } => (Some(session_id), false),
        SessionReport::Clear => (None, false),
    };
    if id.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return Ok(false);
    }
    store.edit_harness(SESSION_REPORT_ORIGIN, room_id, harness_id, |h| {
        let blank = h.session_id.as_deref().is_none_or(|s| s.trim().is_empty());
        if !only_if_blank || blank {
            h.session_id = id;
        }
    })
}

/// Write the session id the record should now hold (`session_now`: the
/// derived one, or opencode v2's own; `None` when a stale id was dropped) when
/// it differs from the record's, and, for opencode, the probed CLI version
/// (none clears a stored one: a harness recorded as v2 must not keep reading
/// as v2 once the CLI is gone). A failure is logged; the process is running
/// regardless.
///
/// The write is field-level under the store lock (#755), on the harness as
/// stored now, so a concurrent client put neither loses it nor is lost. A
/// session id that no longer equals what the spawn read was recorded by
/// someone newer during the spawn and is kept.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_back(
    store: &RoomStore,
    room_id: &str,
    harness_id: &str,
    kind: HarnessKind,
    spawn: &PtySpawnResult,
    session_now: Option<&str>,
    session_before: Option<&str>,
    version_before: Option<&OpencodeVersion>,
) {
    let session_changed = session_now != session_before;
    let new_version = (kind == HarnessKind::Opencode).then(|| {
        spawn.opencode_version.as_ref().map(|v| OpencodeVersion {
            major: v.major,
            version: v.version.clone(),
        })
    });
    let version_changed = new_version
        .as_ref()
        .is_some_and(|v| v.as_ref() != version_before);
    if !session_changed && !version_changed {
        return;
    }
    let mut kept_newer = false;
    let result = store.edit_harness(RUNNER_ORIGIN, room_id, harness_id, |h| {
        if session_changed {
            if h.session_id.as_deref() == session_before {
                h.session_id = session_now.map(str::to_owned);
            } else {
                kept_newer = true;
            }
        }
        if version_changed && let Some(v) = new_version {
            h.opencode_version = v;
        }
    });
    if kept_newer {
        tracing::info!(
            harness = harness_id,
            "harness runner: a newer session id was recorded during the spawn; kept it"
        );
    }
    if let Err(e) = result {
        tracing::warn!(harness = harness_id, error = %e, "harness runner: recording the spawn outcome failed");
    }
}
