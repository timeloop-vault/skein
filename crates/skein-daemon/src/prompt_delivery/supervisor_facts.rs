//! The mail facts the phase supervisor's `mail_held` invariant reads
//! (#733 part B). The delivery worker publishes the nudge time and the
//! refusal into [`Shared`](super::shared::Shared); the unread count is read
//! fresh here, so a harness that has read its mail does not look held. The
//! daemon's facts closure and the tests build the facts through this one
//! function.

use super::env::MailFacts;
use crate::harness_phase::{ActivityPhase, MailState};

/// One harness's [`MailState`], or `None` when the invariant cannot apply:
/// it only fires in `Waiting`, so any other phase neither reads the unread
/// count nor the published facts, and a failed unread lookup reads as no
/// facts (the supervisor stays silent rather than guess).
#[must_use]
pub fn mail_state_for(
    harness_id: &str,
    phase: ActivityPhase,
    published: impl FnOnce() -> MailFacts,
    unread: impl FnOnce() -> Option<u32>,
) -> Option<MailState> {
    if phase != ActivityPhase::Waiting {
        return None;
    }
    let Some(unread) = unread() else {
        tracing::debug!(
            harness_id,
            "supervisor: unread mail lookup failed, no mail facts"
        );
        return None;
    };
    let MailFacts {
        last_nudge_at,
        last_refusal,
    } = published();
    Some(MailState {
        unread,
        last_refusal,
        last_nudge_at,
    })
}

#[cfg(test)]
mod tests;
