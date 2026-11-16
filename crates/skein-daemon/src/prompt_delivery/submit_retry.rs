//! The #380/#381 gap-then-retry policy: a port of `app/src/submitRetry.ts`.

use crate::harness_phase::ActivityPhase;

/// Wait after the paste before the submit.
pub const SUBMIT_GAP_MS: i64 = 150;
/// Delays, measured from the first submit, of the Enter retries.
pub const SUBMIT_RETRY_SCHEDULE_MS: [i64; 3] = [2_000, 4_500, 8_000];

#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct RetryInput {
    pub capable: bool,
    pub watched: bool,
    pub left_stopping_point_since_send: bool,
    pub phase: Option<ActivityPhase>,
    pub deferred_at_send: Option<i64>,
    pub deferred_at_now: Option<i64>,
    pub user_input_since_send: bool,
    pub same_target: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    Retry,
    Stop(String),
}

/// `decideSubmitRetry`, check order and reasons verbatim.
#[must_use]
pub fn decide_submit_retry(i: &RetryInput) -> RetryDecision {
    let stop = |s: &str| RetryDecision::Stop(s.to_string());
    if !i.capable {
        return stop("this harness kind doesn't opt into a submit retry");
    }
    if !i.watched {
        return stop(
            "no confirmed adapter is watching this harness any more — retrying blind isn't safe",
        );
    }
    if i.left_stopping_point_since_send {
        return stop("the harness left waiting since the submit — the first Enter landed");
    }
    if !i.same_target {
        return stop("the terminal registration changed since the submit");
    }
    if i.user_input_since_send {
        return stop("the user typed into the harness since the submit");
    }
    let at_point = (i.phase == Some(ActivityPhase::Waiting) && i.deferred_at_send.is_none())
        || (i.phase == Some(ActivityPhase::Running)
            && i.deferred_at_send.is_some()
            && i.deferred_at_now == i.deferred_at_send);
    if !at_point {
        return RetryDecision::Stop(format!(
            "the harness isn't at its stopping point any more (currently {})",
            i.phase.map_or("no activity record", ActivityPhase::as_str)
        ));
    }
    RetryDecision::Retry
}
