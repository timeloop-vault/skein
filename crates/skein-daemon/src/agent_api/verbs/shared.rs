//! Constants and helpers more than one verb family uses.

use std::time::Duration;

use super::VerbError;
use crate::harness_kind::{ALL, HarnessKind};

/// The largest a mailbox message body may be (#327). A message is a
/// nudge, not a document.
pub(super) const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Whether `kind` names a harness kind Skein knows. Derived from the
/// `HarnessKind` enum (itself parity-tested against the TS registry), so
/// a new kind cannot be forgotten here. `remote` is deliberately known:
/// the frontend refuses it for agents with an explanatory message
/// (`remoteRefusal`), which beats a bare "unknown kind" from this check.
pub(super) fn is_known_kind(kind: &str) -> bool {
    HarnessKind::from_name(kind).is_some()
}

/// The accepted kind names, for refusal messages.
pub(super) fn known_kind_names() -> Vec<&'static str> {
    ALL.iter().map(|k| k.as_str()).collect()
}

/// How long to wait for the frontend to resolve a `(kind, agent)` — a
/// folder lookup plus a couple of `localStorage` reads, so this should
/// never be close.
pub(super) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);

/// Map a [`AgentApiState::request_frontend`] error string to the right
/// [`VerbError`]. Infrastructure problems — no webview, a timeout, a
/// disconnect mid-wait (see `PendingRequestGuard`'s doc for why that one
/// is possible at all) — are [`VerbError::Unavailable`]: not the
/// caller's fault, and plausibly transient. Anything else is the
/// frontend actively saying no to this exact request (an unresolvable
/// folder, a bad branch, …), which the caller could fix and retry, so
/// it is [`VerbError::Refused`].
pub(super) fn frontend_error(message: &str) -> VerbError {
    if message.contains("no webview is listening")
        || message.contains("timed out after")
        || message.contains("dropped the request")
    {
        VerbError::Unavailable(message.to_owned())
    } else {
        VerbError::Refused(message.to_owned())
    }
}

/// Cap on how much of a message or prompt becomes a one-line title —
/// enough to read as a summary, short enough that a title never turns
/// into a second message body.
const FIRST_LINE_CAP: usize = 200;

/// The first non-empty line of `s`, trimmed and capped at
/// [`FIRST_LINE_CAP`] **characters** (never bytes, so a cut never lands
/// mid multi-byte character). `""` when `s` has no non-empty line —
/// callers that mean "absent" rather than "blank" filter that out
/// themselves, the same way `create_room`'s `promptFirstLine` does.
pub(super) fn first_non_empty_line(s: &str) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > FIRST_LINE_CAP {
        line.chars().take(FIRST_LINE_CAP).collect()
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn every_registry_kind_is_known_and_unknown_is_not() {
        for kind in ALL {
            assert!(is_known_kind(kind.as_str()), "{kind} must be accepted");
        }
        assert!(is_known_kind("hermes"));
        assert!(is_known_kind("remote"));
        assert!(!is_known_kind("bogus"));
        assert_eq!(known_kind_names().len(), ALL.len());
    }
}
