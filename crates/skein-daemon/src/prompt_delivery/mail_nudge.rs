//! Whether to paste a mailbox nudge into a waiting harness, and the nudge
//! text: ports of `decideMailNudge`, `mailHeld`, `releaseRefusalReason`,
//! `shouldCheckOnTransition`, `mailNudgeText` (`app/src/mailNudge.ts`) and
//! `mailPending` (`app/src/mailRetry.ts`). Pure.

use super::gate::{Refusal, RefusalCode};
use crate::harness_phase::ActivityPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDecision {
    pub nudge: bool,
    /// The next value to remember as `lastNudged`.
    pub last_nudged: u32,
}

/// One nudge per arrival: only when `unread` grew past `last_nudged`, the
/// harness is at a safe stopping point and the gate is open. A read lowers
/// `unread` below `last_nudged`, which resets the remembered value, whatever
/// the stopping point or gate.
#[must_use]
pub fn decide_mail_nudge(
    at_stopping_point: bool,
    unread: u32,
    last_nudged: u32,
    gate_ok: bool,
) -> MailDecision {
    let last_nudged = unread.min(last_nudged);
    if at_stopping_point && gate_ok && unread > last_nudged && unread > 0 {
        return MailDecision {
            nudge: true,
            last_nudged: unread,
        };
    }
    MailDecision {
        nudge: false,
        last_nudged,
    }
}

/// Whether a refusal is the draft guard's.
#[must_use]
pub fn is_draft_hold(refusal: Option<&Refusal>) -> bool {
    refusal.is_some_and(|r| r.code == RefusalCode::DraftHeld)
}

/// Undelivered mail is held exactly when unread exceeds what was last
/// nudged AND the latest automatic attempt's refusal was the draft guard's.
#[must_use]
pub fn mail_held(unread: u32, last_nudged: u32, refusal: Option<&Refusal>) -> bool {
    unread > 0 && unread > last_nudged && is_draft_hold(refusal)
}

/// Why a "deliver now" that did not send was refused, as the refusal the
/// caller gets back: the gate's own, else a described one.
#[must_use]
pub fn release_refusal(
    at_stopping_point: bool,
    unread: u32,
    last_nudged: u32,
    gate: Option<&Refusal>,
) -> Refusal {
    if let Some(g) = gate {
        return g.clone();
    }
    if unread == 0 || unread <= last_nudged {
        return Refusal {
            code: RefusalCode::NothingToDeliver,
            reason: "no undelivered mail".to_string(),
        };
    }
    if !at_stopping_point {
        return Refusal {
            code: RefusalCode::NotAtStoppingPoint,
            reason: "the harness isn't at a safe stopping point".to_string(),
        };
    }
    Refusal {
        code: RefusalCode::NothingToDeliver,
        reason: "nothing to deliver".to_string(),
    }
}

/// A transition landing on `to` is worth a check when it is `waiting`, or
/// when the harness is at a safe stopping point right now (#381).
#[must_use]
pub fn should_check_on_transition(to: ActivityPhase, at_stopping_point_now: bool) -> bool {
    to == ActivityPhase::Waiting || at_stopping_point_now
}

/// One-line nudge text, never the message bodies.
#[must_use]
pub fn mail_nudge_text(count: u32, from: &[String]) -> String {
    let noun = if count == 1 { "message" } else { "messages" };
    let from = if from.is_empty() {
        String::new()
    } else {
        format!(" from {}", from.join(", "))
    };
    format!("You have {count} new {noun} in Skein{from}. Call read_messages.")
}

/// `mailPending`: `unread` has grown past `last_nudged`.
#[must_use]
pub fn mail_pending(unread: u32, last_nudged: u32) -> bool {
    unread > 0 && unread > last_nudged
}

#[cfg(test)]
mod tests;
